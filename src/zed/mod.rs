mod extension;
mod error;
mod version;
mod local_server;
mod downloader;
mod health;

pub use extension::{Extensions, WrappedExtensions, ExtensionVersionTracker};
pub use extension::Extension;
pub use extension::extensions_utils;
pub use version::Version;
pub use local_server::{LocalServer, ServerConfig};
pub use downloader::{download_extensions, download_extension_by_id, download_extension_index, download_zed_release, DownloadOptions};

use anyhow::Result;
use log::{debug, info, error};
use std::sync::Arc;

/// Client configuration for interacting with Zed's API
#[derive(Clone)]
pub struct Client {
    api_host: String,
    host: String,
    max_schema_version: i32,
    extensions_local_dir: Option<String>,
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
            http_client: Arc::new(http_client),
        }
    }

    /// Set the local directory for extension storage
    pub fn with_extensions_local_dir(mut self, dir: String) -> Self {
        self.extensions_local_dir = Some(dir);
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