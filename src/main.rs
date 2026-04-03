use anyhow::Context;
use clap::{Parser, Subcommand};

use std::{env, io::Write};

mod config;
mod lock;
mod init;

#[derive(Debug, Parser)]
#[command(name = "east")]
#[command(about = "An experimental management tool")]
struct East {
    #[clap(subcommand)]
    command: Commands
}

#[derive(Debug, Subcommand)]
enum Commands {
    Init
}

fn main() -> anyhow::Result<()> {
    let cli = East::parse();
    match cli.command {
        Commands::Init => {
            let cwd = env::current_dir()
                .context("could not find the current directory")?;
            let lock = init::init(&cwd)
                .context("initialization was not successful")?;
            let mut file = std::fs::File::create(cwd.join("east.lock"))
                .context("failed to create lock file")?;
            let lock_content = toml::to_string_pretty(&lock)
                .context("failed to serialize lock file content")?;
            std::fs::write(
                cwd.join("east.lock"),
                lock_content
            ).context("failed to write to lockfile")?;
        }
    }

    Ok(())
}
