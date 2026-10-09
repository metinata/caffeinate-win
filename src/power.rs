//! Power requests: the mechanism behind `powercfg /requests`.
//!
//! The kernel releases a power request when its handle is closed, which also
//! happens when the process dies. A killed caffeinate therefore never leaves
//! the system stuck awake.

use std::io;
use std::mem;
use std::os::windows::io::{AsRawHandle, HandleOrInvalid, OwnedHandle};
use std::ptr;

use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::Power::{
    ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, POWER_REQUEST_TYPE, PowerClearRequest,
    PowerCreateRequest, PowerRequestDisplayRequired, PowerRequestSystemRequired, PowerSetRequest,
    SetThreadExecutionState,
};
use windows_sys::Win32::System::SystemServices::POWER_REQUEST_CONTEXT_VERSION;
use windows_sys::Win32::System::Threading::{
    POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT,
};
use windows_sys::core::{s, w};

/// A power request. Dropping it closes the handle, which releases it.
pub struct PowerRequest {
    handle: OwnedHandle,
    display: bool,
    system: bool,
}

impl PowerRequest {
    /// Creates an inactive request. `reason` is what `powercfg /requests` shows.
    pub fn new(reason: &str) -> io::Result<Self> {
        let mut wide: Vec<u16> = reason.encode_utf16().chain(Some(0)).collect();
        let context = REASON_CONTEXT {
            Version: POWER_REQUEST_CONTEXT_VERSION,
            Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
            Reason: REASON_CONTEXT_0 {
                SimpleReasonString: wide.as_mut_ptr(),
            },
        };
        // SAFETY: `context` holds a NUL-terminated string that outlives the call;
        // the kernel copies it. The result is a new handle that nothing else
        // owns, or INVALID_HANDLE_VALUE.
        let handle =
            unsafe { HandleOrInvalid::from_raw_handle(PowerCreateRequest(&raw const context)) };
        Ok(Self {
            handle: OwnedHandle::try_from(handle).map_err(|_| io::Error::last_os_error())?,
            display: false,
            system: false,
        })
    }

    /// Brings the request to the wanted state. The kernel counts set/clear
    /// calls per type, so only transitions are forwarded.
    pub fn update(&mut self, display: bool, system: bool) -> io::Result<()> {
        if display != self.display {
            self.apply(PowerRequestDisplayRequired, display)?;
            self.display = display;
        }
        if system != self.system {
            self.apply(PowerRequestSystemRequired, system)?;
            self.system = system;
        }
        Ok(())
    }

    fn apply(&self, kind: POWER_REQUEST_TYPE, on: bool) -> io::Result<()> {
        let handle = self.handle.as_raw_handle();
        // SAFETY: `handle` is a live power request handle owned by `self`.
        let ok = unsafe {
            if on {
                PowerSetRequest(handle, kind)
            } else {
                PowerClearRequest(handle, kind)
            }
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

/// `-u`: declares that the user is active, as input would. Resets the system
/// and display idle timers, and turns the display on if it is off.
pub fn declare_user_active() {
    // SAFETY: plain FFI call without ES_CONTINUOUS, so nothing stays set.
    unsafe { SetThreadExecutionState(ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED) };
    // That keeps the display on, but does not turn on a display that is off:
    // only input does. Best effort: a session without a desktop (over SSH,
    // for example) cannot send input.
    let _ = send_null_input();
}

/// Sends a mouse movement of zero distance: input that leaves the pointer
/// where it is. user32.dll is loaded only here rather than linked, so that
/// runs without -u do not load the GUI libraries.
fn send_null_input() -> io::Result<()> {
    type SendInput = unsafe extern "system" fn(u32, *const INPUT, i32) -> u32;
    // SAFETY: a NUL-terminated name, looked up in System32 only.
    let user32 = unsafe {
        LoadLibraryExW(
            w!("user32.dll"),
            ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    if user32.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `user32` is a loaded module, which is never unloaded, and the
    // name is NUL-terminated.
    let send_input =
        unsafe { GetProcAddress(user32, s!("SendInput")) }.ok_or_else(io::Error::last_os_error)?;
    // SAFETY: SendInput has this signature.
    let send_input =
        unsafe { mem::transmute::<unsafe extern "system" fn() -> isize, SendInput>(send_input) };
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dwFlags: MOUSEEVENTF_MOVE,
                ..MOUSEINPUT::default()
            },
        },
    };
    let size = i32::try_from(size_of::<INPUT>()).expect("size fits in i32");
    // SAFETY: `input` is one INPUT of `size` bytes.
    if unsafe { send_input(1, &raw const input, size) } == 1 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
