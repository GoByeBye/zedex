use anyhow::Result;
use futures_util::future;
use indicatif::{ProgressBar, ProgressStyle};
use log::{debug, error, info};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

use crate::zed::{Client, Extension, ExtensionVersionTracker, WrappedExtensions};

/// Options for downloading extensions
#[derive(Clone, Copy)]
pub struct DownloadOptions {
    pub async_mode: bool,
    pub all_versions: bool,
    pub rate_limit: u64,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            async_mode: false,
            all_versions: false,
            rate_limit: 0,
        }
    }
}

/// Downloads extensions with given options
pub async fn download_extensions(
    extensions: Vec<Extension>,
    client: Client,
    output_dir: impl AsRef<Path>,
    mut version_tracker: ExtensionVersionTracker,
    options: DownloadOptions,
) -> Result<ExtensionVersionTracker> {
    let output_dir = output_dir.as_ref().to_path_buf();
    
    info!(
        "Downloading {} extensions{}...", 
        extensions.len(), 
        if options.all_versions { " (all versions)" } else { " (latest version only)" }
    );
    
    if options.async_mode {
        // Fully asynchronous mode - no throttling
        info!("Using fully asynchronous mode - be careful of rate limiting!");
        
        // Download each extension without throttling
        let futures = extensions.iter().map(|extension| {
            download_extension(
                extension.clone(),
                client.clone(),
                output_dir.clone(),
                options.all_versions,
                options.rate_limit,
                version_tracker.clone(),
            )
        });
        
        // Wait for all downloads to complete (fully parallel)
        let results = future::join_all(futures).await;
        
        // Merge all trackers
        for result in results {
            if let Ok(tracker) = result {
                version_tracker.merge(tracker);
            }
        }    } else {
        // Throttled mode - default safe behavior
        info!("Using throttled download mode to avoid rate limiting");
        
        // Create a semaphore to limit concurrent downloads
        const MAX_CONCURRENT_DOWNLOADS: usize = 1;
        let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_DOWNLOADS));
        
        // Download each extension with throttling
        let mut handles = Vec::new();
        
        for extension in extensions.iter() {
            let ext_client = client.clone();
            let ext_output_dir = output_dir.clone();
            let semaphore = semaphore.clone();            let extension_clone = extension.clone();
            let all_versions = options.all_versions;
            let rate_limit = options.rate_limit;
            let tracker = version_tracker.clone();
            
            let handle = tokio::spawn(async move {
                // Acquire a permit from the semaphore (this limits concurrency)
                let _permit = semaphore.acquire().await.unwrap();
                
                download_extension(
                    extension_clone, 
                    ext_client, 
                    ext_output_dir, 
                    all_versions, 
                    rate_limit, 
                    tracker,
                ).await
            });
            
            handles.push(handle);
        }
        
        // Wait for all downloads to complete
        for handle in handles {
            if let Ok(Ok(tracker)) = handle.await {
                version_tracker.merge(tracker);
            }
        }
    }
    
    Ok(version_tracker)
}

