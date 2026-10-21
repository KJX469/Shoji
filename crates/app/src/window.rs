//! Top-level window setup: size, crane icons, and native frame colours that match the
//! Washi (light) or Sumi (dark) theme.

use shoji_brand::{render_kage_rgba, theme, Accent};
use winit::dpi::LogicalSize;
use winit::window::{Icon, Theme, Window, WindowAttributes};

pub const DEFAULT_WIDTH: f64 = 1280.0;
pub const DEFAULT_HEIGHT: f64 = 800.0;

/// Title-bar icon (small) and taskbar icon (large): the kage tile, the crane's shadow on a
/// lit shoji screen.
pub fn icons(accent: Accent) -> (Option<Icon>, Option<Icon>) {
    let small = Icon::from_rgba(render_kage_rgba(accent, 32), 32, 32).ok();
    let large = Icon::from_rgba(render_kage_rgba(accent, 64), 64, 64).ok();
    (small, large)
}

pub fn attributes(accent: Accent, theme_pref: &str) -> WindowAttributes {
    let (small, large) = icons(accent);
    let attrs = Window::default_attributes()
        .with_title(shoji_core::APP_NAME)
        .with_inner_size(LogicalSize::new(DEFAULT_WIDTH, DEFAULT_HEIGHT))
        .with_theme(forced_theme(theme_pref))
        .with_visible(false)
        .with_window_icon(small);
    #[cfg(windows)]
    let attrs = {
        use winit::platform::windows::WindowAttributesExtWindows;
        attrs.with_taskbar_icon(large)
    };
    #[cfg(not(windows))]
    let _ = large;
    attrs
}

/// Apply a new crane colour to an existing window's title-bar and taskbar icons.
pub fn set_icons(window: &Window, accent: Accent) {
    let (small, large) = icons(accent);
    window.set_window_icon(small);
    #[cfg(windows)]
    {
        use winit::platform::windows::WindowExtWindows;
        window.set_taskbar_icon(large);
    }
    #[cfg(not(windows))]
    let _ = large;
}

/// `None` follows the system; otherwise the user's explicit choice.
pub fn forced_theme(pref: &str) -> Option<Theme> {
    match pref {
        "light" => Some(Theme::Light),
        "dark" => Some(Theme::Dark),
        _ => None,
    }
}

/// Whether the window is currently rendered dark.
pub fn is_dark(window: &Window, pref: &str) -> bool {
    match forced_theme(pref) {
        Some(t) => t == Theme::Dark,
        None => window.theme() == Some(Theme::Dark),
    }
}

/// The colour mode handed to every webview (drives `prefers-color-scheme`).
pub fn webview_theme(pref: &str) -> wry::Theme {
    match pref {
        "light" => wry::Theme::Light,
        "dark" => wry::Theme::Dark,
        _ => wry::Theme::Auto,
    }
}

static DARK_FRAME: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(theme::FRAME_DARK);
static DARK_PAGE: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(theme::PAGE_DARK);

/// Set the colours the dark browser is drawn with (the title bar and the pre-paint colour of
/// webviews), as `0xRRGGBB`. Follows the user's dark-mode adjustments.
pub fn set_dark_colors(frame: u32, page: u32) {
    use std::sync::atomic::Ordering::Relaxed;
    DARK_FRAME.store(frame, Relaxed);
    DARK_PAGE.store(page, Relaxed);
}

/// Page background used to pre-paint webviews so they never flash white.
pub fn page_rgba(dark: bool) -> (u8, u8, u8, u8) {
    let c = if dark {
        DARK_PAGE.load(std::sync::atomic::Ordering::Relaxed)
    } else {
        theme::PAGE_LIGHT
    };
    ((c >> 16) as u8, (c >> 8) as u8, c as u8, 255)
}

