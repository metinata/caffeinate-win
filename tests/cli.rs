//! End-to-end tests against the built `caffeinate.exe`.
//!
//! The power tests read the system-wide execution state. Every test that starts
//! caffeinate with a power request holds `SERIAL`, so the tests never see each
//! other's state. Other programs (a video player, a keep-awake tool, ...) can
//! hold the same state, so the tests compare against the state before
//! caffeinate starts, cannot check bits that are already set, and warn about
//! them. Close such programs for meaningful results. Which flags hold what is
//! unit tested in `src/main.rs`.

#![cfg(windows)]

// Shared with caffeinate itself.
#[allow(dead_code)]
#[path = "../src/ac.rs"]
mod ac;
#[allow(dead_code)]
#[path = "../src/job.rs"]
mod job;
#[allow(dead_code)]
#[path = "support/temp_dir.rs"]
mod temp_dir;
#[allow(dead_code)]
#[path = "../src/wait.rs"]
mod wait;

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::{Mutex, MutexGuard, Once, PoisonError};
use std::thread::{self, JoinHandle, sleep};
use std::time::{Duration, Instant};

use temp_dir::TempDir;
use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent};
use windows_sys::Win32::System::Power::{CallNtPowerInformation, SystemExecutionState};
use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetLastInputInfo, INPUT, INPUT_0, INPUT_MOUSE, LASTINPUTINFO, MOUSEEVENTF_MOVE, MOUSEINPUT,
    SendInput,
};

const SYSTEM: u32 = 0x1;
const DISPLAY: u32 = 0x2;
const BOTH: u32 = SYSTEM | DISPLAY;

/// How long a caffeinate run may take before the test fails.
const TIMEOUT: Duration = Duration::from_secs(10);

/// caffeinate changes system-wide state, so tests that start it run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

fn exe() -> Command {
    command(Path::new(env!("CARGO_BIN_EXE_caffeinate")))
}

fn command(program: &Path) -> Command {
    // Programs that caffeinate leaves running on purpose (a GUI utility after
    // Ctrl+Break, or what a utility started) must not outlive the test run:
    // the test process joins a job that ends them when it exits.
    static JOB: Once = Once::new();
    JOB.call_once(|| {
        if let Ok(job) = job::Job::enter() {
            std::mem::forget(job);
        }
    });
    Command::new(program)
}

fn system32(name: &str) -> PathBuf {
    Path::new(&std::env::var_os("SystemRoot").unwrap())
        .join("System32")
        .join(name)
}

/// A child process that is killed if the test ends before it exits, so that a
/// failing test cannot leave the system held awake.
struct Running(Child);

impl Running {
    fn new(command: &mut Command) -> Self {
        Self(command.spawn().unwrap())
    }

    fn wait_timeout(&mut self, timeout: Duration) -> Option<ExitStatus> {
        exits_within(&self.0, timeout).then(|| self.0.wait().unwrap())
    }

    fn is_running(&mut self) -> bool {
        self.0.try_wait().unwrap().is_none()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn(args: &[&str]) -> Running {
    Running::new(exe().args(args).stdout(Stdio::null()).stderr(Stdio::null()))
}

/// Spawns caffeinate in its own process group, so that Ctrl+Break can be sent to it alone.
fn spawn_in_group(args: &[&str]) -> Running {
    Running::new(
        exe()
            .args(args)
            .creation_flags(CREATE_NEW_PROCESS_GROUP)
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
    )
}

/// Runs a command to completion and returns its output. Kills it and fails the
/// test if it takes longer than `TIMEOUT`.
fn run(command: &mut Command) -> Output {
    let mut child = Running::new(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    );
    let stdout = read_in_background(child.0.stdout.take().unwrap());
    let stderr = read_in_background(child.0.stderr.take().unwrap());
    let status = child
        .wait_timeout(TIMEOUT)
        .expect("caffeinate did not exit");
    Output {
        status,
        stdout: stdout.join().unwrap(),
        stderr: stderr.join().unwrap(),
    }
}

fn read_in_background(mut pipe: impl Read + Send + 'static) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut out = Vec::new();
        let _ = pipe.read_to_end(&mut out);
        out
    })
}

