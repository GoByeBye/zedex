use crate::zed::{LocalServer, ServerConfig};
use anyhow::Result;
use std::path::PathBuf;

pub struct ServeOptions {
    pub port: u16,
    pub host: String,
    pub extensions_dir: Option<PathBuf>,
    pub proxy_mode: bool,
    pub domain: Option<String>,
}

pub async fn run(options: ServeOptions, root_dir: PathBuf) -> Result<()> {
    let mut config = ServerConfig::default();
    config.port = options.port;
    config.host = options.host;
    config.proxy_mode = options.proxy_mode;
    config.domain = options.domain;

    let resolved_extensions_dir = options.extensions_dir.unwrap_or(root_dir);
    config.extensions_dir = resolved_extensions_dir.clone();

    if let Some(releases_dir) = config.releases_dir.as_mut() {
        *releases_dir = resolved_extensions_dir.join("releases");
    }

    let server = LocalServer::new(config);
    server.run().await
}
