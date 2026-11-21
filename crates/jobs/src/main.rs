use analytics_jobs::attribution::{self, Mode, worker};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

/// jobs Postgres 풀 기본 크기. 잡은 배치 단위로 순차 실행한다.
const DEFAULT_PG_MAX_CONNECTIONS: u32 = 3;

#[derive(Parser)]
#[command(name = "analytics-jobs", about = "22-analytics 운영 CLI")]
struct Cli {
    #[arg(
        long,
        env = "DATABASE_URL",
        default_value = "postgres://analytics:analytics@localhost:5432/analytics"
    )]
    database_url: String,
    /// Postgres 풀 크기
    #[arg(long, env = analytics_pg::ENV_MAX_CONNECTIONS, default_value_t = DEFAULT_PG_MAX_CONNECTIONS)]
    pg_max_connections: u32,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Postgres와 ClickHouse 스키마 마이그레이션
    Migrate,
    /// 로컬 개발용 광고주·프로젝트 생성 (공개 키 pk_devlocal...)
    SeedDev,
    /// S2S 주문 API 비밀 키(sk_live_...) 발급. 키는 이때 한 번만 출력된다.
    CreateKey {
        #[arg(long)]
        project: i32,
    },
    /// 기여 연산 (기본: watermark 이후 바뀐 주문만 증분 처리)
    Attribution {
        /// 이 프로젝트만 (없으면 활성 프로젝트 전체)
        #[arg(long)]
        project: Option<u32>,
        /// 최근 N일 주문 전체를 다시 연산 (늦은 터치포인트·identity·채널 규칙 반영)
        #[arg(long)]
        full: bool,
        /// --full 기간 (일)
        #[arg(long, default_value_t = attribution::DEFAULT_FULL_DAYS)]
        since_days: u32,
    },
    /// 매출 보존 검증. 위반 주문이 있으면 출력하고 0이 아닌 코드로 끝난다.
    ConservationCheck {
        #[arg(long)]
        project: Option<u32>,
        /// 전체 기간 검사 (기본: 최근 40일 주문만)
        #[arg(long)]
        full: bool,
    },
    /// 만료된 대시보드 로그인 세션 삭제 (cron 등으로 주기 실행)
    CleanupSessions,
    /// 상주 워커: 증분(ATTRIBUTION_INTERVAL_SECS, 기본 300초), 전체(매일 04:00 KST), 보존 검증(매시간)
    Worker,
    /// 슈퍼 관리자(대행사 관리자) 계정 부트스트랩.
    /// 이미 있는 이메일이면 슈퍼 관리자로 승격만 하고, `--reset-password`일 때만 비밀번호를 바꾼다.
    CreateAdmin {
        #[arg(long)]
        email: String,
        /// 10~256자. 셸 기록에 남지 않게 `ADMIN_PASSWORD` 환경 변수를 권장한다.
        #[arg(long, env = "ADMIN_PASSWORD", hide_env_values = true)]
        password: String,
        #[arg(long, default_value = "")]
        name: String,
        #[arg(long)]
        reset_password: bool,
    },
}

/// `cleanup-sessions` 본체. 지운 세션 수.
async fn cleanup_sessions(pool: &analytics_pg::PgPool) -> anyhow::Result<u64> {
    analytics_pg::sessions::delete_expired(pool).await
}

#[derive(Debug, PartialEq, Eq)]
enum AdminOutcome {
    Created(i64),
    Promoted(i64),
}

