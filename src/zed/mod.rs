mod extension;
mod error;
mod version;
mod local_server;

pub use extension::{Extensions, WrappedExtensions, ExtensionVersionTracker};
pub use extension::Extension;
pub use extension::extensions_utils;
pub use version::Version;
pub use local_server::{LocalServer, ServerConfig};
pub use error::ZedError;

use anyhow::Result;
use std::env;
use log::{debug, info, error};
use std::sync::Arc;

/// Client configuration for interacting with Zed's API
#[derive(Clone)]
pub struct Client {
    api_host: String,
    host: String,
    max_schema_version: i32,
    extensions_local_dir: Option<String>,
    platform_os: Option<String>,
    platform_arch: Option<String>,
    http_client: Arc<reqwest::Client>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// Creates a new client with default configuration
    pub fn new() -> Self {
        let http_client = reqwest::Client::builder()
            .user_agent("zedex")
            .build()
            .expect("Failed to create HTTP client");
            
        Self {
            api_host: std::env::var("ZED_API_HOST").unwrap_or_else(|_| "https://api.zed.dev".to_string()),
            host: std::env::var("ZED_HOST").unwrap_or_else(|_| "https://zed.dev".to_string()),
            max_schema_version: 1, // Default max schema version
            extensions_local_dir: None,
            platform_os: None,
            platform_arch: None,
            http_client: Arc::new(http_client),
        }
    }

    /// Set the local directory for extension storage
    pub fn with_extensions_local_dir(mut self, dir: String) -> Self {
        self.extensions_local_dir = Some(dir);
        self
    }

    /// Set the platform OS and architecture 
    pub fn with_platform(mut self, os: String, arch: String) -> Self {
        self.platform_os = Some(os);
        self.platform_arch = Some(arch);
        self
    }

    /// Get the current extensions index, optionally filtering by a capability
    pub async fn get_extensions_index(&self, provides: Option<&str>) -> Result<Extensions> {
        // Build base URL
        let mut url = format!(
            "{}/extensions?max_schema_version={}&include_native=false",
            self.api_host, self.max_schema_version
        );
        // Append provides filter if present
        if let Some(cap) = provides {
            url.push_str(&format!("&provides={}", cap));
        }
        info!("Fetching extensions index from URL: {}", url);
        // Send request
        let response = self.http_client
            .get(&url)
            .send()
            .await?
            .error_for_status()?;
        // Parse and return data
        let wrapped: WrappedExtensions = response.json().await?;
        Ok(wrapped.data)
    }

    /// Download an extension archive with progress reporting
    pub async fn download_extension_archive_with_progress(
        &self, 
        extension_id: &str, 
        min_schema_version: i32,
        progress_callback: impl Fn(u64, u64) + 'static
    ) -> Result<Vec<u8>> {
        let url = format!(
            "{}/extensions/{}/download?min_schema_version={}&max_schema_version={}&min_wasm_api_version=0.0.0&max_wasm_api_version=100.0.0",
            self.api_host, extension_id, min_schema_version, self.max_schema_version
        );
        
        debug!("Requesting extension from URL: {}", url);
        
        let response = match self.http_client
            .get(&url)
            .send()
            .await {
                Ok(resp) => {
                    debug!("Received response with status: {}", resp.status());
                    match resp.error_for_status() {
                        Ok(r) => r,
                        Err(e) => {
                            error!("Error status from response: {}", e);
                            return Err(anyhow::anyhow!("Request failed: {}", e));
                        }
                    }
                },
                Err(e) => {
                    error!("Error sending request: {}", e);
                    return Err(anyhow::anyhow!("Request failed: {}", e));
                }
            };
            
        let total_size = response.content_length().unwrap_or(0);
        let mut downloaded: u64 = 0;
        let mut bytes = Vec::new();
        
        let mut stream = response.bytes_stream();
        use futures_util::StreamExt;
        
        while let Some(item) = stream.next().await {
            let chunk = item?;
            downloaded += chunk.len() as u64;
            bytes.extend_from_slice(&chunk);
            progress_callback(downloaded, total_size);
        }
        
        debug!("Downloaded {} bytes", bytes.len());
        Ok(bytes)
    }
    
    /// Download an extension archive with progress reporting using default schema version
    pub async fn download_extension_archive_default_with_progress(
        &self, 
        extension_id: &str,
        progress_callback: impl Fn(u64, u64) + 'static
    ) -> Result<Vec<u8>> {
        self.download_extension_archive_with_progress(extension_id, 0, progress_callback).await
    }
    
    /// Get the latest Zed version information for the current platform
    pub async fn get_latest_version(&self) -> Result<Version> {
        self.get_latest_release_version("zed").await
    }

