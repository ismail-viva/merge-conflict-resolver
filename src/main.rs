use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

use mcr::app::App;
use mcr::config::{default_config_toml, Config};
use mcr::git::repo::GitOptions;
use mcr::git::{FileBackend, GitBackend, SharedBackend};
use mcr::tui;

/// A keyboard-first, three-pane Git merge conflict resolver.
///
/// Run it inside a repository with a merge, rebase or cherry-pick in
/// progress to resolve the conflicted files.
#[derive(Debug, Parser)]
#[command(name = "mcr", version, about)]
struct Cli {
    /// Run as if started in this directory.
    #[arg(short = 'C', value_name = "DIR", global = true)]
    directory: Option<PathBuf>,

    /// Configuration file (default: $XDG_CONFIG_HOME/mcr/config.toml).
    #[arg(long, value_name = "FILE", global = true)]
    config: Option<PathBuf>,

    /// Theme to start with: dark, light, auto or a custom theme's name.
    #[arg(long, value_name = "NAME", global = true)]
    theme: Option<String>,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Run `git merge` and resolve whatever conflicts it leaves.
    Merge {
        /// Branch (or any revision) to merge, plus extra `git merge` options.
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Merge three files, e.g. as a `git mergetool` backend.
    File {
        /// The current branch's version ($LOCAL).
        local: PathBuf,
        /// The incoming version ($REMOTE).
        remote: PathBuf,
        /// The common ancestor ($BASE).
        #[arg(long)]
        base: Option<PathBuf>,
        /// Where to write the result ($MERGED).
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Print a commented configuration file with every default.
    PrintConfig,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("mcr: {err:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    if matches!(cli.command, Some(Cmd::PrintConfig)) {
        print!("{}", default_config_toml());
        return Ok(ExitCode::SUCCESS);
    }
    let config = Config::load(cli.config.as_deref())?;
    let theme = config.initial_theme(cli.theme.as_deref())?;
    let directory = match cli.directory {
        Some(dir) => dir,
        None => std::env::current_dir().context("cannot determine the current directory")?,
    };

    let (backend, file_mode): (SharedBackend, bool) = match cli.command {
        Some(Cmd::File {
            local,
            remote,
            base,
            output,
        }) => (
            Arc::new(FileBackend::new(base, local, remote, output)),
            true,
        ),
        command => {
            let options = GitOptions {
                auto_stage: config.git.auto_stage,
                backup: config.git.backup,
            };
            let repo = GitBackend::discover(&directory, options)?;
            if let Some(Cmd::Merge { args }) = command {
                if repo.start_merge(&args)? {
                    return Ok(ExitCode::SUCCESS);
                }
            }
            (Arc::new(repo), false)
        }
    };

    let conflicts = backend.conflicts()?;
    if conflicts.is_empty() {
        println!("No merge conflicts to resolve.");
        return Ok(ExitCode::SUCCESS);
    }
    if conflicts.iter().all(|c| c.unsupported.is_some()) {
        for conflict in &conflicts {
            eprintln!(
                "  {}: {}",
                conflict.path,
                conflict.unsupported.as_deref().unwrap_or("")
            );
        }
        bail!("none of the conflicts can be resolved line by line");
    }

    let mut app = App::new(backend, config, theme)?;
    tui::run(&mut app)?;

    for line in app.summary() {
        println!("{line}");
    }
    let done = app.all_resolved();
    if done && !file_mode {
        println!("All conflicts resolved. Finish with `git commit` (or `git rebase --continue`).");
    }
    // As a mergetool, the exit code tells git whether the merge succeeded.
    Ok(if file_mode && !done {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
