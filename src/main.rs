//! `seogeo` — the single-binary execution engine behind the SEO + GEO
//! agent skills.
//!
//! Every skill in `skills/` calls this binary instead of shelling out to a
//! Python interpreter, so a skill install has exactly one runtime
//! dependency. Each subcommand maps 1:1 onto an invocation documented in a
//! `SKILL.md`; `seogeo commands --json` prints that mapping.

mod chrome;
mod cli;
mod cmd;
mod html;
mod http;
mod output;
mod paths;
mod safety;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    match cmd::dispatch(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::from(1)
        }
    }
}
