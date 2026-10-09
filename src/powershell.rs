//! Starting Windows PowerShell from PowerShell 7.
//!
//! PowerShell 7 puts its own module folders in front of `PSModulePath`, and the
//! programs it starts inherit them. Windows PowerShell started by one of those
//! programs then finds PowerShell 7's modules before its own, and they fail to
//! load: PSReadLine, and even the module of `Get-ExecutionPolicy`. PowerShell 7
//! takes its folders out when it starts Windows PowerShell itself, and
//! caffeinate does the same, so that `caffeinate powershell` works like
//! `powershell` does.

use std::env;
use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr;

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};

/// The `PSModulePath` to start `program` with, if it is Windows PowerShell and
/// the current value has PowerShell 7's module folders.
pub fn module_path_for(program: &Path) -> Option<OsString> {
    let name = program.file_name()?;
    let windows_powershell = ["powershell.exe", "powershell_ise.exe"]
        .iter()
        .any(|exe| name.eq_ignore_ascii_case(exe));
    if !windows_powershell {
        return None;
    }
    let current = env::var_os("PSModulePath")?;
    // The folders PowerShell 7 adds for the user and for all users.
    let own: Vec<PathBuf> = [
        documents_dir(),
        env::var_os("ProgramFiles").map(PathBuf::from),
    ]
    .into_iter()
    .flatten()
    .map(|dir| dir.join(r"PowerShell\Modules"))
    .collect();
    let cleaned = without_powershell_7_folders(&current, &own);
    (cleaned != current).then_some(cleaned)
}

/// `module_path` without `own` and without the module folder of any
/// PowerShell 7 installation: a folder next to `pwsh.dll`. These are the
/// folders PowerShell 7 removes.
fn without_powershell_7_folders(module_path: &OsStr, own: &[PathBuf]) -> OsString {
    let kept: Vec<PathBuf> = env::split_paths(module_path)
        .filter(|dir| {
            let is_own = own
                .iter()
                .any(|own| own.as_os_str().eq_ignore_ascii_case(dir));
            let in_installation = dir
                .parent()
                .is_some_and(|parent| parent.join("pwsh.dll").is_file());
            !(is_own || in_installation)
        })
        .collect();
    env::join_paths(kept).unwrap_or_else(|_| module_path.to_owned())
}

/// The user's Documents folder, which may be redirected (to OneDrive, for
/// example), as the shell records it.
fn documents_dir() -> Option<PathBuf> {
    let wide = |s: &str| -> Vec<u16> { OsStr::new(s).encode_wide().chain(Some(0)).collect() };
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders");
    let value = wide("Personal");
    let mut data = [0u16; 1024];
    let mut size = u32::try_from(size_of_val(&data)).expect("size fits in u32");
    // SAFETY: `key` and `value` are NUL-terminated, and `data` has room for
    // `size` bytes. RRF_RT_REG_SZ also accepts a REG_EXPAND_SZ value and
    // expands the environment variables in it.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            data.as_mut_ptr().cast(),
            &raw mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = data.iter().position(|&c| c == 0).unwrap_or(data.len());
    Some(OsString::from_wide(&data[..len]).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::temp_dir::TempDir;

    #[test]
    fn removes_powershell_7_folders_only() {
        let install = TempDir::new("pwsh-install");
        install.file("pwsh.dll", "");
        let ps_home = install.path().join("Modules");
        let own = [PathBuf::from(r"C:\Users\me\Documents\PowerShell\Modules")];
        let module_path = env::join_paths([
            r"c:\users\ME\documents\powershell\modules",
            ps_home.to_str().unwrap(),
            r"C:\Program Files (x86)\Microsoft SQL Server\160\Tools\PowerShell\Modules\",
            r"C:\Program Files\WindowsPowerShell\Modules",
        ])
        .unwrap();
        assert_eq!(
            without_powershell_7_folders(&module_path, &own),
            OsString::from(
                r"C:\Program Files (x86)\Microsoft SQL Server\160\Tools\PowerShell\Modules\;C:\Program Files\WindowsPowerShell\Modules"
            )
        );
    }

    #[test]
    fn only_windows_powershell_gets_a_new_module_path() {
        assert_eq!(
            module_path_for(Path::new(r"C:\Windows\System32\cmd.exe")),
            None
        );
        assert_eq!(
            module_path_for(Path::new(r"C:\tools\mypowershell.exe")),
            None
        );
    }

    #[test]
    fn finds_the_documents_folder() {
        let documents = documents_dir().expect("Documents folder");
        // Expanded, as in C:\Users\me\Documents rather than %USERPROFILE%\Documents.
        assert!(documents.is_absolute(), "{}", documents.display());
    }
}
