//! One running copy per install, and a notification-area icon for when Shoji keeps running
//! with no windows open (so Byōbu's lists keep updating).
//!
//! A small message-only window on its own thread does three jobs:
//! * receives launches from later copies of Shoji (`WM_COPYDATA`) and forwards them to the
//!   browser's event loop, so starting Shoji again opens a window in the running copy;
//! * owns the tray icon, shown for as long as Shoji runs;
//! * shows the icon's menu (Open / Quit).

use crate::BrowserEvent;
use winit::event_loop::EventLoopProxy;

/// What the tray thread asks the browser to do.
#[derive(Debug, Clone)]
pub enum Action {
    /// Show a window, optionally opening this address (as typed on the command line).
    Open(Option<String>),
    /// Save the session and exit.
    Quit,
}

#[cfg(windows)]
pub use imp::*;

#[cfg(windows)]
mod imp {
    use super::*;
    use std::hash::{Hash, Hasher};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{
        GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, WPARAM,
    };
    use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
    use windows::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
    };
    use windows::Win32::UI::WindowsAndMessaging::*;

    const WM_TRAY: u32 = WM_APP + 1;
    const WM_SHOW_ICON: u32 = WM_APP + 2;
    const WM_NEW_ICON: u32 = WM_APP + 3;
    const ID_OPEN: usize = 1;
    const ID_QUIT: usize = 2;
    const ICON_ID: u32 = 1;
    const CLASS: PCWSTR = w!("ShojiInstanceWindow");
    /// `dwData` tag on forwarded launches, so unrelated `WM_COPYDATA` is ignored.
    const COPYDATA_TAG: usize = 0x5348_4F4A; // "SHOJ"

    static PROXY: OnceLock<EventLoopProxy<BrowserEvent>> = OnceLock::new();
    static ICON_SHOWN: AtomicBool = AtomicBool::new(false);
    /// The crane in the chosen colour, as RGBA and its side in pixels (None: the executable's own)
    static ICON_RGBA: Mutex<Option<(Vec<u8>, u32)>> = Mutex::new(None);
    static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

    /// Stable per-install name: two different copies of `shoji.exe` (an install and a build
    /// in a repo, say) don't see each other.
    pub fn instance_name() -> String {
        let exe = std::env::current_exe().unwrap_or_default();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        exe.to_string_lossy().to_lowercase().hash(&mut h);
        format!("Shoji-{:016x}", h.finish())
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// Claim this install's single instance. Returns false when another copy already runs.
    pub fn claim_instance(name: &str) -> bool {
        let n = wide(&format!("Local\\{name}"));
        // SAFETY: plain Win32 calls with valid, NUL-terminated strings. The mutex handle is
        // intentionally never closed: it lives as long as the process.
        unsafe {
            match CreateMutexW(None, true, PCWSTR(n.as_ptr())) {
                Ok(_) => GetLastError() != ERROR_ALREADY_EXISTS,
                Err(_) => true, // can't tell: run rather than refuse to start
            }
        }
    }

    /// Hand a launch to the running copy: open `url` (or just show a window), or quit it.
    /// Waits briefly for that copy's window to exist if it is still starting.
    pub fn forward_launch(name: &str, quit: bool, url: Option<&str>) {
        let payload = if quit {
            "quit".to_string()
        } else {
            format!("open:{}", url.unwrap_or_default())
        };
        let title = wide(name);
        let units = wide(&payload);
        for _ in 0..30 {
            // SAFETY: valid strings; the COPYDATASTRUCT points at `units`, which outlives the
            // synchronous SendMessageTimeoutW.
            unsafe {
                if let Ok(hwnd) =
                    FindWindowExW(Some(HWND_MESSAGE), None, CLASS, PCWSTR(title.as_ptr()))
                {
                    let _ = AllowSetForegroundWindow(ASFW_ANY);
                    let cds = COPYDATASTRUCT {
                        dwData: COPYDATA_TAG,
                        cbData: (units.len() * 2) as u32,
                        lpData: units.as_ptr() as *mut _,
                    };
                    let mut result = 0usize;
                    SendMessageTimeoutW(
                        hwnd,
                        WM_COPYDATA,
                        WPARAM(0),
                        LPARAM(&cds as *const _ as isize),
                        SMTO_ABORTIFHUNG,
                        5000,
                        Some(&mut result),
                    );
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    /// Handle to the tray window, for showing and hiding the icon from the UI thread.
    pub struct Tray {
        hwnd: isize,
    }

    impl Tray {
        /// Create the instance window and its message loop on a new thread.
        pub fn start(proxy: EventLoopProxy<BrowserEvent>, name: &str) -> Option<Tray> {
            let _ = PROXY.set(proxy);
            let title = wide(name);
            let (tx, rx) = std::sync::mpsc::channel::<isize>();
            std::thread::Builder::new()
                .name("shoji-tray".into())
                .spawn(move || {
                    // SAFETY: standard window-class registration and message loop on the
                    // thread that owns the window.
                    unsafe {
                        let hinst = GetModuleHandleW(None).unwrap_or_default();
                        let wc = WNDCLASSW {
                            lpfnWndProc: Some(wnd_proc),
                            hInstance: hinst.into(),
                            lpszClassName: CLASS,
                            ..Default::default()
                        };
                        RegisterClassW(&wc);
                        TASKBAR_CREATED.store(
                            RegisterWindowMessageW(w!("TaskbarCreated")),
                            Ordering::Relaxed,
                        );
                        let hwnd = CreateWindowExW(
                            WINDOW_EX_STYLE(0),
                            CLASS,
                            PCWSTR(title.as_ptr()),
                            WINDOW_STYLE(0),
                            0,
                            0,
                            0,
                            0,
                            Some(HWND_MESSAGE),
                            None,
                            Some(hinst.into()),
                            None,
                        );
                        let Ok(hwnd) = hwnd else {
                            let _ = tx.send(0);
                            return;
                        };
                        let _ = tx.send(hwnd.0 as isize);
                        let mut msg = MSG::default();
                        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                })
                .ok()?;
            match rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(h) if h != 0 => Some(Tray { hwnd: h }),
                _ => None,
            }
        }

        fn post(&self, msg: u32) {
            // SAFETY: posting to a window owned by our own tray thread.
            unsafe {
                let _ = PostMessageW(Some(HWND(self.hwnd as *mut _)), msg, WPARAM(0), LPARAM(0));
            }
        }

        /// Side of the icon the notification area wants, in pixels.
        pub fn icon_size() -> u32 {
            // SAFETY: plain metrics query.
            let n = unsafe { GetSystemMetrics(SM_CXSMICON) };
            if n > 0 {
                n as u32
            } else {
                16
            }
        }

        /// Use this picture (RGBA, `size` x `size`) as the icon, now and after Explorer restarts.
        pub fn set_icon(&self, rgba: Vec<u8>, size: u32) {
            if let Ok(mut slot) = ICON_RGBA.lock() {
                *slot = Some((rgba, size));
            }
            self.post(WM_NEW_ICON);
        }

        /// Show the notification-area icon.
        pub fn show(&self) {
            self.post(WM_SHOW_ICON);
        }
    }

    /// An icon from the tinted picture, or None to fall back to the executable's.
    unsafe fn tinted_icon() -> Option<HICON> {
        let (rgba, size) = ICON_RGBA.lock().ok()?.clone()?;
        if rgba.len() != (size * size * 4) as usize {
            return None;
        }
        // Windows wants BGRA.
        let mut bgra = rgba;
        for px in bgra.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        let color = CreateBitmap(
            size as i32,
            size as i32,
            1,
            32,
            Some(bgra.as_ptr() as *const _),
        );
        // The alpha channel of the colour bitmap does the cutting; the mask stays empty.
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, None);
        let info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }

    fn notify(hwnd: HWND, add: bool) {
        // SAFETY: NOTIFYICONDATAW is fully initialised; the icon comes from our own resources.
        unsafe {
            let mut data = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: ICON_ID,
                ..Default::default()
            };
            if add {
                let hinst = GetModuleHandleW(None).unwrap_or_default();
                // The executable's icon resource has id 1 (see build.rs): MAKEINTRESOURCE(1), a
                // small integer in the pointer, not an address (a dangling pointer is 2 here,
                // which is no icon at all, so the tray showed an empty slot).
                #[allow(clippy::manual_dangling_ptr)]
                let icon = tinted_icon()
                    .or_else(|| LoadIconW(Some(hinst.into()), PCWSTR(1 as *const u16)).ok());
                data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
                data.uCallbackMessage = WM_TRAY;
                data.hIcon = icon.unwrap_or_default();
                let tip: Vec<u16> = "Shoji".encode_utf16().collect();
                let n = tip.len().min(data.szTip.len() - 1);
                data.szTip[..n].copy_from_slice(&tip[..n]);
                let _ = Shell_NotifyIconW(NIM_ADD, &data);
            } else {
                let _ = Shell_NotifyIconW(NIM_DELETE, &data);
            }
        }
    }

    fn send(action: Action) {
        if let Some(p) = PROXY.get() {
            let _ = p.send_event(BrowserEvent::Tray(action));
        }
    }

    unsafe fn show_menu(hwnd: HWND) {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_STRING, ID_OPEN, w!("Open Shoji"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, w!("Quit Shoji"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY,
            pt.x,
            pt.y,
            Some(0),
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        match cmd.0 as usize {
            ID_OPEN => send(Action::Open(None)),
            ID_QUIT => send(Action::Quit),
            _ => {}
        }
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_COPYDATA => {
                let cds = lparam.0 as *const COPYDATASTRUCT;
                if !cds.is_null() && (*cds).dwData == COPYDATA_TAG {
                    let len = ((*cds).cbData as usize / 2).min(4096);
                    let units = std::slice::from_raw_parts((*cds).lpData as *const u16, len);
                    let text = String::from_utf16_lossy(units);
                    let text = text.trim_end_matches('\0');
                    if text == "quit" {
                        send(Action::Quit);
                    } else if let Some(url) = text.strip_prefix("open:") {
                        let url = url.trim();
                        send(Action::Open((!url.is_empty()).then(|| url.to_string())));
                    }
                }
                LRESULT(1)
            }
            WM_SHOW_ICON => {
                ICON_SHOWN.store(true, Ordering::Relaxed);
                notify(hwnd, true);
                LRESULT(0)
            }
            WM_NEW_ICON => {
                if ICON_SHOWN.load(Ordering::Relaxed) {
                    // Remove and add again: the simplest way to swap the picture.
                    notify(hwnd, false);
                    notify(hwnd, true);
                }
                LRESULT(0)
            }
            WM_TRAY => {
                match (lparam.0 as u32) & 0xFFFF {
                    WM_LBUTTONUP | WM_LBUTTONDBLCLK => send(Action::Open(None)),
                    WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                    _ => {}
                }
                LRESULT(0)
            }
            m if m != 0
                && m == TASKBAR_CREATED.load(Ordering::Relaxed)
                && ICON_SHOWN.load(Ordering::Relaxed) =>
            {
                // Explorer restarted: its notification area is empty again.
                notify(hwnd, true);
                LRESULT(0)
            }
            WM_DESTROY => {
                notify(hwnd, false);
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn instance_name() -> String {
        String::new()
    }
    pub fn claim_instance(_: &str) -> bool {
        true
    }
    pub fn forward_launch(_: &str, _: bool, _: Option<&str>) {}

    pub struct Tray;
    impl Tray {
        pub fn start(_: EventLoopProxy<BrowserEvent>, _: &str) -> Option<Tray> {
            None
        }
        pub fn icon_size() -> u32 {
            16
        }
        pub fn set_icon(&self, _: Vec<u8>, _: u32) {}
        pub fn show(&self) {}
    }
}

#[cfg(not(windows))]
pub use imp::*;
