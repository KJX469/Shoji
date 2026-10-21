//! What Shoji keeps when something goes wrong, all of it local: a report for each panic, a
//! minidump for each native crash (both in `<data>/crashes`), and a log of the times the UI thread
//! was blocked for more than [`STALL`] (`<data>/logs/ui-stalls.log`). Nothing is sent anywhere;
//! after a crash the next start offers to open a prefilled GitHub issue, and only that click
//! leaves the PC.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, AtomicPtr, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A UI thread blocked for longer than this is logged.
pub const STALL: Duration = Duration::from_millis(100);
/// Log files are started over beyond this size.
const LOG_LIMIT: u64 = 256 * 1024;
const ISSUES: &str = "https://github.com/KJX469/Shoji/issues/new";

static CRASH_DIR: OnceLock<PathBuf> = OnceLock::new();

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn crash_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("crashes")
}

/// Record panics and native crashes from here on.
pub fn install(data_dir: &Path) {
    let dir = crash_dir(data_dir);
    let _ = std::fs::create_dir_all(&dir);
    let _ = CRASH_DIR.set(dir);
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(dir) = CRASH_DIR.get() {
            let thread = std::thread::current();
            let report = panic_report(
                thread.name().unwrap_or("unnamed"),
                info.payload_as_str().unwrap_or("(no message)"),
                &info.location().map(|l| l.to_string()).unwrap_or_default(),
                &std::backtrace::Backtrace::force_capture().to_string(),
            );
            let _ = std::fs::write(dir.join(format!("panic-{}.txt", now_secs())), report);
        }
        default(info);
    }));
    #[cfg(windows)]
    native::install();
}

fn panic_report(thread: &str, message: &str, location: &str, backtrace: &str) -> String {
    format!(
        "Shoji {}\nthread: {thread}\npanic: {message}\nat: {location}\n\n{backtrace}",
        env!("CARGO_PKG_VERSION")
    )
}

/// Crash reports written since the last start that the user hasn't been told about, newest
/// first. Telling them is recorded in `seen` so each crash is mentioned once.
pub fn unseen_reports(data_dir: &Path) -> Vec<PathBuf> {
    let dir = crash_dir(data_dir);
    let seen: u64 = std::fs::read_to_string(dir.join("seen"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let mut out: Vec<(u64, PathBuf)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stamp = name
                .strip_prefix("panic-")
                .or_else(|| name.strip_prefix("native-"))?
                .split('.')
                .next()?
                .parse()
                .ok()?;
            (stamp > seen && (name.ends_with(".txt"))).then(|| (stamp, e.path()))
        })
        .collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.0));
    // Keep the folder small: the newest twenty reports and dumps.
    prune(&dir, 20);
    out.into_iter().map(|(_, p)| p).collect()
}

pub fn mark_seen(data_dir: &Path) {
    let _ = std::fs::write(crash_dir(data_dir).join("seen"), now_secs().to_string());
}

fn prune(dir: &Path, keep: usize) {
    let mut files: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name() != "seen")
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|e| std::cmp::Reverse(e.0));
    for (_, p) in files.into_iter().skip(keep * 2) {
        let _ = std::fs::remove_file(p);
    }
}

/// A GitHub "new issue" link with the report's first lines filled in. Opening it sends nothing;
/// the user reads it and decides whether to submit.
pub fn issue_url(report: &str) -> String {
    let head: String = report.lines().take(12).collect::<Vec<_>>().join("\n");
    let what = format!("Shoji closed unexpectedly.\n\n```\n{head}\n```\n");
    format!(
        "{ISSUES}?template=bug_report.yml&title={}&what={}&version={}",
        shoji_core::tabs::url_encode("Shoji closed unexpectedly"),
        shoji_core::tabs::url_encode(&what),
        shoji_core::tabs::url_encode(&format!("Shoji {}", env!("CARGO_PKG_VERSION"))),
    )
}

/* ------------------------------------------------------------------ UI thread watchdog */

/// The window the watchdog pings: any top-level window owned by the UI thread.
static UI_HWND: AtomicIsize = AtomicIsize::new(0);
/// What the UI thread is handling right now, as a static label ("" when in the engine or idle).
static DOING: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static DOING_LEN: AtomicIsize = AtomicIsize::new(0);

pub fn set_ui_window(hwnd: isize) {
    UI_HWND.store(hwnd, Ordering::Relaxed);
}

