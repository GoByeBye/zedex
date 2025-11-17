use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Command Line Interface definition for the zedex binary.
#[derive(Parser, Debug)]
#[clap(author, version, about = "Zed Extension Mirror")]
pub struct Cli {
    /// Root directory for all cache files
    #[clap(long, default_value = ".zedex-cache")]
    pub root_dir: PathBuf,

    /// Log level: trace, debug, info, warn, error
    #[clap(long, default_value = "info")]
    pub log_level: String,

    /// Enable timestamp in logs
    #[clap(long)]
    pub log_timestamp: bool,

    #[clap(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
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
        #[clap(long, default_value = "2654")]
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

#[derive(Subcommand, Debug)]
pub enum GetTarget {
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

#[derive(Subcommand, Debug)]
pub enum ReleaseTarget {
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
