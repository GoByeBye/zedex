use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use actix_files::Files;
use actix_web::{web, HttpResponse, Responder};
use log::{debug, error, info, warn};

use crate::zed::Version;

use super::super::state::ServerState;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(web::resource("/api/releases/latest").to(get_latest_version))
        .service(
            web::resource("/api/releases/{channel}/latest").to(get_latest_version),
        )
        .service(
            web::resource("/api/releases/{channel}/{version}/{filename}")
                .to(serve_release_api),
        );
}

pub fn configure_static_assets(cfg: &mut web::ServiceConfig, releases_dir: PathBuf) {
    cfg.service(Files::new("/releases", releases_dir));
}

pub async fn get_latest_version(
    path: Option<web::Path<String>>,
    state: web::Data<ServerState>,
    query: web::Query<HashMap<String, String>>,
) -> impl Responder {
    let os = query.get("os").cloned().unwrap_or_else(|| "macos".to_string());
    let arch = query
        .get("arch")
        .cloned()
        .unwrap_or_else(|| "x86_64".to_string());
    let asset = query
        .get("asset")
        .cloned()
        .unwrap_or_else(|| "zed".to_string());

    if let Some(path) = &path {
        let channel = path.as_str();
        info!("Latest version request for channel={channel}, asset={asset}, os={os}, arch={arch}");
    } else {
        info!("Latest version request for asset={asset}, os={os}, arch={arch}");
    }

    if let Some(releases_dir) = &state.config.releases_dir {
        let platform_version_file = releases_dir.join(format!("{asset}-{os}-{arch}.json"));
        info!(
            "Looking for platform-specific version file: {:?}",
            platform_version_file
        );

        if platform_version_file.exists() {
            info!(
                "Found platform-specific version file: {:?}",
                platform_version_file
            );
            return read_version_file(
                platform_version_file,
                state.config.domain.as_ref().map(|x| x.as_str()),
            );
        }

        if state.config.proxy_mode {
            return super::proxy::proxy_version_request(os, arch, asset).await;
        }

        HttpResponse::NotFound().content_type("text/plain").body(format!(
            "Version file not found for asset {} on platform {}-{}",
            asset, os, arch
        ))
    } else {
        HttpResponse::NotFound()
            .content_type("text/plain")
            .body("Releases directory not configured")
    }
}

pub fn read_version_file(file_path: PathBuf, domain: Option<&str>) -> HttpResponse {
    debug!("Reading version file: {:?}", file_path);
    match fs::read_to_string(&file_path) {
        Ok(content) => match serde_json::from_str::<Version>(&content) {
            Ok(mut version) => {
                if let Some(domain) = domain {
                    version.url = version.url.replace("https://zed.dev", domain);
                }

                info!("Successfully read version file: {:?}", file_path);
                HttpResponse::Ok()
                    .content_type("application/json")
                    .json(version)
            }
            Err(e) => {
                error!(
                    "Failed to parse version file {}: {}",
                    file_path.display(),
                    e
                );
                HttpResponse::InternalServerError()
                    .body(format!("Error parsing version file: {}", e))
            }
        },
        Err(e) => {
            error!("Failed to read version file {}: {}", file_path.display(), e);
            HttpResponse::InternalServerError().body(format!("Error reading version file: {}", e))
        }
    }
}

pub fn serve_release_file(file_path: &PathBuf) -> HttpResponse {
    match fs::read(file_path) {
        Ok(bytes) => {
            let content_type = match file_path.extension().and_then(|e| e.to_str()) {
                Some("dmg") => "application/x-apple-diskimage",
                Some("zip") => "application/zip",
                Some("exe") => "application/vnd.microsoft.portable-executable",
                Some("AppImage") => "application/x-executable",
                Some("json") => "application/json",
                Some("gz") => "application/gzip",
                Some("tar") => "application/x-tar",
                _ => "application/octet-stream",
            };

            info!("Serving release file with content type: {}", content_type);
            HttpResponse::Ok().content_type(content_type).body(bytes)
        }
        Err(e) => {
            error!("Error reading release file: {}", e);
            HttpResponse::InternalServerError().body(format!("Error reading release file: {}", e))
        }
    }
}

pub async fn serve_release_api(
    path: web::Path<(String, String, String)>,
    state: web::Data<ServerState>,
) -> impl Responder {
    let (channel, version, asset) = path.into_inner();

    info!(
        "Serving release API request: channel={}, version={}, asset={}",
        channel, version, asset
    );

    if let Some(releases_dir) = &state.config.releases_dir {
        let file_path = releases_dir.join(format!("{version}/{asset}"));

        info!("Looking for release file at: {:?}", file_path);

        if file_path.exists() {
            return serve_release_file(&file_path);
        } else {
            warn!("Release file not found: {:?}", file_path);
        }
    }

    HttpResponse::NotFound().body(format!(
        "Release file not found for {} {} {}",
        channel, version, asset
    ))
}
