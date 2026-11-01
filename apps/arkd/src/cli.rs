//! Command line parsing for `arkd` (design §5.3).
//!
//! Hand-rolled on purpose: three flags do not justify a parser dependency, and
//! keeping it here makes the accepted surface auditable.

use std::path::PathBuf;

use ark_config::DEFAULT_PORT;

/// What the process was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Run the HTTP/SSE runtime.
    Serve(ServeArgs),
    /// Run one non-interactive turn (requirements/headless-run.md).
    Run(RunArgs),
    /// Print usage.
    Help,
    /// Print the version.
    Version,
}

/// `arkd run` options: one turn, streamed to stdout, then exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunArgs {
    /// The single user prompt for the turn.
    pub prompt: String,
    /// Listen port; `0` probes a free ephemeral port.
    pub port: u16,
    pub data_dir: PathBuf,
    pub auth_token: Option<String>,
    pub workspace: PathBuf,
}

/// `arkd serve` options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeArgs {
    pub port: u16,
    pub data_dir: PathBuf,
    /// Optional bearer token; when absent the runtime is local-only.
    pub auth_token: Option<String>,
    /// Workspace root handed to tools.
    pub workspace: PathBuf,
}

impl ServeArgs {
    /// Defaults: port 18790, data in `~/.ark/data`, workspace = cwd.
    pub fn defaults() -> Self {
        Self {
            port: DEFAULT_PORT,
            data_dir: default_data_dir(),
            auth_token: None,
            workspace: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }
}

/// `~/.ark/data`, resolved from the platform home directory.
pub fn default_data_dir() -> PathBuf {
    home_dir().join(".ark").join("data")
}

fn home_dir() -> PathBuf {
    for key in ["ARK_HOME", "USERPROFILE", "HOME"] {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                return PathBuf::from(value);
            }
        }
    }
    PathBuf::from(".")
}

/// Parse `args` (without the program name).
pub fn parse<I, S>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut items: Vec<String> = args.into_iter().map(Into::into).collect();
    if items.is_empty() {
        return Ok(Command::Serve(ServeArgs::defaults()));
    }
    let sub = items.remove(0);
    match sub.as_str() {
        "serve" => parse_serve(items),
        "run" => parse_run(items),
        "help" | "--help" | "-h" => Ok(Command::Help),
        "version" | "--version" | "-V" => Ok(Command::Version),
        other => Err(format!(
            "unknown command `{other}` (expected `serve`, `run`, `help` or `version`)"
        )),
    }
}

/// `arkd run [PROMPT] [--port <p>] [--data-dir <dir>] [--workspace <dir>] [--auth-token <t>]`.
///
/// The prompt is the first positional argument; flags may follow in any order.
fn parse_run(args: Vec<String>) -> Result<Command, String> {
    let mut run = RunArgs {
        prompt: String::new(),
        port: 0,
        data_dir: default_data_dir(),
        auth_token: None,
        workspace: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    let mut positional: Vec<String> = Vec::new();
    let mut iter = args.into_iter();
    while let Some(item) = iter.next() {
        match item.as_str() {
            "--port" => {
                let raw = iter.next().ok_or("--port requires a value")?;
                let port: u16 = raw
                    .parse()
                    .map_err(|err| format!("`{raw}` is not a port: {err}"))?;
                run.port = port;
            }
            "--data-dir" => {
                run.data_dir = PathBuf::from(iter.next().ok_or("--data-dir requires a value")?);
            }
            "--workspace" => {
                run.workspace = PathBuf::from(iter.next().ok_or("--workspace requires a value")?);
            }
            "--auth-token" => {
                run.auth_token = Some(iter.next().ok_or("--auth-token requires a value")?);
            }
            "--help" | "-h" => return Ok(Command::Help),
            other if other.starts_with('-') => {
                return Err(format!("unknown flag `{other}` for `run`"));
            }
            other => positional.push(other.to_string()),
        }
    }
    if positional.len() > 1 {
        return Err(
            "run accepts exactly one prompt argument (quote it if it contains spaces)".into(),
        );
    }
    run.prompt = positional.pop().unwrap_or_default();
    if run.prompt.trim().is_empty() {
        return Err("run requires a prompt argument".into());
    }
    Ok(Command::Run(run))
}

fn parse_serve(args: Vec<String>) -> Result<Command, String> {
    let mut serve = ServeArgs::defaults();
    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--port" => {
                let raw = iter.next().ok_or("--port requires a value")?;
                let port: u16 = raw
                    .parse()
                    .map_err(|err| format!("`{raw}` is not a port: {err}"))?;
                if port == 0 {
                    return Err("--port 0 is not listenable".to_string());
                }
                serve.port = port;
            }
            "--data-dir" => {
                serve.data_dir = PathBuf::from(iter.next().ok_or("--data-dir requires a value")?);
            }
            "--workspace" => {
                serve.workspace = PathBuf::from(iter.next().ok_or("--workspace requires a value")?);
            }
            "--auth-token" => {
                serve.auth_token = Some(iter.next().ok_or("--auth-token requires a value")?);
            }
            "--help" | "-h" => return Ok(Command::Help),
            other => return Err(format!("unknown flag `{other}` for `serve`")),
        }
    }
    Ok(Command::Serve(serve))
}

