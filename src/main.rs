mod zed;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use log::{debug, error, info, LevelFilter};
use env_logger::Builder;
use indicatif::{ProgressBar, ProgressStyle};
use std::sync::Arc;
use std::io::Write;
use std::time::Duration;
use std::fs;

#[derive(Parser)]
#[clap(author, version, about = "Zed Extension Mirror")]
struct Cli {
    /// Root directory for all cache files
    #[clap(long, default_value = ".zedex-cache")]
    root_dir: PathBuf,
    
    /// Log level: trace, debug, info, warn, error
    #[clap(long, default_value = "info")]
    log_level: String,
    
    /// Enable timestamp in logs
    #[clap(long)]
    log_timestamp: bool,
    
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Fetch extensions
    Get {
        #[clap(subcommand)]
        target: GetTarget,
    },
    
    /// Get latest Zed release information or download releases
    Release {
        #[clap(subcommand)]
        target: ReleaseTarget,
    },

    /// Start a local server to serve Zed extensions API
    Serve {
        /// Port to run the server on
        #[clap(long, default_value = "2654")] // If you're reading this, you're a nerd. And yes it's ZED. Z=26 E=5 D=4
        port: u16,

        /// Host IP address to bind the server to
        #[clap(long, default_value = "127.0.0.1")]
        host: String,
        
        /// Directory containing extension archives and metadata
        #[clap(long)]
        extensions_dir: Option<PathBuf>,
        
        /// Directory containing release information
        #[clap(long)]
        releases_dir: Option<PathBuf>,

        /// Whether to proxy requests to zed.dev for missing content
        #[clap(long)]
        proxy_mode: bool,

        /// Domain to use in URLs (e.g. http://localhost:2654)
        #[clap(long)]
        domain: Option<String>,
    },
}

#[derive(Subcommand)]
enum GetTarget {
    /// Fetch the extension index
    ExtensionIndex,
    
    /// Fetch a specific extension by ID
    Extension {
        /// The IDs of the extensions to download
        #[clap(required = true)]
        ids: Vec<String>,
        
        /// Output directory for downloaded extensions
        #[clap(long)]
        output_dir: Option<PathBuf>,
    },

    /// Fetch all extensions listed in extensions.json
    AllExtensions {
        /// Output directory for downloaded extensions
        #[clap(long)]
        output_dir: Option<PathBuf>,
        
        /// Use fully asynchronous downloads without throttling (faster but may trigger rate limiting)
        #[clap(long)]
        async_mode: bool,

        /// Whether to download all versions of each extension
        #[clap(long)]
        all_versions: bool,

        /// Rate limit between API requests in seconds (to avoid overwhelming the server)
        #[clap(long, default_value = "10")]
        rate_limit: u64,
    },
}

#[derive(Subcommand)]
enum ReleaseTarget {
    /// Get the latest Zed release version info (does not download the file)
    Latest,
    
    /// Get the latest Zed Remote Server release version info (does not download the file)
    RemoteServerLatest,
    
    /// Download the latest Zed release
    Download {
        /// Output directory for downloaded release
        #[clap(long)]
        output_dir: Option<PathBuf>,
    },
    
