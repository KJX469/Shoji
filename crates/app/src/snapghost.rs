//! The translucent tile that previews where a dragged tab will snap, and the maths that moves it.
//! A plain layered Win32 popup (not a browser window): it never takes focus or clicks, so the drag
//! underneath is undisturbed.

/// `(x, y, width, height)` in physical screen pixels.
pub type Rect = (f64, f64, f64, f64);

/// Ease-out cubic: fast at first, settling gently, like a window tiling into place.
pub fn ease_out_cubic(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

pub fn lerp_rect(a: Rect, b: Rect, t: f64) -> Rect {
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
        a.3 + (b.3 - a.3) * t,
    )
}

/// One animation of the ghost between two rectangles and two opacities.
#[derive(Clone, Copy)]
struct Tween {
    from: Rect,
    to: Rect,
    alpha_from: f64,
    alpha_to: f64,
    start: std::time::Instant,
    duration: std::time::Duration,
    /// Hide the window once the tween ends (a fade-out)
    then_hide: bool,
}

pub struct SnapGhost {
    inner: imp::Ghost,
    now: Rect,
    alpha: f64,
    tween: Option<Tween>,
    visible: bool,
}

impl SnapGhost {
    /// A hidden ghost tinted with `rgb` (0xRRGGBB).
    pub fn new(rgb: u32) -> Self {
        Self {
            inner: imp::Ghost::new(rgb),
            now: (0.0, 0.0, 0.0, 0.0),
            alpha: 0.0,
            tween: None,
            visible: false,
        }
    }

    /// Start showing: begin at `from` (transparent) and ease to `to` over `ms`.
    pub fn appear(&mut self, from: Rect, to: Rect, ms: u64) {
        self.now = from;
        self.alpha = 0.0;
        self.visible = true;
        self.tween = Some(Tween {
            from,
            to,
            alpha_from: 0.0,
            alpha_to: 1.0,
            start: std::time::Instant::now(),
            duration: std::time::Duration::from_millis(ms),
            then_hide: false,
        });
        self.inner.place(self.now, self.alpha);
    }

    /// Ease back out to `to` while fading away.
    pub fn vanish(&mut self, to: Rect, ms: u64) {
        if !self.visible {
            return;
        }
        self.tween = Some(Tween {
            from: self.now,
            to,
            alpha_from: self.alpha,
            alpha_to: 0.0,
            start: std::time::Instant::now(),
            duration: std::time::Duration::from_millis(ms),
            then_hide: true,
        });
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// The rectangle the ghost is at right now.
    pub fn rect(&self) -> Rect {
        self.now
    }

    /// Advance the animation; true while it is still moving.
    pub fn step(&mut self) -> bool {
        let Some(t) = self.tween else {
            return false;
        };
        let p = if t.duration.is_zero() {
            1.0
        } else {
            (t.start.elapsed().as_secs_f64() / t.duration.as_secs_f64()).min(1.0)
        };
        let e = ease_out_cubic(p);
        self.now = lerp_rect(t.from, t.to, e);
        self.alpha = t.alpha_from + (t.alpha_to - t.alpha_from) * e;
        if p >= 1.0 {
            self.tween = None;
            if t.then_hide {
                self.visible = false;
                self.inner.hide();
                return false;
            }
        }
        self.inner.place(self.now, self.alpha);
        p < 1.0
    }
}

#[cfg(windows)]
mod imp {
    use super::Rect;
    use windows::core::w;
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, CreateSolidBrush, SetWindowRgn};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SetLayeredWindowAttributes,
        SetWindowPos, ShowWindow, HWND_TOPMOST, LWA_ALPHA, SWP_NOACTIVATE, SW_HIDE,
        SW_SHOWNOACTIVATE, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    };

    /// Peak opacity of the tile (0-255).
    const PEAK_ALPHA: f64 = 120.0;
    const CORNER: i32 = 22;

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        DefWindowProcW(hwnd, msg, wp, lp)
    }

    pub struct Ghost {
        hwnd: Option<HWND>,
        shown: bool,
    }

    impl Ghost {
        pub fn new(rgb: u32) -> Self {
            let hwnd = unsafe { create(rgb) };
            Self { hwnd, shown: false }
        }

        pub fn place(&mut self, r: Rect, alpha: f64) {
            let Some(hwnd) = self.hwnd else { return };
            let (x, y, w, h) = (
                r.0.round() as i32,
                r.1.round() as i32,
                r.2.round().max(2.0) as i32,
                r.3.round().max(2.0) as i32,
            );
            unsafe {
                let _ = SetLayeredWindowAttributes(
                    hwnd,
                    COLORREF(0),
                    (alpha.clamp(0.0, 1.0) * PEAK_ALPHA) as u8,
                    LWA_ALPHA,
                );
                let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
                let region = CreateRoundRectRgn(0, 0, w + 1, h + 1, CORNER * 2, CORNER * 2);
                SetWindowRgn(hwnd, Some(region), true);
                if !self.shown {
                    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    self.shown = true;
                }
            }
        }

        pub fn hide(&mut self) {
            if let Some(hwnd) = self.hwnd {
                unsafe {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
            }
            self.shown = false;
        }
    }

    impl Drop for Ghost {
        fn drop(&mut self) {
            if let Some(hwnd) = self.hwnd.take() {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
        }
    }

    unsafe fn create(rgb: u32) -> Option<HWND> {
        let inst = GetModuleHandleW(None).ok()?;
        // COLORREF is 0x00BBGGRR.
        let colour = ((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff);
        let class = w!("ShojiSnapGhost");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: inst.into(),
            lpszClassName: class,
            hbrBackground: CreateSolidBrush(COLORREF(colour)),
            ..Default::default()
        };
        // A second registration just fails with "class exists"; that is fine.
        RegisterClassW(&wc);
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            class,
            w!(""),
            WS_POPUP,
            0,
            0,
            10,
            10,
            None,
            None,
            Some(inst.into()),
            None,
        )
        .ok()
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Rect;
    pub struct Ghost;
    impl Ghost {
        pub fn new(_: u32) -> Self {
            Ghost
        }
        pub fn place(&mut self, _: Rect, _: f64) {}
        pub fn hide(&mut self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_runs_from_zero_to_one_and_never_overshoots() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        let mut last = 0.0;
        for i in 1..=100 {
            let v = ease_out_cubic(i as f64 / 100.0);
            assert!(v >= last && v <= 1.0);
            last = v;
        }
        // Front-loaded: more than half the way at the half-way time.
        assert!(ease_out_cubic(0.5) > 0.8);
    }

    #[test]
    fn rects_interpolate_linearly() {
        let r = lerp_rect((0.0, 0.0, 100.0, 100.0), (100.0, 50.0, 200.0, 300.0), 0.5);
        assert_eq!(r, (50.0, 25.0, 150.0, 200.0));
    }
}