fn output(args: &[&str]) -> Output {
    run(exe().args(args))
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The system-wide execution state: which of SYSTEM and DISPLAY are held.
fn state() -> u32 {
    let mut out = 0u32;
    // SAFETY: SystemExecutionState writes a single u32 to the output buffer.
    let status = unsafe {
        CallNtPowerInformation(
            SystemExecutionState,
            std::ptr::null(),
            0,
            (&raw mut out).cast(),
            4,
        )
    };
    assert_eq!(status, 0, "CallNtPowerInformation failed");
    out & BOTH
}

/// The state before caffeinate starts. Bits that are already set cannot be checked.
fn baseline() -> u32 {
    let state = state();
    if state != 0 {
        eprintln!(
            "warning: another program keeps the system awake ({state:#x}), so these bits are not checked"
        );
    }
    state
}

/// Polls until `f` holds, for up to five seconds.
fn eventually(f: impl Fn() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if f() {
            return true;
        }
        sleep(Duration::from_millis(50));
    }
    f()
}

/// Whether the process exits within `timeout`.
fn exits_within(process: &impl AsRawHandle, timeout: Duration) -> bool {
    wait::wait_any(&[process.as_raw_handle()], Some(timeout)).unwrap() == Some(0)
}

/// Sends Ctrl+Break to a caffeinate started with `spawn_in_group` once it holds
/// the display. Returns `false` when there is no console to send it through.
fn ctrl_break(child: &Running, baseline: u32) -> bool {
    // The console handler is installed before the request is applied, so once
    // the bits show up, Ctrl+Break is handled. If they were set already, wait.
    assert!(eventually(|| state() & BOTH == BOTH));
    if baseline & BOTH == BOTH {
        sleep(Duration::from_millis(300));
    }
    // SAFETY: targets the process group created for this child alone.
    let sent = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.0.id()) != 0 };
    if !sent {
        eprintln!("skipped: no console to deliver Ctrl+Break to");
    }
    sent
}

/// Starts caffeinate with `args`, checks that it holds exactly the `expected`
/// bits, and that they are released when it is killed.
fn assert_holds(args: &[&str], expected: u32) {
    let _serial = serial();
    let baseline = baseline();
    let mut child = spawn(args);
    assert!(
        eventually(|| state() & expected == expected),
        "{args:?}: expected bits {expected:#x}, state is {:#x}",
        state()
    );
    if expected == 0 {
        // Nothing to observe: give caffeinate time to (wrongly) set something.
        sleep(Duration::from_millis(500));
    }
    let unexpected = BOTH & !expected & !baseline;
    assert_eq!(
        state() & unexpected,
        0,
        "{args:?} holds more than {expected:#x}"
    );
    assert!(child.is_running(), "{args:?}: caffeinate exited");
    drop(child);
    assert!(
        eventually(|| state() == baseline),
        "{args:?}: state {:#x} not released",
        state()
    );
}

#[test]
fn display_flag_holds_display_and_system() {
    assert_holds(&["-d"], BOTH);
}

#[test]
fn idle_flag_holds_system_only() {
    assert_holds(&["-i"], SYSTEM);
}

#[test]
fn no_flags_means_idle() {
    assert_holds(&[], SYSTEM);
}

#[test]
fn user_flag_holds_the_display() {
    assert_holds(&["-u"], BOTH);
}

/// When the session last got input, real or simulated, as a tick count.
fn last_input() -> u32 {
    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: `info` is a LASTINPUTINFO with its size set.
    assert_ne!(unsafe { GetLastInputInfo(&raw mut info) }, 0);
    info.dwTime
}

/// Whether simulated input reaches this session, which needs an unlocked desktop.
fn can_send_input() -> bool {
    let before = last_input();
    sleep(Duration::from_millis(50));
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: MOUSEEVENTF_MOVE,
                ..MOUSEINPUT::default()
            },
        },
    };
    // SAFETY: `input` is one INPUT of the size passed.
    let sent = unsafe { SendInput(1, &raw const input, size_of::<INPUT>() as i32) } == 1;
    sent && eventually(|| last_input() != before)
}

#[test]
fn user_flag_counts_as_input() {
    let _serial = serial();
    if !can_send_input() {
        eprintln!("skipped: simulated input does not reach this session");
        return;
    }
    // Input is what turns a display back on; resetting the idle timers is not enough.
    let before = last_input();
    sleep(Duration::from_millis(50));
    assert!(output(&["-u", "cmd", "/c", "exit"]).status.success());
    assert!(
        eventually(|| last_input() != before),
        "caffeinate -u sent no input"
    );
}

#[test]
fn system_flag_follows_the_power_source() {
    if ac::on_ac_power() {
        assert_holds(&["-s"], SYSTEM);
    } else {
        // On battery, -s must not hold anything.
        assert_holds(&["-s"], 0);
    }
}

