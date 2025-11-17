use crate::{
    cli::GetTarget,
    zed::{
        Client, DownloadOptions, Extension, ExtensionVersionTracker, WrappedExtensions,
        download_extension_by_id, download_extension_index, download_extensions,
    },
};
use anyhow::Result;
use futures_util::future;
use log::{error, info};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Entry point for handling `zedex get ...` commands.
pub async fn run(target: GetTarget, root_dir: PathBuf) -> Result<()> {
    match target {
        GetTarget::ExtensionIndex { provides } => handle_extension_index(root_dir, provides).await,
        GetTarget::Extension { ids, output_dir } => {
            handle_extension(ids, output_dir, root_dir).await
        }
        GetTarget::AllExtensions {
            output_dir,
            async_mode,
            all_versions,
            rate_limit,
        } => {
            handle_all_extensions(output_dir, root_dir, async_mode, all_versions, rate_limit).await
        }
    }
}

async fn handle_extension_index(root_dir: PathBuf, provides: Vec<String>) -> Result<()> {
    let client = Client::new();
    download_extension_index(&client, &root_dir, &provides).await?;
    Ok(())
}

async fn handle_extension(
    ids: Vec<String>,
    output_dir: Option<PathBuf>,
    root_dir: PathBuf,
) -> Result<()> {
    let output_dir = resolve_output_dir(output_dir, &root_dir);
    fs::create_dir_all(&output_dir)?;

    let client = Client::new().with_extensions_local_dir(output_dir.to_string_lossy().to_string());
    let extensions = ensure_extensions_index(&client, &output_dir, &[]).await?;

    let futures = ids.into_iter().map(|id| {
        let client = client.clone();
        let output_dir = output_dir.clone();
        let extensions = extensions.clone();

        async move { download_extension_by_id(&id, client, &output_dir, &extensions).await }
    });

    let results = future::join_all(futures).await;
    for (idx, result) in results.into_iter().enumerate() {
        if let Err(err) = result {
            error!("Failed to download extension #{}: {}", idx, err);
        }
    }

    Ok(())
}

async fn handle_all_extensions(
    output_dir: Option<PathBuf>,
    root_dir: PathBuf,
    async_mode: bool,
    all_versions: bool,
    rate_limit: u64,
) -> Result<()> {
    let output_dir = resolve_output_dir(output_dir, &root_dir);
    fs::create_dir_all(&output_dir)?;

    let client = Client::new().with_extensions_local_dir(output_dir.to_string_lossy().to_string());
    let extensions = ensure_extensions_index(&client, &output_dir, &[]).await?;
    let mut version_tracker = load_version_tracker(&output_dir);

    let options = DownloadOptions {
        async_mode,
        all_versions,
        rate_limit,
    };

    let updated_tracker = download_extensions(
        extensions,
        client,
        &output_dir,
        version_tracker.clone(),
        options,
    )
    .await?;

    version_tracker.merge(updated_tracker);
    persist_version_tracker(&output_dir, &version_tracker)?;

    info!("All extensions downloaded to {:?}", output_dir);
    Ok(())
}

fn resolve_output_dir(option: Option<PathBuf>, fallback: &Path) -> PathBuf {
    option.unwrap_or_else(|| fallback.to_path_buf())
}

async fn ensure_extensions_index(
    client: &Client,
    output_dir: &Path,
    provides: &[String],
) -> Result<Vec<Extension>> {
    let extensions_file = output_dir.join("extensions.json");

    if extensions_file.exists() {
        info!("Loading extension index from {:?}", extensions_file);
        load_extensions_file(&extensions_file)
    } else {
        info!("Extension index not found. Fetching from API...");
        download_extension_index(client, output_dir, provides).await
    }
}

fn load_extensions_file(path: &Path) -> Result<Vec<Extension>> {
    let content = fs::read_to_string(path)?;
    let wrapped: WrappedExtensions = serde_json::from_str(&content)?;
    Ok(wrapped.data)
}

fn load_version_tracker(output_dir: &Path) -> ExtensionVersionTracker {
    let version_tracker_file = output_dir.join("version_tracker.json");
    if version_tracker_file.exists() {
        if let Ok(content) = fs::read_to_string(&version_tracker_file) {
            if let Ok(tracker) = serde_json::from_str(&content) {
                return tracker;
            }
        }
    }

    ExtensionVersionTracker::new()
}

fn persist_version_tracker(output_dir: &Path, tracker: &ExtensionVersionTracker) -> Result<()> {
    let version_tracker_file = output_dir.join("version_tracker.json");
    let version_tracker_json = serde_json::to_string_pretty(tracker)?;
    fs::write(&version_tracker_file, version_tracker_json)?;
    Ok(())
}
