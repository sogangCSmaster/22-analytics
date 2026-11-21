use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "analytics-jobs", about = "22-analytics 운영 CLI")]
struct Cli {
    #[arg(
        long,
        env = "DATABASE_URL",
        default_value = "postgres://analytics:analytics@localhost:5432/analytics"
    )]
    database_url: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Postgres와 ClickHouse 스키마 마이그레이션
    Migrate,
    /// 로컬 개발용 광고주·프로젝트 생성 (공개 키 pk_devlocal...)
    SeedDev,
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
            let pool = analytics_pg::connect(&cli.database_url).await?;
            analytics_pg::migrate(&pool).await?;
            tracing::info!("Postgres 마이그레이션 완료");
            let applied = analytics_ch::migrate::run(&analytics_ch::ChConfig::from_env()).await?;
            tracing::info!(?applied, "ClickHouse 마이그레이션 완료");
        }
        Cmd::SeedDev => {
            let pool = analytics_pg::connect(&cli.database_url).await?;
            let seed = analytics_pg::seed::dev(&pool).await?;
            println!("광고주 id      : {}", seed.org_id);
            println!("프로젝트 id    : {}", seed.project_id);
            println!("공개 키        : {}", seed.public_key);
            println!("테스트 페이지  : http://localhost:8081/dev/");
        }
        Cmd::CreateAdmin {
            email,
            password,
            name,
            reset_password,
        } => {
            let pool = analytics_pg::connect(&cli.database_url).await?;
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
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
