//! Dragging a tab out of the strip. The tab's real window follows the cursor; let go over another
//! Shoji window's strip and it joins that strip at the spot, let go near the left or right edge of
//! one and it snaps in beside the page that is open there as a split-view group, let go anywhere
//! else and it stays a window of its own. The host polls the real cursor and mouse button, since
//! the pointer has left the web page that started the drag.

use super::{left_button_down, BrowserApp, SPLIT_GAP};
use crate::snapghost::{ease_out_cubic, lerp_rect, Rect, SnapGhost};
use shoji_core::tabs::{SplitState, TabId};
use std::time::{Duration, Instant};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

/// Height of the strip row of a window, in logical px (title bar with the tabs in it).
pub const STRIP_HEIGHT: f64 = 44.0;
/// How close to a window's left or right edge a drop snaps, in logical px.
const SNAP_EDGE: f64 = 72.0;
/// How long the cursor must rest in a snap zone before the tile appears.
const SNAP_HOVER: Duration = Duration::from_millis(110);
const GHOST_IN_MS: u64 = 190;
const GHOST_OUT_MS: u64 = 130;
const COMMIT_MS: u64 = 170;
/// How long the dragged window takes to melt into a strip it hovers over (and back out)
const FADE_MS: f64 = 130.0;
/// Shortest time between two frames of a drag
const FRAME: Duration = Duration::from_millis(7);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DropTarget {
    Nothing,
    Strip {
        win: WindowId,
        index: usize,
    },
    /// Over the middle of a split pill: the tab joins that split
    Join {
        win: WindowId,
        split: u32,
    },
    Snap {
        win: WindowId,
        side: Side,
    },
}

struct Commit {
    start: Instant,
    from: Rect,
    to: Rect,
    win: WindowId,
    side: Side,
}

pub struct TabDrag {
    tab_id: TabId,
    /// The dragged tab's title, shown in the gap it will take
    title: String,
    /// Its icon, for the preview tab that follows the pointer over a strip
    favicon: String,
    /// Where the pointer last was over the strip it hovers (logical px), to send only changes
    last_x: f64,
    src_win: WindowId,
    /// The window that follows the cursor (a new one, or the source when it had only this tab)
    drag_win: WindowId,
    created: bool,
    orig_index: usize,
    orig_rect: Option<(PhysicalPosition<i32>, PhysicalSize<u32>)>,
    /// Cursor offset inside the dragged window, physical px
    grab: (i32, i32),
    target: DropTarget,
    snap_since: Option<Instant>,
    ghost: Option<SnapGhost>,
    commit: Option<Commit>,
    /// When the last frame ran (frames are paced; see `step_tab_drag`)
    last_step: Instant,
    /// Where the cursor was at the last frame
    cursor: (i32, i32),
    /// How opaque the dragged window is right now (it melts away over a strip it can join)
    alpha: f64,
}

/// Index a tab dragged to horizontal position `x` (logical px) would be inserted at, given the
/// centres of the tabs already in the strip.
pub fn strip_index(centers: &[f64], x: f64) -> usize {
    centers.iter().filter(|&&c| c < x).count()
}

/// Which snap zone `x` lies in, for a content area starting at `left` and `width` wide.
pub fn snap_side(x: f64, left: f64, width: f64, edge: f64) -> Option<Side> {
    if x >= left && x < left + edge {
        Some(Side::Left)
    } else if x <= left + width && x > left + width - edge {
        Some(Side::Right)
    } else {
        None
    }
}

/// The rectangle a snapped tab will occupy, laid out exactly as split view does: its half of
/// `content`, or (when the window already shows a split of `existing` panes) its share of the
/// split with one more pane.
pub fn snap_target_rect(content: Rect, gap: f64, side: Side, existing: usize) -> Rect {
    let total = if existing >= 2 { existing + 1 } else { 2 };
    let split = SplitState::with_panes((1..=total as u64).map(TabId).collect());
    let rects = split.layout(content, gap);
    match side {
        Side::Left => rects[0],
        Side::Right => rects[total - 1],
    }
}

