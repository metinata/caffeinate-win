//! Running the utility: finding the program and starting it.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{self, Path, PathBuf};
use std::process::{Child, Command};

use windows_sys::Win32::Foundation::HANDLE;

use crate::job::Job;
use crate::powershell;

/// Used when `PATHEXT` is not set; the Windows default.
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD;.VBS;.VBE;.JS;.JSE;.WSF;.WSH;.MSC";

/// The extensions of the files caffeinate can start: programs, and batch
/// files, which std runs through `cmd.exe`. Other scripts, such as `.vbs` or
/// `.py`, need their interpreter: `caffeinate cscript script.vbs`.
const STARTABLE: [&str; 4] = [".com", ".exe", ".bat", ".cmd"];

/// A running utility.
pub struct Utility {
    child: Child,
    /// GUI programs are not attached to the console, so they never get Ctrl+C.
    pub gui: bool,
    /// Ends the utility if caffeinate is killed.
    _job: Option<Job>,
}

impl Utility {
    pub fn handle(&self) -> HANDLE {
        self.child.as_raw_handle()
    }

    /// The utility's exit code, once it has exited.
    pub fn exit_code(&mut self) -> io::Result<i32> {
        // Windows exit codes are 32-bit; `code()` is always `Some` on Windows.
        Ok(self.child.wait()?.code().unwrap_or(1))
    }
}

/// Finds the program to start, following `cmd.exe`'s `PATHEXT` rules among the
/// files caffeinate can start. See `resolve`.
pub fn find(program: &OsStr) -> io::Result<PathBuf> {
    let path = env::var_os("PATH").unwrap_or_default();
    let pathext = env::var_os("PATHEXT").unwrap_or_else(|| DEFAULT_PATHEXT.into());
    let program = resolve(program, &path, &pathext).ok_or(io::ErrorKind::NotFound)?;
    if let Some(shadow) = exe_shadow(&program) {
        return Err(io::Error::other(format!(
            "refusing to start {}, because {} would run instead",
            program.display(),
            shadow.display()
        )));
    }
    Ok(program)
}

/// Starts `program` with `raw_args`, the utility's arguments as written on
/// caffeinate's command line (see `cmdline`), or with `args` if those are not
/// available. The utility joins caffeinate's job.
pub fn start(program: &Path, args: &[OsString], raw_args: Option<&OsStr>) -> io::Result<Utility> {
    let mut command = Command::new(program);
    match raw_args {
        // Batch files always get std's quoting, which keeps cmd.exe from
        // running parts of an argument like `a&b` as commands.
        Some(raw) if !is_batch_file(program) => {
            if !raw.is_empty() {
                command.raw_arg(raw);
            }
        }
        _ => {
            command.args(args);
        }
    }
    if let Some(module_path) = powershell::module_path_for(program) {
        command.env("PSModulePath", module_path);
    }
    let gui = is_gui_program(program);
    // Best effort: without the job, the utility merely outlives a killed caffeinate.
    let job = Job::enter().ok();
    let child = command.spawn()?;
    Ok(Utility {
        child,
        gui,
        _job: job,
    })
}

/// Finds the program like `cmd.exe` does, but only among the files caffeinate
/// can start:
///
/// - A name with a `PATHEXT` extension is used as is: `tool.cmd`.
/// - Other names are tried with each startable `PATHEXT` extension in turn, so
///   `npm` finds `npm.cmd` and `.\gradlew` finds `.\gradlew.bat`. A name with
///   some other extension is tried as is first: `notes.txt`.
/// - A name with a directory is looked up only there. A bare name is searched
///   for in each directory of `PATH`; like PowerShell, and unlike `cmd.exe`,
///   caffeinate does not search the current directory.
///
/// Returns the full path, so that std starts exactly this file.
fn resolve(program: &OsStr, path: &OsStr, pathext: &OsStr) -> Option<PathBuf> {
    let given = Path::new(program);
    let ends_with_separator = matches!(program.as_encoded_bytes().last(), Some(b'\\' | b'/'));
    if program.is_empty() || ends_with_separator {
        return None;
    }
    let pathext = pathext.to_string_lossy().to_ascii_lowercase();
    let exts: Vec<&str> = pathext
        .split(';')
        .filter(|ext| ext.starts_with('.') && ext.len() > 1)
        .collect();
    let given_ext = given
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy().to_ascii_lowercase()));
    let known_ext = given_ext.as_deref().is_some_and(|ext| exts.contains(&ext));
    let suffixes: Vec<&str> = if known_ext {
        vec![""]
    } else {
        given_ext
            .is_some()
            .then_some("")
            .into_iter()
            .chain(exts.iter().copied().filter(|ext| STARTABLE.contains(ext)))
            .collect()
    };
    let find = |base: &Path| {
        suffixes
            .iter()
            .map(|suffix| with_suffix(base, suffix))
            .find(|candidate| candidate.is_file())
    };

    let found = if given.components().count() == 1 {
        env::split_paths(path)
            .filter(|dir| !dir.as_os_str().is_empty())
            .find_map(|dir| find(&dir.join(given)))
    } else {
        find(given)
    };
    found.and_then(|found| path::absolute(found).ok())
}