    /// Download the latest Zed Remote Server release
    DownloadRemoteServer {
        /// Output directory for downloaded remote server release
        #[clap(long)]
        output_dir: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    
    // Initialize logger with user-specified configuration
    let mut builder = Builder::new();
    
    // Set log level from command line
    let log_level = match cli.log_level.as_str() {
        "trace" => LevelFilter::Trace,
        "debug" => LevelFilter::Debug,
        "info" => LevelFilter::Info,
        "warn" => LevelFilter::Warn,
        "error" => LevelFilter::Error,
        _ => LevelFilter::Info,
    };
    
    builder.filter_level(log_level);
    
    // Configure format with optional timestamp
    if cli.log_timestamp {
        builder.format(|buf, record| {
            writeln!(
                buf,
                "{} [{}] - {}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                record.args()
            )
        });
    } else {
        builder.format(|buf, record| {
            writeln!(
                buf,
                "[{}] - {}",
                record.level(),
                record.args()
            )
        });
    }
    
    builder.init();
    
    // Log the startup information
    info!("Starting Zed Extension Mirror");
    debug!("Using root directory: {:?}", cli.root_dir);
    
    let root_dir = cli.root_dir.clone();

    match cli.command {
        Commands::Get { target } => match target {
            GetTarget::ExtensionIndex => {
                let client = zed::Client::new();
                let extensions = client.get_extensions_index().await?;
                info!("Found {} extensions", extensions.len());
                
                // Save extensions to file
                std::fs::create_dir_all(&root_dir)?;
                let extension_path = root_dir.join("extensions.json");
                let wrapped = zed::WrappedExtensions { data: extensions };
                let json = serde_json::to_string_pretty(&wrapped)?;
                std::fs::write(extension_path, json)?;
                info!("Saved extension index to {:?}", root_dir.join("extensions.json"));
            },
            GetTarget::Extension { ids, output_dir } => {
                // Resolve output directory from root_dir if not specified
                let output_dir = output_dir.unwrap_or_else(|| root_dir.clone());
                
                // Create output directory
                std::fs::create_dir_all(&output_dir)?;
                
                // Create a client with the output directory set
                let client = zed::Client::new()
                    .with_extensions_local_dir(output_dir.to_string_lossy().to_string());
                
                // Get the extension index to get metadata for the extensions
                let extensions_file = output_dir.join("extensions.json");
                let extensions = if extensions_file.exists() {
                    info!("Loading extension index from {:?}", extensions_file);
                    let content = std::fs::read_to_string(&extensions_file)?;
                    let wrapped: zed::WrappedExtensions = serde_json::from_str(&content)?;
                    wrapped.data
                } else {
                    info!("Extension index not found. Fetching from API...");
                    let extensions = client.get_extensions_index().await?;
                    info!("Found {} extensions", extensions.len());
                    
                    // Save extensions to file
                    let wrapped = zed::WrappedExtensions { data: extensions.clone() };
                    let json = serde_json::to_string_pretty(&wrapped)?;
                    std::fs::write(&extensions_file, json)?;
                    info!("Saved extension index to {:?}", extensions_file);
                    extensions
                };
                
                // Download each extension with a progress bar
                let futures = ids.iter().map(|id| {
                    let id = id.clone();
                    let client = client.clone();
                    let output_dir = output_dir.clone();
                    let extensions = extensions.clone();
                    
                    async move {
                        // Find the extension in the index to get its metadata
                        let extension = extensions.iter().find(|e| e.id == id);
                        
                        if let Some(extension) = extension {
                            info!("Downloading extension: {} (version {})", id, extension.version);
                            
                            // Create extension-specific directory
                            let ext_dir = output_dir.join(&id);
                            if !ext_dir.exists() {
                                if let Err(e) = fs::create_dir_all(&ext_dir) {
                                    error!("Failed to create directory {:?}: {}", ext_dir, e);
                                    return;
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
                            
                            match client.download_extension_version_with_progress(&id, &extension.version, 
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
                    }
                });
                
                // Wait for all downloads to complete
                futures_util::future::join_all(futures).await;
            },
            GetTarget::AllExtensions { output_dir, async_mode, all_versions, rate_limit } => {
                // Resolve output directory from root_dir if not specified
                let output_dir = output_dir.unwrap_or_else(|| root_dir.clone());
                
                // Create output directory
                std::fs::create_dir_all(&output_dir)?;
                
                // Create a client
                let client = zed::Client::new()
                    .with_extensions_local_dir(output_dir.to_string_lossy().to_string());
                
                // Get the extension index
                let extensions_file = output_dir.join("extensions.json");
                let extensions = if extensions_file.exists() {
                    info!("Loading extension index from {:?}", extensions_file);
                    let content = std::fs::read_to_string(&extensions_file)?;
                    let wrapped: zed::WrappedExtensions = serde_json::from_str(&content)?;
                    wrapped.data
                } else {
                    info!("Extension index not found. Fetching from API...");
                    let extensions = client.get_extensions_index().await?;
                    info!("Found {} extensions", extensions.len());
                    
                    // Save extensions to file
                    let wrapped = zed::WrappedExtensions { data: extensions.clone() };
                    let json = serde_json::to_string_pretty(&wrapped)?;
                    std::fs::write(&extensions_file, json)?;
                    info!("Saved extension index to {:?}", extensions_file);
                    extensions
                };
                
                // Load or create the version tracker
                let version_tracker_file = output_dir.join("version_tracker.json");
                let mut version_tracker = if version_tracker_file.exists() {
                    info!("Loading version tracker from {:?}", version_tracker_file);
                    let content = std::fs::read_to_string(&version_tracker_file)?;
                    serde_json::from_str(&content).unwrap_or_else(|_| zed::ExtensionVersionTracker::new())
                } else {
                    zed::ExtensionVersionTracker::new()
                };
                
                info!("Downloading {} extensions{}...", 
                    extensions.len(), 
                    if all_versions { " (all versions)" } else { " (latest version only)" });
                
                if async_mode {
                    // Fully asynchronous mode - no throttling
                    info!("Using fully asynchronous mode - be careful of rate limiting!");
                    
                    // Download each extension without throttling
                    let futures = extensions.iter().map(|extension| {
                        let id = extension.id.clone();
                        let client = client.clone();
                        let output_dir = output_dir.clone();
                        let all_versions = all_versions;
                        let rate_limit = rate_limit;
                        let mut version_tracker = version_tracker.clone();
                        let extension_clone = extension.clone();
                        
                        async move {
                            // Create extension-specific directory
                            let ext_dir = output_dir.join(&id);
                            if !ext_dir.exists() {
                                if let Err(e) = fs::create_dir_all(&ext_dir) {
                                    error!("Failed to create directory {:?}: {}", ext_dir, e);
                                    return Ok(());
                                }
                            }
                            
                            if all_versions {
                                // Fetch all versions of this extension
                                let versions = client.get_extension_versions(&id).await?;
                                
                                // Save versions metadata
                                let versions_file = ext_dir.join("versions.json");
                                let versions_json = serde_json::to_string_pretty(&zed::WrappedExtensions { data: versions.clone() })?;
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
                                if file_path.exists() && !version_tracker.has_newer_version(&extension_clone) {
                                    debug!("Extension {} latest version already downloaded, skipping", id);
                                    return Ok(());
                                }
                                
                                info!("Downloading extension: {}", id);
                                
                                // Create a progress bar for this download
                                let pb = Arc::new(ProgressBar::new(0));
                                pb.set_style(ProgressStyle::default_bar()
                                    .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                                    .unwrap()
                                    .progress_chars("#>-"));
                                
                                let pb_clone = pb.clone();
                                match client.download_extension_version_with_progress(&id, &extension_clone.version, 
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
                                                version_tracker.update_extension(&extension_clone);
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
                            
                            Ok::<(), anyhow::Error>(())
                        }
                    });
                    
                    // Wait for all downloads to complete (fully parallel)
                    futures_util::future::join_all(futures).await;
                } else {
                    // Throttled mode - default safe behavior
                    info!("Using throttled download mode to avoid rate limiting");
                    
                    // Create a semaphore to limit concurrent downloads
                    const MAX_CONCURRENT_DOWNLOADS: usize = 1;
                    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_DOWNLOADS));
                    
                    // Download each extension with throttling
                    let mut handles = Vec::new();
                    
                    for extension in extensions.iter() {
                        let id = extension.id.clone();
                        let client = client.clone();
                        let output_dir = output_dir.clone();
                        let semaphore = semaphore.clone();
                        let extension_clone = extension.clone();
                        let all_versions = all_versions;
                        let rate_limit = rate_limit;
                        let mut version_tracker = version_tracker.clone();
                        
                        let handle = tokio::spawn(async move {
                            // Acquire a permit from the semaphore (this limits concurrency)
                            let _permit = semaphore.acquire().await.unwrap();
                            
                            // Create extension-specific directory
                            let ext_dir = output_dir.join(&id);
                            if !ext_dir.exists() {
                                if let Err(e) = fs::create_dir_all(&ext_dir) {
                                    error!("Failed to create directory {:?}: {}", ext_dir, e);
                                    return Ok(());
                                }
                            }
                            
                            if all_versions {
                                // Fetch all versions of this extension
                                let versions = client.get_extension_versions(&id).await?;
                                
                                // Save versions metadata
                                let versions_file = ext_dir.join("versions.json");
                                let versions_json = serde_json::to_string_pretty(&zed::WrappedExtensions { data: versions.clone() })?;
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
                                if file_path.exists() && !version_tracker.has_newer_version(&extension_clone) {
                                    debug!("Extension {} latest version already downloaded, skipping", id);
                                    return Ok(());
                                }
                                
                                info!("Downloading extension: {}", id);
                                
                                // Create a progress bar for this download
                                let pb = Arc::new(ProgressBar::new(0));
                                pb.set_style(ProgressStyle::default_bar()
                                    .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                                    .unwrap()
                                    .progress_chars("#>-"));
                                
                                let pb_clone = pb.clone();
                                match client.download_extension_version_with_progress(&id, &extension_clone.version, 
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
                                                version_tracker.update_extension(&extension_clone);
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
                            
                            Ok::<(), anyhow::Error>(())
                        });
                        
                        handles.push(handle);
                    }
                    
                    // Wait for all downloads to complete
                    for handle in handles {
                        handle.await??;
                    }
                }
                
                // Save the updated version tracker
                let version_tracker_json = serde_json::to_string_pretty(&version_tracker)?;
                fs::write(&version_tracker_file, version_tracker_json)?;
                
                info!("All extensions downloaded to {:?}", output_dir);
            },
        },
        Commands::Release { target } => match target {
            ReleaseTarget::Latest => {
                let client = zed::Client::new();
                match client.get_latest_version().await {
                    Ok(version) => {
                        println!("Latest Zed version: {}", version.version);
                        println!("Download URL: {}", version.url);
                    },
                    Err(e) => {
                        error!("Failed to get latest version: {}", e);
                    }
                }
            },
            ReleaseTarget::RemoteServerLatest => {
                let client = zed::Client::new();
                match client.get_latest_remote_server_version().await {
                    Ok(version) => {
                        println!("Latest Zed Remote Server version: {}", version.version);
                        println!("Download URL: {}", version.url);
                    },
                    Err(e) => {
                        error!("Failed to get latest remote server version: {}", e);
                    }
                }
            },
            ReleaseTarget::Download { output_dir } => {
                // Create the release cache directory structure
                let release_dir = if let Some(dir) = output_dir {
                    dir
                } else {
                    let mut dir = root_dir.clone();
                    dir.push("releases");
                    dir.push("zed");
                    dir
                };
                std::fs::create_dir_all(&release_dir)?;
                
                let client = zed::Client::new();
                
                // Define all supported platforms
                let platforms = [
                    ("linux", "x86_64"),
                    ("macos", "x86_64"),
                    ("macos", "aarch64")
                ];
                
                // Download for each platform
                for (os, arch) in platforms.iter() {
                    info!("Fetching Zed for {}-{}...", os, arch);
                    
                    // Create a client with platform set
                    let platform_client = client.clone().with_platform(os.to_string(), arch.to_string());
                    
                    match platform_client.get_latest_version().await {
                        Ok(version) => {
                            info!("Latest Zed version for {}-{}: {}", os, arch, version.version);
                            
                            // Create a progress bar for this download
                            let pb = Arc::new(ProgressBar::new(0));
                            pb.set_style(ProgressStyle::default_bar()
                                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                                .unwrap()
                                .progress_chars("#>-"));
                            
                            let pb_clone = pb.clone();
                            match platform_client.download_release_asset_with_progress(&version, 
                                move |downloaded, total| {
                                    pb_clone.set_length(total);
                                    pb_clone.set_position(downloaded);
                                }).await {
                                Ok(bytes) => {
                                    pb.finish_with_message(format!("Downloaded Zed {} for {}-{}", version.version, os, arch));
                                    
                                    // Save version info with platform-specific filename
                                    let version_file = release_dir.join(format!("latest-version-{}-{}.json", os, arch));
                                    std::fs::write(&version_file, serde_json::to_string_pretty(&version)?)?;
                                    info!("Saved version info to {:?}", version_file);
                                    
                                    // Save the release asset with platform in filename
                                    let file_path = release_dir.join(format!("zed-{}-{}-{}.gz", version.version, os, arch));
                                    match std::fs::write(&file_path, bytes) {
                                        Ok(_) => info!("Successfully downloaded Zed {} for {}-{} to {:?}", version.version, os, arch, file_path),
                                        Err(e) => error!("Failed to write release file for {}-{}: {}", os, arch, e),
                                    }
                                },
                                Err(e) => {
                                    pb.finish_with_message(format!("Failed to download for {}-{}", os, arch));
                                    error!("Failed to download release for {}-{}: {}", os, arch, e);
                                }
                            }
                        },
                        Err(e) => {
                            error!("Failed to get latest version for {}-{}: {}", os, arch, e);
                        }
                    }
                }
            },
            ReleaseTarget::DownloadRemoteServer { output_dir } => {
                // Create the release cache directory structure
                let release_dir = if let Some(dir) = output_dir {
                    dir
                } else {
                    let mut dir = root_dir.clone();
                    dir.push("releases");
                    dir.push("zed-remote-server");
                    dir
                };
                std::fs::create_dir_all(&release_dir)?;
                
                let client = zed::Client::new();
                
                // Define all supported platforms
                let platforms = [
                    ("linux", "x86_64"),
                    ("linux", "aarch64"),
                    ("macos", "x86_64"),
                    ("macos", "aarch64")
                ];
                
                // Download for each platform
                for (os, arch) in platforms.iter() {
                    info!("Fetching Zed Remote Server for {}-{}...", os, arch);
                    
                    // Create a client with platform set
                    let platform_client = client.clone().with_platform(os.to_string(), arch.to_string());
                    
                    match platform_client.get_latest_remote_server_version().await {
                        Ok(version) => {
                            info!("Latest Zed Remote Server version for {}-{}: {}", os, arch, version.version);
                            
                            // Create a progress bar for this download
                            let pb = Arc::new(ProgressBar::new(0));
                            pb.set_style(ProgressStyle::default_bar()
                                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                                .unwrap()
                                .progress_chars("#>-"));
                            
                            let pb_clone = pb.clone();
                            match platform_client.download_release_asset_with_progress(&version, 
                                move |downloaded, total| {
                                    pb_clone.set_length(total);
                                    pb_clone.set_position(downloaded);
                                }).await {
                                Ok(bytes) => {
                                    pb.finish_with_message(format!("Downloaded Zed Remote Server {} for {}-{}", version.version, os, arch));
                                    
                                    // Save version info with platform-specific filename
                                    let version_file = release_dir.join(format!("latest-version-{}-{}.json", os, arch));
                                    std::fs::write(&version_file, serde_json::to_string_pretty(&version)?)?;
                                    info!("Saved version info to {:?}", version_file);
                                    
                                    // Save the release asset with platform in filename
                                    let file_path = release_dir.join(format!("zed-remote-server-{}-{}-{}.gz", version.version, os, arch));
                                    match std::fs::write(&file_path, bytes) {
                                        Ok(_) => info!("Successfully downloaded Zed Remote Server {} for {}-{} to {:?}", version.version, os, arch, file_path),
                                        Err(e) => error!("Failed to write release file for {}-{}: {}", os, arch, e),
                                    }
                                },
                                Err(e) => {
                                    pb.finish_with_message(format!("Failed to download for {}-{}", os, arch));
                                    error!("Failed to download release for {}-{}: {}", os, arch, e);
                                }
                            }
                        },
                        Err(e) => {
                            error!("Failed to get latest version for {}-{}: {}", os, arch, e);
                        }
                    }
                }
            },
        },
        Commands::Serve { port, host, extensions_dir, releases_dir, proxy_mode, domain } => {
            // Set up the server configuration
            let mut config = zed::ServerConfig::default();
            config.port = port;
            config.host = host;
            config.proxy_mode = proxy_mode;
            config.domain = domain;
            
            // Set the extensions directory if provided, otherwise use the default
            if let Some(ext_dir) = extensions_dir {
                config.extensions_dir = ext_dir;
            } else {
                config.extensions_dir = root_dir.clone();
            }
            
            // Set the releases directory if provided, otherwise use the default
            if let Some(rel_dir) = releases_dir {
                config.releases_dir = Some(rel_dir);
            } else {
                config.releases_dir = Some(root_dir.join("releases"));
            }
            
            // Make sure all required directories exist
            std::fs::create_dir_all(&config.extensions_dir)?;
            if let Some(releases_dir) = &config.releases_dir {
                std::fs::create_dir_all(releases_dir)?;
                
                // Create zed and zed-remote-server directories if they don't exist
                let zed_dir = releases_dir.join("zed");
                let remote_server_dir = releases_dir.join("zed-remote-server");
                
                std::fs::create_dir_all(&zed_dir)?;
                std::fs::create_dir_all(&remote_server_dir)?;
                
                info!("Created release directories:");
                info!("  - {:?}", zed_dir);
                info!("  - {:?}", remote_server_dir);
            }
            
            // Start the server
            let server = zed::LocalServer::new(config);
            server.run().await?;
        }
    }

    Ok(())
}
