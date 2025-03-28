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
    pub domain: Option<String>,
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
            domain: None,
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
                .service(web::resource("/extensions/{id}/{version}/download").to(download_extension_with_version))
                .service(web::resource("/extensions/{id}").to(get_extension_versions));
            
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

// Shared filtering function to handle all extension filtering use cases
fn filter_extensions_with_params(
    extensions: &WrappedExtensions,
    filter: Option<&str>,
    min_schema_version: Option<i32>,
    max_schema_version: Option<i32>,
    min_wasm_api_version: Option<&str>,
    max_wasm_api_version: Option<&str>,
    provides: Option<&str>,
    extension_ids: Option<&[&str]>,
) -> crate::zed::Extensions {
    // First apply the standard extensions_utils filtering (text filter, max_schema, provides)
    let filtered_by_standard = extensions_utils::filter_extensions(
        &extensions.data,
        filter,
        max_schema_version,
        provides,
    );
    
    // Then filter by min_schema_version if specified
    let filtered_by_min_schema = if let Some(min_version) = min_schema_version {
        filtered_by_standard.into_iter()
            .filter(|ext| ext.schema_version >= min_version)
            .collect()
    } else {
        filtered_by_standard
    };
    
    // Then filter by extension IDs if specified
    let filtered_by_ids = if let Some(ids) = extension_ids {
        if !ids.is_empty() {
            filtered_by_min_schema.into_iter()
                .filter(|ext| ids.contains(&ext.id.as_str()))
                .collect()
        } else {
            filtered_by_min_schema
        }
    } else {
        filtered_by_min_schema
    };
    
    // Apply WASM API version filtering if specified
    if min_wasm_api_version.is_some() || max_wasm_api_version.is_some() {
        filtered_by_ids.into_iter()
            .filter(|ext| {
                // For extensions without a WASM API version, include them in the results
                if ext.wasm_api_version.is_none() {
                    return true;
                }
                
                let ext_version = ext.wasm_api_version.as_ref().unwrap();
                
                // Check min version if specified
                if let Some(min_version) = min_wasm_api_version {
                    if ext_version.as_str() < min_version {
                        return false;
                    }
                }
                
                // Check max version if specified
                if let Some(max_version) = max_wasm_api_version {
                    if ext_version.as_str() > max_version {
                        return false;
                    }
                }
                
                true
            })
            .collect()
    } else {
        filtered_by_ids
    }
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
                    
                    // Apply filtering using the consolidated function
                    let filtered_extensions = filter_extensions_with_params(
                        &extensions,
                        filter,
                        None, // min_schema_version not used for /extensions
                        max_schema_version,
                        None, // min_wasm_api_version not used for /extensions
                        None, // max_wasm_api_version not used for /extensions
                        provides,
                        None, // extension_ids not used for /extensions
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
    
    // First, try to find the extension in its own directory (new structure)
    let ext_dir = data.config.extensions_dir.join(&id);
    let file_path = if ext_dir.exists() {
        // Look for the latest version in the extension directory
        // The file should be named "{id}.tgz" for the latest version
        ext_dir.join(format!("{}.tgz", id))
    } else {
        // Fall back to the old structure (flat directory with all extensions)
        data.config.extensions_dir.join(format!("{}.tar.gz", id))
    };
    
    debug!("Attempting to serve extension archive for id: {} from {:?}", id, file_path);
    match fs::read(&file_path) {
        Ok(bytes) => {
            info!("Successfully served extension archive: {}", id);
            HttpResponse::Ok()
                .content_type("application/gzip")
                .body(bytes)
        },
        Err(e) => {
            if data.config.proxy_mode {
                error!("Extension file not found, proxying: {}", id);
                // In proxy mode, forward the request to Zed API
                proxy_download_request(id).await
            } else {
                error!("Extension file not found: {}", id);
                HttpResponse::NotFound().body(format!("Extension archive not found: {}", e))
            }
        }
    }
}

async fn download_extension_with_version(
    path: web::Path<(String, String)>,
    data: web::Data<ServerData>
) -> impl Responder {
    let (id, version) = path.into_inner();
    debug!("Requested extension {} with version {}", id, version);
    
    // Check for the extension in its own directory with the specified version
    let ext_dir = data.config.extensions_dir.join(&id);
    let versioned_file_path = ext_dir.join(format!("{}-{}.tgz", id, version));
    
    debug!("Looking for versioned extension at {:?}", versioned_file_path);
    match fs::read(&versioned_file_path) {
        Ok(bytes) => {
            info!("Successfully served extension archive: {} version {}", id, version);
            HttpResponse::Ok()
                .content_type("application/gzip")
                .body(bytes)
        },
        Err(e) => {
            if data.config.proxy_mode {
                error!("Extension version file not found, proxying: {} version {}", id, version);
                // In proxy mode, forward the request to Zed API with specific version
                proxy_download_version_request(id, version).await
            } else {
                error!("Extension version file not found: {} version {}", id, version);
                HttpResponse::NotFound().body(format!("Extension version archive not found: {}", e))
            }
        }
    }
}

/// Proxy extension download request for a specific version to Zed's API
async fn proxy_download_version_request(extension_id: String, version: String) -> HttpResponse {
    let url = format!("https://api.zed.dev/extensions/{}/{}/download", extension_id, version);
    debug!("Proxying versioned extension download request to: {}", url);
    
    let client = reqwest::Client::new();
    match client.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let headers = resp.headers().clone();
            
            match resp.bytes().await {
                Ok(bytes) => {
                    let mut builder = HttpResponse::build(status);
                    
                    // Copy relevant headers
                    for (key, value) in headers.iter() {
                        if let Ok(header_value) = http::header::HeaderValue::from_bytes(value.as_bytes()) {
                            builder.append_header((key.clone(), header_value));
                        }
                    }
                    
                    builder.body(bytes)
                },
                Err(e) => {
                    error!("Failed to get response body from proxy request: {}", e);
                    HttpResponse::InternalServerError().body(format!("Proxy error: {}", e))
                }
            }
        },
        Err(e) => {
            error!("Failed to proxy extension version download request: {}", e);
            HttpResponse::InternalServerError().body(format!("Proxy error: {}", e))
        }
    }
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
            return read_version_file(platform_version_file, os.clone(), arch.clone(), &asset, data.config.domain.as_ref().map(|x| x.as_str()));
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
fn read_version_file(file_path: PathBuf, os: String, arch: String, asset: &str, domain: Option<&str>) -> HttpResponse {
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
                        
                        // If domain is provided, prepend it to the local path
                        let full_url = if let Some(domain) = domain {
                            format!("{}{}", domain, local_path)
                        } else {
                            local_path
                        };
                        
                        debug!("Using local file path: {}", full_url);
                        version.url = full_url;
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
    
    // Handle paths that start with api/releases/stable/ or releases/stable/
    if path_str.starts_with("api/releases/stable/") || path_str.starts_with("releases/stable/") {
        // Remove the api/ prefix if present
        let clean_path = path_str.trim_start_matches("api/");
        
        // Split the path into components
        let parts: Vec<&str> = clean_path.split('/').collect();
        if parts.len() >= 4 {
            let version = parts[2];
            let filename = parts[3];
            
            // Try to find the file in the releases directory
            if let Some(releases_dir) = &data.config.releases_dir {
                // First try the zed directory
                let zed_path = releases_dir.join("zed").join(format!("zed-{}-{}.gz", version, filename.replace(".tar.gz", "")));
                if zed_path.exists() {
                    return serve_release_file(&zed_path);
                }
                
                // Then try the zed-remote-server directory
                let remote_server_path = releases_dir.join("zed-remote-server").join(format!("zed-remote-server-{}-{}.gz", version, filename.replace(".tar.gz", "")));
                if remote_server_path.exists() {
                    return serve_release_file(&remote_server_path);
                }
            }
        }
    }
    
    // Skip proxying requests for release files if we have a releases directory
    if data.config.releases_dir.is_some() && path_str.starts_with("releases/") && path_str != "releases/latest" {
        // Instead of returning 404, try to serve the file directly
        // The path will be something like "releases/stable/0.178.0/Zed.dmg"
        // Remove any query parameters from the path
        let clean_path = path_str.split('?').next().unwrap_or(&path_str);
        let file_path = data.config.releases_dir.as_ref().unwrap()
            .join(clean_path.trim_start_matches("releases/"));
        debug!("Attempting to serve release file from: {:?}", file_path);
        
        if file_path.exists() {
            return serve_release_file(&file_path);
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

// Helper function to serve a release file with appropriate content type
fn serve_release_file(file_path: &PathBuf) -> HttpResponse {
    match fs::read(file_path) {
        Ok(bytes) => {
            // Determine content type based on file extension
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
            HttpResponse::Ok()
                .content_type(content_type)
                .body(bytes)
        },
        Err(e) => {
            error!("Error reading release file: {}", e);
            HttpResponse::InternalServerError().body(format!("Error reading release file: {}", e))
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
                    // Apply all filters using the consolidated function
                    let filtered_extensions = filter_extensions_with_params(
                        &extensions,
                        None, // No text filter for updates
                        min_schema_version,
                        max_schema_version,
                        min_wasm_api_version,
                        max_wasm_api_version,
                        None, // No provides filter for updates
                        if extension_ids.is_empty() { None } else { Some(&extension_ids) },
                    );
                    
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

/// Get all versions of a specific extension by ID
async fn get_extension_versions(
    path: web::Path<String>,
    data: web::Data<ServerData>
) -> impl Responder {
    let id = path.into_inner();
    let ext_dir = data.config.extensions_dir.join(&id);
    let versions_file = ext_dir.join("versions.json");
    
    debug!("Attempting to serve versions for extension id: {}", id);
    
    if versions_file.exists() {
        match fs::read_to_string(&versions_file) {
            Ok(content) => {
                match serde_json::from_str::<WrappedExtensions>(&content) {
                    Ok(extensions) => {
                        info!("Successfully served {} versions for extension: {}", extensions.data.len(), id);
                        HttpResponse::Ok().json(extensions)
                    },
                    Err(e) => {
                        error!("Error parsing versions.json for {}: {}", id, e);
                        HttpResponse::InternalServerError().body(format!("Error parsing versions file: {}", e))
                    }
                }
            },
            Err(e) => {
                error!("Error reading versions.json for {}: {}", id, e);
                HttpResponse::InternalServerError().body(format!("Error reading versions file: {}", e))
            }
        }
    } else if data.config.proxy_mode {
        // In proxy mode, forward the request to Zed's API
        info!("Extension versions file not found for {}. Proxying request in proxy mode.", id);
        proxy_extension_versions(id).await
    } else {
        error!("Extension versions file not found for {}: {:?}", id, versions_file);
        HttpResponse::NotFound().body(format!("Extension versions not found for: {}", id))
    }
}

/// Proxy request for extension versions to Zed's API
async fn proxy_extension_versions(extension_id: String) -> HttpResponse {
    let url = format!("https://api.zed.dev/extensions/{}", extension_id);
    debug!("Proxying extension versions request to: {}", url);
    
    let client = reqwest::Client::new();
    match client.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let headers = resp.headers().clone();
            
            match resp.bytes().await {
                Ok(bytes) => {
                    let mut builder = HttpResponse::build(status);
                    
                    // Copy relevant headers
                    for (key, value) in headers.iter() {
                        if let Ok(header_value) = http::header::HeaderValue::from_bytes(value.as_bytes()) {
                            builder.append_header((key.clone(), header_value));
                        }
                    }
                    
                    builder.body(bytes)
                },
                Err(e) => {
                    error!("Failed to get response body from proxy request: {}", e);
                    HttpResponse::InternalServerError().body(format!("Proxy error: {}", e))
                }
            }
        },
        Err(e) => {
            error!("Failed to proxy extension versions request: {}", e);
            HttpResponse::InternalServerError().body(format!("Proxy error: {}", e))
        }
    }
}

/// Proxy extension download request to Zed's API
async fn proxy_download_request(extension_id: String) -> HttpResponse {
    let url = format!("https://api.zed.dev/extensions/{}/download?min_schema_version=0&max_schema_version=100&min_wasm_api_version=0.0.0&max_wasm_api_version=100.0.0", extension_id);
    debug!("Proxying extension download request to: {}", url);
    
    let client = reqwest::Client::new();
    match client.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            let headers = resp.headers().clone();
            
            match resp.bytes().await {
                Ok(bytes) => {
                    let mut builder = HttpResponse::build(status);
                    
                    // Copy relevant headers
                    for (key, value) in headers.iter() {
                        if let Ok(header_value) = http::header::HeaderValue::from_bytes(value.as_bytes()) {
                            builder.append_header((key.clone(), header_value));
                        }
                    }
                    
                    builder.body(bytes)
                },
                Err(e) => {
                    error!("Failed to get response body from proxy request: {}", e);
                    HttpResponse::InternalServerError().body(format!("Proxy error: {}", e))
                }
            }
        },
        Err(e) => {
            error!("Failed to proxy extension download request: {}", e);
            HttpResponse::InternalServerError().body(format!("Proxy error: {}", e))
        }
    }
}
