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
    }
    Ok(())
}
