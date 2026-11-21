use analytics_jobs::attribution::{self, Mode, worker};
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
    },
    /// 상주 워커: 증분(ATTRIBUTION_INTERVAL_SECS, 기본 300초), 전체(매일 04:00 KST), 보존 검증(매시간)
    Worker,
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
        Cmd::CreateKey { project } => {
            let pool = analytics_pg::connect(&cli.database_url).await?;
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
            let pool = analytics_pg::connect(&cli.database_url).await?;
            let ch = analytics_ch::ChConfig::from_env().client();
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
        Cmd::ConservationCheck { project } => {
            let pool = analytics_pg::connect(&cli.database_url).await?;
            let ch = analytics_ch::ChConfig::from_env().client();
            let broken = attribution::conservation_check(&ch, &pool, project).await?;
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
        Cmd::Worker => {
            let pool = analytics_pg::connect(&cli.database_url).await?;
            let ch = analytics_ch::ChConfig::from_env().client();
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