#[cfg(windows)]
pub fn cursor_pos() -> (i32, i32) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT::default();
    let _ = unsafe { GetCursorPos(&mut p) };
    (p.x, p.y)
}
#[cfg(not(windows))]
pub fn cursor_pos() -> (i32, i32) {
    (0, 0)
}

#[cfg(windows)]
fn escape_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};
    (unsafe { GetAsyncKeyState(VK_ESCAPE.0 as i32) } as u16 & 0x8000) != 0
}
#[cfg(not(windows))]
fn escape_down() -> bool {
    false
}

/// Make a window see-through (1.0 is normal). Layered only while it is partly transparent, so a
/// normal window keeps drawing the ordinary way.
#[cfg(windows)]
fn set_alpha(win: &winit::window::Window, alpha: f64) {
    use windows::Win32::Foundation::{COLORREF, HWND};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, GWL_EXSTYLE, LWA_ALPHA,
        WS_EX_LAYERED,
    };
    let Ok(handle) = winit::raw_window_handle::HasWindowHandle::window_handle(win) else {
        return;
    };
    let winit::raw_window_handle::RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(h.hwnd.get() as *mut _);
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let layered = ex & WS_EX_LAYERED.0 as isize != 0;
        if alpha >= 0.999 {
            if layered {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex & !(WS_EX_LAYERED.0 as isize));
            }
        } else {
            if !layered {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as isize);
            }
            let _ = SetLayeredWindowAttributes(
                hwnd,
                COLORREF(0),
                (alpha.clamp(0.0, 1.0) * 255.0) as u8,
                LWA_ALPHA,
            );
        }
    }
}
#[cfg(not(windows))]
fn set_alpha(_: &winit::window::Window, _: f64) {}

/// Position of a window in the desktop's stacking order (0 = front).
#[cfg(windows)]
fn z_rank(win: &winit::window::Window) -> usize {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{GetTopWindow, GetWindow, GW_HWNDNEXT};
    let Ok(handle) = winit::raw_window_handle::HasWindowHandle::window_handle(win) else {
        return usize::MAX;
    };
    let winit::raw_window_handle::RawWindowHandle::Win32(h) = handle.as_raw() else {
        return usize::MAX;
    };
    let mine = h.hwnd.get();
    let mut cur = unsafe { GetTopWindow(None) }.ok();
    let mut rank = 0;
    while let Some(hwnd) = cur {
        if hwnd == HWND(mine as *mut _) {
            return rank;
        }
        rank += 1;
        if rank > 4000 {
            break;
        }
        cur = unsafe { GetWindow(hwnd, GW_HWNDNEXT) }.ok();
    }
    usize::MAX
}
#[cfg(not(windows))]
fn z_rank(_: &winit::window::Window) -> usize {
    0
}

fn inner_rect(w: &winit::window::Window) -> Rect {
    let p = w.inner_position().unwrap_or_default();
    let s = w.inner_size();
    (p.x as f64, p.y as f64, s.width as f64, s.height as f64)
}

fn outer_rect(w: &winit::window::Window) -> Rect {
    let p = w.outer_position().unwrap_or_default();
    let s = w.outer_size();
    (p.x as f64, p.y as f64, s.width as f64, s.height as f64)
}

fn contains(r: Rect, p: (i32, i32)) -> bool {
    let (x, y) = (p.0 as f64, p.1 as f64);
    x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3
}