/// `create-admin` 본체 (테스트 가능하도록 분리).
async fn create_admin(
    pool: &analytics_pg::PgPool,
    email: &str,
    password: &str,
    name: &str,
    reset_password: bool,
) -> anyhow::Result<AdminOutcome> {
    let email = email.trim().to_lowercase();
    anyhow::ensure!(
        email.split('@').count() == 2
            && email.split('@').all(|p| !p.is_empty())
            && !email.chars().any(char::is_whitespace),
        "올바른 이메일이 아닙니다"
    );
    let n = password.chars().count();
    anyhow::ensure!((10..=256).contains(&n), "비밀번호는 10~256자");
    let hash = analytics_pg::users::hash_password(password)?;
    if let Some(u) = analytics_pg::users::by_email(pool, &email).await? {
        analytics_pg::users::promote_super_admin(pool, u.id, reset_password.then_some(&hash))
            .await?;
        return Ok(AdminOutcome::Promoted(u.id));
    }
    let u = analytics_pg::users::create(
        pool,
        &analytics_pg::users::NewUser {
            email,
            password_hash: hash,
            name: name.trim().to_owned(),
            is_super_admin: true,
        },
    )
    .await?;
    analytics_pg::audit::record(
        pool,
        analytics_pg::audit::Entry {
            org_id: None,
            user_id: None,
            action: "user.bootstrap_admin",
            target: format!("user:{}", u.id),
            payload: serde_json::json!({ "email": u.email }),
        },
    )
    .await?;
    Ok(AdminOutcome::Created(u.id))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let cli = Cli::parse();

    match cli.cmd {
        Cmd::Migrate => {
            let pool = analytics_pg::connect(&cli.database_url, cli.pg_max_connections).await?;
            analytics_pg::migrate(&pool).await?;
            tracing::info!("Postgres 마이그레이션 완료");
            let applied = analytics_ch::migrate::run(&analytics_ch::ChConfig::from_env()).await?;
            tracing::info!(?applied, "ClickHouse 마이그레이션 완료");
        }
        Cmd::SeedDev => {
            let pool = analytics_pg::connect(&cli.database_url, cli.pg_max_connections).await?;
            let seed = analytics_pg::seed::dev(&pool).await?;
            println!("광고주 id      : {}", seed.org_id);
            println!("프로젝트 id    : {}", seed.project_id);
            println!("공개 키        : {}", seed.public_key);
            println!("테스트 페이지  : http://localhost:8081/dev/");
        }
        Cmd::CreateKey { project } => {
            let pool = analytics_pg::connect(&cli.database_url, cli.pg_max_connections).await?;
            let (plain, row) = analytics_pg::api_keys::create(&pool, project, None).await?;
            println!("키 id   : {}", row.id);
            println!("prefix  : {}", row.prefix);
            println!("비밀 키 : {plain}");
            println!("※ 비밀 키는 다시 볼 수 없습니다. 안전한 곳에 보관하세요.");
        }
        Cmd::Attribution {
            project,
            full,
            since_days,
        } => {
            let pool = analytics_pg::connect(&cli.database_url, cli.pg_max_connections).await?;
            let ch = analytics_ch::ChConfig::from_env().guarded_client();
            let mode = if full {
                Mode::Full { since_days }
            } else {
                Mode::Incremental
            };
            let r = attribution::run(&ch, &pool, project, &mode, attribution::now_ms()).await?;
            println!(
                "프로젝트 {} / 주문 {} / 변경 {} / 행 {}",
                r.projects, r.orders_seen, r.orders_written, r.rows_written
            );
        }
        Cmd::ConservationCheck { project, full } => {
            let pool = analytics_pg::connect(&cli.database_url, cli.pg_max_connections).await?;
            let ch = analytics_ch::ChConfig::from_env().guarded_client();
            let window = if full {
                attribution::ConservationWindow::Full
            } else {
                attribution::ConservationWindow::Recent {
                    days: attribution::DEFAULT_CONSERVATION_DAYS,
                    now_ms: attribution::now_ms(),
                }
            };
            let broken =
                attribution::conservation_check_window(&ch, &pool, project, window).await?;
            if broken.is_empty() {
                println!("매출 보존 검증 통과");
                return Ok(());
            }
            println!("project_id\tmodel\tlookback_days\torder_id\tattributed\teffective_amount");
            for (p, b) in &broken {
                println!(
                    "{p}\t{}\t{}\t{}\t{}\t{}",
                    b.model, b.lookback_days, b.order_id, b.attributed, b.effective_amount
                );
            }
            eprintln!("매출 보존 법칙 위반 {}건", broken.len());
            std::process::exit(1);
        }
        Cmd::CreateAdmin {
            email,
            password,
            name,
            reset_password,
        } => {
            let pool = analytics_pg::connect(&cli.database_url, cli.pg_max_connections).await?;
            match create_admin(&pool, &email, &password, &name, reset_password).await? {
                AdminOutcome::Created(id) => println!("슈퍼 관리자 생성: id={id}"),
                AdminOutcome::Promoted(id) if reset_password => {
                    println!("기존 계정 승격 + 비밀번호 재설정: id={id}")
                }
                AdminOutcome::Promoted(id) => {
                    println!("기존 계정을 슈퍼 관리자로 승격 (비밀번호 유지): id={id}")
                }
            }
        }
        Cmd::CleanupSessions => {
            let pool = analytics_pg::connect(&cli.database_url, cli.pg_max_connections).await?;
            let n = cleanup_sessions(&pool).await?;
            println!("만료 세션 {n}건 삭제");
        }
        Cmd::Worker => {
            let pool = analytics_pg::connect(&cli.database_url, cli.pg_max_connections).await?;
            let ch = analytics_ch::ChConfig::from_env().guarded_client();
            worker::run(
                &ch,
                &pool,
                &worker::WorkerConfig::from_env(),
                shutdown_signal(),
            )
            .await?;
        }
    }
    Ok(())
}