/// `path` with `suffix` added to its file name, as in `tool` + `.cmd`.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    name.into()
}

/// For a path that does not end in `.exe`, std first tries the path with
/// `.exe` added, and starts that if anything by that name exists. So a
/// `tool.cmd.exe` next to `tool.cmd` would run instead of it. Returns that
/// file, if any.
fn exe_shadow(program: &Path) -> Option<PathBuf> {
    let is_exe = program
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"));
    let shadow = with_suffix(program, ".exe");
    // Like std, count anything by that name: a directory, or a dangling link.
    (!is_exe && fs::symlink_metadata(&shadow).is_ok()).then_some(shadow)
}

/// Whether std runs `program` through `cmd.exe`, which it decides by how the
/// path ends.
fn is_batch_file(program: &Path) -> bool {
    let name = program.as_os_str().as_encoded_bytes();
    let end = &name[name.len().saturating_sub(4)..];
    end.eq_ignore_ascii_case(b".bat") || end.eq_ignore_ascii_case(b".cmd")
}

/// Whether `program` is built for the GUI subsystem. Such a program is not
/// attached to the console, so it never receives Ctrl+C. Anything else,
/// including batch files (run by `cmd.exe`), is a console program.
///
/// Asking the console which processes are attached would not do: a console
/// program shows up there only once it has started up, so a Ctrl+C right
/// after the start would take it for a GUI program.
fn is_gui_program(program: &Path) -> bool {
    const IMAGE_SUBSYSTEM_WINDOWS_GUI: u16 = 2;
    let subsystem = || -> io::Result<u16> {
        let mut file = File::open(program)?;
        let mut dos_header = [0; 64];
        file.read_exact(&mut dos_header)?;
        if !dos_header.starts_with(b"MZ") {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let pe_header = u32::from_le_bytes([
            dos_header[60],
            dos_header[61],
            dos_header[62],
            dos_header[63],
        ]);
        // The PE signature, the 20-byte file header, then the optional header,
        // which has the subsystem at offset 68 in both PE32 and PE32+.
        let mut headers = [0; 4 + 20 + 70];
        file.seek(SeekFrom::Start(pe_header.into()))?;
        file.read_exact(&mut headers)?;
        if !headers.starts_with(b"PE\0\0") {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(u16::from_le_bytes([headers[92], headers[93]]))
    };
    subsystem().is_ok_and(|subsystem| subsystem == IMAGE_SUBSYSTEM_WINDOWS_GUI)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::temp_dir::TempDir;

    const PATHEXT: &str = ".COM;.EXE;.BAT;.CMD;.VBS;.JS";

    fn resolve_with(program: impl AsRef<OsStr>, dirs: &[&Path]) -> Option<PathBuf> {
        let path = env::join_paths(dirs).unwrap();
        resolve(program.as_ref(), &path, OsStr::new(PATHEXT))
    }

    #[test]
    fn finds_scripts_by_pathext() {
        let dir = TempDir::new("pathext");
        let script = dir.file("tool.cmd", "");
        assert_eq!(resolve_with("tool", &[dir.path()]), Some(script));
    }

    #[test]
    fn prefers_earlier_pathext_entries() {
        let dir = TempDir::new("order");
        dir.file("tool.cmd", "");
        let exe = dir.file("tool.exe", "");
        assert_eq!(resolve_with("tool", &[dir.path()]), Some(exe));
    }

    #[test]
    fn prefers_earlier_path_entries() {
        let first = TempDir::new("first");
        let second = TempDir::new("second");
        let expected = first.file("tool.cmd", "");
        second.file("tool.exe", "");
        assert_eq!(
            resolve_with("tool", &[first.path(), second.path()]),
            Some(expected)
        );
    }

    #[test]
    fn skips_scripts_that_need_an_interpreter() {
        let first = TempDir::new("interpreted");
        let second = TempDir::new("startable");
        first.file("tool.js", "");
        let exe = second.file("tool.exe", "");
        assert_eq!(
            resolve_with("tool", &[first.path(), second.path()]),
            Some(exe)
        );
    }

    #[test]
    fn known_extension_is_used_as_is() {
        let dir = TempDir::new("known");
        let script = dir.file("tool.cmd", "");
        dir.file("tool.cmd.exe", "");
        assert_eq!(resolve_with("tool.cmd", &[dir.path()]), Some(script));
        // Found, although it cannot be started: that is reported when starting it.
        let vbs = dir.file("tool.vbs", "");
        assert_eq!(resolve_with("tool.vbs", &[dir.path()]), Some(vbs));
    }

    #[test]
    fn other_extensions_are_tried_as_is_then_with_pathext() {
        let dir = TempDir::new("dotted");
        let script = dir.file("tool.v2.cmd", "");
        assert_eq!(resolve_with("tool.v2", &[dir.path()]), Some(script));
        let notes = dir.file("notes.txt", "");
        assert_eq!(resolve_with(&notes, &[]), Some(notes));
    }

    #[test]
    fn paths_get_pathext_in_their_own_directory() {
        let dir = TempDir::new("relative");
        dir.file("gradlew", ""); // The POSIX script that sits next to gradlew.bat.
        let script = dir.file("gradlew.bat", "");
        let other = TempDir::new("relative-other");
        other.file("gradlew.exe", "");
        assert_eq!(
            resolve_with(dir.path().join("gradlew"), &[other.path()]),
            Some(script)
        );
    }

    #[test]
    fn missing_programs_are_not_found() {
        let dir = TempDir::new("missing");
        assert_eq!(resolve_with(dir.path().join("tool"), &[dir.path()]), None);
        assert_eq!(
            resolve_with(dir.path().join("tool.cmd"), &[dir.path()]),
            None
        );
        assert_eq!(resolve_with("missing", &[dir.path()]), None);
        assert_eq!(resolve_with("", &[dir.path()]), None);
        dir.file("tool.exe", "");
        assert_eq!(resolve_with(r"tool\", &[dir.path()]), None);
    }

    #[test]
    fn detects_an_exe_that_would_shadow_a_script() {
        let dir = TempDir::new("shadow");
        let script = dir.file("tool.cmd", "");
        assert_eq!(exe_shadow(&script), None);
        let shadow = dir.file("tool.cmd.exe", "");
        assert_eq!(exe_shadow(&script), Some(shadow.clone()));
        assert_eq!(exe_shadow(&shadow), None);
        // std would start a directory of that name too, and fail.
        let other = dir.file("other.bat", "");
        fs::create_dir(dir.path().join("other.bat.exe")).unwrap();
        assert!(exe_shadow(&other).is_some());
    }

    #[test]
    fn recognizes_batch_files() {
        assert!(is_batch_file(Path::new(r"C:\x\build.CMD")));
        assert!(is_batch_file(Path::new(r"C:\x\gradlew.bat")));
        assert!(!is_batch_file(Path::new(r"C:\x\tool.exe")));
        assert!(!is_batch_file(Path::new("cmd")));
    }

    #[test]
    fn tells_gui_programs_from_console_programs() {
        assert!(!is_gui_program(&env::current_exe().unwrap()));
        let dir = TempDir::new("subsystem");
        assert!(!is_gui_program(&dir.file("tool.cmd", "@exit /b 0")));
        let wscript = Path::new(&env::var_os("SystemRoot").unwrap()).join(r"System32\wscript.exe");
        if wscript.is_file() {
            assert!(is_gui_program(&wscript));
        }
    }
}