impl BrowserApp {
    /// The chrome strip handed over a tab drag: start moving a window with the cursor.
    pub(super) fn start_tab_drag(
        &mut self,
        win_id: WindowId,
        tab_id: TabId,
        (grab_dx, grab_dy, strip_left): (f64, f64, f64),
        event_loop: &ActiveEventLoop,
    ) {
        if self.tab_drag.is_some() || !self.settings.tabs.enable_tab_tearoff || !left_button_down()
        {
            return;
        }
        let Some(src) = self.windows.get(&win_id) else {
            return;
        };
        let Some(orig_index) = src.tabs.tabs().iter().position(|t| t.id == tab_id) else {
            return;
        };
        let title = src.tabs.tabs()[orig_index].title.clone();
        let favicon = src.tabs.tabs()[orig_index]
            .favicon_uri
            .clone()
            .unwrap_or_default();
        let single = src.tabs.tabs().len() + src.tabs.inactive_tabs().len() == 1;
        let cursor = cursor_pos();
        let (drag_win, created, orig_rect, grab) = if single {
            let pos = src.window.outer_position().unwrap_or_default();
            (
                win_id,
                false,
                Some((pos, src.window.outer_size())),
                (cursor.0 - pos.x, cursor.1 - pos.y),
            )
        } else {
            let scale = src.window.scale_factor();
            let size = src.window.inner_size().to_logical::<f64>(scale);
            let Some(new) = self.move_tab(win_id, tab_id, None, None, event_loop) else {
                return;
            };
            let Some(w) = self.windows.get(&new) else {
                return;
            };
            let s = w.window.scale_factor();
            let _ = w.window.request_inner_size(winit::dpi::LogicalSize::new(
                (size.width * 0.7).max(720.0),
                (size.height * 0.7).max(480.0),
            ));
            // The dragged tab is the only one in its new window, at the start of the strip: put
            // the pointer on it, where it was on the tab, so it stays on the tab all the way.
            let frame = {
                let (o, i) = (
                    w.window.outer_position().unwrap_or_default(),
                    w.window.inner_position().unwrap_or_default(),
                );
                (i.x - o.x, i.y - o.y)
            };
            let grab = (
                ((strip_left + grab_dx) * s) as i32 + frame.0,
                ((6.0 + grab_dy) * s) as i32 + frame.1,
            );
            w.window
                .set_outer_position(PhysicalPosition::new(cursor.0 - grab.0, cursor.1 - grab.1));
            (new, true, None, grab)
        };
        self.set_drag_level(drag_win, true);
        self.tab_drag = Some(TabDrag {
            tab_id,
            title,
            favicon,
            last_x: f64::MIN,
            src_win: win_id,
            drag_win,
            created,
            orig_index,
            orig_rect,
            grab,
            target: DropTarget::Nothing,
            snap_since: None,
            ghost: Some(SnapGhost::new(self.accent_rgb())),
            commit: None,
            last_step: Instant::now(),
            cursor: (i32::MIN, i32::MIN),
            alpha: 1.0,
        });
    }

    /// Where would the dragged tab land if released at `cursor`?
    fn tab_drop_target(&self, cursor: (i32, i32), exclude: WindowId) -> DropTarget {
        let Some((id, w)) = self
            .windows
            .iter()
            .filter(|(id, w)| **id != exclude && contains(outer_rect(&w.window), cursor))
            .min_by_key(|(_, w)| z_rank(&w.window))
        else {
            return DropTarget::Nothing;
        };
        let scale = w.window.scale_factor();
        let (ix, iy, iw, ih) = inner_rect(&w.window);
        let strip = (ix, iy, iw, STRIP_HEIGHT * scale);
        if contains(strip, cursor) {
            let x = (cursor.0 as f64 - ix) / scale;
            for &(split, left, right) in &w.split_zones {
                let inset = (right - left) * 0.2;
                if x > left + inset && x < right - inset {
                    return DropTarget::Join { win: *id, split };
                }
            }
            return DropTarget::Strip {
                win: *id,
                index: strip_index(&w.tab_centers, x),
            };
        }
        let top = iy + w.chrome_height() * scale;
        let content = (ix, top, iw, (ih - w.chrome_height() * scale).max(1.0));
        // A split pairs the page open there with the new tab: it needs an ordinary active tab.
        let can_split = w.tabs.active_tab().is_some_and(|t| !t.pinned)
            && w.fullscreen_tab.is_none()
            && w.tabs.visible_split().map_or(0, |s| s.panes.len())
                < shoji_core::tabs::MAX_SPLIT_PANES;
        if can_split && contains(content, cursor) {
            if let Some(side) = snap_side(cursor.0 as f64, ix, iw, SNAP_EDGE * scale) {
                return DropTarget::Snap { win: *id, side };
            }
        }
        DropTarget::Nothing
    }

