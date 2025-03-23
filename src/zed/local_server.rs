use std::path::PathBuf;
use std::fs;
use anyhow::Result;
use actix_web::{web, App, HttpServer, HttpResponse, Responder, http};
use actix_web::middleware::Logger;
use actix_files::Files;
use log::{debug, error, info, trace, warn};

use crate::zed::{WrappedExtensions, Version, extensions_utils};

#[derive(Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub extensions_dir: PathBuf,
    pub releases_dir: Option<PathBuf>,
    pub proxy_mode: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        let root_dir = PathBuf::from(".zedex-cache");
        Self {
            port: 3000,
            extensions_dir: root_dir.clone(),
            releases_dir: Some(root_dir.join("releases")),
            proxy_mode: false,
        }
    }
}

pub struct LocalServer {
    config: ServerConfig,
}

impl LocalServer {
    pub fn new(config: ServerConfig) -> Self {
        Self { config }
    }

    pub async fn run(&self) -> Result<()> {
        let config = self.config.clone();
        let server_data = web::Data::new(ServerData {
            config: config.clone(),
        });

        info!("Starting local Zed extension server on port {}", config.port);
        info!("Serving extensions from {:?}", config.extensions_dir);
        if let Some(releases_dir) = &config.releases_dir {
            info!("Serving releases from {:?}", releases_dir);
            info!("Platform-specific version files available:");
            // List available platform-specific version files
            if releases_dir.exists() {
                for entry in (fs::read_dir(releases_dir)?).flatten() {
                    let path = entry.path();
                    if path.is_file() && path.file_name().and_then(|n| n.to_str()).is_some_and(|s| s.starts_with("latest-version-")) {
                        info!("  - {:?}", path.file_name().unwrap_or_default());
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

        HttpServer::new(move || {
            let mut app = App::new()
                .app_data(server_data.clone())
                .wrap(Logger::default())
                .service(web::resource("/extensions").to(get_extensions_index))
                .service(web::resource("/extensions/{id}/download").to(download_extension))
                .service(web::resource("/extensions/{id}/{version}/download").to(download_extension_with_version));
            
            // Add the /api/releases/latest endpoint with query parameters
            app = app.service(
                web::resource("/api/releases/latest")
                    .to(get_latest_version)
            );
            
            // API proxy should come after specific routes but before generic file serving
            app = app.service(web::resource("/api/{path:.*}").to(proxy_api_request));
            
            // Extensions archive comes last as it's the most generic
            app = app.service(
                Files::new("/extensions-archive", &config.extensions_dir)
                    .show_files_listing()
            );
            
            app
        })
        .bind(("127.0.0.1", config.port))?
        .run()
        .await?;

        Ok(())
    }
}

#[derive(Clone)]
struct ServerData {
    config: ServerConfig,
}

async fn get_extensions_index(
    data: web::Data<ServerData>,
    query: web::Query<std::collections::HashMap<String, String>>,
) -> impl Responder {
    let extensions_file = data.config.extensions_dir.join("extensions.json");
    
    match fs::read_to_string(&extensions_file) {
        Ok(content) => {
            match serde_json::from_str::<WrappedExtensions>(&content) {
                Ok(extensions) => {
                    // Extract query parameters
                    let filter = query.get("filter").map(|s| s.as_str());
                    let max_schema_version = query.get("max_schema_version")
                        .and_then(|v| v.parse::<i32>().ok());
                    let provides = query.get("provides").map(|s| s.as_str());
                    
                    debug!("Filtering extensions: filter={:?}, max_schema_version={:?}, provides={:?}", 
                          filter, max_schema_version, provides);
                    
                    // Apply filtering
                    let filtered_extensions = extensions_utils::filter_extensions(
                        &extensions.data,
                        filter,
                        max_schema_version,
                        provides,
                    );
                    
                    info!("Serving {} filtered extensions from index", filtered_extensions.len());
                    // Return filtered extensions
                    let wrapped = WrappedExtensions { data: filtered_extensions };
                    HttpResponse::Ok().json(wrapped)
                },
                Err(e) => {
                    error!("Error parsing extensions.json: {}", e);
                    HttpResponse::InternalServerError().body(format!("Error parsing extensions file: {}", e))
                }
            }
        },
        Err(e) => {
            error!("Error reading extensions.json: {}", e);
            HttpResponse::NotFound().body(format!("Extensions file not found: {}", e))
        }
    }
}

async fn download_extension(
    path: web::Path<String>,
    data: web::Data<ServerData>
) -> impl Responder {
    let id = path.into_inner();
    let file_path = data.config.extensions_dir.join(format!("{}.tar.gz", id));
    
    debug!("Attempting to serve extension archive for id: {}", id);
    match fs::read(&file_path) {
        Ok(bytes) => {
            info!("Successfully served extension archive: {}", id);
            HttpResponse::Ok()
                .content_type("application/gzip")
                .body(bytes)
        },
        Err(e) => {
            error!("Error reading extension archive {}: {}", id, e);
            HttpResponse::NotFound().body(format!("Extension archive not found: {}", e))
        }
    }
}

async fn download_extension_with_version(
    path: web::Path<(String, String)>,
    data: web::Data<ServerData>
) -> impl Responder {
    let (id, version) = path.into_inner();
    debug!("Requested extension {} with version {}", id, version);
    // Version is ignored for now, since we only keep the latest version
    download_extension(web::Path::from(id), data).await
}

async fn get_latest_version(
    data: web::Data<ServerData>,
    query: web::Query<std::collections::HashMap<String, String>>,
) -> impl Responder {
    // Extract OS and architecture from query parameters
    // Default to macos-x86_64 if not provided
    let os = query.get("os").cloned().unwrap_or_else(|| "macos".to_string());
    let arch = query.get("arch").cloned().unwrap_or_else(|| "x86_64".to_string());
    
    info!("Latest version request for os={}, arch={}", os, arch);
    
    if let Some(releases_dir) = &data.config.releases_dir {
        // Try to find platform-specific version file
        let version_file = releases_dir.join(format!("latest-version-{}-{}.json", os, arch));
        
        if !version_file.exists() {
            debug!("Platform-specific version file not found: {:?}", version_file);
            // If platform-specific file doesn't exist, try generic one (for backward compatibility)
            let generic_file = releases_dir.join("latest-version.json");
            if generic_file.exists() {
                info!("Using generic version file: {:?}", generic_file);
                return read_version_file(generic_file);
            }
        } else {
            info!("Found platform-specific version file: {:?}", version_file);
            return read_version_file(version_file);
        }
        
        HttpResponse::NotFound()
            .content_type("text/plain")
            .body(format!("Version file not found for platform {}-{}", os, arch))
    } else {
        HttpResponse::NotFound()
            .content_type("text/plain")
            .body("Releases directory not configured")
    }
}

// Helper function to read and parse a version file
fn read_version_file(file_path: PathBuf) -> HttpResponse {
    match fs::read_to_string(&file_path) {
        Ok(content) => {
            match serde_json::from_str::<Version>(&content) {
                Ok(version) => {
                    debug!("Successfully parsed version: {}", version.version);
                    HttpResponse::Ok()
                        .content_type("application/json")
                        .json(version)
                },
                Err(e) => {
                    error!("Error parsing version file: {}", e);
                    HttpResponse::InternalServerError()
                        .content_type("text/plain")
                        .body(format!("Error parsing version file: {}", e))
                }
            }
        },
        Err(e) => {
            error!("Error reading version file: {}", e);
            HttpResponse::NotFound()
                .content_type("text/plain")
                .body(format!("Version file not found: {}", e))
        }
    }
}

async fn proxy_api_request(
    path: web::Path<String>,
    query: web::Query<std::collections::HashMap<String, String>>,
    data: web::Data<ServerData>
) -> impl Responder {
    let path_str = path.into_inner();
    
    // Log the request path and query parameters for debugging
    debug!("API request for path: {}", path_str);
    for (key, value) in query.iter() {
        trace!("  Query param: {}={}", key, value);
    }
    
    // Don't proxy if not in proxy mode
    if !data.config.proxy_mode {
        warn!("Rejecting proxy request in local mode: {}", path_str);
        return HttpResponse::NotFound().body(format!("API path not found locally: {}", path_str));
    }
    
    // Skip proxying requests for release files if we have a releases directory
    if data.config.releases_dir.is_some() && path_str.starts_with("releases/") && path_str != "releases/latest" {
        // Instead of returning 404, try to serve the file directly
        // The path will be something like "releases/stable/0.178.0/Zed.dmg"
        let file_path = data.config.releases_dir.as_ref().unwrap()
            .join(path_str.trim_start_matches("releases/"));
        debug!("Attempting to serve release file from: {:?}", file_path);
        
        if file_path.exists() {
            match fs::read(&file_path) {
                Ok(bytes) => {
                    // Determine content type based on file extension
                    let content_type = match file_path.extension().and_then(|e| e.to_str()) {
                        Some("dmg") => "application/x-apple-diskimage",
                        Some("zip") => "application/zip",
                        Some("exe") => "application/vnd.microsoft.portable-executable",
                        Some("AppImage") => "application/x-executable",
                        Some("json") => "application/json",
                        _ => "application/octet-stream",
                    };
                    
                    info!("Serving release file with content type: {}", content_type);
                    return HttpResponse::Ok()
                        .content_type(content_type)
                        .body(bytes);
                },
                Err(e) => {
                    error!("Error reading release file: {}", e);
                }
            }
        } else {
            debug!("Release file not found locally: {:?}", file_path);
        }
        
        // If we couldn't serve the file locally, fall through to proxy to zed.dev
    }
    
    // Append query parameters to the URL
    let client = match reqwest::Client::builder()
        .user_agent("zedex")
        .build() {
        Ok(client) => client,
        Err(e) => {
            error!("Error creating HTTP client: {}", e);
            return HttpResponse::InternalServerError().body(format!("Error creating HTTP client: {}", e));
        }
    };
    let mut url = format!("https://zed.dev/api/{}", path_str);
    
    // Add query parameters to the URL
    if !query.is_empty() {
        url.push('?');
        let query_string = query.iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");
        url.push_str(&query_string);
    }
    
    debug!("Proxying request to: {}", url);
    
    match client.get(&url).send().await {
        Ok(response) => {
            let status = response.status();
            debug!("Proxy response status: {}", status);
            
            // Get content type from response before consuming it
            let content_type = response.headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|h| h.to_str().ok())
                .unwrap_or("application/json")
                .to_string();
            
            let body = response.bytes().await.unwrap_or_default();
            
            debug!("Response content type: {}", content_type);
            debug!("Response size: {} bytes", body.len());
            
            HttpResponse::build(http::StatusCode::from_u16(status.as_u16()).unwrap_or(http::StatusCode::OK))
                .content_type(content_type)
                .body(body)
        },
        Err(e) => {
            error!("Error proxying request: {}", e);
            HttpResponse::InternalServerError().body(format!("Error proxying request: {}", e))
        }
    }
} 