    /// Get the latest Zed Remote Server version information for the current platform
    pub async fn get_latest_remote_server_version(&self) -> Result<Version> {
        self.get_latest_release_version("zed-remote-server").await
    }

    /// Get the latest version information for a specific Zed asset
    pub async fn get_latest_release_version(&self, asset: &str) -> Result<Version> {
        // Determine OS and architecture
        let os = self.platform_os.clone().unwrap_or_else(|| env::consts::OS.to_string());
        let arch = self.platform_arch.clone().unwrap_or_else(|| {
            if env::consts::ARCH == "x86_64" {
                "x86_64".to_string()
            } else {
                env::consts::ARCH.to_string()
            }
        });
        
        // Check if platform is Windows
        if os == "windows" {
            return Err(ZedError::PlatformNotSupported("Windows is not yet supported by Zed. Currently, Zed is only available for macOS and Linux.".to_string()).into());
        }
        
        let url = format!("{}/api/releases/latest?asset={}&os={}&arch={}", self.host, asset, os, arch);
        debug!("Fetching latest version information from URL: {}", url);
        
        let response = self.http_client
            .get(&url)
            .send()
            .await?
            .error_for_status()?;

        let version: Version = response.json().await?;
        Ok(version)
    }

    /// Download a release asset with progress reporting
    pub async fn download_release_asset_with_progress(
        &self,
        version: &Version,
        progress_callback: impl Fn(u64, u64) + 'static
    ) -> Result<Vec<u8>> {
        debug!("Downloading release asset from URL: {}", version.url);
        
        let response = match self.http_client
            .get(&version.url)
            .send()
            .await {
                Ok(resp) => {
                    debug!("Received response with status: {}", resp.status());
                    match resp.error_for_status() {
                        Ok(r) => r,
                        Err(e) => {
                            error!("Error status from response: {}", e);
                            return Err(anyhow::anyhow!("Request failed: {}", e));
                        }
                    }
                },
                Err(e) => {
                    error!("Error sending request: {}", e);
                    return Err(anyhow::anyhow!("Request failed: {}", e));
                }
            };
            
        let total_size = response.content_length().unwrap_or(0);
        let mut downloaded: u64 = 0;
        let mut bytes = Vec::new();
        
        let mut stream = response.bytes_stream();
        use futures_util::StreamExt;
        
        while let Some(item) = stream.next().await {
            let chunk = item?;
            downloaded += chunk.len() as u64;
            bytes.extend_from_slice(&chunk);
            progress_callback(downloaded, total_size);
        }
        
        debug!("Downloaded {} bytes", bytes.len());
        Ok(bytes)
    }

    /// Get all versions of a specific extension
    pub async fn get_extension_versions(&self, extension_id: &str) -> Result<Extensions> {
        let url = format!("{}/extensions/{}", self.api_host, extension_id);
        
        debug!("Fetching all versions for extension {} from URL: {}", extension_id, url);
        
        let response = self.http_client
            .get(&url)
            .send()
            .await?
            .error_for_status()?;

        let wrapped: WrappedExtensions = response.json().await?;
        Ok(wrapped.data)
    }

    /// Download a specific version of an extension archive with progress reporting
    pub async fn download_extension_version_with_progress(
        &self, 
        extension_id: &str,
        version: &str,
        progress_callback: impl Fn(u64, u64) + 'static
    ) -> Result<Vec<u8>> {
        let url = format!(
            "{}/extensions/{}/{}/download",
            self.api_host, extension_id, version
        );
        
        debug!("Requesting specific extension version from URL: {}", url);
        
        let response = match self.http_client
            .get(&url)
            .send()
            .await {
                Ok(resp) => {
                    debug!("Received response with status: {}", resp.status());
                    match resp.error_for_status() {
                        Ok(r) => r,
                        Err(e) => {
                            error!("Error status from response: {}", e);
                            return Err(anyhow::anyhow!("Request failed: {}", e));
                        }
                    }
                },
                Err(e) => {
                    error!("Error sending request: {}", e);
                    return Err(anyhow::anyhow!("Request failed: {}", e));
                }
            };
            
        let total_size = response.content_length().unwrap_or(0);
        let mut downloaded: u64 = 0;
        let mut bytes = Vec::new();
        
        let mut stream = response.bytes_stream();
        use futures_util::StreamExt;
        
        while let Some(item) = stream.next().await {
            let chunk = item?;
            downloaded += chunk.len() as u64;
            bytes.extend_from_slice(&chunk);
            progress_callback(downloaded, total_size);
        }
        
        debug!("Downloaded {} bytes for extension {} version {}", bytes.len(), extension_id, version);
        Ok(bytes)
    }
}