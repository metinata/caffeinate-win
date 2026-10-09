//! getopt-compatible argument parsing, mirroring macOS `caffeinate(8)`.

use std::ffi::OsString;
use std::time::Duration;

pub const USAGE: &str = "\
usage: caffeinate [-dimsu] [-t timeout] [-w pid] [utility [argument ...]]

Prevents the system from sleeping on behalf of a utility, or until interrupted.

  -d          prevent the display from sleeping
  -i          prevent the system from idle sleeping (default)
  -m          treated as -i (Windows has no request to keep disks spinning)
  -s          prevent the system from sleeping, only while on AC power
  -u          declare that the user is active: turn the display on and keep
              it on for 5 seconds, or for the -t timeout
  -t timeout  release after <timeout> seconds
  -w pid      release when process <pid> exits (ignored with a utility)
  -h, --help     print this help
  -V, --version  print the version

With a utility, caffeinate runs it, keeps the system awake while it runs (up
to the timeout) and exits with its exit status. Otherwise it runs until the
timeout, until <pid> exits, or until Ctrl+C.
";

#[derive(Debug, Default, PartialEq)]
pub struct Options {
    /// `-d`
    pub display: bool,
    /// `-i` / `-m`
    pub idle: bool,
    /// `-s`: system awake, but only on AC power
    pub system_on_ac: bool,
    /// `-u`
    pub user: bool,
    /// `-t`; `None` without one, or with 0.
    pub timeout: Option<Duration>,
    pub pid: Option<u32>,
    pub command: Vec<OsString>,
}

#[derive(Debug, PartialEq)]
pub enum Parsed {
    Run(Options),
    Help,
    Version,
}

/// Parses arguments; the first item is the program name, as in `std::env::args_os`.
pub fn parse<I>(args: I) -> Result<Parsed, lexopt::Error>
where
    I: IntoIterator,
    I::Item: Into<OsString>,
{
    use lexopt::prelude::*;

    let mut o = Options::default();
    let mut parser = lexopt::Parser::from_iter(args);
    while let Some(arg) = parser.next()? {
        match arg {
            Short('d') => o.display = true,
            Short('i' | 'm') => o.idle = true,
            Short('s') => o.system_on_ac = true,
            Short('u') => o.user = true,
            Short('t') => {
                // As on macOS, 0 means no timeout.
                let secs = parser.value()?.parse()?;
                o.timeout = (secs > 0).then(|| Duration::from_secs(secs));
            }
            Short('w') => o.pid = Some(parser.value()?.parse()?),
            Short('h') | Long("help") => return Ok(Parsed::Help),
            Short('V') | Long("version") => return Ok(Parsed::Version),
            Value(program) => {
                // Like getopt: everything from the first non-option on belongs to the utility.
                o.command.push(program);
                o.command.extend(parser.raw_args()?);
                break;
            }
            _ => return Err(arg.unexpected()),
        }
    }

    if !(o.display || o.idle || o.system_on_ac || o.user) {
        o.idle = true;
    }
    Ok(Parsed::Run(o))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Options {
        let full = std::iter::once("caffeinate").chain(args.iter().copied());
        match parse(full).unwrap() {
            Parsed::Run(o) => o,
            other => panic!("expected Run, got {other:?}"),
        }
    }

    fn fails(args: &[&str]) -> bool {
        parse(std::iter::once("caffeinate").chain(args.iter().copied())).is_err()
    }

    #[test]
    fn no_flags_defaults_to_idle() {
        let o = run(&[]);
        assert!(o.idle && !o.display && !o.system_on_ac && !o.user);
        assert_eq!(o.timeout, None);
    }

    #[test]
    fn combined_flags() {
        let o = run(&["-ds"]);
        assert!(o.display && o.system_on_ac && !o.idle);
    }

    #[test]
    fn m_is_idle() {
        assert!(run(&["-m"]).idle);
    }

    #[test]
    fn timeout_and_pid_forms() {
        let o = run(&["-d", "-t", "60", "-w", "1234"]);
        assert_eq!(o.timeout, Some(Duration::from_secs(60)));
        assert_eq!(o.pid, Some(1234));
        assert_eq!(run(&["-t60"]).timeout, Some(Duration::from_secs(60)));
        assert_eq!(run(&["-dt", "5"]).timeout, Some(Duration::from_secs(5)));
    }

    #[test]
    fn zero_timeout_means_none() {
        assert_eq!(run(&["-t", "0"]).timeout, None);
        assert_eq!(run(&["-u", "-t0"]).timeout, None);
    }

    #[test]
    fn user_does_not_set_a_timeout() {
        // -u's own 5 seconds do not end the run (see `Deadlines` in main.rs).
        assert_eq!(run(&["-u"]).timeout, None);
        assert_eq!(
            run(&["-u", "-t", "30"]).timeout,
            Some(Duration::from_secs(30))
        );
    }

    #[test]
    fn command_stops_option_parsing() {
        let o = run(&["-d", "npm", "run", "build", "--watch", "-d"]);
        assert!(o.display);
        assert_eq!(o.command, ["npm", "run", "build", "--watch", "-d"]);
    }

    #[test]
    fn double_dash_starts_command() {
        assert_eq!(run(&["-i", "--", "-weird"]).command, ["-weird"]);
    }

    #[test]
    fn help_and_version() {
        let p = |a: &str| parse(["caffeinate", a]).unwrap();
        assert_eq!(p("-h"), Parsed::Help);
        assert_eq!(p("--help"), Parsed::Help);
        assert_eq!(p("-V"), Parsed::Version);
    }

    #[test]
    fn bad_input() {
        assert!(fails(&["-x"]));
        assert!(fails(&["-t"]));
        assert!(fails(&["-t", "abc"]));
        assert!(fails(&["-w", "-5"]));
        assert!(fails(&["--display"]));
    }
}
