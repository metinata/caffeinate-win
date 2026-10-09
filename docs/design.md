# Design

This document explains how caffeinate for Windows maps the macOS command onto
Windows, and why it is built the way it is.

## Goals

- **Compatibility with macOS `caffeinate(8)`.** The same options and the same
  behavior, so one command works on both systems.
- **A single, small, self-contained executable.** It needs no runtime and runs no
  service. The C runtime is linked statically (`.cargo/config.toml`), so the release
  build depends only on DLLs that ship with Windows.
- **No stuck state.** However caffeinate ends (normal exit, Ctrl+C, closed window,
  killed from Task Manager), the system must return to its normal sleep behavior.
- **Transparency.** Users can see what caffeinate holds and why with
  `powercfg /requests`.

Out of scope: a tray icon or other GUI, running as a service, and preventing sleep
that the user triggers explicitly (power button, Start menu, lid), which macOS
`caffeinate` does not prevent either.

## Mapping the options

macOS behavior, from `caffeinate(8)` and Apple's source:

- With none of `-d`, `-i`, `-m`, `-s`, `-u`, caffeinate acts as if `-i` was given.
- `-t` sets how long the assertions last; `-t 0` means no timeout. Without a
  utility, caffeinate also exits when the timeout elapses.
- `-u` lasts 5 seconds when no `-t` is given. That applies to `-u` alone: the
  other assertions are not shortened, and caffeinate keeps running until it is
  interrupted, so `caffeinate -u -t 1` is how a script wakes the display.
- With a utility, `-w` is ignored, `-t` still limits the assertions, and
  caffeinate exits with the utility's exit status.

On Windows each option becomes a state of one power request:

| Option | macOS | Windows |
|---|---|---|
| `-d` | Prevent display sleep | `PowerRequestDisplayRequired` + `PowerRequestSystemRequired` |
| `-i` | Prevent idle system sleep | `PowerRequestSystemRequired` |
| `-m` | Prevent idle disk sleep | Same as `-i`; Windows offers no per-process request for disks |
| `-s` | Prevent system sleep, on AC power only | `PowerRequestSystemRequired` while `GetSystemPowerStatus` reports AC; re-checked every 5 seconds |
| `-u` | Declare user activity | A mouse movement of zero distance (`SendInput`) to turn the display on, `SetThreadExecutionState(ES_DISPLAY_REQUIRED \| ES_SYSTEM_REQUIRED)` once to reset the idle timers, plus the `-d` requests for 5 seconds or the timeout |
| `-t` | Timeout | Releases the requests when it elapses, and ends the run without a utility |
| `-w` | Wait for a PID | `OpenProcess(SYNCHRONIZE)` and wait on the handle |
| utility | Run it, hold while it runs | `CreateProcess` (through `std::process::Command`) and wait on the process handle |

`-d` also sets `PowerRequestSystemRequired`: the system must stay awake for the
display to stay on.

On macOS, `-u` turns the display on if it is off. On Windows, neither
`SetThreadExecutionState`, nor a display power request, nor
`SC_MONITORPOWER` turns on a display that the idle timeout has already turned off
(measured on Windows 11 by watching `GUID_CONSOLE_DISPLAY_STATE`); only input
does. caffeinate therefore sends a mouse movement of zero distance, which leaves
the pointer where it is. It loads user32.dll for that alone, so that runs without
`-u` do not load the GUI libraries. If the screen is locked, the display turns on
and shows the lock screen, as on macOS. A session without a desktop (a service,
or SSH) cannot send input; `-u` then only resets the idle timers.

Deadlines are measured in time the system is awake (`QueryUnbiasedInterruptTime`),
which is also what wait timeouts count on Windows 8 and later. Time asleep, after
the user closed the lid for example, does not count against `-t`, as on macOS.

`-w` needs a handle to the process. From a normal prompt, Windows grants one for
the user's own processes but not for services or other users' processes; waiting
for those needs an elevated prompt. caffeinate says so when access is denied. The
kernel ignores the two low bits of a process ID, which are 0 in every real one, so
caffeinate rejects other IDs instead of waiting for a neighboring process.

## Why power requests

Windows has two user-mode APIs for this.

`SetThreadExecutionState` sets flags on the calling thread. It is simple, but
anonymous: nothing tells the user which program keeps the PC awake.

[Power requests](https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-powercreaterequest)
(`PowerCreateRequest`, `PowerSetRequest`, `PowerClearRequest`) carry a reason
string that `powercfg /requests` shows next to the process. caffeinate uses its
command line as the reason, for example `caffeinate -d: npm run build`, cut to 256
characters because the kernel rejects very long reasons.