/// A `'static` copy of `name`, made once per distinct name (labels are variant names, a few dozen).
pub fn intern(name: String) -> &'static str {
    static NAMES: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let Ok(mut names) = NAMES.lock() else {
        return "";
    };
    if let Some(n) = names.iter().find(|n| **n == name) {
        return n;
    }
    let leaked: &'static str = Box::leak(name.into_boxed_str());
    names.push(leaked);
    leaked
}

/// Marks what the UI thread is doing until the returned guard is dropped (labels nest).
pub fn doing(label: &'static str) -> Doing {
    let previous = (
        DOING.load(Ordering::Acquire),
        DOING_LEN.load(Ordering::Relaxed),
    );
    DOING_LEN.store(label.len() as isize, Ordering::Relaxed);
    DOING.store(label.as_ptr() as *mut u8, Ordering::Release);
    Doing { previous }
}

pub struct Doing {
    previous: (*mut u8, isize),
}

impl Drop for Doing {
    fn drop(&mut self) {
        DOING_LEN.store(self.previous.1, Ordering::Relaxed);
        DOING.store(self.previous.0, Ordering::Release);
    }
}

fn current_label() -> &'static str {
    let p = DOING.load(Ordering::Acquire);
    if p.is_null() {
        return "";
    }
    let len = DOING_LEN.load(Ordering::Relaxed) as usize;
    // SAFETY: `p`/`len` always come from a `&'static str` stored by `doing`.
    std::str::from_utf8(unsafe { std::slice::from_raw_parts(p, len) }).unwrap_or("")
}

/// The variant name of a `Debug` value (`Foo(..)` gives `Foo`), cut short without formatting the
/// rest, for labelling what the UI thread is busy with.
pub fn variant_name(value: &dyn std::fmt::Debug) -> String {
    struct Head(String);
    impl std::fmt::Write for Head {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for c in s.chars() {
                if !(c.is_alphanumeric() || c == '_') || self.0.len() >= 48 {
                    return Err(std::fmt::Error);
                }
                self.0.push(c);
            }
            Ok(())
        }
    }
    let mut h = Head(String::new());
    let _ = std::fmt::write(&mut h, format_args!("{value:?}"));
    h.0
}

/// Watch the UI thread from a background thread: ping its window, and when a ping isn't answered
/// within [`STALL`], wait for the answer and log how long the thread was blocked and in what.
pub fn spawn_watchdog(data_dir: &Path) {
    #[cfg(windows)]
    {
        let log = data_dir.join("logs").join("ui-stalls.log");
        let _ = std::thread::Builder::new()
            .name("ui-watchdog".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_millis(250));
                let hwnd = UI_HWND.load(Ordering::Relaxed);
                if hwnd == 0 {
                    continue;
                }
                let start = Instant::now();
                if native::ping(hwnd, STALL) {
                    continue;
                }
                let label = current_label();
                // Wait it out (up to a minute) to know how long it lasted.
                native::ping(hwnd, Duration::from_secs(60));
                append_log(&log, &stall_line(start.elapsed(), label));
            });
    }
    #[cfg(not(windows))]
    let _ = data_dir;
}

fn stall_line(blocked: Duration, label: &str) -> String {
    let doing = if label.is_empty() {
        "the engine or the system"
    } else {
        label
    };
    format!("{} {} ms in {doing}\n", now_secs(), blocked.as_millis())
}

/// Note an engine process failure (`ProcessFailedKind`) in `<data>/logs/engine.log`.
pub fn log_engine_failure(data_dir: &Path, kind: i32, runtime: &str) {
    let what = match kind {
        0 => "browser process exited",
        1 => "renderer exited",
        2 => "renderer unresponsive",
        3 => "frame renderer exited",
        4 => "utility process exited",
        5 => "sandbox helper exited",
        6 => "GPU process exited",
        7 => "PPAPI plugin exited",
        8 => "PPAPI broker exited",
        _ => "unknown process failed",
    };
    append_log(
        &data_dir.join("logs").join("engine.log"),
        &format!(
            "{} {what} (kind {kind}), WebView2 {runtime}, Shoji {}\n",
            now_secs(),
            env!("CARGO_PKG_VERSION")
        ),
    );
}

