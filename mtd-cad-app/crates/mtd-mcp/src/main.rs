use anyhow::Result;
use mtd_index::{Store, paths::default_db_path};
use mtd_mcp::MemoryServer;
use rmcp::{ServiceExt, transport::stdio};

#[tokio::main]
async fn main() -> Result<()> {
    // stdout is the protocol channel: log to stderr only.
    tracing_subscriber::fmt().with_writer(std::io::stderr).with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();

    let mut args = std::env::args().skip(1);
    let mut db = None;
    while let Some(a) = args.next() {
        if a == "--db" {
            db = args.next().map(std::path::PathBuf::from);
        }
    }
    let db = db.unwrap_or_else(default_db_path);
    tracing::info!("mtd-mcp serving {}", db.display());
    let service = MemoryServer::new(Store::open(&db)?).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
