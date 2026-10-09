//! Power source detection for `-s`.

use std::time::Duration;

use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

/// How often `-s` re-checks the power source.
pub const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// `ACLineStatus` value for "on battery".
const AC_OFFLINE: u8 = 0;

/// Whether the machine runs on AC power. An unknown status counts as AC, so
/// `-s` still works on machines that do not report a power source.
pub fn on_ac_power() -> bool {
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: `status` is a valid, writable SYSTEM_POWER_STATUS.
    if unsafe { GetSystemPowerStatus(&raw mut status) } == 0 {
        return true;
    }
    status.ACLineStatus != AC_OFFLINE
}
