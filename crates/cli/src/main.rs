#![forbid(unsafe_code)]

mod args;
mod commands;
mod output;

use std::process::ExitCode;

use args::Args;
use clap::Parser;

fn main() -> ExitCode {
    let args = Args::parse();
    let json_errors = args.command.json();
    match commands::run(args.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            output::print_error(&error, json_errors);
            ExitCode::from(error.code().exit_code())
        }
    }
}