#[test]
fn timeout_ends_the_run_and_releases_the_request() {
    let _serial = serial();
    let baseline = baseline();
    let start = Instant::now();
    assert!(output(&["-d", "-t", "1"]).status.success());
    assert!(start.elapsed() < Duration::from_secs(4));
    assert!(
        eventually(|| state() == baseline),
        "state {:#x} not released",
        state()
    );
}

#[test]
fn timeout_releases_while_the_utility_keeps_running() {
    let _serial = serial();
    let baseline = baseline();
    // ping -n 4 takes about three seconds; -d and -u hold the display for one.
    let mut child = Running::new(
        exe()
            .args(["-du", "-t", "1", "ping", "-n", "4", "127.0.0.1"])
            .stdout(Stdio::null()),
    );
    assert!(eventually(|| state() & BOTH == BOTH));
    assert!(
        eventually(|| state() == baseline),
        "not released at the timeout"
    );
    assert!(
        child.is_running(),
        "caffeinate did not wait for the utility"
    );
    let status = child
        .wait_timeout(TIMEOUT)
        .expect("caffeinate did not exit");
    assert!(status.success());
}

#[test]
fn huge_timeouts_do_not_crash() {
    let _serial = serial();
    let mut child = spawn(&["-i", "-t", "18446744073709551615"]);
    assert!(
        child.wait_timeout(Duration::from_millis(300)).is_none(),
        "caffeinate exited"
    );
}

#[test]
fn killing_caffeinate_releases_the_request() {
    let _serial = serial();
    let baseline = baseline();
    let mut child = spawn(&["-d"]);
    assert!(eventually(|| state() & BOTH == BOTH));
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    assert!(
        eventually(|| state() == baseline),
        "state {:#x} not released after kill",
        state()
    );
}

#[test]
fn ctrl_break_exits_130_and_releases_the_request() {
    let _serial = serial();
    let baseline = baseline();
    let mut child = spawn_in_group(&["-d"]);
    if !ctrl_break(&child, baseline) {
        return;
    }
    let status = child
        .wait_timeout(Duration::from_secs(5))
        .expect("caffeinate ignored Ctrl+Break");
    assert_eq!(status.code(), Some(130));
    assert!(
        eventually(|| state() == baseline),
        "state {:#x} not released",
        state()
    );
}

#[test]
fn ctrl_break_goes_to_a_console_utility() {
    let _serial = serial();
    let baseline = baseline();
    // ping shows its statistics on Ctrl+Break and keeps going.
    let mut child = Running::new(
        exe()
            .args(["-d", "ping", "-n", "3", "127.0.0.1"])
            .creation_flags(CREATE_NEW_PROCESS_GROUP)
            .stdout(Stdio::piped()),
    );
    // Once ping prints a reply, it runs and is attached to the console.
    let mut out = BufReader::new(child.0.stdout.take().unwrap());
    let mut line = String::new();
    while !line.contains("127.0.0.1:") {
        line.clear();
        assert_ne!(out.read_line(&mut line).unwrap(), 0, "ping did not reply");
    }
    if !ctrl_break(&child, baseline) {
        return;
    }
    let mut rest = String::new();
    out.read_to_string(&mut rest).unwrap();
    let status = child
        .wait_timeout(TIMEOUT)
        .expect("caffeinate did not exit");
    // ping's exit code rather than 130: caffeinate waited for ping to finish.
    assert_eq!(status.code(), Some(0), "{rest}");
}

#[test]
fn ctrl_break_ends_caffeinate_when_the_utility_is_a_gui_program() {
    let wscript = system32("wscript.exe");
    if !wscript.is_file() {
        eprintln!("skipped: wscript.exe (a GUI-subsystem program) is not available");
        return;
    }
    let _serial = serial();
    let baseline = baseline();
    let dir = TempDir::new("gui");
    // Runs without a window for 10 seconds, and never receives console Ctrl+Break.
    let started = dir.path().join("started");
    let script = dir.file(
        "sleep.js",
        &format!(
            "new ActiveXObject('Scripting.FileSystemObject').CreateTextFile('{}').Close();\n\
             WScript.Sleep(10000);\n",
            started.display().to_string().replace('\\', "\\\\")
        ),
    );
    let mut child = spawn_in_group(&[
        "-d",
        wscript.to_str().unwrap(),
        "//B",
        script.to_str().unwrap(),
    ]);
    assert!(eventually(|| started.exists()), "wscript did not start");
    if !ctrl_break(&child, baseline) {
        return;
    }
    // 130 rather than wscript's 0: caffeinate stopped waiting while wscript still runs.
    let status = child
        .wait_timeout(Duration::from_secs(5))
        .expect("Ctrl+Break was swallowed");
    assert_eq!(status.code(), Some(130));
}

