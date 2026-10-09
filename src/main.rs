//! `caffeinate` for Windows: prevents the system and display from sleeping,
//! with the same command line as macOS `caffeinate(8)`.
//!
//! See `docs/design.md` for how the options map to Windows power requests.

#[cfg(not(windows))]
compile_error!("caffeinate-win only runs on Windows. On macOS, use the built-in `caffeinate`.");

mod ac;
mod cli;
mod cmdline;
mod exec;
mod job;
mod power;
mod powershell;
mod wait;

#[cfg(test)]
#[path = "../tests/support/temp_dir.rs"]
mod temp_dir;

use std::ffi::{OsStr, OsString};
use std::io::{self, ErrorKind, Write};
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_BAD_EXE_FORMAT, ERROR_INVALID_PARAMETER, HANDLE,
};

use cli::{Options, Parsed};
use exec::Utility;
use power::PowerRequest;

const EXIT_FAILURE: i32 = 1;
const EXIT_CANNOT_EXECUTE: i32 = 126;
const EXIT_NOT_FOUND: i32 = 127;
const EXIT_INTERRUPTED: i32 = 130;

/// How long `-u` keeps the display on without `-t` (as on macOS).
const USER_ACTIVE_DEFAULT: Duration = Duration::from_secs(5);

/// Longest reason passed to `powercfg /requests`. The kernel rejects reasons
/// longer than 32766 UTF-16 units, which a long command line can reach.
const MAX_REASON_CHARS: usize = 256;

fn main() {
    // process::exit keeps all 32 bits of a utility's exit code; ExitCode only takes a u8.
    std::process::exit(real_main());
}

fn real_main() -> i32 {
    let args: Vec<OsString> = std::env::args_os().collect();
    let opts = match cli::parse(args.iter().cloned()) {
        Ok(Parsed::Run(opts)) => opts,
        Ok(Parsed::Help) => {
            write_out(cli::USAGE);
            return 0;
        }
        Ok(Parsed::Version) => {
            write_out(&format!("caffeinate {}\n", env!("CARGO_PKG_VERSION")));
            return 0;
        }
        Err(e) => {
            write_err(&format!("caffeinate: {e}\n{}", cli::USAGE));
            return EXIT_FAILURE;
        }
    };
    // The utility's arguments as written: everything after the utility's name.
    let raw_args = if opts.command.is_empty() {
        None
    } else {
        cmdline::tail(&args, args.len() - opts.command.len() + 1)
    };
    match run(&opts, raw_args.as_deref()) {
        Ok(code) => code,
        Err(msg) => {
            write_err(&format!("caffeinate: {msg}\n"));
            EXIT_FAILURE
        }
    }
}

/// What caffeinate waits for, besides the timeout and Ctrl+C.
enum Target {
    /// A utility it runs; it ends with the utility.
    Utility(Utility),
    /// The `-w` process.
    Process(OwnedHandle),
    Nothing,
}

fn run(opts: &Options, raw_args: Option<&OsStr>) -> Result<i32, String> {
    // Check the input before changing anything. Ctrl+C is not handled yet, so
    // it still ends caffeinate at once, for example during a slow PATH lookup.
    let utility = match opts.command.split_first() {
        Some((name, args)) => match exec::find(name) {
            Ok(program) => Some((name, program, args)),
            Err(e) => return Ok(not_started(name, &e)),
        },
        None => None,
    };
    let process = match opts.pid {
        // As on macOS, -w is ignored when a utility is given.
        Some(pid) if utility.is_none() => {
            Some(wait::open_process(pid).map_err(|e| process_error(pid, &e))?)
        }
        _ => None,
    };

    let mut request =
        PowerRequest::new(&reason(opts)).map_err(|e| format!("power request: {e}"))?;
    let interrupt = wait::interrupt_event().map_err(|e| format!("interrupt event: {e}"))?;
    if opts.user {
        power::declare_user_active();
    }
    let start = wait::awake_time();
    let deadlines = Deadlines::new(opts, start);
    let ((display, system), _) = deadlines.plan(opts, start);
    request
        .update(display, system)
        .map_err(|e| format!("power request: {e}"))?;

    let target = match (utility, process) {
        (Some((name, program, args)), _) => {
            // Ctrl+C before the utility starts ends caffeinate without starting it.
            if wait::is_set(interrupt) {
                return Ok(EXIT_INTERRUPTED);
            }
            match exec::start(&program, args, raw_args) {
                Ok(utility) => Target::Utility(utility),
                Err(e) => return Ok(not_started(name, &e)),
            }
        }
        (None, Some(process)) => Target::Process(process),
        (None, None) => Target::Nothing,
    };
    hold(opts, &deadlines, &mut request, interrupt, target)
}

