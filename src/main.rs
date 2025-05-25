mod zed;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use log::{debug, info, LevelFilter};
use env_logger::Builder;
use std::io::Write;
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
    /// Fetch Zed releases
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
    ExtensionIndex {
        /// Filter extensions by provides tags (e.g. languages, language-servers)
        #[clap(long)]
        provides: Vec<String>,
    },
    
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
        #[clap(long)]
        /// Output directory for downloaded Zed release
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
    
    let root_dir = cli.root_dir.clone();    match cli.command {
        Commands::Get { target } => match target {            GetTarget::ExtensionIndex { provides } => {
                let client = zed::Client::new();
                zed::download_extension_index(&client, &root_dir, &provides).await?;
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
                    zed::download_extension_index(&client, &output_dir, &Vec::new()).await?
                };
                
                // Download each extension with a progress bar
                let futures = ids.iter().map(|id| {
                    let id = id.clone();
                    let client = client.clone();
                    let output_dir = output_dir.clone();
                    let extensions = extensions.clone();
                    
                    async move {
                        zed::download_extension_by_id(&id, client, &output_dir, &extensions).await
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
                    zed::download_extension_index(&client, &output_dir, &Vec::new()).await?
                };
                  // Load or create the version tracker
                let version_tracker_file = output_dir.join("version_tracker.json");
                let version_tracker = if version_tracker_file.exists() {                info!("Loading version tracker from {:?}", version_tracker_file);
                    let content = std::fs::read_to_string(&version_tracker_file)?;
                    serde_json::from_str(&content).unwrap_or_else(|_| zed::ExtensionVersionTracker::new())
                } else {
                    zed::ExtensionVersionTracker::new()
                };
                
                // Set up download options
                let options = zed::DownloadOptions {
                    async_mode,
                    all_versions,
                    rate_limit,
                };
                
                // Download all extensions
                let updated_tracker = zed::download_extensions(
                    extensions, 
                    client, 
                    &output_dir, 
                    version_tracker, 
                    options
                ).await?;
                
                // Save the updated version tracker
                let version_tracker_json = serde_json::to_string_pretty(&updated_tracker)?;
                fs::write(&version_tracker_file, version_tracker_json)?;
                
                info!("All extensions downloaded to {:?}", output_dir);
            },
        },
        
        Commands::Release { target } => match target {
            ReleaseTarget::Latest => {
                info!("Not implemented yet: Fetching latest Zed release info");
                // Would implement version info fetching without download
            },
            ReleaseTarget::RemoteServerLatest => {
                info!("Not implemented yet: Fetching latest Zed Remote Server release info");
                // Would implement remote server version info fetching without download
            },
            ReleaseTarget::Download {output_dir} => {
                let output_dir = output_dir.unwrap_or_else(|| root_dir.clone());
                
                // Create output directory
                std::fs::create_dir_all(&output_dir)?;
                
                // Create a client
                let client = zed::Client::new();
                
                // Download the latest Zed release
                info!("Downloading latest Zed release to {:?}", output_dir);
                zed::download_zed_release(&client, &output_dir).await;
                
                info!("Zed release download complete");
            },
            ReleaseTarget::DownloadRemoteServer { output_dir: _ } => {
                info!("Not implemented yet: Downloading latest Zed Remote Server release");
                // Would implement remote server download logic
            },
        },

        Commands::Serve { port, host, extensions_dir, proxy_mode, domain } => {
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
            
            // Make sure all required directories exist
            if !config.extensions_dir.exists() {
                std::fs::create_dir_all(&config.extensions_dir)?;
            }
            if let Some(releases_dir) = &config.releases_dir {
                if !releases_dir.exists() {
                    std::fs::create_dir_all(releases_dir)?;
                }
                
                // Create zed and zed-remote-server directories if they don't exist
                let zed_dir = releases_dir.join("zed");
                let remote_server_dir = releases_dir.join("zed-remote-server");
                
                if !zed_dir.exists() {
                    std::fs::create_dir_all(&zed_dir)?;
                }
                if !remote_server_dir.exists() {
                    std::fs::create_dir_all(&remote_server_dir)?;
                }
                
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