#[test]
fn waits_for_pid() {
    let _serial = serial();
    let target = Running::new(
        Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null()),
    );
    let mut child = spawn(&["-i", "-w", &target.0.id().to_string()]);
    assert!(
        child.wait_timeout(Duration::from_millis(500)).is_none(),
        "caffeinate exited before the process did"
    );
    drop(target);
    let status = child
        .wait_timeout(Duration::from_secs(5))
        .expect("caffeinate kept running after the process exited");
    assert!(status.success());
}

#[test]
fn unknown_pids_are_an_error() {
    // Windows ignores the two low bits of a process ID, so this test's own ID
    // plus one would open this test process without the check.
    for pid in [4_294_967_292, std::process::id() + 1] {
        let out = output(&["-w", &pid.to_string()]);
        assert_eq!(out.status.code(), Some(1));
        assert!(
            stderr(&out).contains(&format!("process {pid}: no such process")),
            "{}",
            stderr(&out)
        );
    }
}

#[test]
fn passes_through_utility_exit_code() {
    let _serial = serial();
    assert_eq!(output(&["cmd", "/c", "exit 7"]).status.code(), Some(7));
    assert_eq!(output(&["cmd", "/c", "exit 300"]).status.code(), Some(300));
    assert_eq!(
        output(&["cmd", "/c", "exit -1073741510"]).status.code(),
        Some(-1_073_741_510)
    );
}

#[test]
fn utility_arguments_are_not_parsed_as_options() {
    let _serial = serial();
    let out = output(&["-d", "cmd", "/c", "echo", "-t", "-x"]);
    assert!(out.status.success());
    assert_eq!(stdout(&out), "-t -x");
}

