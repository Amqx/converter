//! The command line front end. Everything it does goes through
//! `converter_core::orchestrator`; the formats it can reach are whichever
//! backend crates are compiled in.

// A terminal program is the one place the workspace's ban on printing has to
// give, and tests here assert against hand-built paths and fixtures.
#![allow(clippy::print_stdout, clippy::print_stderr)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::float_cmp
    )
)]

mod cli;
mod formats;
mod report;
mod run;

use std::process::ExitCode;

use clap::Parser as _;

use crate::cli::{Cli, Command};

#[tokio::main]
async fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Convert(args) => run::convert(args).await,
        Command::Inspect(args) => run::inspect(&args),
        Command::Formats => {
            run::formats();
            Ok(())
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
