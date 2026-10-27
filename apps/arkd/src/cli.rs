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
    /// Print usage.
    Help,
    /// Print the version.
    Version,
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
        "help" | "--help" | "-h" => Ok(Command::Help),
        "version" | "--version" | "-V" => Ok(Command::Version),
        other => Err(format!(
            "unknown command `{other}` (expected `serve`, `help` or `version`)"
        )),
    }
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
    arkd help
    arkd version

DEFAULTS:
    --port       18790
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
        assert!(parse(["run"]).is_err());
        assert!(parse(["serve", "--nope"]).is_err());
    }

    #[test]
    fn help_and_version_are_recognized() {
        assert_eq!(parse(["help"]).unwrap(), Command::Help);
        assert_eq!(parse(["--version"]).unwrap(), Command::Version);
    }
}
