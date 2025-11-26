use std::path::PathBuf;
use std::process;
use std::str::FromStr;
use std::time::Duration;

use anyhow::Result;
use clap::{ArgGroup, Parser};
use file_syncer::{Config, MODE_PULL, MODE_PUSH, Mode, init_logger, init_sentry, run};
use sentry::ClientInitGuard;

#[derive(Parser, Debug)]
#[command(
    name = "file-syncer",
    about = "Sync a local folder with a git repository using push or pull operations.",
    group(
        ArgGroup::new("compression-level")
            .args(&["compression_fast", "compression_default", "compression_max"])
            .multiple(false)
    )
)]
struct CliArgs {
    #[arg(long, value_name = "MODE", value_parser = [MODE_PUSH, MODE_PULL])]
    mode: String,
    #[arg(long, value_name = "PATH", help = "Path to the folder to sync")]
    folder: String,
    #[arg(long, value_name = "URL", help = "Git repository URL")]
    repo: String,
    #[arg(long, default_value = "main", help = "Git branch to use")]
    branch: String,
    #[arg(long, value_name = "PATH", help = "SSH private key for git operations")]
    ssh_key: Option<String>,
    #[arg(
        long,
        default_value_t = false,
        help = "Compress files with zstd during sync"
    )]
    compress: bool,
    #[arg(
        long,
        default_value_t = false,
        help = "Use fast zstd compression level"
    )]
    compression_fast: bool,
    #[arg(
        long,
        default_value_t = false,
        help = "Use default zstd compression level"
    )]
    compression_default: bool,
    #[arg(long, default_value_t = false, help = "Use max zstd compression level")]
    compression_max: bool,
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(usize), help = "Set number of rayon worker threads")]
    threads: Option<usize>,
    #[arg(
        long,
        env = "SENTRY_DSN",
        value_name = "DSN",
        help = "Sentry DSN for error reporting"
    )]
    sentry_dsn: Option<String>,
}

impl TryFrom<CliArgs> for Config {
    type Error = anyhow::Error;

    fn try_from(args: CliArgs) -> Result<Self, Self::Error> {
        let level = if args.compression_fast {
            file_syncer::CompressionLevel::Fast
        } else if args.compression_max {
            file_syncer::CompressionLevel::Max
        } else {
            file_syncer::CompressionLevel::Default
        };

        Ok(Config {
            mode: Mode::from_str(&args.mode)?,
            folder_path: PathBuf::from(args.folder),
            repo_url: args.repo,
            branch: args.branch,
            ssh_key_path: args.ssh_key,
            compress: args.compress
                || args.compression_fast
                || args.compression_default
                || args.compression_max,
            compression_level: level,
            thread_count: args.threads,
            sentry_dsn: args.sentry_dsn,
        })
    }
}

fn main() {
    let mut sentry_guard: Option<ClientInitGuard> = None;

    let result = (|| -> Result<()> {
        init_logger()?;
        let args = CliArgs::parse();
        let config = Config::try_from(args)?;
        sentry_guard = init_sentry(config.sentry_dsn.as_deref())?;
        run(&config)
    })();

    if let Err(err) = &result {
        sentry::capture_message(&format!("{err:?}"), sentry::Level::Error);
        if let Some(guard) = sentry_guard.take() {
            guard.close(Some(Duration::from_secs(2)));
        }
        eprintln!("Error: {err:?}");
        process::exit(1);
    }

    if let Some(guard) = sentry_guard {
        guard.close(None);
    }
}
