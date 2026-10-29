//! arkd: the composition root (design §5.3).
//!
//! This binary is the only place allowed to know every concrete implementation.
//! It parses the CLI, wires the runtime in [`wiring`] and hands the listener to
//! `ark-server`. The startup line it prints is the address that is actually
//! bound, so a smoke test never has to guess.

mod base;
mod child;
mod cli;
mod load_skill;
mod model;
mod wiring;
mod workspace;

use std::process::ExitCode;

use ark_obs::TracingConfig;
use ark_ports::ToolRegistryPort;

use crate::cli::{Command, ServeArgs};

#[tokio::main]
async fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse(argv) {
        Ok(Command::Help) => {
            print!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("arkd {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Command::Serve(args)) => serve(args).await,
        Err(message) => {
            eprintln!("arkd: {message}");
            eprintln!("{}", cli::USAGE);
            ExitCode::from(2)
        }
    }
}

/// Run the HTTP/SSE runtime until shutdown.
async fn serve(args: ServeArgs) -> ExitCode {
    let _ = ark_obs::try_init_tracing(&TracingConfig::from_env().with_ansi(false));
    let wiring = match wiring::build(&args).await {
        Ok(wiring) => wiring,
        Err(err) => {
            eprintln!("arkd: cannot start runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    let handle = match ark_server::bind(wiring.state.clone(), args.port).await {
        Ok(handle) => handle,
        Err(err) => {
            eprintln!("arkd: {err}");
            return ExitCode::FAILURE;
        }
    };
    println!("arkd listening on {}", handle.addr());
    println!("arkd data dir {}", args.data_dir.display());
    println!("arkd workspace {}", args.workspace.display());
    println!("arkd model {}", wiring.model_label);
    println!("arkd tools {}", wiring.tools.names().join(","));
    println!("arkd config version {}", wiring.config.version());
    println!(
        "arkd session store {}",
        wiring.store.data_paths().root().display()
    );
    println!("arkd usage turns {}", wiring.usage.turns());

    match handle.serve().await {
        Ok(()) => {
            println!("arkd stopped");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("arkd: {err}");
            ExitCode::FAILURE
        }
    }
}
