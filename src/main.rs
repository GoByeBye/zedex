mod zed;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use log::{debug, error, info, LevelFilter};
use env_logger::Builder;
use indicatif::{ProgressBar, ProgressStyle};
use futures_util::StreamExt;
use std::sync::Arc;
use std::io::Write;

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
    
    /// Get latest Zed release information
    Release {
        #[clap(subcommand)]
        target: ReleaseTarget,
    },

    /// Start a local server to serve Zed extensions API
    Serve {
        /// Port to run the server on
        #[clap(long, default_value = "2654")] // If you're reading this, you're a nerd. And yes it's ZED. Z=26 E=5 D=4
        port: u16,
        
        /// Directory containing extension archives and metadata
        #[clap(long)]
        extensions_dir: Option<PathBuf>,
        
        /// Directory containing release information
        #[clap(long)]
        releases_dir: Option<PathBuf>,

        /// Whether to proxy requests to zed.dev for missing content
        #[clap(long)]
        proxy_mode: bool,
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
    },
}

#[derive(Subcommand)]
enum ReleaseTarget {
    /// Get the latest Zed release version
    Latest,
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
                
                // Download each extension with a progress bar
                let futures = ids.iter().map(|id| {
                    let id = id.clone();
                    let client = client.clone();
                    let output_dir = output_dir.clone();
                    
                    async move {
                        info!("Downloading extension: {}", id);
                        
                        // Create a progress bar for this download
                        let pb = Arc::new(ProgressBar::new(0));
                        pb.set_style(ProgressStyle::default_bar()
                            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                            .unwrap()
                            .progress_chars("#>-"));
                        
                        let pb_clone = pb.clone();
                        match client.download_extension_archive_default_with_progress(&id, 
                            move |downloaded, total| {
                                pb_clone.set_length(total);
                                pb_clone.set_position(downloaded);
                            }).await {
                            Ok(bytes) => {
                                pb.finish_with_message(format!("Downloaded {}", id));
                                let file_path = output_dir.join(format!("{}.tar.gz", id));
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
                    }
                });
                
                // Wait for all downloads to complete
                futures_util::future::join_all(futures).await;
            },
            GetTarget::AllExtensions { output_dir, async_mode } => {
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
                
                info!("Downloading {} extensions...", extensions.len());
                
                if async_mode {
                    // Fully asynchronous mode - no throttling
                    info!("Using fully asynchronous mode - be careful of rate limiting!");
                    
                    // Download each extension without throttling
                    let futures = extensions.iter().map(|extension| {
                        let id = extension.id.clone();
                        let client = client.clone();
                        let output_dir = output_dir.clone();
                        
                        async move {
                            let file_path = output_dir.join(format!("{}.tar.gz", id));
                            
                            // Skip if already downloaded
                            if file_path.exists() {
                                debug!("Extension {} already downloaded, skipping", id);
                                return;
                            }
                            
                            info!("Downloading extension: {}", id);
                            
                            // Create a progress bar for this download
                            let pb = Arc::new(ProgressBar::new(0));
                            pb.set_style(ProgressStyle::default_bar()
                                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                                .unwrap()
                                .progress_chars("#>-"));
                            
                            let pb_clone = pb.clone();
                            match client.download_extension_archive_default_with_progress(&id, 
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
                        
                        let handle = tokio::spawn(async move {
                            // Acquire a permit from the semaphore (this limits concurrency)
                            let _permit = semaphore.acquire().await.unwrap();
                            
                            let file_path = output_dir.join(format!("{}.tar.gz", id));
                            
                            // Skip if already downloaded
                            if file_path.exists() {
                                debug!("Extension {} already downloaded, skipping", id);
                                return;
                            }
                            
                            info!("Downloading extension: {}", id);
                            
                            // Create a progress bar for this download
                            let pb = Arc::new(ProgressBar::new(0));
                            pb.set_style(ProgressStyle::default_bar()
                                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                                .unwrap()
                                .progress_chars("#>-"));
                            
                            let pb_clone = pb.clone();
                            match client.download_extension_archive_default_with_progress(&id, 
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
                        });
                        
                        handles.push(handle);
                    }
                    
                    // Wait for all downloads to complete
                    for handle in handles {
                        handle.await?;
                    }
                }
                
                info!("All extensions downloaded to {:?}", output_dir);
            },
        },
        Commands::Release { target } => match target {
            ReleaseTarget::Latest => {
                let client = zed::Client::new();
                
                // Download for each platform
                let platforms = [
                    ("linux", "x86_64"),
                    // TODO: RE add when windows is supported ("windows", "x86_64"),
                    ("macos", "x86_64"),
                    ("macos", "aarch64")
                ];
                
                // Create releases directory
                let releases_dir = root_dir.join("releases");
                std::fs::create_dir_all(&releases_dir)?;
                
                // Variable to store a default version (macOS x86_64) for backward compatibility
                let mut default_version = None;
                
                // Create futures for parallel downloads
                let futures = platforms.iter().map(|(os, arch)| {
                    let os = os.to_string();
                    let arch = arch.to_string();
                    let platform_client = client.clone().with_platform(os.clone(), arch.clone());
                    let releases_dir = releases_dir.clone();
                    
                    async move {
                        // Get the latest version for this platform
                        match platform_client.get_latest_version().await {
                            Ok(version) => {
                                info!("\nLatest Zed Version for {}-{}: {}", os, arch, version.version);
                                info!("Downloading from: {}", version.url);
                                
                                // Create a progress bar for this download
                                let pb = ProgressBar::new(0);
                                pb.set_style(ProgressStyle::default_bar()
                                    .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                                    .unwrap()
                                    .progress_chars("#>-"));
                                
                                // Use reqwest client with a streaming download to update progress
                                let client = reqwest::Client::new();
                                let res = client.get(&version.url).send().await?;
                                
                                // Get the content length if available
                                let total_size = res.content_length().unwrap_or(0);
                                pb.set_length(total_size);
                                
                                // Download and collect the chunks while updating the progress bar
                                let mut downloaded_bytes = Vec::new();
                                let mut stream = res.bytes_stream();
                                
                                while let Some(chunk) = stream.next().await {
                                    let chunk = chunk?;
                                    pb.inc(chunk.len() as u64);
                                    downloaded_bytes.extend_from_slice(&chunk);
                                }
                                
                                // Finish the progress bar
                                pb.finish_with_message(format!("Downloaded {} for {}-{}", version.version, os, arch));
                                
                                // Extract original filename from URL (remove query parameters if present)
                                let file_name = version.url
                                    .split('?').next().unwrap_or(&version.url)
                                    .split('/').last().unwrap_or("zed.zip");
                                
                                // Create version-specific directory to match Zed's URL pattern
                                let version_dir = releases_dir.join("stable").join(&version.version);
                                std::fs::create_dir_all(&version_dir)?;
                                info!("Created version directory: {:?}", version_dir);
                                
                                // Save release file with original filename in the version directory
                                let file_path = version_dir.join(file_name);
                                std::fs::write(&file_path, downloaded_bytes)?;
                                info!("Downloaded latest release to {:?}", file_path);
                                
                                // Save a platform-specific latest-version.json file
                                let version_json = serde_json::to_string_pretty(&version)?;
                                let version_file_path = releases_dir.join(format!("latest-version-{}-{}.json", os, arch));
                                std::fs::write(&version_file_path, version_json)?;
                                info!("Saved latest version info to {:?}", version_file_path);
                                
                                // Return successful result with platform info and version
                                Ok::<_, anyhow::Error>((os, arch, version))
                            },
                            Err(err) => {
                                // Special handling for Windows
                                if os == "windows" {
                                    info!("\n{}-{}: {}", os, arch, err);
                                    // Create a placeholder version just to continue execution
                                    Ok((
                                        "windows".to_string(), 
                                        arch, 
                                        zed::Version { 
                                            version: "0.0.0".to_string(), 
                                            url: "".to_string()
                                        }
                                    ))
                                } else {
                                    // For other platforms, propagate the error
                                    Err(err)
                                }
                            }
                        }
                    }
                });
                
                // Wait for all downloads to complete
                let results = futures_util::future::join_all(futures).await;
                
                // Process results for default version
                for result in results {
                    match result {
                        Ok((os, arch, version)) => {
                            // Store macOS x86_64 version for backward compatibility
                            if os == "macos" && arch == "x86_64" {
                                default_version = Some(version);
                            }
                        },
                        Err(err) => {
                            // If there's an error with a non-Windows platform, return it
                            return Err(err);
                        }
                    }
                }
                
                // Save a generic latest-version.json for backward compatibility
                if let Some(version) = default_version {
                    let version_json = serde_json::to_string_pretty(&version)?;
                    let version_file_path = releases_dir.join("latest-version.json");
                    std::fs::write(&version_file_path, version_json)?;
                    info!("Saved generic latest version info to {:?} (for backward compatibility)", version_file_path);
                }
            },
        },
        Commands::Serve { port, extensions_dir, releases_dir, proxy_mode } => {
            // Resolve directories from root_dir if not provided
            let extensions_dir = extensions_dir.unwrap_or_else(|| root_dir.clone());
            let releases_dir = releases_dir.or_else(|| Some(root_dir.join("releases")));
            
            // Create and configure the local server
            let config = zed::ServerConfig {
                port,
                extensions_dir,
                releases_dir,
                proxy_mode,
            };
            
            let server = zed::LocalServer::new(config);
            server.run().await?;
        }
    }

    Ok(())
}