/// Keeps the request up to date until the run ends, and returns the exit code.
fn hold(
    opts: &Options,
    deadlines: &Deadlines,
    request: &mut PowerRequest,
    interrupt: HANDLE,
    mut target: Target,
) -> Result<i32, String> {
    let mut handles = vec![interrupt];
    match &target {
        Target::Utility(utility) => handles.push(utility.handle()),
        Target::Process(process) => handles.push(process.as_raw_handle()),
        Target::Nothing => {}
    }
    let mut warned = false;
    loop {
        let now = wait::awake_time();
        // Without a utility, the timeout ends the run.
        if deadlines.expired(now) && !matches!(target, Target::Utility(_)) {
            return Ok(0);
        }
        let ((display, system), next) = deadlines.plan(opts, now);
        if let Err(e) = request.update(display, system) {
            if !warned {
                write_err(&format!("caffeinate: power request: {e}\n"));
                warned = true;
            }
        }
        let timeout = next.map(|next| next.saturating_sub(now));
        match wait::wait_any(&handles, timeout).map_err(|e| format!("wait: {e}"))? {
            Some(0) => match &target {
                // A console utility got the Ctrl+C too, and decides whether to exit.
                Target::Utility(utility) if !utility.gui => wait::rearm(interrupt),
                _ => return Ok(EXIT_INTERRUPTED),
            },
            Some(_) => {
                return match &mut target {
                    Target::Utility(utility) => {
                        utility.exit_code().map_err(|e| format!("wait: {e}"))
                    }
                    _ => Ok(0),
                };
            }
            // A deadline, or time to check the power source again.
            None => {}
        }
    }
}

/// When the parts of the request end, in awake time (see `wait::awake_time`);
/// `None` means never. As on macOS, `-t` applies to everything, and `-u` lasts
/// 5 seconds without it.
struct Deadlines {
    end: Option<Duration>,
    user_end: Option<Duration>,
}

impl Deadlines {
    fn new(opts: &Options, start: Duration) -> Self {
        // A timeout too large to represent is as good as none.
        let end = opts.timeout.and_then(|timeout| start.checked_add(timeout));
        let user_end = match opts.timeout {
            Some(_) => end,
            None => start.checked_add(USER_ACTIVE_DEFAULT),
        };
        Self { end, user_end }
    }

    /// Whether the `-t` timeout has passed.
    fn expired(&self, now: Duration) -> bool {
        self.end.is_some_and(|end| now >= end)
    }

    /// What to hold at `now`, `(display, system)`, and when that may change.
    fn plan(&self, opts: &Options, now: Duration) -> ((bool, bool), Option<Duration>) {
        let held = !self.expired(now);
        let user = opts.user && self.user_end.is_none_or(|end| now < end);
        let state = |on_ac| wanted_state(opts, held, user, on_ac);
        // The power source changes by itself, so while it decides the state
        // (with -s, and nothing else holding the system), it is checked again
        // after a while.
        let poll = state(true) != state(false);
        let next = [self.end.filter(|_| held), self.user_end.filter(|_| user)]
            .into_iter()
            .flatten()
            .chain(poll.then(|| now + ac::POLL_INTERVAL))
            .min();
        (state(poll && ac::on_ac_power()), next)
    }
}

/// Which power request types to hold, `(display, system)`: `held` is whether
/// the `-t` timeout has not passed yet, `user` whether `-u`'s own time has not.
fn wanted_state(opts: &Options, held: bool, user: bool, on_ac: bool) -> (bool, bool) {
    let display = user || (held && opts.display);
    // The system has to stay awake for the display to stay on.
    let system = display || (held && (opts.idle || (opts.system_on_ac && on_ac)));
    (display, system)
}

/// Reports a utility that could not be started and returns the exit code: as
/// on macOS, 127 if it was not found and 126 if it could not be run.
fn not_started(name: &OsStr, e: &io::Error) -> i32 {
    let name = name.to_string_lossy();
    let (code, why) = match e.raw_os_error() {
        _ if e.kind() == ErrorKind::NotFound => (EXIT_NOT_FOUND, "command not found".into()),
        Some(code) if code == ERROR_BAD_EXE_FORMAT as i32 => (
            EXIT_CANNOT_EXECUTE,
            "not a program or batch file (scripts need their interpreter)".into(),
        ),
        _ => (EXIT_CANNOT_EXECUTE, e.to_string()),
    };
    write_err(&format!("caffeinate: {name}: {why}\n"));
    code
}

/// Explains why the `-w` process cannot be waited for.
fn process_error(pid: u32, e: &io::Error) -> String {
    match e.raw_os_error() {
        // What OpenProcess reports for a process ID that does not (or no longer) exist.
        Some(code) if code == ERROR_INVALID_PARAMETER as i32 => {
            format!("process {pid}: no such process")
        }
        Some(code) if code == ERROR_ACCESS_DENIED as i32 => format!(
            "process {pid}: access denied; waiting for another user's process, \
             such as a service, needs an elevated prompt"
        ),
        _ => format!("process {pid}: {e}"),
    }
}

