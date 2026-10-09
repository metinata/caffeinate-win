//! Waiting on kernel handles, the clock for deadlines, and Ctrl+C handling.

use std::ffi::c_void;
use std::io;
use std::os::windows::io::{HandleOrNull, OwnedHandle};
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    ERROR_INVALID_PARAMETER, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, ResetEvent, SetEvent,
    WaitForMultipleObjects, WaitForSingleObject,
};
use windows_sys::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime;
use windows_sys::core::BOOL;

/// Event signaled by the console control handler. Created once and never closed.
static INTERRUPT: AtomicPtr<c_void> = AtomicPtr::new(ptr::null_mut());

unsafe extern "system" fn on_console_event(ctrl_type: u32) -> BOOL {
    if ctrl_type != CTRL_C_EVENT && ctrl_type != CTRL_BREAK_EVENT {
        // The console is being closed, or the session is ending. Not handled,
        // so Windows ends caffeinate, and the utility with it (see `Job`).
        return 0;
    }
    let event = INTERRUPT.load(Ordering::SeqCst);
    if !event.is_null() {
        // SAFETY: `event` is a valid event handle that is never closed.
        unsafe { SetEvent(event) };
    }
    // Handled: the main thread decides what the interrupt means.
    1
}

/// Returns a manual-reset event that is signaled on Ctrl+C and Ctrl+Break.
///
/// A program started with Ctrl+C turned off, as `start /B` does for programs
/// it runs in the background, keeps it off: only Ctrl+Break reaches it, and
/// the utility inherits the same setting.
pub fn interrupt_event() -> io::Result<HANDLE> {
    // SAFETY: no security attributes and no name; manual reset, initially unsignaled.
    let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
    if event.is_null() {
        return Err(io::Error::last_os_error());
    }
    INTERRUPT.store(event, Ordering::SeqCst);
    // Best effort: without the handler, Ctrl+C simply ends the process, and
    // the kernel still releases the power request.
    // SAFETY: `on_console_event` matches PHANDLER_ROUTINE and only touches the static above.
    unsafe { SetConsoleCtrlHandler(Some(on_console_event), 1) };
    Ok(event)
}

/// Whether the event is signaled.
pub fn is_set(event: HANDLE) -> bool {
    // SAFETY: `event` is a valid event handle; a zero timeout only checks it.
    unsafe { WaitForSingleObject(event, 0) == WAIT_OBJECT_0 }
}

/// Resets the interrupt event after an interrupt was handled without exiting.
pub fn rearm(event: HANDLE) {
    // SAFETY: `event` is the event handle returned by `interrupt_event`.
    unsafe { ResetEvent(event) };
}

/// Opens a process just to wait for it to exit.
pub fn open_process(pid: u32) -> io::Result<OwnedHandle> {
    // Windows ignores the two low bits of a process ID, which are 0 in every
    // real one: OpenProcess(8045) would open process 8044.
    if pid % 4 != 0 {
        return Err(io::Error::from_raw_os_error(ERROR_INVALID_PARAMETER as i32));
    }
    // SAFETY: plain FFI call; the result is a new handle that nothing else owns, or null.
    let handle = unsafe { HandleOrNull::from_raw_handle(OpenProcess(PROCESS_SYNCHRONIZE, 0, pid)) };
    OwnedHandle::try_from(handle).map_err(|_| io::Error::last_os_error())
}

/// How long the system has been awake: the time since it started, minus time
/// spent asleep. Wait timeouts count this time too (on Windows 8 and later),
/// so deadlines use it, and -t does not count time asleep, as on macOS.
pub fn awake_time() -> Duration {
    let mut ticks = 0u64;
    // SAFETY: `ticks` is a valid u64 for the count of 100 ns intervals.
    unsafe { QueryUnbiasedInterruptTime(&raw mut ticks) };
    Duration::from_nanos(ticks.saturating_mul(100))
}

/// Waits until one of `handles` is signaled and returns its index, or returns
/// `None` once `timeout` has passed. Without a timeout, the thread sleeps until
/// something happens.
pub fn wait_any(handles: &[HANDLE], timeout: Option<Duration>) -> io::Result<Option<usize>> {
    let count = u32::try_from(handles.len()).expect("handle count fits in u32");
    // Rounded up, so that a wait never ends just before a deadline. Timeouts
    // beyond ~49 days end early; callers wait again.
    let millis = timeout.map_or(INFINITE, |timeout| {
        u32::try_from(timeout.as_nanos().div_ceil(1_000_000))
            .map_or(INFINITE - 1, |millis| millis.min(INFINITE - 1))
    });
    // SAFETY: `handles` points to `count` valid handles for the duration of the call.
    match unsafe { WaitForMultipleObjects(count, handles.as_ptr(), 0, millis) } {
        WAIT_TIMEOUT => Ok(None),
        r if (WAIT_OBJECT_0..WAIT_OBJECT_0 + count).contains(&r) => {
            Ok(Some((r - WAIT_OBJECT_0) as usize))
        }
        _ => Err(io::Error::last_os_error()),
    }
}