fn append_log(path: &Path, line: &str) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > LOG_LIMIT) {
        let _ = std::fs::remove_file(path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

#[cfg(windows)]
mod native {
    use super::{now_secs, CRASH_DIR};
    use std::time::Duration;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GENERIC_WRITE, HWND, LPARAM, WPARAM};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE,
    };
    use windows::Win32::System::Diagnostics::Debug::{
        MiniDumpNormal, MiniDumpWriteDump, SetUnhandledExceptionFilter, EXCEPTION_POINTERS,
        MINIDUMP_EXCEPTION_INFORMATION,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        IsWindow, SendMessageTimeoutW, SMTO_ABORTIFHUNG, SMTO_BLOCK, WM_NULL,
    };

    pub fn install() {
        unsafe { SetUnhandledExceptionFilter(Some(on_crash)) };
    }

    /// Write `native-<time>.dmp` (and a `.txt` naming it) for an unhandled exception.
    unsafe extern "system" fn on_crash(info: *const EXCEPTION_POINTERS) -> i32 {
        const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
        let Some(dir) = CRASH_DIR.get() else {
            return EXCEPTION_CONTINUE_SEARCH;
        };
        let stamp = now_secs();
        let dump = dir.join(format!("native-{stamp}.dmp"));
        let wide: Vec<u16> = dump
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let file = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                CREATE_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        };
        let code = if info.is_null() {
            0
        } else {
            unsafe { (*(*info).ExceptionRecord).ExceptionCode.0 }
        };
        if let Ok(file) = file {
            let exception = MINIDUMP_EXCEPTION_INFORMATION {
                ThreadId: unsafe { GetCurrentThreadId() },
                ExceptionPointers: info as *mut _,
                ClientPointers: false.into(),
            };
            let _ = unsafe {
                MiniDumpWriteDump(
                    GetCurrentProcess(),
                    GetCurrentProcessId(),
                    file,
                    MiniDumpNormal,
                    Some(&exception),
                    None,
                    None,
                )
            };
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(file) };
        }
        let _ = std::fs::write(
            dir.join(format!("native-{stamp}.txt")),
            format!(
                "Shoji {}\nnative crash: exception 0x{code:08X}\ndump: {}\n",
                env!("CARGO_PKG_VERSION"),
                dump.display()
            ),
        );
        EXCEPTION_CONTINUE_SEARCH
    }

    /// Does the thread owning `hwnd` answer a no-op message within `timeout`?
    pub fn ping(hwnd: isize, timeout: Duration) -> bool {
        let mut result = 0usize;
        let answered = unsafe {
            SendMessageTimeoutW(
                HWND(hwnd as *mut _),
                WM_NULL,
                WPARAM(0),
                LPARAM(0),
                SMTO_BLOCK | SMTO_ABORTIFHUNG,
                timeout.as_millis() as u32,
                Some(&mut result),
            )
        };
        // A window that's gone counts as answered.
        answered.0 != 0 || !unsafe { IsWindow(Some(HWND(hwnd as *mut _))) }.as_bool()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_names_stop_at_the_payload() {
        #[derive(Debug)]
        #[allow(dead_code)]
        enum E {
            TabIpc(u64, String),
            Quit,
            Named { a: u32 },
        }
        assert_eq!(variant_name(&E::TabIpc(1, "x".repeat(10_000))), "TabIpc");
        assert_eq!(variant_name(&E::Quit), "Quit");
        assert_eq!(variant_name(&E::Named { a: 1 }), "Named");
    }

    #[test]
    fn reports_are_listed_once_and_linked_to_a_prefilled_issue() {
        let data = std::env::temp_dir().join(format!("shoji-crash-{}", std::process::id()));
        let dir = crash_dir(&data);
        let _ = std::fs::remove_dir_all(&data);
        std::fs::create_dir_all(&dir).unwrap();
        let report = panic_report("main", "boom", "src/x.rs:1:2", "trace");
        std::fs::write(dir.join("panic-100.txt"), &report).unwrap();
        std::fs::write(dir.join("native-200.txt"), "native").unwrap();
        std::fs::write(dir.join("native-200.dmp"), "dump").unwrap();
        let found = unseen_reports(&data);
        assert_eq!(found.len(), 2);
        assert!(found[0].ends_with("native-200.txt"), "newest first");
        mark_seen(&data);
        assert!(unseen_reports(&data).is_empty());
        let url = issue_url(&report);
        assert!(url.starts_with(ISSUES));
        assert!(url.contains("boom"));
        assert!(!url.contains(' '));
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn stall_lines_name_what_was_running() {
        assert!(stall_line(Duration::from_millis(250), "user_event:TabIpc")
            .ends_with(" 250 ms in user_event:TabIpc\n"));
        assert!(stall_line(Duration::from_millis(120), "").contains("the engine"));
    }
}