    fn tab_snap_rect(&self, win: WindowId, side: Side) -> Option<Rect> {
        let w = self.windows.get(&win)?;
        let scale = w.window.scale_factor();
        let (ix, iy, iw, ih) = inner_rect(&w.window);
        let content = (
            ix,
            iy + w.chrome_height() * scale,
            iw,
            (ih - w.chrome_height() * scale).max(1.0),
        );
        let existing = w.tabs.visible_split().map_or(0, |s| s.panes.len());
        Some(snap_target_rect(content, SPLIT_GAP * scale, side, existing))
    }

    /// Open (or close) the gap a tab dragged over `win`'s strip would take. `instant` skips the
    /// animation, for when the drag is over and the real tab is about to appear.
    fn set_drop_mark(
        &self,
        win: WindowId,
        index: Option<usize>,
        title: &str,
        favicon: &str,
        instant: bool,
    ) {
        if let Some(w) = self.windows.get(&win) {
            let arg = index.map_or("null".to_string(), |i| i.to_string());
            let title = serde_json::to_string(title).unwrap_or_else(|_| "\"\"".into());
            let icon = serde_json::to_string(favicon).unwrap_or_else(|_| "\"\"".into());
            w.eval_chrome(&format!(
                "window.__dropMark && window.__dropMark({arg},{title},{instant},{icon})"
            ));
        }
    }

    /// Highlight the split pill a dragged tab would join (or clear it).
    fn set_join_mark(&self, win: WindowId, split: Option<u32>) {
        if let Some(w) = self.windows.get(&win) {
            let arg = split.map_or("null".to_string(), |s| s.to_string());
            w.eval_chrome(&format!("window.__dropJoin && window.__dropJoin({arg})"));
        }
    }

