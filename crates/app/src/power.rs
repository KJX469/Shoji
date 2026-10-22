//! What Windows says about memory and power, for resting tabs sooner. A background thread
//! looks every few seconds and tells the browser when either changes:
//! - **Low memory**: most of the RAM is in use. Hidden tabs rest after [`PRESSURE_REST`]
//!   instead of the usual wait, so the longest-hidden go first and at once.
//! - **Battery saver** (Energy saver on Windows 11): hidden tabs rest after [`SAVER_REST`] and
//!   the crane holds still. Chromium already slows timers in hidden pages; a resting tab runs none.

use crate::BrowserEvent;
use std::time::Duration;
use winit::event_loop::EventLoopProxy;

pub const PRESSURE_REST: Duration = Duration::from_secs(30);
pub const SAVER_REST: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct State {
    pub low_memory: bool,
    pub saver: bool,
}

impl State {
    /// How long a hidden tab waits before it rests, given the user's choice (`None`: never).
    pub fn rest_after(&self, chosen: Option<Duration>) -> Option<Duration> {
        let chosen = chosen?;
        let mut after = chosen;
        if self.saver {
            after = after.min(SAVER_REST);
        }
        if self.low_memory {
            after = after.min(PRESSURE_REST);
        }
        Some(after)
    }
}

/// Memory pressure with some give, so it doesn't flicker around the line: on at 90% of RAM in
/// use or under 512 MB free, off again below 85% with over 768 MB free.
pub fn under_pressure(was: bool, load_pct: u32, avail_mb: u64) -> bool {
    if was {
        !(load_pct < 85 && avail_mb > 768)
    } else {
        load_pct >= 90 || avail_mb < 512
    }
}

pub fn spawn(proxy: EventLoopProxy<BrowserEvent>) {
    let _ = std::thread::Builder::new()
        .name("shoji-power".into())
        .spawn(move || {
            let mut state = State::default();
            loop {
                let mut next = state;
                if let Some((load, avail_mb)) = native::memory() {
                    next.low_memory = under_pressure(state.low_memory, load, avail_mb);
                }
                next.saver = native::battery_saver();
                if next != state {
                    state = next;
                    if proxy.send_event(BrowserEvent::Power(state)).is_err() {
                        return;
                    }
                }
                std::thread::sleep(POLL);
            }
        });
}

#[cfg(windows)]
mod native {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    /// (percent of RAM in use, MB of RAM free)
    pub fn memory() -> Option<(u32, u64)> {
        let mut m = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        unsafe { GlobalMemoryStatusEx(&mut m) }.ok()?;
        Some((m.dwMemoryLoad, m.ullAvailPhys / (1024 * 1024)))
    }

    pub fn battery_saver() -> bool {
        let mut s = SYSTEM_POWER_STATUS::default();
        unsafe { GetSystemPowerStatus(&mut s) }.is_ok() && s.SystemStatusFlag == 1
    }
}

#[cfg(not(windows))]
mod native {
    pub fn memory() -> Option<(u32, u64)> {
        None
    }
    pub fn battery_saver() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_turns_on_and_off_with_some_give() {
        assert!(!under_pressure(false, 80, 4096));
        assert!(under_pressure(false, 91, 4096));
        assert!(under_pressure(false, 50, 400));
        assert!(
            under_pressure(true, 87, 4096),
            "stays on until clearly over"
        );
        assert!(under_pressure(true, 80, 700));
        assert!(!under_pressure(true, 80, 1024));
    }

    #[test]
    fn tabs_rest_sooner_under_pressure_and_on_battery_saver() {
        let five = Some(Duration::from_secs(300));
        let s = |low_memory, saver| State { low_memory, saver };
        assert_eq!(s(false, false).rest_after(five), five);
        assert_eq!(s(false, true).rest_after(five), Some(SAVER_REST));
        assert_eq!(s(true, true).rest_after(five), Some(PRESSURE_REST));
        assert_eq!(
            s(true, false).rest_after(Some(Duration::from_secs(10))),
            Some(Duration::from_secs(10)),
            "never later than the user chose"
        );
        assert_eq!(
            s(true, true).rest_after(None),
            None,
            "\"Never\" stays never"
        );
    }
}