/// Shown by `powercfg /requests`, e.g. "caffeinate -ds: npm run build".
fn reason(opts: &Options) -> String {
    let flags: String = [
        (opts.display, 'd'),
        (opts.idle, 'i'),
        (opts.system_on_ac, 's'),
        (opts.user, 'u'),
    ]
    .iter()
    .filter_map(|&(on, flag)| on.then_some(flag))
    .collect();
    let mut reason = format!("caffeinate -{flags}");
    if !opts.command.is_empty() {
        let command: Vec<_> = opts
            .command
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect();
        reason.push_str(": ");
        reason.push_str(&command.join(" "));
    }
    if reason.chars().count() > MAX_REASON_CHARS {
        reason = reason.chars().take(MAX_REASON_CHARS - 3).collect();
        reason.push_str("...");
    }
    reason
}

/// Writes to stdout. Errors are ignored: if the reader is gone (`caffeinate -h | ...`
/// after the reader quit), there is nobody left to tell.
fn write_out(text: &str) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

/// Writes to stderr, ignoring errors like `write_out`.
fn write_err(text: &str) {
    let _ = io::stderr().lock().write_all(text.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(args: &str) -> Options {
        let args = std::iter::once("caffeinate").chain(args.split_whitespace());
        match cli::parse(args).unwrap() {
            Parsed::Run(opts) => opts,
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn flags_map_to_request_types() {
        // (flags, held, user, on AC) -> (display, system)
        let cases = [
            ("-d", true, false, false, (true, true)),
            ("-i", true, false, false, (false, true)),
            ("-m", true, false, false, (false, true)),
            ("", true, false, false, (false, true)),
            ("-s", true, false, true, (false, true)),
            ("-s", true, false, false, (false, false)),
            ("-ds", true, false, false, (true, true)),
            ("-is", true, false, false, (false, true)),
            ("-u", true, true, false, (true, true)),
            ("-u", true, false, false, (false, false)),
            ("-su", true, false, true, (false, true)),
            ("-dimsu", false, false, true, (false, false)),
        ];
        for (flags, held, user, on_ac, expected) in cases {
            assert_eq!(
                wanted_state(&opts(flags), held, user, on_ac),
                expected,
                "{flags:?}, held {held}, user {user}, on AC {on_ac}"
            );
        }
    }

    #[test]
    fn user_activity_lasts_five_seconds_or_the_timeout() {
        let start = Duration::from_secs(100);
        let state_at = |flags: &str, secs| {
            let opts = opts(flags);
            let deadlines = Deadlines::new(&opts, start);
            deadlines.plan(&opts, start + Duration::from_secs(secs)).0
        };
        assert_eq!(state_at("-u", 4), (true, true));
        assert_eq!(state_at("-u", 5), (false, false));
        // -u does not cut the other flags short: -iu holds the system until Ctrl+C.
        assert_eq!(state_at("-iu", 3600), (false, true));
        assert_eq!(state_at("-u -t 30", 29), (true, true));
        assert_eq!(state_at("-du -t 30", 30), (false, false));
    }

    #[test]
    fn only_the_timeout_ends_the_run() {
        let start = Duration::from_secs(100);
        let expired = |flags: &str, secs| {
            Deadlines::new(&opts(flags), start).expired(start + Duration::from_secs(secs))
        };
        assert!(!expired("-u", 3600));
        assert!(!expired("-t 30", 29));
        assert!(expired("-t 30", 30));
        // A timeout too large to represent is as good as none.
        assert!(!expired("-t 18446744073709551615", u64::MAX / 2));
    }

    #[test]
    fn wakes_up_only_when_the_state_can_change() {
        let start = Duration::from_secs(100);
        let next = |flags: &str| {
            let opts = opts(flags);
            Deadlines::new(&opts, start).plan(&opts, start).1
        };
        assert_eq!(next("-d"), None);
        assert_eq!(next("-iu"), Some(start + USER_ACTIVE_DEFAULT));
        assert_eq!(next("-d -t 30"), Some(start + Duration::from_secs(30)));
        // The power source decides with -s alone, but not when -d holds the system anyway.
        assert_eq!(next("-s"), Some(start + ac::POLL_INTERVAL));
        assert_eq!(next("-ds"), None);
    }

    #[test]
    fn reason_names_the_flags_and_the_command() {
        let opts = Options {
            display: true,
            system_on_ac: true,
            command: vec!["npm".into(), "run".into(), "build".into()],
            ..Options::default()
        };
        assert_eq!(reason(&opts), "caffeinate -ds: npm run build");
    }

    #[test]
    fn long_reasons_are_truncated() {
        let opts = Options {
            idle: true,
            command: vec![OsString::from("x".repeat(40_000))],
            ..Options::default()
        };
        let reason = reason(&opts);
        assert_eq!(reason.chars().count(), MAX_REASON_CHARS);
        assert!(reason.ends_with("..."));
    }
}
