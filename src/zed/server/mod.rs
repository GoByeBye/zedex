mod config;
mod handlers;
mod state;

pub use config::ServerConfig;

use super::health;
use actix_files::Files;
use actix_web::{App, HttpServer, middleware::Logger, web};
use anyhow::Result;
use handlers::{extensions, proxy, releases};
use log::{info, warn};
use state::ServerState;
use std::fs;

pub struct LocalServer {
    config: ServerConfig,
}

impl LocalServer {
    pub fn new(config: ServerConfig) -> Self {
        Self { config }
    }

    pub async fn run(&self) -> Result<()> {
        const HEALTH_CHECK_PATH: &str = "/health";

        health::init();
        log_server_banner(&self.config, HEALTH_CHECK_PATH)?;

        let server_state = web::Data::new(ServerState::new(self.config.clone()));

        HttpServer::new(move || {
            let state = server_state.clone();
            let config = state.config();

            let mut app = App::new()
                .app_data(state.clone())
                .wrap(Logger::default())
                .service(web::resource(HEALTH_CHECK_PATH).to(health::health_check))
                .configure(extensions::configure)
                .configure(releases::configure);

            if let Some(releases_dir) = config.releases_dir.clone() {
                if releases_dir.exists() {
                    app = app.configure({
                        let dir = releases_dir.clone();
                        move |cfg| releases::configure_static_assets(cfg, dir.clone())
                    });
                }
            }

            app = app.service(web::resource("/api/{path:.*}").to(proxy::proxy_api_request));
            app = app.service(Files::new(
                "/extensions-archive",
                config.extensions_dir.clone(),
            ));

            app
        })
        .bind((self.config.host.as_str(), self.config.port))?
        .run()
        .await?;

        Ok(())
    }
}

fn log_server_banner(config: &ServerConfig, health_path: &str) -> Result<()> {
    info!(
        "Starting local Zed extension server on {}:{}",
        config.host, config.port
    );
    info!("Serving extensions from {:?}", config.extensions_dir);
    info!(
        "Health check available at http://{}:{}{}",
        config.host, config.port, health_path
    );

    if let Some(releases_dir) = &config.releases_dir {
        info!("Serving releases from {:?}", releases_dir);

        if releases_dir.exists() {
            for entry in (fs::read_dir(releases_dir)?).flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let asset_name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown");
                    info!("Asset directory: {}", asset_name);

                    let mut found_files = false;
                    if let Ok(dir_entries) = fs::read_dir(&path) {
                        for file_entry in dir_entries.flatten() {
                            let file_path = file_entry.path();
                            if file_path.is_file()
                                && file_path
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .map(|s| s.starts_with("latest-version-"))
                                    .unwrap_or(false)
                            {
                                if !found_files {
                                    info!("  Platform-specific version files:");
                                    found_files = true;
                                }
                                info!(
                                    "    - {}",
                                    file_path.file_name().unwrap_or_default().to_string_lossy()
                                );
                            }
                        }
                    }

                    if !found_files {
                        info!("  No version files available yet");
                    }
                }
            }
        } else {
            warn!("Releases directory does not exist yet");
        }
    } else {
        info!("No releases directory configured");
    }

    if config.proxy_mode {
        info!("Running in PROXY mode - will proxy to zed.dev for missing content");
    } else {
        info!("Running in LOCAL mode - all content served locally, no proxying");
    }

    Ok(())
}