Both are released by the kernel when the process ends: closing the request's
handle releases it, and the kernel closes all handles of a process that ends.
Since caffeinate never changes persistent settings (power plans, registry, config
files), killing it cannot leave the system in a modified state.

`PowerSetRequest` and `PowerClearRequest` are reference counted per request type.
`PowerRequest::update` therefore only forwards state transitions, so a set is never
issued twice in a row.

## Program flow

1. Parse the arguments (`src/cli.rs`). Parsing follows POSIX `getopt` through
   [lexopt](https://github.com/blyxxyz/lexopt): flags can be combined (`-ds`), option
   values can be attached (`-t60`), and parsing stops at the first operand, so the
   utility's own options are passed to it untouched (`caffeinate -d npm run dev --port 3000`).
2. Check the input before changing anything: find the utility, or open the `-w`
   process. Invalid input therefore has no side effects. Ctrl+C is not handled
   yet, so it ends caffeinate at once, even during a slow `PATH` lookup.
3. Create the power request with the reason string (`src/power.rs`).
4. Create an event and register a console control handler that signals it on
   Ctrl+C and Ctrl+Break (`src/wait.rs`). A program that `start /B` runs in the
   background starts with Ctrl+C turned off; caffeinate keeps it that way, so only
   Ctrl+Break ends it, and the utility inherits the same setting.
5. With `-u`, declare the user active once: turn the display on and reset the
   idle timers. Apply the requested state.
6. With a utility: if Ctrl+C was pressed by now, exit without starting it;
   otherwise start it.
7. Wait (`src/main.rs`, `src/wait.rs`): `WaitForMultipleObjects` on the interrupt
   event and the utility's or the `-w` process's handle, with a timeout up to the
   next time the state changes (the end of `-u` or `-t`). While `-s` decides
   whether the system is held, the wait also wakes up every 5 seconds to check
   the power source again. Otherwise the thread sleeps until something happens.
8. Exit with the right code. The kernel releases the request.

Output goes through `write_all` with errors ignored, so `caffeinate -h` into a
pipe whose reader has quit does not panic.

## Running a utility

Implemented in `src/exec.rs` and `src/cmdline.rs`.

### Finding the program

`std::process::Command` only looks for `.exe` files, but many command-line tools
are scripts: `npm` is `npm.cmd`, and a Gradle project has `gradlew.bat` next to a
POSIX `gradlew` script. caffeinate therefore follows `cmd.exe`'s `PATHEXT` rules,
limited to the file types it can start: programs (`.exe`, `.com`) and batch files
(`.bat`, `.cmd`), which std runs through `cmd.exe`. Other script types (`.vbs`,
`.js`, `.py`) need their interpreter and are skipped, so `deploy.js` early in
`PATH` does not hide `deploy.exe` later on.

- A name with a `PATHEXT` extension is used as is (`tool.cmd`). Any other name is
  tried with each startable `PATHEXT` extension in turn; a name with some other
  extension (`notes.txt`) is tried as is first, so that a file that exists but
  cannot be run is reported as such (exit status `126`).
- A name with a directory (`.\gradlew`, `C:\tools\build`) is looked up only there.
- A bare name is searched for in each directory of `PATH`. The current directory
  is not searched. That matches PowerShell and macOS `execvp`, and avoids running
  a program planted in whatever directory the user happens to be in.
- A name that is not found is reported as not found (exit status `127`). It is
  not handed to std, whose own search would also look in caffeinate's directory
  and the Windows directories.

The program is passed to std as a full path. For a path that does not end in
`.exe`, std first tries the path with `.exe` added, and starts that if anything by
that name exists. A `tool.cmd.exe` next to `tool.cmd` would therefore run instead
of the script that was found. caffeinate detects this case and refuses to start
either program, rather than run a different one than it reports.

### Passing the arguments

Windows passes a program its command line as one string, which the program splits
into arguments itself. Most programs use the C runtime's rules, but some read the
string their own way: `cmd /c`, `find "text" file` and `msiexec PROP="a b"` all
depend on where the quotes are. Splitting caffeinate's command line and quoting
the arguments again for the utility would move those quotes. caffeinate instead
splits its own command line with std's rules, checks that the result matches
`std::env::args_os`, and passes everything after the utility's name on unchanged,
as `cmd.exe` does when it runs a program.

The shell still decides how caffeinate's own command line is written. PowerShell
7.3 and later escape quotes inside arguments (`\"`) for all programs except a
built-in list of ones that parse their command line their own way (`cmd`, `find`,
`cscript`, `wscript`, `sqlcmd`, and scripts). caffeinate is not on that list, so
those programs get the escaped form through it. caffeinate cannot undo this
without knowing which shell started it, since `cmd.exe` writes the same quotes
unescaped; the README points PowerShell users to
`$PSNativeCommandArgumentPassing = 'Legacy'`.

Batch files are the exception. `cmd.exe` would treat characters such as `&` and
`%` in a raw argument as commands, so their arguments go through std's quoting for
batch files, which quotes them safely and refuses arguments it cannot quote safely.

### Windows PowerShell

The environment is passed on unchanged, with one exception (`src/powershell.rs`).
PowerShell 7 puts its own module folders in front of `PSModulePath`, and every
program it starts inherits them. Windows PowerShell started by such a program
finds PowerShell 7's modules first, and they fail to load: PSReadLine (the console
prints "Cannot load PSReadline module"), and with the default `Restricted`
execution policy even the module of `Get-ExecutionPolicy`. PowerShell 7 avoids
this when it starts `powershell.exe` or `powershell_ise.exe` itself, by removing
its folders from the child's `PSModulePath`: `PowerShell\Modules` in Documents and
in Program Files, and any folder next to `pwsh.dll`. caffeinate applies the same
rule to the same programs, so `caffeinate powershell` behaves like `powershell`.
The Documents folder is read from the shell's record of it, so a folder that was
moved, to OneDrive for example, is found too.