/// Ctrl-C 또는 SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
    }
    tracing::info!("종료 신호 수신");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg_pool_default_is_3_and_configurable() {
        // 잡은 배치 단위로 순차 실행한다: 연결 3개면 충분하다 (PG_MAX_CONNECTIONS로 바꾼다)
        let cli = Cli::try_parse_from(["analytics-jobs", "migrate"]).unwrap();
        assert_eq!(cli.pg_max_connections, 3);
        let cli = Cli::try_parse_from(["analytics-jobs", "--pg-max-connections", "8", "migrate"])
            .unwrap();
        assert_eq!(cli.pg_max_connections, 8);
    }

    #[test]
    fn cleanup_sessions_subcommand_parses() {
        let cli = Cli::try_parse_from(["analytics-jobs", "cleanup-sessions"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::CleanupSessions));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cleanup_sessions_removes_only_expired() {
        let db = analytics_testkit::TestDb::new().await;
        let u = analytics_pg::users::create(
            &db.pg,
            &analytics_pg::users::NewUser {
                email: "a@agency.test".into(),
                password_hash: analytics_pg::users::hash_password("password!!").unwrap(),
                name: String::new(),
                is_super_admin: false,
            },
        )
        .await
        .unwrap();
        let live = analytics_pg::sessions::create(&db.pg, u.id).await.unwrap();
        let dead = analytics_pg::sessions::create(&db.pg, u.id).await.unwrap();
        sqlx::query("UPDATE sessions SET expires_at = now() - interval '1 second' WHERE id = $1")
            .bind(analytics_pg::sessions::token_hash(&dead))
            .execute(&db.pg)
            .await
            .unwrap();
        assert_eq!(cleanup_sessions(&db.pg).await.unwrap(), 1);
        assert_eq!(cleanup_sessions(&db.pg).await.unwrap(), 0, "멱등");
        assert!(
            analytics_pg::sessions::lookup(&db.pg, &live)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_admin_is_idempotent_and_promotes() {
        let db = analytics_testkit::TestDb::new().await;
        let pw = "bootstrap password";
        let AdminOutcome::Created(id) =
            create_admin(&db.pg, " Root@Agency.test ", pw, "관리자", false)
                .await
                .unwrap()
        else {
            panic!("새로 만들어야 함");
        };
        assert_eq!(
            create_admin(&db.pg, "root@agency.test", "another password", "", false)
                .await
                .unwrap(),
            AdminOutcome::Promoted(id)
        );
        // 비밀번호는 그대로
        let (_, hash) = analytics_pg::users::credentials_by_email(&db.pg, "root@agency.test")
            .await
            .unwrap()
            .unwrap();
        assert!(analytics_pg::users::verify_password(pw, &hash));
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
            .fetch_one(&db.pg)
            .await
            .unwrap();
        assert_eq!(n, 1);

        // 일반 사용자 승격 + 비밀번호 재설정
        analytics_pg::users::create(
            &db.pg,
            &analytics_pg::users::NewUser {
                email: "ae@agency.test".into(),
                password_hash: analytics_pg::users::hash_password("old password!").unwrap(),
                name: String::new(),
                is_super_admin: false,
            },
        )
        .await
        .unwrap();
        create_admin(&db.pg, "ae@agency.test", "new password!!", "", true)
            .await
            .unwrap();
        let u = analytics_pg::users::by_email(&db.pg, "ae@agency.test")
            .await
            .unwrap()
            .unwrap();
        assert!(u.is_super_admin);
        let (_, hash) = analytics_pg::users::credentials_by_email(&db.pg, "ae@agency.test")
            .await
            .unwrap()
            .unwrap();
        assert!(analytics_pg::users::verify_password(
            "new password!!",
            &hash
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_admin_rejects_bad_input() {
        let db = analytics_testkit::TestDb::new().await;
        assert!(
            create_admin(&db.pg, "no-at", "long enough pw", "", false)
                .await
                .is_err()
        );
        assert!(
            create_admin(&db.pg, "a@b.c", "short", "", false)
                .await
                .is_err()
        );
    }
}
