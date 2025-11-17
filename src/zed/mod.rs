mod client;
mod downloader;
mod error;
mod extension;
mod health;
mod server;
mod version;

pub use client::Client;
pub use downloader::{
    DownloadOptions, download_extension_by_id, download_extension_index, download_extensions,
    download_zed_release,
};
pub use error::ZedError;
pub use extension::extensions_utils;
pub use extension::{Extension, ExtensionVersionTracker, Extensions, WrappedExtensions};
pub use health::health_check;
pub use server::{LocalServer, ServerConfig};
pub use version::Version;
