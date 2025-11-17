use crate::cli::ReleaseTarget;
use crate::zed::{self, Client};
use anyhow::Result;
use log::info;
use std::path::PathBuf;

/// Entry point for handling `zedex release ...` commands.
pub async fn run(target: ReleaseTarget, root_dir: PathBuf) -> Result<()> {
    match target {
        ReleaseTarget::Latest => {
            info!("Not implemented yet: Fetching latest Zed release info");
            Ok(())
        }
        ReleaseTarget::RemoteServerLatest => {
            info!("Not implemented yet: Fetching latest Zed Remote Server release info");
            Ok(())
        }
        ReleaseTarget::Download { output_dir } => {
            let output_dir = output_dir.unwrap_or_else(|| root_dir.clone());
            let client = Client::new();

            info!("Downloading latest Zed release to {:?}", output_dir);
            zed::download_zed_release(&client, &output_dir).await;
            info!("Zed release download complete");
            Ok(())
        }
        ReleaseTarget::DownloadRemoteServer { output_dir: _ } => {
            info!("Not implemented yet: Downloading latest Zed Remote Server release");
            Ok(())
        }
    }
}
