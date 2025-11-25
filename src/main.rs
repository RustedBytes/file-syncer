use std::path::PathBuf;
use std::process;
use std::str::FromStr;

use anyhow::Result;
use clap::Parser;
use file_syncer::{Config, MODE_PULL, MODE_PUSH, Mode, init_logger, run};

#[derive(Parser, Debug)]
#[command(
    name = "file-syncer",
    about = "Sync a local folder with a git repository using push or pull operations."
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
}

impl TryFrom<CliArgs> for Config {
    type Error = anyhow::Error;

    fn try_from(args: CliArgs) -> Result<Self, Self::Error> {
        Ok(Config {
            mode: Mode::from_str(&args.mode)?,
            folder_path: PathBuf::from(args.folder),
            repo_url: args.repo,
            branch: args.branch,
            ssh_key_path: args.ssh_key,
        })
    }
}

fn main() {
    if let Err(err) = real_main() {
        eprintln!("Error: {err:?}");
        process::exit(1);
    }
}

fn real_main() -> Result<()> {
    init_logger()?;
    let args = CliArgs::parse();
    let config = Config::try_from(args)?;
    run(&config)
}