/// Tint the native caption bar and window background to match the tab strip.
#[cfg(windows)]
pub fn apply_frame(window: &Window, dark: bool) {
    use windows::Win32::Foundation::{COLORREF, HWND};
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
    };
    use windows::Win32::Graphics::Gdi::{CreateSolidBrush, InvalidateRect};
    use windows::Win32::UI::WindowsAndMessaging::{SetClassLongPtrW, GCLP_HBRBACKGROUND};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    // One brush per colour for the life of the process (window classes share them).
    static BRUSHES: std::sync::Mutex<Vec<(u32, isize)>> = std::sync::Mutex::new(Vec::new());
    let bgr = |rgb: u32| COLORREF(((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff));
    let dark_frame = DARK_FRAME.load(std::sync::atomic::Ordering::Relaxed);
    let brush_for = |rgb: u32| -> isize {
        let mut brushes = BRUSHES.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, b)) = brushes.iter().find(|(c, _)| *c == rgb) {
            return *b;
        }
        let b = unsafe { CreateSolidBrush(bgr(rgb)).0 as isize };
        brushes.push((rgb, b));
        b
    };
    let (light_brush, dark_brush) = (brush_for(theme::FRAME_LIGHT), brush_for(dark_frame));

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(h.hwnd.get() as _);
    let caption = bgr(if dark { dark_frame } else { theme::FRAME_LIGHT });
    let text = bgr(if dark { 0xebe6dc } else { 0x2b2924 });
    let dark_flag: i32 = dark.into();
    unsafe {
        let set = |attr, value: *const std::ffi::c_void, size: usize| {
            let _ = DwmSetWindowAttribute(hwnd, attr, value, size as u32);
        };
        set(
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark_flag as *const _ as _,
            4,
        );
        set(DWMWA_CAPTION_COLOR, &caption as *const _ as _, 4);
        set(DWMWA_TEXT_COLOR, &text as *const _ as _, 4);
        // Frosted toolbar: a black client background lets the window backdrop show through.
        let brush = if FROSTED.load(std::sync::atomic::Ordering::Relaxed) {
            use windows::Win32::Graphics::Gdi::{GetStockObject, BLACK_BRUSH};
            GetStockObject(BLACK_BRUSH).0 as isize
        } else if dark {
            dark_brush
        } else {
            light_brush
        };
        SetClassLongPtrW(hwnd, GCLP_HBRBACKGROUND, brush);
        let _ = InvalidateRect(Some(hwnd), None, true);
    }
}

#[cfg(not(windows))]
pub fn apply_frame(_window: &Window, _dark: bool) {}

/// Whether the frosted toolbar is on (changes the window's background brush).
#[cfg(windows)]
static FROSTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Windows 11: give the window a Mica backdrop that the transparent toolbar shows through.
/// Silently does nothing on Windows versions without system backdrops.
#[cfg(windows)]
pub fn apply_backdrop(window: &Window, frosted: bool, dark: bool) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMSBT_MAINWINDOW, DWMSBT_NONE,
        DWMWA_SYSTEMBACKDROP_TYPE,
    };
    use windows::Win32::UI::Controls::MARGINS;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    FROSTED.store(frosted, std::sync::atomic::Ordering::Relaxed);
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(h.hwnd.get() as _);
    let kind = if frosted {
        DWMSBT_MAINWINDOW
    } else {
        DWMSBT_NONE
    };
    let margin = if frosted { -1 } else { 0 };
    let margins = MARGINS {
        cxLeftWidth: margin,
        cxRightWidth: margin,
        cyTopHeight: margin,
        cyBottomHeight: margin,
    };
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &kind as *const _ as _,
            std::mem::size_of_val(&kind) as u32,
        );
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
    }
    apply_frame(window, dark);
}

#[cfg(not(windows))]
pub fn apply_backdrop(_window: &Window, _frosted: bool, _dark: bool) {}

/// Remove the native caption bar while keeping the rest of the frame, so the tab strip can
/// sit in the title-bar row. Windows still provides the shadow, edge resizing and Aero Snap;
/// the chrome strip draws its own minimize / maximize / close buttons.
#[cfg(windows)]
pub fn remove_caption(window: &Window) {
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{
        IsZoomed, SetWindowPos, NCCALCSIZE_PARAMS, SM_CXPADDEDBORDER, SM_CYFRAME, SWP_FRAMECHANGED,
        SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WM_NCCALCSIZE,
    };
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    unsafe extern "system" fn proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        if msg == WM_NCCALCSIZE && wparam.0 != 0 {
            // Let Windows size the frame, then give the caption's height back to the client.
            let params = &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS);
            let top = params.rgrc[0].top;
            let result = DefSubclassProc(hwnd, msg, wparam, lparam);
            params.rgrc[0].top = top;
            if IsZoomed(hwnd).as_bool() {
                // A maximized window hangs its frame off-screen; keep the content on-screen.
                let dpi = GetDpiForWindow(hwnd);
                params.rgrc[0].top += GetSystemMetricsForDpi(SM_CYFRAME, dpi)
                    + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
            }
            return result;
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(h.hwnd.get() as _);
    unsafe {
        if SetWindowSubclass(hwnd, Some(proc), 1, 0).as_bool() {
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
            );
        }
    }
}

#[cfg(not(windows))]
pub fn remove_caption(_window: &Window) {}
