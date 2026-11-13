//! arkd: the composition root (design §5.3).
//!
//! This binary is the only place allowed to know every concrete implementation.
//! It parses the CLI, wires the runtime in [`wiring`] and hands the listener to
//! `ark-server`. The startup line it prints is the address that is actually
//! bound, so a smoke test never has to guess.

#![deny(clippy::all)]

mod attachment_seed;
mod base;
mod child;
mod cli;
mod headless;
mod hooks;
mod load_skill;
mod memory_seed;
mod model;
mod paths;
mod plugin_host;
mod todo_tool;
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
        Ok(Command::Run(args)) => {
            if let Err(message) = init_runtime_tracing(&args.data_dir, true) {
                eprintln!("arkd: cannot initialize logging: {message}");
                return ExitCode::FAILURE;
            }
            headless::run(args).await
        }
        Err(message) => {
            eprintln!("arkd: {message}");
            eprintln!("{}", cli::USAGE);
            ExitCode::from(2)
        }
    }
}

/// Run the HTTP/SSE runtime until shutdown.
async fn serve(args: ServeArgs) -> ExitCode {
    if let Err(message) = init_runtime_tracing(&args.data_dir, false) {
        eprintln!("arkd: cannot initialize logging: {message}");
        return ExitCode::FAILURE;
    }
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
    // The scheduled-task loop starts last, once `state` is final and the
    // listener is bound: its first act is reconciliation, which must observe
    // the same state every route will use. A one-shot `run` never reaches
    // this, so headless turns cannot fire due schedules behind the user's
    // back.
    wiring.schedules.spawn(wiring.state.clone());
    // IM channel connections start after the listener, same reasoning as the
    // scheduler loop: the first sync must observe the final state.
    wiring.im.spawn(wiring.state.clone());
    println!("arkd model {}", wiring.model_label);
    println!("arkd tools {}", wiring.tools.names().join(","));
    println!("arkd config version {}", wiring.config.version());
    println!(
        "arkd session store {}",
        wiring.store.data_paths().root().display()
    );
    println!("arkd usage turns {}", wiring.usage.turns());
    println!("arkd plugins dir {}", wiring.plugins.root().display());
    println!(
        "arkd plugins loaded {}",
        wiring.plugins.host().report().loaded.join(",")
    );

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

fn init_runtime_tracing(data_dir: &std::path::Path, stderr: bool) -> Result<(), String> {
    let paths = paths::RuntimePaths::from_data_dir(data_dir);
    paths.ensure().map_err(|error| error.to_string())?;
    let config = if stderr {
        TracingConfig::from_env().with_ansi(false).to_stderr()
    } else {
        TracingConfig::from_env().with_ansi(false)
    };
    ark_obs::try_init_tracing_with_file(&config, paths.log_file()).map(|_| ())
}