#[test]
fn utility_arguments_are_passed_on_as_written() {
    let _serial = serial();
    // cmd /c echo prints its command line as it gets it, quotes included.
    let out = run(exe().raw_arg(r#"cmd /c echo "quoted" PROP="a b""#));
    assert_eq!(stdout(&out), r#""quoted" PROP="a b""#);
}

#[test]
fn batch_file_arguments_are_quoted_safely() {
    let _serial = serial();
    let dir = TempDir::new("batch-args");
    let script = dir.file("probe.cmd", "@echo %1\r\n");
    // Passed on as written, `a&b` would make cmd.exe run `b` as a second command.
    let out = run(exe().arg(&script).arg("a&b"));
    assert_eq!(stdout(&out), r#""a&b""#, "{}", stderr(&out));
}

/// `PATH` with `dir` in front.
fn path_with(dir: &Path) -> std::ffi::OsString {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::join_paths(std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&path)))
        .unwrap()
}

#[test]
fn finds_scripts_on_path_through_pathext() {
    let _serial = serial();
    let dir = TempDir::new("pathext");
    dir.file("caffeinate-probe.cmd", "@exit /b 42\r\n");
    let out = run(exe()
        .arg("caffeinate-probe")
        .env("PATH", path_with(dir.path())));
    assert_eq!(out.status.code(), Some(42));
}

#[test]
fn relative_paths_use_pathext() {
    let _serial = serial();
    let dir = TempDir::new("relative");
    dir.file("gradlew", "#!/bin/sh\n"); // Next to gradlew.bat, as in a Gradle checkout.
    dir.file("gradlew.bat", "@exit /b 42\r\n");
    let out = run(exe().arg(r".\gradlew").current_dir(dir.path()));
    assert_eq!(out.status.code(), Some(42), "{}", stderr(&out));
}

#[test]
fn the_current_directory_is_not_searched() {
    let _serial = serial();
    let dir = TempDir::new("cwd");
    dir.file("caffeinate-probe.cmd", "@exit /b 42\r\n");
    let out = run(exe().arg("caffeinate-probe").current_dir(dir.path()));
    assert_eq!(out.status.code(), Some(127));
}

#[test]
fn the_directory_of_caffeinate_is_not_searched() {
    let _serial = serial();
    let dir = TempDir::new("app-dir");
    let copy = dir.path().join("caffeinate.exe");
    fs::copy(env!("CARGO_BIN_EXE_caffeinate"), &copy).unwrap();
    fs::copy(
        system32("whoami.exe"),
        dir.path().join("caffeinate-probe.exe"),
    )
    .unwrap();
    let out = run(command(&copy).arg("caffeinate-probe"));
    assert_eq!(out.status.code(), Some(127), "{}", stderr(&out));
}

#[test]
fn refuses_a_script_that_an_exe_would_shadow() {
    let _serial = serial();
    let dir = TempDir::new("shadow");
    dir.file("tool.cmd", "@exit /b 42\r\n");
    fs::copy(system32("whoami.exe"), dir.path().join("tool.cmd.exe")).unwrap();
    let out = run(exe().arg("tool").env("PATH", path_with(dir.path())));
    assert_eq!(out.status.code(), Some(126));
    assert!(
        stderr(&out).contains("would run instead"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn files_that_cannot_run_exit_126() {
    let _serial = serial();
    let dir = TempDir::new("not-a-program");
    dir.file("notes.txt", "hello");
    let out = run(exe().arg(r".\notes.txt").current_dir(dir.path()));
    assert_eq!(out.status.code(), Some(126), "{}", stderr(&out));
}

#[test]
fn missing_utilities_exit_127() {
    let _serial = serial();
    let dir = TempDir::new("missing");
    for name in ["definitely-not-a-real-command-xyz", r".\missing.cmd"] {
        let out = run(exe().arg(name).current_dir(dir.path()));
        assert_eq!(out.status.code(), Some(127), "{name}");
        assert!(
            stderr(&out).contains("command not found"),
            "{}",
            stderr(&out)
        );
    }
}

#[test]
fn killing_caffeinate_ends_the_utility() {
    let _serial = serial();
    let mut child = Running::new(
        exe()
            .args([
                "powershell",
                "-NoProfile",
                "-Command",
                "$PID; Start-Sleep 30",
            ])
            .stdout(Stdio::piped()),
    );
    let mut line = String::new();
    BufReader::new(child.0.stdout.as_mut().unwrap())
        .read_line(&mut line)
        .unwrap();
    let pid: u32 = line
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("expected a PID, got {line:?}"));
    let utility = wait::open_process(pid).unwrap();
    drop(child);
    assert!(
        exits_within(&utility, Duration::from_secs(5)),
        "the utility outlived caffeinate"
    );
}

#[test]
fn what_the_utility_started_outlives_a_normal_exit() {
    let _serial = serial();
    let dir = TempDir::new("background");
    // Starts a background command that writes a file a second after the utility,
    // and with it caffeinate, has exited.
    let out = run(exe()
        .raw_arg(r#"cmd /c start "" /b cmd /c "ping -n 2 127.0.0.1 >nul & echo alive> marker.txt""#)
        .current_dir(dir.path()));
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        eventually(|| dir.path().join("marker.txt").exists()),
        "the background command was ended with caffeinate"
    );
}

#[test]
fn windows_powershell_does_not_see_powershell_7_modules() {
    let _serial = serial();
    // A PowerShell 7 installation keeps its modules next to pwsh.dll.
    let install = TempDir::new("pwsh-install");
    install.file("pwsh.dll", "");
    let pwsh_modules = install.path().join("Modules");
    let other = Path::new(r"C:\caffeinate-test\Modules");
    let module_path = std::env::join_paths([pwsh_modules.as_path(), other]).unwrap();
    let out = run(exe()
        .args(["powershell", "-NoProfile", "-Command", "$env:PSModulePath"])
        .env("PSModulePath", module_path));
    let seen = stdout(&out);
    assert!(!seen.contains(pwsh_modules.to_str().unwrap()), "{seen}");
    assert!(seen.contains(other.to_str().unwrap()), "{seen}");
}

#[test]
fn usage_errors_exit_1() {
    let out = output(&["-x"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("usage: caffeinate"));
}

#[test]
fn help_and_version() {
    let help = output(&["-h"]);
    assert!(help.status.success());
    assert!(stdout(&help).starts_with("usage: caffeinate"));

    let version = output(&["--version"]);
    assert!(version.status.success());
    assert_eq!(
        stdout(&version),
        concat!("caffeinate ", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn does_not_need_the_visual_cpp_runtime() {
    // .cargo/config.toml links the C runtime statically; RUSTFLAGS overrides that.
    let exe = fs::read(env!("CARGO_BIN_EXE_caffeinate"))
        .unwrap()
        .to_ascii_lowercase();
    assert!(
        !exe.windows(b"vcruntime".len()).any(|w| w == b"vcruntime"),
        "caffeinate.exe needs vcruntime140.dll; is RUSTFLAGS set?"
    );
}