/// Usage text printed by `arkd help`.
pub const USAGE: &str = "\
arkd - smart-ark runtime

USAGE:
    arkd serve [--port <p>] [--data-dir <dir>] [--workspace <dir>] [--auth-token <t>]
    arkd run [PROMPT] [--port <p>] [--data-dir <dir>] [--workspace <dir>] [--auth-token <t>]
    arkd help
    arkd version

DEFAULTS:
    --port       18790 (run: probe a free ephemeral port)
    --data-dir   ~/.ark/data
    --workspace  current directory
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_serves_with_defaults() {
        match parse(Vec::<String>::new()).unwrap() {
            Command::Serve(args) => assert_eq!(args.port, DEFAULT_PORT),
            other => panic!("expected serve, got {other:?}"),
        }
    }

    #[test]
    fn flags_are_parsed() {
        let command = parse(["serve", "--port", "19001", "--data-dir", "D:/tmp/ark"]).unwrap();
        match command {
            Command::Serve(args) => {
                assert_eq!(args.port, 19001);
                assert_eq!(args.data_dir, PathBuf::from("D:/tmp/ark"));
            }
            other => panic!("expected serve, got {other:?}"),
        }
    }

    #[test]
    fn invalid_port_is_rejected() {
        assert!(parse(["serve", "--port", "abc"]).is_err());
        assert!(parse(["serve", "--port", "0"]).is_err());
        assert!(parse(["serve", "--port"]).is_err());
    }

    #[test]
    fn unknown_command_is_rejected() {
        assert!(parse(["launch"]).is_err());
        assert!(parse(["serve", "--nope"]).is_err());
    }

    #[test]
    fn run_parses_a_quoted_prompt_and_flags() {
        match parse(["run", "fix the bug", "--data-dir", "D:/tmp/ark"]).unwrap() {
            Command::Run(args) => {
                assert_eq!(args.prompt, "fix the bug");
                assert_eq!(args.data_dir, PathBuf::from("D:/tmp/ark"));
                assert_eq!(args.port, 0, "run defaults to an ephemeral port");
            }
            other => panic!("expected run, got {other:?}"),
        }
    }

    #[test]
    fn run_requires_a_prompt_and_rejects_flags_without_values() {
        assert!(parse(["run"]).is_err());
        assert!(parse(["run", "a", "b"]).is_err());
        assert!(parse(["run", "x", "--nope"]).is_err());
        assert!(parse(["run", "x", "--port"]).is_err());
    }

    #[test]
    fn help_and_version_are_recognized() {
        assert_eq!(parse(["help"]).unwrap(), Command::Help);
        assert_eq!(parse(["--version"]).unwrap(), Command::Version);
    }
}
