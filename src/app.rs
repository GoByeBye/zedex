use crate::{
    cli::{Cli, Commands},
    commands::{self, serve::ServeOptions},
};
use anyhow::Result;
use clap::Parser;
use env_logger::Builder;
use log::{LevelFilter, debug, info};
use std::io::Write;

pub async fn run() -> Result<()> {
    let cli = Cli::parse();
    init_logging(&cli.log_level, cli.log_timestamp);

    info!("Starting Zed Extension Mirror");
    debug!("Using root directory: {:?}", cli.root_dir);

    match cli.command {
        Commands::Get { target } => {
            commands::get::run(target, cli.root_dir.clone()).await?;
        }
        Commands::Release { target } => {
            commands::release::run(target, cli.root_dir.clone()).await?;
        }
        Commands::Serve {
            port,
            host,
            extensions_dir,
            proxy_mode,
            domain,
        } => {
            let options = ServeOptions {
                port,
                host,
                extensions_dir,
                proxy_mode,
                domain,
            };
            commands::serve::run(options, cli.root_dir.clone()).await?;
        }
    }

    Ok(())
}

fn init_logging(log_level: &str, log_timestamp: bool) {
    let mut builder = Builder::new();

    let chosen_level = match log_level {
        "trace" => LevelFilter::Trace,
        "debug" => LevelFilter::Debug,
        "info" => LevelFilter::Info,
        "warn" => LevelFilter::Warn,
        "error" => LevelFilter::Error,
        _ => LevelFilter::Info,
    };

    builder.filter_level(chosen_level);

    if log_timestamp {
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
        builder.format(|buf, record| writeln!(buf, "[{}] - {}", record.level(), record.args()));
    }

    // It's OK if init() fails because it was already initialized in tests.
    let _ = builder.try_init();
}