/// Downloads a single extension (and its versions if requested)
async fn download_extension(
    extension: Extension,
    client: Client,
    output_dir: impl AsRef<Path>,
    all_versions: bool,
    rate_limit: u64,
    mut version_tracker: ExtensionVersionTracker,
) -> Result<ExtensionVersionTracker> {
    let output_dir = output_dir.as_ref().to_path_buf();
    let id = extension.id.clone();
    
    // Create extension-specific directory
    let ext_dir = output_dir.join(&id);
    if !ext_dir.exists() {
        if let Err(e) = fs::create_dir_all(&ext_dir) {
            error!("Failed to create directory {:?}: {}", ext_dir, e);
            return Ok(version_tracker);
        }
    }
    
    if all_versions {
        // Fetch all versions of this extension
        let versions = client.get_extension_versions(&id).await?;
        
        // Save versions metadata
        let versions_file = ext_dir.join("versions.json");
        let versions_json = serde_json::to_string_pretty(&WrappedExtensions { data: versions.clone() })?;
        fs::write(&versions_file, versions_json)?;
        
        // Download each version
        for version in versions.iter() {
            let file_path = ext_dir.join(format!("{}-{}.tgz", id, version.version));
            
            // Skip if already downloaded
            if file_path.exists() {
                debug!("Extension {} version {} already downloaded, skipping", id, version.version);
                // Update version tracker
                version_tracker.update_extension(version);
                continue;
            }
            
            info!("Downloading extension: {} version {}", id, version.version);
            
            // Create a progress bar for this download
            let pb = Arc::new(ProgressBar::new(0));
            pb.set_style(ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap()
                .progress_chars("#>-"));
            
            let pb_clone = pb.clone();
            match client.download_extension_version_with_progress(&id, &version.version, 
                move |downloaded, total| {
                    pb_clone.set_length(total);
                    pb_clone.set_position(downloaded);
                }).await {
                Ok(bytes) => {
                    pb.finish_with_message(format!("Downloaded {} v{}", id, version.version));
                    match std::fs::write(&file_path, bytes) {
                        Ok(_) => {
                            info!("Successfully downloaded extension: {} version {} to {:?}", id, version.version, file_path);
                            // Update version tracker
                            version_tracker.update_extension(version);
                        },
                        Err(e) => error!("Failed to write extension file {}: {}", id, e),
                    }
                },
                Err(e) => {
                    pb.finish_with_message(format!("Failed to download {} v{}", id, version.version));
                    if let Some(err) = e.downcast_ref::<reqwest::Error>() {
                        error!("Failed to download extension {} version {}: {}", id, version.version, err);
                    } else {
                        error!("Failed to download extension {} version {}: {}", id, version.version, e);
                    }
                },
            }
            
            // Apply rate limiting between downloads
            if rate_limit > 0 {
                tokio::time::sleep(Duration::from_secs(rate_limit)).await;
            }
        }
    } else {
        // Download only the latest version
        let file_path = ext_dir.join(format!("{}.tgz", id));
        
        // Skip if already downloaded and version hasn't changed
        if file_path.exists() && !version_tracker.has_newer_version(&extension) {
            debug!("Extension {} latest version already downloaded, skipping", id);
            return Ok(version_tracker);
        }
        
        info!("Downloading extension: {}", id);
        
        // Create a progress bar for this download
        let pb = Arc::new(ProgressBar::new(0));
        pb.set_style(ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("#>-"));
        
        let pb_clone = pb.clone();
        match client.download_extension_version_with_progress(&id, &extension.version, 
            move |downloaded, total| {
                pb_clone.set_length(total);
                pb_clone.set_position(downloaded);
            }).await {
            Ok(bytes) => {
                pb.finish_with_message(format!("Downloaded {}", id));
                match std::fs::write(&file_path, bytes) {
                    Ok(_) => {
                        info!("Successfully downloaded extension: {} to {:?}", id, file_path);
                        // Update version tracker
                        version_tracker.update_extension(&extension);
                    },
                    Err(e) => error!("Failed to write extension file {}: {}", id, e),
                }
            },
            Err(e) => {
                pb.finish_with_message(format!("Failed to download {}", id));
                if let Some(err) = e.downcast_ref::<reqwest::Error>() {
                    error!("Failed to download extension {}: {}", id, err);
                } else {
                    error!("Failed to download extension {}: {}", id, e);
                }
            },
        }
    }
    
    Ok(version_tracker)
}

/// Downloads a single extension by ID
pub async fn download_extension_by_id(
    id: &str, 
    client: Client, 
    output_dir: impl AsRef<Path>,
    extensions: &[Extension],
) -> Result<()> {
    let output_dir = output_dir.as_ref().to_path_buf();
    
    // Find the extension in the index to get its metadata
    let extension = extensions.iter().find(|e| e.id == id);
    
    if let Some(extension) = extension {
        info!("Downloading extension: {} (version {})", id, extension.version);
        
        // Create extension-specific directory
        let ext_dir = output_dir.join(id);
        if !ext_dir.exists() {
            if let Err(e) = fs::create_dir_all(&ext_dir) {
                error!("Failed to create directory {:?}: {}", ext_dir, e);
                return Ok(());
            }
        }
        
        // Create a progress bar for this download
        let pb = Arc::new(ProgressBar::new(0));
        pb.set_style(ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("#>-"));
        
        let pb_clone = pb.clone();
        let file_path = ext_dir.join(format!("{}.tgz", id));
        
        match client.download_extension_version_with_progress(id, &extension.version, 
            move |downloaded, total| {
                pb_clone.set_length(total);
                pb_clone.set_position(downloaded);
            }).await {
            Ok(bytes) => {
                pb.finish_with_message(format!("Downloaded {}", id));
                match std::fs::write(&file_path, bytes) {
                    Ok(_) => info!("Successfully downloaded extension: {} to {:?}", id, file_path),
                    Err(e) => error!("Failed to write extension file {}: {}", id, e),
                }
            },
            Err(e) => {
                pb.finish_with_message(format!("Failed to download {}", id));
                if let Some(err) = e.downcast_ref::<reqwest::Error>() {
                    error!("Failed to download extension {}: {}", id, err);
                } else {
                    error!("Failed to download extension {}: {}", id, e);
                }
            },
        }
    } else {
        error!("Extension {} not found in index", id);
    }
    
    Ok(())
}

