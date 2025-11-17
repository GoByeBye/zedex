mod app;
mod cli;
mod commands;
mod zed;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    app::run().await
}