    fn accent_rgb(&self) -> u32 {
        let hex = shoji_brand::theme::tones(self.settings.accent()).1;
        u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0x3f5f98)
    }

    /// One frame of an active tab drag. Returns true while it needs the next frame.
    pub(super) fn step_tab_drag(&mut self, event_loop: &ActiveEventLoop) -> bool {
        // Moving the window raises more events, and every event ends in a call here. Unpaced,
        // that feeds itself: thousands of window moves a second, which is what made a torn-off
        // tab stutter. One frame per display refresh is plenty.
        if self
            .tab_drag
            .as_ref()
            .is_some_and(|d| d.last_step.elapsed() < FRAME)
        {
            return true;
        }
        let dt = self
            .tab_drag
            .as_ref()
            .map_or(0.0, |d| d.last_step.elapsed().as_secs_f64() * 1000.0)
            .min(60.0);
        let r = self.step_tab_drag_inner(event_loop, dt);
        if let Some(d) = self.tab_drag.as_mut() {
            d.last_step = Instant::now();
        }
        r
    }

    fn step_tab_drag_inner(&mut self, event_loop: &ActiveEventLoop, frame_dt: f64) -> bool {
        let Some(mut drag) = self.tab_drag.take() else {
            return false;
        };
        if !self.windows.contains_key(&drag.drag_win) {
            self.clear_drop_marks(false);
            return false; // the dragged window is gone
        }
        // Snapping in: the window glides into its half, then joins.
        if let Some(c) = &drag.commit {
            let p = (c.start.elapsed().as_secs_f64() / (COMMIT_MS as f64 / 1000.0)).min(1.0);
            let r = lerp_rect(c.from, c.to, ease_out_cubic(p));
            if let Some(w) = self.windows.get(&drag.drag_win) {
                set_alpha(
                    &w.window,
                    drag.alpha + (1.0 - drag.alpha) * ease_out_cubic(p),
                );
                w.window
                    .set_outer_position(PhysicalPosition::new(r.0 as i32, r.1 as i32));
                let _ = w.window.request_inner_size(PhysicalSize::new(
                    r.2.max(1.0) as u32,
                    r.3.max(1.0) as u32,
                ));
            }
            if let Some(g) = drag.ghost.as_mut() {
                g.step();
            }
            if p >= 1.0 {
                let (win, side) = (c.win, c.side);
                self.complete_snap(drag, win, side, event_loop);
                return false;
            }
            self.tab_drag = Some(drag);
            return true;
        }
        if escape_down() {
            self.cancel_tab_drag(drag, event_loop);
            return false;
        }
        if !left_button_down() {
            return self.finish_tab_drag(drag, event_loop);
        }
        let cursor = cursor_pos();
        if cursor != drag.cursor {
            drag.cursor = cursor;
            if let Some(w) = self.windows.get(&drag.drag_win) {
                w.window.set_outer_position(PhysicalPosition::new(
                    cursor.0 - drag.grab.0,
                    cursor.1 - drag.grab.1,
                ));
            }
        }
        let target = self.tab_drop_target(cursor, drag.drag_win);
        if target != drag.target {
            if let DropTarget::Strip { win, .. } = drag.target {
                if !matches!(target, DropTarget::Strip { win: w, .. } if w == win) {
                    self.set_drop_mark(win, None, "", "", false);
                }
            }
            if let DropTarget::Join { win, split } = drag.target {
                if !matches!(target, DropTarget::Join { win: w, split: s } if w == win && s == split)
                {
                    self.set_join_mark(win, None);
                }
            }
            if let DropTarget::Join { win, split } = target {
                self.set_join_mark(win, Some(split));
            }
            if let DropTarget::Strip { win, index } = target {
                self.set_drop_mark(win, Some(index), &drag.title, &drag.favicon, false);
            }
            let leaving_snap = matches!(drag.target, DropTarget::Snap { .. });
            let same_snap = matches!((drag.target, target), (DropTarget::Snap { win: a, side: s }, DropTarget::Snap { win: b, side: t }) if a == b && s == t);
            if leaving_snap && !same_snap {
                if let (Some(g), Some(w)) = (drag.ghost.as_mut(), self.windows.get(&drag.drag_win))
                {
                    g.vanish(outer_rect(&w.window), GHOST_OUT_MS);
                }
                drag.snap_since = None;
            }
            if matches!(target, DropTarget::Snap { .. }) && !same_snap {
                drag.snap_since = Some(Instant::now());
            }
            drag.target = target;
        }
        // Over a strip, the preview tab follows the pointer along it.
        if let DropTarget::Strip { win, .. } = drag.target {
            if let Some(w) = self.windows.get(&win) {
                let (ix, _, _, _) = inner_rect(&w.window);
                let x = (cursor.0 as f64 - ix) / w.window.scale_factor();
                if (x - drag.last_x).abs() >= 1.0 {
                    drag.last_x = x;
                    w.eval_chrome(&format!("window.__dropMove && window.__dropMove({x:.1})"));
                }
            }
        } else {
            drag.last_x = f64::MIN;
        }
        // The tile appears after the cursor has rested in a snap zone for a moment.
        if let (DropTarget::Snap { win, side }, Some(since)) = (drag.target, drag.snap_since) {
            if since.elapsed() >= SNAP_HOVER {
                drag.snap_since = None;
                if let (Some(to), Some(w)) = (
                    self.tab_snap_rect(win, side),
                    self.windows.get(&drag.drag_win),
                ) {
                    let from = outer_rect(&w.window);
                    let rgb = self.accent_rgb();
                    drag.ghost
                        .get_or_insert_with(|| SnapGhost::new(rgb))
                        .appear(from, to, GHOST_IN_MS);
                }
            }
        }
        // Over a strip it can join, the window melts away (the strip opens a slot for the tab);
        // anywhere else it is back.
        let want = if matches!(
            drag.target,
            DropTarget::Strip { .. } | DropTarget::Join { .. }
        ) {
            0.0
        } else {
            1.0
        };
        if (drag.alpha - want).abs() > 0.001 {
            let step = frame_dt / FADE_MS;
            drag.alpha = if want > drag.alpha {
                (drag.alpha + step).min(want)
            } else {
                (drag.alpha - step).max(want)
            };
            if let Some(w) = self.windows.get(&drag.drag_win) {
                set_alpha(&w.window, drag.alpha);
            }
        }
        if let Some(g) = drag.ghost.as_mut() {
            g.step();
        }
        self.tab_drag = Some(drag);
        true
    }

    /// Keep the dragged window above every other window for the length of the drag.
    fn set_drag_level(&self, win: WindowId, on: bool) {
        if let Some(w) = self.windows.get(&win) {
            w.window.set_window_level(if on {
                winit::window::WindowLevel::AlwaysOnTop
            } else {
                winit::window::WindowLevel::Normal
            });
            if on {
                w.window.focus_window();
            }
        }
    }

    /// Clear every gap at once (the drag ended).
    ///
    /// `dropped`: the tab is about to land (the preview glides into its slot), as opposed to the
    /// drag being cancelled (the gap simply closes).
    fn clear_drop_marks(&self, dropped: bool) {
        for id in self.windows.keys() {
            self.set_drop_mark(*id, None, "", "", dropped);
            self.set_join_mark(*id, None);
        }
    }

    fn finish_tab_drag(&mut self, mut drag: TabDrag, event_loop: &ActiveEventLoop) -> bool {
        self.clear_drop_marks(true);
        match drag.target {
            DropTarget::Strip { win, index } => {
                self.move_tab(
                    drag.drag_win,
                    drag.tab_id,
                    Some(win),
                    Some(index),
                    event_loop,
                );
                false
            }
            DropTarget::Join { win, split } => {
                if self
                    .move_tab(drag.drag_win, drag.tab_id, Some(win), None, event_loop)
                    .is_some()
                {
                    if let Some(w) = self.windows.get_mut(&win) {
                        if w.tabs.add_to_split(split, drag.tab_id, None) {
                            w.show_tab(drag.tab_id);
                            w.layout();
                            w.ui_dirty = true;
                            w.sidebar_dirty = true;
                        }
                    }
                }
                false
            }
            DropTarget::Snap { win, side } => {
                let Some(to) = self.tab_snap_rect(win, side) else {
                    return false;
                };
                let Some(w) = self.windows.get(&drag.drag_win) else {
                    return false;
                };
                let from = outer_rect(&w.window);
                if let Some(g) = drag.ghost.as_mut() {
                    if !g.is_visible() {
                        g.appear(from, to, 0);
                    }
                }
                drag.commit = Some(Commit {
                    start: Instant::now(),
                    from,
                    to,
                    win,
                    side,
                });
                self.tab_drag = Some(drag);
                true
            }
            DropTarget::Nothing => {
                if let Some(w) = self.windows.get(&drag.drag_win) {
                    set_alpha(&w.window, 1.0);
                }
                self.set_drag_level(drag.drag_win, false);
                // Stay a window of its own; keep its title bar reachable.
                if let Some(w) = self.windows.get(&drag.drag_win) {
                    let p = w.window.outer_position().unwrap_or_default();
                    if p.y < 0 {
                        w.window.set_outer_position(PhysicalPosition::new(p.x, 0));
                    }
                    w.window.focus_window();
                }
                false
            }
        }
    }

    fn cancel_tab_drag(&mut self, mut drag: TabDrag, event_loop: &ActiveEventLoop) {
        self.clear_drop_marks(false);
        if let Some(w) = self.windows.get(&drag.drag_win) {
            set_alpha(&w.window, 1.0);
        }
        self.set_drag_level(drag.drag_win, false);
        if let Some(g) = drag.ghost.as_mut() {
            g.vanish(g.rect(), 1);
            g.step();
        }
        if drag.created {
            if self.windows.contains_key(&drag.src_win) {
                self.move_tab(
                    drag.drag_win,
                    drag.tab_id,
                    Some(drag.src_win),
                    Some(drag.orig_index),
                    event_loop,
                );
            }
        } else if let (Some((pos, size)), Some(w)) =
            (drag.orig_rect, self.windows.get(&drag.drag_win))
        {
            w.window.set_outer_position(pos);
            let _ = w.window.request_inner_size(size);
        }
    }

    /// The window has glided into place: join the target as a split group beside its open page.
    fn complete_snap(
        &mut self,
        drag: TabDrag,
        win: WindowId,
        side: Side,
        event_loop: &ActiveEventLoop,
    ) {
        let prev = self.windows.get(&win).and_then(|w| w.tabs.active_tab_id());
        if self
            .move_tab(drag.drag_win, drag.tab_id, Some(win), None, event_loop)
            .is_none()
        {
            return;
        }
        let Some(w) = self.windows.get_mut(&win) else {
            return;
        };
        if let Some(prev) = prev {
            // Beside a split that is already open, the tab becomes one more pane of it.
            let joined = match w.tabs.split_of(prev).map(|s| s.id) {
                Some(group) => {
                    let at = (side == Side::Left).then_some(0);
                    w.tabs.add_to_split(group, drag.tab_id, at)
                }
                None => {
                    let color = super::next_split_color(w);
                    let (a, b) = match side {
                        Side::Left => (drag.tab_id, prev),
                        Side::Right => (prev, drag.tab_id),
                    };
                    w.tabs.open_split_with(a, b, "", color).is_some()
                }
            };
            if joined {
                w.show_tab(drag.tab_id);
                w.layout();
                w.ui_dirty = true;
                w.sidebar_dirty = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insertion_index_counts_tabs_left_of_the_cursor() {
        let centers = [100.0, 300.0, 500.0];
        assert_eq!(strip_index(&centers, 10.0), 0);
        assert_eq!(strip_index(&centers, 200.0), 1);
        assert_eq!(strip_index(&centers, 400.0), 2);
        assert_eq!(strip_index(&centers, 900.0), 3);
        assert_eq!(strip_index(&[], 50.0), 0);
    }

    #[test]
    fn snap_zones_are_the_outer_edges_only() {
        assert_eq!(snap_side(5.0, 0.0, 1000.0, 72.0), Some(Side::Left));
        assert_eq!(snap_side(995.0, 0.0, 1000.0, 72.0), Some(Side::Right));
        assert_eq!(snap_side(500.0, 0.0, 1000.0, 72.0), None);
        assert_eq!(snap_side(-5.0, 0.0, 1000.0, 72.0), None);
    }

    #[test]
    fn snap_target_is_the_matching_half_of_the_content() {
        let content = (100.0, 200.0, 1010.0, 600.0);
        let l = snap_target_rect(content, 10.0, Side::Left, 0);
        let r = snap_target_rect(content, 10.0, Side::Right, 0);
        assert_eq!((l.0, l.1, l.3), (100.0, 200.0, 600.0));
        assert_eq!(l.2, 500.0);
        assert_eq!(r.0, 100.0 + 500.0 + 10.0);
        assert_eq!(r.2, 500.0);
        assert!(l.0 + l.2 + 10.0 <= r.0 + 0.001);
        // Beside a three-pane split the tile is one quarter of the content.
        let l = snap_target_rect(content, 10.0, Side::Left, 3);
        let r = snap_target_rect(content, 10.0, Side::Right, 3);
        assert_eq!(l.0, 100.0);
        assert!(l.2 < 260.0 && r.2 < 260.0);
        assert_eq!(r.0 + r.2, 100.0 + 1010.0);
    }
}
