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
    pub host: String,
    pub extensions_dir: PathBuf,
    pub releases_dir: Option<PathBuf>,
    pub proxy_mode: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        let root_dir = PathBuf::from(".zedex-cache");
        Self {
            port: 2654,
            host: "127.0.0.1".to_string(),
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

        info!("Starting local Zed extension server on {}:{}", config.host, config.port);
        info!("Serving extensions from {:?}", config.extensions_dir);
        if let Some(releases_dir) = &config.releases_dir {
            info!("Serving releases from {:?}", releases_dir);
            
            // List available assets and platform-specific version files
            if releases_dir.exists() {
                for entry in (fs::read_dir(releases_dir)?).flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let asset_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("unknown");
                        info!("Asset directory: {}", asset_name);
                        
                        // List platform-specific version files
                        let mut found_files = false;
                        if let Ok(dir_entries) = fs::read_dir(&path) {
                            for file_entry in dir_entries.flatten() {
                                let file_path = file_entry.path();
                                if file_path.is_file() && file_path.file_name()
                                    .and_then(|n| n.to_str())
                                    .map(|s| s.starts_with("latest-version-"))
                                    .unwrap_or(false) {
                                    if !found_files {
                                        info!("  Platform-specific version files:");
                                        found_files = true;
                                    }
                                    info!("    - {}", file_path.file_name().unwrap_or_default().to_string_lossy());
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

        HttpServer::new(move || {
            let mut app = App::new()
                .app_data(server_data.clone())
                .wrap(Logger::default())
                .service(web::resource("/extensions").to(get_extensions_index))
                .service(web::resource("/extensions/updates").to(check_extension_updates))
                .service(web::resource("/extensions/{id}/download").to(download_extension))
                .service(web::resource("/extensions/{id}/{version}/download").to(download_extension_with_version));
            
            // Add the /api/releases/latest endpoint with query parameters
            app = app.service(
                web::resource("/api/releases/latest")
                    .to(get_latest_version)
            );
            
            // Add static file serving for releases if directory is configured
            if let Some(releases_dir) = &config.releases_dir {
                if releases_dir.exists() {
                    app = app.service(
                        Files::new("/releases", releases_dir)
                            .show_files_listing()
                    );
                }
            }
            
            // API proxy should come after specific routes but before generic file serving
            app = app.service(web::resource("/api/{path:.*}").to(proxy_api_request));
            
            // Extensions archive comes last as it's the most generic
            app = app.service(
                Files::new("/extensions-archive", &config.extensions_dir)
                    .show_files_listing()
            );
            
            app
        })
        .bind((config.host.as_str(), config.port))?
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
    // Extract OS, architecture and asset type from query parameters
    let os = query.get("os").cloned().unwrap_or_else(|| "macos".to_string());
    let arch = query.get("arch").cloned().unwrap_or_else(|| "x86_64".to_string());
    let asset = query.get("asset").cloned().unwrap_or_else(|| "zed".to_string());
    
    info!("Latest version request for asset={}, os={}, arch={}", asset, os, arch);
    
    if let Some(releases_dir) = &data.config.releases_dir {
        // Determine the asset-specific directory
        let asset_dir = releases_dir.join(&asset);
        
        // Try to find platform-specific version file
        let platform_version_file = asset_dir.join(format!("latest-version-{}-{}.json", os, arch));
        
        if platform_version_file.exists() {
            info!("Found platform-specific version file: {:?}", platform_version_file);
            return read_version_file(platform_version_file, os.clone(), arch.clone(), &asset);
        }
        
        // If we're in proxy mode and the file doesn't exist, proxy the request
        if data.config.proxy_mode {
            return proxy_version_request(os, arch, asset).await;
        }
        
        HttpResponse::NotFound()
            .content_type("text/plain")
            .body(format!("Version file not found for asset {} on platform {}-{}", asset, os, arch))
    } else {
        HttpResponse::NotFound()
            .content_type("text/plain")
            .body("Releases directory not configured")
    }
}

// Helper function to read and parse a version file, replacing URLs with local ones if needed
fn read_version_file(file_path: PathBuf, os: String, arch: String, asset: &str) -> HttpResponse {
    match fs::read_to_string(&file_path) {
        Ok(content) => {
            match serde_json::from_str::<Version>(&content) {
                Ok(mut version) => {
                    debug!("Successfully parsed version: {}", version.version);
                    
                    // Extract version from the parsed version object
                    let version_number = &version.version;
                    
                    // First check for platform-specific file
                    let parent_dir = file_path.parent().unwrap_or(&file_path);
                    let local_file = parent_dir.join(format!("{}-{}-{}-{}.gz", 
                        asset, version_number, os, arch));
                    
                    if local_file.exists() {
                        // If the local file exists, replace the URL with a local one
                        // This assumes the server is running and accessible
                        let local_path = format!("/releases/{}/{}-{}-{}-{}.gz", 
                            asset, asset, version_number, os, arch);
                        
                        debug!("Using local file path: {}", local_path);
                        version.url = local_path;
                    } else {
                        // If platform-specific file doesn't exist, check if we have any matching
                        // files that might work (for different platforms)
                        let asset_dir = parent_dir;
                        
                        if let Ok(dir_entries) = fs::read_dir(asset_dir) {
                            for entry in dir_entries.flatten() {
                                let path = entry.path();
                                let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                                
                                // Check if the filename contains the asset and version
                                if filename.starts_with(&format!("{}-{}", asset, version_number)) && 
                                   filename.ends_with(".gz") {
                                    
                                    // Use this file, even if for a different platform
                                    let local_path = format!("/releases/{}/{}", asset, filename);
                                    debug!("No exact platform match. Using alternative: {}", local_path);
                                    version.url = local_path;
                                    break;
                                }
                            }
                        }
                    }
                    
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

// Proxy a request for the latest version to zed.dev
async fn proxy_version_request(os: String, arch: String, asset: String) -> HttpResponse {
    debug!("Proxying version request for {}-{}-{} to zed.dev", asset, os, arch);
    
    let client = reqwest::Client::new();
    let url = format!("https://zed.dev/api/releases/latest?asset={}&os={}&arch={}", 
                      asset, os, arch);
    
    match client.get(&url).send().await {
        Ok(response) => {
            match response.error_for_status() {
                Ok(response) => {
                    match response.bytes().await {
                        Ok(bytes) => {
                            HttpResponse::Ok()
                                .content_type("application/json")
                                .body(bytes)
                        },
                        Err(e) => {
                            error!("Error reading proxied response: {}", e);
                            HttpResponse::InternalServerError()
                                .body(format!("Error reading proxied response: {}", e))
                        }
                    }
                },
                Err(e) => {
                    error!("Error from proxied server: {}", e);
                    match e.status() {
                        Some(status) => HttpResponse::build(http::StatusCode::from_u16(status.as_u16()).unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR))
                            .body(format!("Error from zed.dev: {}", e)),
                        None => HttpResponse::InternalServerError()
                            .body(format!("Error from zed.dev: {}", e))
                    }
                }
            }
        },
        Err(e) => {
            error!("Error proxying request: {}", e);
            HttpResponse::InternalServerError()
                .body(format!("Error proxying request: {}", e))
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

async fn check_extension_updates(
    data: web::Data<ServerData>,
    query: web::Query<std::collections::HashMap<String, String>>,
) -> impl Responder {
    // Extract query parameters
    let min_schema_version = query.get("min_schema_version").and_then(|v| v.parse::<i32>().ok());
    let max_schema_version = query.get("max_schema_version").and_then(|v| v.parse::<i32>().ok());
    let min_wasm_api_version = query.get("min_wasm_api_version").map(|s| s.as_str());
    let max_wasm_api_version = query.get("max_wasm_api_version").map(|s| s.as_str());
    let ids_param = query.get("ids").cloned().unwrap_or_default();
    
    // Parse the comma-separated IDs
    let extension_ids: Vec<&str> = if !ids_param.is_empty() {
        ids_param.split(',').collect()
    } else {
        Vec::new()
    };
    
    debug!("Extension update check: min_schema={:?}, max_schema={:?}, min_wasm_api={:?}, max_wasm_api={:?}, ids={:?}",
        min_schema_version, max_schema_version, min_wasm_api_version, max_wasm_api_version, extension_ids);
    
    // Read the full extensions index
    let extensions_file = data.config.extensions_dir.join("extensions.json");
    
    match fs::read_to_string(&extensions_file) {
        Ok(content) => {
            match serde_json::from_str::<WrappedExtensions>(&content) {
                Ok(extensions) => {
                    // First filter by schema version and provides (similar to get_extensions_index)
                    let filtered_by_schema = extensions_utils::filter_extensions(
                        &extensions.data,
                        None, // No text filter
                        max_schema_version,
                        None, // No provides filter
                    );
                    
                    // Then filter by min_schema_version (not part of the extensions_utils::filter_extensions function)
                    let filtered_by_min_schema = if let Some(min_version) = min_schema_version {
                        filtered_by_schema.into_iter()
                            .filter(|ext| ext.schema_version >= min_version)
                            .collect()
                    } else {
                        filtered_by_schema
                    };
                    
                    // Then filter by the requested extension IDs
                    let filtered_extensions = if !extension_ids.is_empty() {
                        filtered_by_min_schema.into_iter()
                            .filter(|ext| extension_ids.contains(&ext.id.as_str()))
                            .collect()
                    } else {
                        filtered_by_min_schema
                    };
                    
                    // Apply WASM API version filtering if specified
                    let filtered_extensions = if min_wasm_api_version.is_some() || max_wasm_api_version.is_some() {
                        filtered_extensions.into_iter()
                            .filter(|ext| {
                                // Skip extensions without a WASM API version
                                if ext.wasm_api_version.is_none() {
                                    return false;
                                }
                                
                                let ext_version = ext.wasm_api_version.as_ref().unwrap();
                                
                                // Check min version if specified
                                if let Some(min_version) = min_wasm_api_version {
                                    // Simple string comparison (assumes semver format)
                                    if ext_version.as_str() < min_version {
                                        return false;
                                    }
                                }
                                
                                // Check max version if specified
                                if let Some(max_version) = max_wasm_api_version {
                                    // Simple string comparison (assumes semver format)
                                    if ext_version.as_str() > max_version {
                                        return false;
                                    }
                                }
                                
                                true
                            })
                            .collect()
                    } else {
                        filtered_extensions
                    };
                    
                    info!("Serving {} updated extensions from index", filtered_extensions.len());
                    
                    // Return filtered extensions in the same format as the extensions index
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
            
            // If we're in proxy mode, try to proxy the request to zed.dev
            if data.config.proxy_mode {
                return proxy_extensions_updates(query).await;
            }
            
            HttpResponse::NotFound().body(format!("Extensions file not found: {}", e))
        }
    }
}

// Proxy a request for extension updates to zed.dev
async fn proxy_extensions_updates(
    query: web::Query<std::collections::HashMap<String, String>>,
) -> HttpResponse {
    debug!("Proxying extension updates request to api.zed.dev");
    
    let client = match reqwest::Client::builder()
        .user_agent("zedex")
        .build() {
        Ok(client) => client,
        Err(e) => {
            error!("Error creating HTTP client: {}", e);
            return HttpResponse::InternalServerError().body(format!("Error creating HTTP client: {}", e));
        }
    };
    
    // Construct the URL with all query parameters
    let mut url = "https://api.zed.dev/extensions/updates".to_string();
    
    // Add query parameters to the URL
    if !query.is_empty() {
        url.push('?');
        let query_string = query.iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("&");
        url.push_str(&query_string);
    }
    
    debug!("Proxying extension updates to: {}", url);
    
    match client.get(&url).send().await {
        Ok(response) => {
            match response.error_for_status() {
                Ok(response) => {
                    match response.bytes().await {
                        Ok(bytes) => {
                            HttpResponse::Ok()
                                .content_type("application/json")
                                .body(bytes)
                        },
                        Err(e) => {
                            error!("Error reading proxied response: {}", e);
                            HttpResponse::InternalServerError()
                                .body(format!("Error reading proxied response: {}", e))
                        }
                    }
                },
                Err(e) => {
                    error!("Error from proxied server: {}", e);
                    match e.status() {
                        Some(status) => HttpResponse::build(http::StatusCode::from_u16(status.as_u16()).unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR))
                            .body(format!("Error from zed.dev: {}", e)),
                        None => HttpResponse::InternalServerError()
                            .body(format!("Error from zed.dev: {}", e))
                    }
                }
            }
        },
        Err(e) => {
            error!("Error proxying request: {}", e);
            HttpResponse::InternalServerError()
                .body(format!("Error proxying request: {}", e))
        }
    }
}
