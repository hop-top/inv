//! inv-cli — CLI channel adapter.
//!
//! Library shape (shape-1 workspace): `bin/inv/` is the thin binary;
//! everything lives here. Decodes clap arguments → command inputs and
//! calls `inv-core::commands::*` (in later tasks). Today carries the
//! scaffolded `hello` / `list` demos until T-0016 replaces them with
//! real subcommands.

use clap::{Command, CommandFactory, Parser, Subcommand};
use hop_top_kit::output::{register_output_flags, RegisterOutputFlagsOptions};

mod commands;

#[derive(Parser)]
#[command(
    name = "inv",
    version,
    about = "Invoicing-as-a-service: composer + lifecycle, multi-channel core, event-bus interop with fin"
)]
struct Cli {
    #[command(subcommand)]
    _cmd: SubCmd,
}

#[derive(Subcommand)]
enum SubCmd {
    /// Say hello to <name>.
    Hello(commands::hello::Args),
    /// Demo list command — exercises the output flag suite.
    List,
}

/// CLI entrypoint. Called by `bin/inv/src/main.rs`.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Augment clap's auto-generated Command with the kit output flag
    // suite, then re-parse the augmented Command.
    let cli_cmd: Command = Cli::command();
    let (cli_cmd, _ctx) = register_output_flags(cli_cmd, RegisterOutputFlagsOptions::default());
    let matches = cli_cmd.get_matches();

    match matches.subcommand() {
        Some(("hello", sub)) => commands::hello::run(sub),
        Some(("list", sub)) => commands::list::run(sub),
        _ => Ok(()),
    }
}