/// Downloads an extension index based on provided filter criteria and saves it to a file
pub async fn download_extension_index(
    client: &Client,
    root_dir: impl AsRef<Path>,
    provides: &[String]
) -> Result<Vec<Extension>> {
    let root_dir = root_dir.as_ref();
    let mut map: HashMap<String, Extension> = HashMap::new();
    
    // Fetch and merge extension lists, deduplicating by id
    if provides.is_empty() {
        // Initial fetch to discover all provides capabilities
        let initial_exts = client.get_extensions_index(None).await?;
        // Insert initial extensions
        for ext in initial_exts.iter() {
            map.insert(ext.id.clone(), ext.clone());
        }
        // Collect unique provides capabilities
        let mut caps = HashSet::new();
        for ext in initial_exts {
            for cap in &ext.provides {
                caps.insert(cap.clone());
            }
        }
        // Fetch and merge by each capability
        for cap in caps {
            let exts = client.get_extensions_index(Some(cap.as_str())).await?;
            for ext in exts {
                map.insert(ext.id.clone(), ext);
            }
        }
    } else {
        // Fetch only for specified provides
        for prov in provides {
            let exts = client.get_extensions_index(Some(prov.as_str())).await?;
            for ext in exts {
                map.insert(ext.id.clone(), ext);
            }
        }
    }
    
    let mut extensions: Vec<Extension> = map.into_values().collect();
    // Sort extensions by download count (highest first)
    extensions.sort_by(|a, b| b.download_count.cmp(&a.download_count));
    info!("Found {} extensions", extensions.len());
    
    // Save extensions to file
    std::fs::create_dir_all(root_dir)?;
    let extension_path = root_dir.join("extensions.json");
    let wrapped = WrappedExtensions { data: extensions.clone() };
    let json = serde_json::to_string_pretty(&wrapped)?;
    std::fs::write(&extension_path, json)?;
    info!("Saved extension index to {:?}", extension_path);
    
    Ok(extensions)
}

// Downloads the latest Zed release for supported platforms
pub async fn download_zed_release(client: &Client, root_dir: impl AsRef<Path>) {
    let platforms = [
        // TODO: Add windows when windows support is implemented
        ("zed", "linux", "x86_64"),
        ("zed-remote-server", "linux", "x86_64"),
        ("zed", "linux", "aarch64"),
        ("zed-remote-server", "linux", "aarch64"),
        ("zed", "macos", "x86_64"),
        ("zed-remote-server", "macos", "x86_64"),
        ("zed", "macos", "aarch64"),
    ];


    for (asset,os, arch) in platforms {
        let url = format!(
            "{}/api/releases/latest?asset={}&os={}&arch={}",
            client.host, asset, os, arch
        );
        info!("Downloading Zed release from {}", url);
        // response from server would be {"version":"0.187.8","url":"https://zed.dev/api/releases/stable/0.187.8/zed-linux-x86_64.tar.gz?update=1"}
        let response = client.http_client.get(&url).send().await;

        match response {
            Ok(resp) => {
                if resp.status().is_success() {
                    let release: serde_json::Value = resp.json().await.unwrap();
                    let version: &str = release["version"].as_str().unwrap_or("unknown");
                    let download_url: &str = release["url"].as_str().unwrap_or("");
                    let releases_path = root_dir.as_ref().join("releases");
                    
                    info!("Latest Zed version: {}", version);
                    info!("Download URL: {}", download_url);
                    
                    // Create output directory if it doesn't exist
                    let output_dir = root_dir.as_ref().join("releases").join(version);

                    if !releases_path.exists() {
                        std::fs::create_dir_all(&releases_path).unwrap();
                    }
                    let cache_file = releases_path.join(format!("{}-{}-{}.json", asset, os, arch));
                    let cache_content = serde_json::to_string(&release).unwrap();
                    std::fs::write(&cache_file, cache_content).unwrap();
                    info!("Zed release cache saved to {:?}", cache_file);

                    std::fs::create_dir_all(&output_dir).unwrap();
                    
                    // Download the file
                    let file_path = output_dir.join(format!("{}-{}-{}.tar.gz", asset, os, arch));
                    let download_result = client.http_client.get(download_url).send().await;
                    match download_result {
                        Ok(resp) => {
                            let bytes_result = resp.bytes().await;
                            match bytes_result {
                                Ok(bytes) => {
                                    use std::io::Write;
                                    match std::fs::File::create(&file_path) {
                                        Ok(mut file) => {
                                            if let Err(e) = file.write_all(&bytes) {
                                                error!("Failed to write Zed release to file: {}", e);
                                            } else {
                                                info!("Zed release downloaded to {:?}", file_path);
                                            }
                                        }
                                        Err(e) => {
                                            error!("Failed to create file for Zed release: {}", e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to read bytes from Zed release response: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to download Zed release: {}", e);
                        }
                    }
                } else {
                    error!("Failed to fetch latest Zed release: {}", resp.status());
                }
            },
            Err(e) => error!("Error fetching latest Zed release: {}", e),
        }
    }
}