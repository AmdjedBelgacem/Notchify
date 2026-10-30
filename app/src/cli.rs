//! Command-line arguments (hand-rolled: four flags don't justify a parser crate).

use std::path::PathBuf;

pub const USAGE: &str = "\
NotchForge — the MacBook notch as a control & notification surface

USAGE:
    notchforge [OPTIONS]

OPTIONS:
    --headless         Run the engine and providers without UI (any OS); events are logged
    --config <PATH>    Use a different config file
    --print-config     Print the default configuration and exit
    -v, --verbose      Debug logging (or set RUST_LOG)
    -V, --version      Print version
    -h, --help         Print this help";

#[derive(Debug, Default, PartialEq)]
pub struct Args {
    pub headless: bool,
    pub config: Option<PathBuf>,
    pub print_config: bool,
    pub verbose: bool,
    pub version: bool,
    pub help: bool,
}

impl Args {
    pub fn parse(mut it: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut a = Args::default();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--headless" => a.headless = true,
                "--config" => a.config = Some(it.next().ok_or("--config needs a path")?.into()),
                "--print-config" => a.print_config = true,
                "-v" | "--verbose" => a.verbose = true,
                "-V" | "--version" => a.version = true,
                "-h" | "--help" => a.help = true,
                // macOS passes -psn_… when launched from Finder on old systems.
                s if s.starts_with("-psn_") => {}
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        Ok(a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(v: &[&str]) -> Result<Args, String> {
        Args::parse(v.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_flags() {
        let a = parse(&["--headless", "--config", "/tmp/x.toml", "-v"]).expect("ok");
        assert!(a.headless && a.verbose);
        assert_eq!(a.config, Some(PathBuf::from("/tmp/x.toml")));
        assert!(parse(&["--config"]).is_err());
        assert!(parse(&["--nope"]).is_err());
        assert_eq!(parse(&["-psn_0_12345"]).expect("ok"), Args::default());
    }
}