### Lifetime and Ctrl+C

On macOS, caffeinate runs the utility in its own process (it calls `execvp` after
forking off a process that holds the assertions), so whatever ends caffeinate ends
the utility. On Windows the utility is a child process. To keep that behavior,
caffeinate puts itself in a job object before starting the utility, which joins
the job too. The job ends all its processes when its last handle is closed, and
only caffeinate holds one: if caffeinate is killed, or its console window is
closed, the utility and what it started end with it. When caffeinate exits by
itself, it lifts that limit first, so whatever is still running keeps running,
such as a server that the utility started in the background, or a GUI utility
after Ctrl+C. Processes that leave the job on purpose (`CREATE_BREAKAWAY_FROM_JOB`)
are allowed to.

A console utility shares caffeinate's console, so Ctrl+C reaches both.
caffeinate's handler reports the event as handled, so the utility decides: if it
exits, caffeinate exits with its exit code; if it handles Ctrl+C and keeps running
(a REPL, for example), caffeinate keeps waiting. This is how macOS caffeinate
behaves too: the process holding the assertions ignores SIGINT.

A GUI program is not attached to the console and never receives Ctrl+C. caffeinate
reads the subsystem from the program's PE header before starting it, and on Ctrl+C
with a GUI program, it stops waiting and exits with `130`, leaving the program
running. Asking the console which processes are attached would not do: a console
program appears there only after it has started up, so a Ctrl+C right after the
start would take it for a GUI program.

## Exit status

| Code | Meaning |
|---|---|
| `0` | The timeout elapsed or the watched process exited |
| utility's code | A utility was given; all 32 bits are passed through via `std::process::exit` |
| `1` | Invalid arguments, an unknown process ID, or a Windows API error |
| `126` | The utility was found but could not be started (as on macOS) |
| `127` | The utility was not found (the shell convention) |
| `130` | Ctrl+C or Ctrl+Break ended caffeinate (the shell convention for SIGINT) |

Ctrl+C before caffeinate handles it (step 2 above) ends the process the default
Windows way, with exit status `0xC000013A`.

## Testing

- **Unit tests** cover argument parsing (`src/cli.rs`), splitting the command line
  (`src/cmdline.rs`), program lookup, the `.exe` shadowing check and the PE
  subsystem check (`src/exec.rs`, against temporary directories), the module
  path for Windows PowerShell (`src/powershell.rs`), and which
  requests each combination of flags holds over time and the reason string
  (`src/main.rs`).
- **Integration tests** (`tests/cli.rs`) run the built executable. They read the
  system-wide execution state with `CallNtPowerInformation(SystemExecutionState)`,
  which needs no administrator rights and reflects power requests. They check that
  each option holds the expected state, and that it is released after a timeout,
  Ctrl+Break and a forced kill. They also cover `-w`, `-t` with a utility, exit
  codes, program lookup, how arguments are passed, Ctrl+Break with console and
  GUI utilities, the job object, Windows PowerShell's module path, that `-u`
  counts as input, and that the executable does not need the Visual C++ runtime.

The execution state is global, so every test that starts caffeinate holds a lock,
and the tests compare against the state before caffeinate starts. Bits that
another program already holds cannot be checked; the tests warn about them.
Children are killed if a test fails, every run has a time limit, and the test
process ends whatever it started when it exits, so a failed test cannot leave the
system held awake.

## Future work

- winget and Scoop packages.
- Publishing to crates.io.
- Code signing for release binaries.
- Power source change notifications (`PowerSettingRegisterNotification`) instead
  of polling for `-s`.
- Waiting for processes caffeinate cannot open, by watching the process list.
