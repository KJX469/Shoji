use serde::{Deserialize, Serialize};

/// Unique identifier for an open browser tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TabId(pub u64);

/// Current lifecycle status of a tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TabStatus {
    /// Actively visible in the main viewport
    Active,
    /// In background, rendered but not visible
    Inactive,
    /// Suspended via TrySuspendAsync to reclaim memory
    Suspended,
    /// Encountered a crash/render failure
    Crashed,
}

/// Metadata and state for an individual tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabState {
    pub id: TabId,
    pub url: String,
    pub title: String,
    pub favicon_uri: Option<String>,
    pub is_loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub is_audio_playing: bool,
    pub is_muted: bool,
    /// Showing one of Shoji's own notices (page not found, crash...) in place of `url`
    #[serde(default)]
    pub notice: bool,
    pub status: TabStatus,
    pub last_active_timestamp_secs: u64,
    /// Tab group this tab belongs to, if any
    #[serde(default)]
    pub group: Option<u32>,
    /// Pinned tabs sit together at the left of the strip, icon only
    #[serde(default)]
    pub pinned: bool,
    /// Workspace this tab belongs to (1-based).
    #[serde(default = "default_workspace_id")]
    pub workspace: u32,
}

pub const fn default_workspace_id() -> u32 {
    1
}

pub const DEFAULT_SPLIT_RATIO: f64 = 0.5;
pub const MIN_SPLIT_RATIO: f64 = 0.2;
pub const MAX_SPLIT_RATIO: f64 = 0.8;
pub const MIN_SPLIT_PANE_PX: f64 = 120.0;

const fn default_split_ratio() -> f64 {
    DEFAULT_SPLIT_RATIO
}

/// Orientation of the divider between two split panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitOrientation {
    /// Side-by-side panes (left and right) with a vertical divider.
    #[default]
    Vertical,
    /// Stacked panes (top and bottom) with a horizontal divider.
    Horizontal,
}

fn default_split_color() -> String {
    "blue".to_string()
}

/// Most panes one split group can hold.
pub const MAX_SPLIT_PANES: usize = 20;

/// A user-created group of tabs shown side by side (or stacked) inside a workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitState {
    /// Group id, unique per window (0 until assigned by the manager).
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub name: String,
    /// Crane accent id, like `TabGroup.color`.
    #[serde(default = "default_split_color")]
    pub color: String,
    /// The tabs, in the order they sit on screen (left to right, or top to bottom).
    pub panes: Vec<TabId>,
    #[serde(default)]
    pub orientation: SplitOrientation,
    /// Each pane's share of the space, in pane order, summing to 1. Empty (older files) means
    /// "work it out": the legacy ratio for two panes, equal shares otherwise.
    #[serde(default)]
    pub weights: Vec<f64>,
    /// Older files: the first of two panes' share.
    #[serde(default = "default_split_ratio", rename = "ratio", skip_serializing)]
    legacy_ratio: f64,
}

impl SplitState {
    pub fn new(first: TabId, second: TabId) -> Self {
        Self::with_panes(vec![first, second])
    }

    pub fn with_panes(panes: Vec<TabId>) -> Self {
        let mut s = Self {
            id: 0,
            name: String::new(),
            color: default_split_color(),
            panes,
            orientation: SplitOrientation::Vertical,
            weights: Vec::new(),
            legacy_ratio: DEFAULT_SPLIT_RATIO,
        };
        s.equalize();
        s
    }

    /// A split restored from an older file or session: the first pane's share, for two panes.
    pub fn with_legacy_ratio(mut self, ratio: f64) -> Self {
        self.legacy_ratio = Self::clamp_ratio(ratio, None, 0.0);
        self.weights.clear();
        self.fix_weights();
        self
    }

    /// Give every pane the same share.
    pub fn equalize(&mut self) {
        let n = self.panes.len().max(1) as f64;
        self.weights = vec![1.0 / n; self.panes.len()];
        self.legacy_ratio = DEFAULT_SPLIT_RATIO;
    }

    /// Make `weights` valid: one positive share per pane, summing to 1.
    pub fn fix_weights(&mut self) {
        let n = self.panes.len();
        let ok = self.weights.len() == n && self.weights.iter().all(|w| w.is_finite() && *w > 0.0);
        if !ok {
            if n == 2 {
                let r = Self::clamp_ratio(self.legacy_ratio, None, 0.0);
                self.weights = vec![r, 1.0 - r];
            } else {
                self.equalize();
            }
        }
        let sum: f64 = self.weights.iter().sum();
        if sum > 0.0 && (sum - 1.0).abs() > 1e-9 {
            for w in &mut self.weights {
                *w /= sum;
            }
        }
    }

    /// The first pane's share of a two-pane split (what the divider label shows).
    pub fn ratio(&self) -> f64 {
        match self.weights.as_slice() {
            [a, b, ..] => a / (a + b),
            _ => DEFAULT_SPLIT_RATIO,
        }
    }

    /// Clamp a divider ratio to valid bounds, respecting [`MIN_SPLIT_PANE_PX`] when the container
    /// extent is known.
    pub fn clamp_ratio(ratio: f64, total_extent_px: Option<f64>, gap_px: f64) -> f64 {
        if !ratio.is_finite() {
            return DEFAULT_SPLIT_RATIO;
        }
        let mut min_r = MIN_SPLIT_RATIO;
        if let Some(total) = total_extent_px {
            let avail = total - gap_px.max(0.0);
            if avail > 2.0 * MIN_SPLIT_PANE_PX {
                min_r = (MIN_SPLIT_PANE_PX / avail).clamp(MIN_SPLIT_RATIO, DEFAULT_SPLIT_RATIO);
            }
        }
        ratio.clamp(min_r, 1.0 - min_r)
    }

    /// The panes' sizes along the split axis for a container `extent` px long with `gap` px between
    /// panes. Sizes are whole pixels, each at least the minimum (or an equal share when the
    /// container is too small for that), and add up to exactly `extent - gap * (n - 1)`.
    pub fn sizes(&self, extent: f64, gap: f64) -> Vec<f64> {
        let n = self.panes.len();
        if n == 0 {
            return Vec::new();
        }
        let gap = gap.max(0.0);
        let avail = (extent - gap * (n as f64 - 1.0)).max(n as f64);
        let min = MIN_SPLIT_PANE_PX.min((avail / n as f64).floor());
        let mut w = self.weights.clone();
        if w.len() != n || w.iter().any(|x| !x.is_finite() || *x <= 0.0) {
            let mut fixed = self.clone();
            fixed.fix_weights();
            w = fixed.weights;
        }
        let sum: f64 = w.iter().sum();
        let mut sizes: Vec<f64> = w.iter().map(|x| avail * x / sum).collect();
        // Panes below the minimum grow to it; the rest give up the difference in proportion.
        for _ in 0..n {
            let short: f64 = sizes.iter().filter(|s| **s < min).map(|s| min - s).sum();
            if short <= 0.0 {
                break;
            }
            let spare: f64 = sizes.iter().filter(|s| **s > min).map(|s| s - min).sum();
            if spare <= 0.0 {
                break;
            }
            let take = short.min(spare);
            for s in &mut sizes {
                if *s < min {
                    *s = min;
                } else {
                    *s -= take * (*s - min) / spare;
                }
            }
        }
        // Whole pixels that still add up.
        let mut out = Vec::with_capacity(n);
        let (mut acc, mut done) = (0.0, 0.0);
        for s in &sizes {
            acc += s;
            let edge = acc.round();
            out.push((edge - done).max(1.0));
            done = edge;
        }
        out
    }

    /// The panes' rectangles `(x, y, width, height)` within `bounds`, in pane order.
    pub fn layout(&self, bounds: (f64, f64, f64, f64), gap_px: f64) -> Vec<(f64, f64, f64, f64)> {
        let (x, y, w, h) = bounds;
        let gap = gap_px.max(0.0);
        let vertical = self.orientation == SplitOrientation::Vertical;
        let sizes = self.sizes(if vertical { w } else { h }, gap);
        let mut at = if vertical { x } else { y };
        sizes
            .into_iter()
            .map(|s| {
                let r = if vertical {
                    (at, y, s, h)
                } else {
                    (x, at, w, s)
                };
                at += s + gap;
                r
            })
            .collect()
    }

    /// The draggable gaps between neighbouring panes, `n - 1` of them.
    pub fn divider_rects(
        &self,
        bounds: (f64, f64, f64, f64),
        gap_px: f64,
    ) -> Vec<(f64, f64, f64, f64)> {
        let gap = gap_px.max(0.0);
        let panes = self.layout(bounds, gap);
        panes
            .iter()
            .take(panes.len().saturating_sub(1))
            .map(|p| match self.orientation {
                SplitOrientation::Vertical => (p.0 + p.2, bounds.1, gap, bounds.3),
                SplitOrientation::Horizontal => (bounds.0, p.1 + p.3, bounds.2, gap),
            })
            .collect()
    }

    /// Move divider `index` (between panes `index` and `index + 1`) so its centre is at `cursor`
    /// (window coordinates along the split axis). The other panes keep their sizes. Returns
    /// true when something changed.
    pub fn drag_divider(
        &mut self,
        index: usize,
        bounds: (f64, f64, f64, f64),
        gap_px: f64,
        cursor: f64,
    ) -> bool {
        let n = self.panes.len();
        if index + 1 >= n {
            return false;
        }
        let gap = gap_px.max(0.0);
        let vertical = self.orientation == SplitOrientation::Vertical;
        let (start, extent) = if vertical {
            (bounds.0, bounds.2)
        } else {
            (bounds.1, bounds.3)
        };
        let sizes = self.sizes(extent, gap);
        let avail: f64 = sizes.iter().sum();
        let min = MIN_SPLIT_PANE_PX.min((avail / n as f64).floor());
        let first_start = start + sizes[..index].iter().map(|s| s + gap).sum::<f64>();
        let pair = sizes[index] + sizes[index + 1];
        let want = (cursor - gap * 0.5 - first_start).clamp(min, (pair - min).max(min));
        if (want - sizes[index]).abs() < 0.5 {
            return false;
        }
        let mut w: Vec<f64> = sizes.iter().map(|s| s / avail).collect();
        w[index] = want / avail;
        w[index + 1] = (pair - want) / avail;
        self.weights = w;
        true
    }
}

/// A named, colour-coded workspace of tabs. Exactly one workspace is active per window; tabs in
/// inactive workspaces stay suspended in memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: u32,
    pub name: String,
    pub color: String,
    #[serde(default)]
    pub active_tab: Option<TabId>,
    /// All split groups of this workspace.
    #[serde(default)]
    pub splits: Vec<SplitState>,
    /// Legacy single split (pre split groups); migrated into `splits` on load.
    #[serde(default, skip_serializing)]
    pub split: Option<SplitState>,
}

impl Workspace {
    pub fn default_main() -> Self {
        Self {
            id: 1,
            name: "Main".to_string(),
            color: "blue".to_string(),
            active_tab: None,
            splits: Vec::new(),
            split: None,
        }
    }
}

fn default_workspaces() -> Vec<Workspace> {
    vec![Workspace::default_main()]
}

fn default_next_workspace_id() -> u32 {
    2
}

fn default_next_split_id() -> u32 {
    1
}

/// A named, colour-coded set of tabs. `color` is a crane accent id ("blue", "gold", ...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabGroup {
    pub id: u32,
    pub name: String,
    pub color: String,
}

/// A recently closed tab, kept in memory for reopening and the new tab page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClosedTab {
    pub url: String,
    pub title: String,
}

/// Pure state manager for all tabs inside a single window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabManager {
    tabs: Vec<TabState>,
    #[serde(default)]
    inactive_tabs: Vec<TabState>,
    active_tab_id: Option<TabId>,
    next_id: u64,
    /// Tab ids handed out by every window of the app, so no two windows ever share one. Unset
    /// (ids count from 1 per manager) outside the app.
    #[serde(skip)]
    shared_ids: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
    closed_history: Vec<ClosedTab>, // closed tabs for Ctrl+Shift+T (in-memory only)
    #[serde(default)]
    groups: Vec<TabGroup>,
    #[serde(default)]
    next_group_id: u32,
    #[serde(default = "default_workspaces")]
    workspaces: Vec<Workspace>,
    #[serde(default = "default_workspace_id")]
    active_workspace_id: u32,
    #[serde(default = "default_next_workspace_id")]
    next_workspace_id: u32,
    #[serde(default = "default_next_split_id")]
    next_split_id: u32,
}

impl Default for TabManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TabManager {
    /// A manager whose tab ids come from `ids`, a counter shared by all windows.
    pub fn with_shared_ids(ids: std::sync::Arc<std::sync::atomic::AtomicU64>) -> Self {
        let mut m = Self::new();
        m.shared_ids = Some(ids);
        m
    }

    fn alloc_id(&mut self) -> TabId {
        use std::sync::atomic::Ordering::SeqCst;
        match &self.shared_ids {
            Some(shared) => TabId(shared.fetch_add(1, SeqCst)),
            None => {
                let id = TabId(self.next_id);
                self.next_id += 1;
                id
            }
        }
    }

    /// Make sure ids up to and including `used` are never handed out again.
    fn note_id(&mut self, used: u64) {
        use std::sync::atomic::Ordering::SeqCst;
        self.next_id = self.next_id.max(used + 1);
        if let Some(shared) = &self.shared_ids {
            shared.fetch_max(used + 1, SeqCst);
        }
    }

    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            inactive_tabs: Vec::new(),
            active_tab_id: None,
            next_id: 1,
            shared_ids: None,
            closed_history: Vec::new(),
            groups: Vec::new(),
            next_group_id: 1,
            workspaces: default_workspaces(),
            active_workspace_id: 1,
            next_workspace_id: 2,
            next_split_id: 1,
        }
    }

    /// Move legacy single `split` fields into `splits` (assigning ids and names) and make sure
    /// every group has a unique id below `next_split_id`.
    fn migrate_splits(&mut self) {
        let mut max_id = self.next_split_id.max(1) - 1;
        for ws in &self.workspaces {
            for s in &ws.splits {
                max_id = max_id.max(s.id);
            }
        }
        self.next_split_id = max_id + 1;
        let mut next = self.next_split_id;
        let mut seen: Vec<u32> = Vec::new();
        for ws in &mut self.workspaces {
            if let Some(mut legacy) = ws.split.take() {
                legacy.id = next;
                next += 1;
                if legacy.name.trim().is_empty() {
                    legacy.name = format!("Split {}", legacy.id);
                }
                if legacy.panes.len() >= 2 && legacy.panes[0] != legacy.panes[1] {
                    ws.splits.push(legacy);
                }
            }
            // Zero or duplicate ids (hand-edited or old data) get fresh ones.
            for s in &mut ws.splits {
                if s.id == 0 || seen.contains(&s.id) {
                    s.id = next;
                    next += 1;
                    if s.name.trim().is_empty() {
                        s.name = format!("Split {}", s.id);
                    }
                }
                seen.push(s.id);
            }
        }
        self.next_split_id = next;
    }

    fn ensure_workspace(&mut self) {
        if self.workspaces.iter().any(|w| w.split.is_some()) {
            self.migrate_splits();
        }
        for s in self.workspaces.iter_mut().flat_map(|w| w.splits.iter_mut()) {
            if s.weights.len() != s.panes.len() {
                s.fix_weights();
            }
        }
        if self.workspaces.is_empty() {
            self.workspaces.push(Workspace::default_main());
            self.active_workspace_id = 1;
            self.next_workspace_id = self.next_workspace_id.max(2);
        } else if !self
            .workspaces
            .iter()
            .any(|w| w.id == self.active_workspace_id)
        {
            self.active_workspace_id = self.workspaces[0].id;
        }
    }

    fn sync_active_workspace_tab(&mut self) {
        self.ensure_workspace();
        let active_id = self.active_tab_id;
        if let Some(ws) = self
            .workspaces
            .iter_mut()
            .find(|w| w.id == self.active_workspace_id)
        {
            ws.active_tab = active_id;
        }
    }

    fn record_closed(&mut self, url: String, title: String) {
        if !url.is_empty() && url != "about:blank" {
            self.closed_history.push(ClosedTab { url, title });
            if self.closed_history.len() > 50 {
                self.closed_history.remove(0);
            }
        }
    }

    pub fn create_tab(&mut self, url: &str, current_time_secs: u64) -> TabId {
        self.ensure_workspace();
        let id = self.alloc_id();

        let new_tab = TabState {
            id,
            url: url.to_string(),
            title: "New Tab".to_string(),
            favicon_uri: None,
            is_loading: false,
            can_go_back: false,
            can_go_forward: false,
            is_audio_playing: false,
            notice: false,
            is_muted: false,
            status: TabStatus::Active,
            last_active_timestamp_secs: current_time_secs,
            group: None,
            pinned: false,
            workspace: self.active_workspace_id,
        };

        // If another tab was active, mark it inactive
        if let Some(prev_active_id) = self.active_tab_id {
            if let Some(prev) = self.tabs.iter_mut().find(|t| t.id == prev_active_id) {
                prev.status = TabStatus::Inactive;
            }
        }

        self.tabs.push(new_tab);
        self.active_tab_id = Some(id);
        self.sync_active_workspace_tab();
        id
    }

    pub fn close_tab(&mut self, id: TabId) -> Option<TabId> {
        self.ensure_workspace();
        if let Some(pos) = self.tabs.iter().position(|t| t.id == id) {
            let removed = self.tabs.remove(pos);
            self.record_closed(removed.url, removed.title);

            // If this tab was in a split group, dissolve that group and prefer the surviving
            // pane as the next active tab.
            let surviving_pane = self.leave_splits(id);

            if self.active_tab_id == Some(id) {
                if self.tabs.is_empty() {
                    self.active_tab_id = None;
                } else if let Some(other) =
                    surviving_pane.filter(|&p| self.tabs.iter().any(|t| t.id == p))
                {
                    if let Some(t) = self.tabs.iter_mut().find(|t| t.id == other) {
                        t.status = TabStatus::Active;
                    }
                    self.active_tab_id = Some(other);
                } else {
                    let next_pos = if pos >= self.tabs.len() {
                        self.tabs.len() - 1
                    } else {
                        pos
                    };
                    let next_id = self.tabs[next_pos].id;
                    self.tabs[next_pos].status = TabStatus::Active;
                    self.active_tab_id = Some(next_id);
                }
            }
            self.sync_active_workspace_tab();
            self.prune_groups();
        } else if let Some(pos) = self.inactive_tabs.iter().position(|t| t.id == id) {
            let removed = self.inactive_tabs.remove(pos);
            let ws_id = removed.workspace;
            self.record_closed(removed.url, removed.title);

            let surviving_pane = self.leave_splits(id);
            if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == ws_id) {
                if ws.active_tab == Some(id) {
                    ws.active_tab = surviving_pane
                        .filter(|&p| {
                            self.inactive_tabs
                                .iter()
                                .any(|t| t.workspace == ws_id && t.id == p)
                        })
                        .or_else(|| {
                            self.inactive_tabs
                                .iter()
                                .find(|t| t.workspace == ws_id)
                                .map(|t| t.id)
                        });
                }
            }
            self.prune_groups();
        }
        self.active_tab_id
    }

    /// Close a tab in its workspace; if it was the last tab of that workspace, leave the workspace
    /// in place with a fresh `"shoji://newtab"` blank tab.
    pub fn close_workspace_tab(&mut self, id: TabId, current_time_secs: u64) -> TabId {
        let target_ws = self.get_tab_by_id(id).map(|t| t.workspace);
        self.close_tab(id);
        match target_ws {
            Some(ws_id) if ws_id != self.active_workspace_id => {
                if !self.inactive_tabs.iter().any(|t| t.workspace == ws_id) {
                    let blank_id = self.alloc_id();
                    self.inactive_tabs.push(TabState {
                        id: blank_id,
                        url: "shoji://newtab".to_string(),
                        title: "New Tab".to_string(),
                        favicon_uri: None,
                        is_loading: false,
                        can_go_back: false,
                        can_go_forward: false,
                        is_audio_playing: false,
                        notice: false,
                        is_muted: false,
                        status: TabStatus::Suspended,
                        last_active_timestamp_secs: current_time_secs,
                        group: None,
                        pinned: false,
                        workspace: ws_id,
                    });
                    if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == ws_id) {
                        ws.active_tab = Some(blank_id);
                    }
                }
            }
            _ => {
                if self.tabs.is_empty() {
                    self.create_tab("shoji://newtab", current_time_secs);
                }
            }
        }
        self.active_tab_id
            .unwrap_or_else(|| self.create_tab("shoji://newtab", current_time_secs))
    }

    pub fn switch_tab(&mut self, id: TabId, current_time_secs: u64) -> bool {
        let exists = self.tabs.iter().any(|t| t.id == id);
        if !exists {
            return false;
        }

        for tab in &mut self.tabs {
            if tab.id == id {
                tab.status = TabStatus::Active;
                tab.last_active_timestamp_secs = current_time_secs;
            } else if tab.status == TabStatus::Active {
                tab.status = TabStatus::Inactive;
            }
        }
        self.active_tab_id = Some(id);
        self.sync_active_workspace_tab();
        true
    }

    pub fn active_tab(&self) -> Option<&TabState> {
        self.active_tab_id
            .and_then(|id| self.tabs.iter().find(|t| t.id == id))
    }

    pub fn active_tab_id(&self) -> Option<TabId> {
        self.active_tab_id
    }

    pub fn tabs(&self) -> &[TabState] {
        &self.tabs
    }

    pub fn tabs_mut(&mut self) -> &mut [TabState] {
        &mut self.tabs
    }

    pub fn inactive_tabs(&self) -> &[TabState] {
        &self.inactive_tabs
    }

    pub fn all_tabs(&self) -> impl Iterator<Item = &TabState> {
        self.tabs.iter().chain(self.inactive_tabs.iter())
    }

    /// Tab `id`, in any workspace.
    pub fn get_tab_by_id_mut(&mut self, id: TabId) -> Option<&mut TabState> {
        self.find_tab_mut(id)
    }

    fn find_tab_mut(&mut self, id: TabId) -> Option<&mut TabState> {
        self.tabs
            .iter_mut()
            .find(|t| t.id == id)
            .or_else(|| self.inactive_tabs.iter_mut().find(|t| t.id == id))
    }

    pub fn update_title(&mut self, id: TabId, title: String) {
        if let Some(tab) = self.find_tab_mut(id) {
            tab.title = title;
        }
    }

    pub fn update_url(&mut self, id: TabId, url: String) {
        if let Some(tab) = self.find_tab_mut(id) {
            tab.url = url;
        }
    }

    pub fn update_favicon(&mut self, id: TabId, favicon_uri: Option<String>) {
        if let Some(tab) = self.find_tab_mut(id) {
            tab.favicon_uri = favicon_uri;
        }
    }

    pub fn update_history_state(&mut self, id: TabId, can_go_back: bool, can_go_forward: bool) {
        if let Some(tab) = self.find_tab_mut(id) {
            tab.can_go_back = can_go_back;
            tab.can_go_forward = can_go_forward;
        }
    }

    pub fn set_tab_suspended(&mut self, id: TabId, suspended: bool) {
        if let Some(tab) = self.find_tab_mut(id) {
            tab.status = if suspended {
                TabStatus::Suspended
            } else {
                TabStatus::Inactive
            };
        }
    }

    pub fn set_audio_playing(&mut self, id: TabId, playing: bool) {
        if let Some(tab) = self.find_tab_mut(id) {
            tab.is_audio_playing = playing;
        }
    }

    pub fn set_tab_muted(&mut self, id: TabId, muted: bool) {
        if let Some(tab) = self.find_tab_mut(id) {
            tab.is_muted = muted;
        }
    }

    pub fn pop_last_closed(&mut self) -> Option<String> {
        self.closed_history.pop().map(|c| c.url)
    }

    /// Recently closed tabs, newest first.
    pub fn recently_closed(&self) -> Vec<ClosedTab> {
        self.closed_history.iter().rev().cloned().collect()
    }

    /* ----- workspaces ----- */

    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }

    pub fn active_workspace_id(&self) -> u32 {
        self.active_workspace_id
    }

    pub fn active_workspace(&self) -> Option<&Workspace> {
        self.workspaces
            .iter()
            .find(|w| w.id == self.active_workspace_id)
    }

    pub fn tabs_for_workspace(&self, workspace_id: u32) -> Vec<&TabState> {
        if workspace_id == self.active_workspace_id {
            self.tabs.iter().collect()
        } else {
            self.inactive_tabs
                .iter()
                .filter(|t| t.workspace == workspace_id)
                .collect()
        }
    }

    /// Create a new workspace and return its id. Names are trimmed and capped at 40 characters.
    pub fn create_workspace(&mut self, name: &str, color: &str) -> u32 {
        self.ensure_workspace();
        let max_existing = self.workspaces.iter().map(|w| w.id).max().unwrap_or(1);
        self.next_workspace_id = self.next_workspace_id.max(max_existing + 1);
        let id = self.next_workspace_id;
        self.next_workspace_id += 1;
        let name: String = name.trim().chars().take(40).collect();
        let color = color.trim();
        self.workspaces.push(Workspace {
            id,
            name: if name.is_empty() {
                "Workspace".to_string()
            } else {
                name
            },
            color: if color.is_empty() {
                "blue".to_string()
            } else {
                color.to_string()
            },
            active_tab: None,
            splits: Vec::new(),
            split: None,
        });
        id
    }

    pub fn rename_workspace(&mut self, id: u32, name: &str) -> bool {
        let name: String = name.trim().chars().take(40).collect();
        if name.is_empty() {
            return false;
        }
        let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == id) else {
            return false;
        };
        ws.name = name;
        true
    }

    pub fn set_workspace_color(&mut self, id: u32, color: &str) -> bool {
        let color = color.trim();
        if color.is_empty() {
            return false;
        }
        let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == id) else {
            return false;
        };
        ws.color = color.to_string();
        true
    }

    pub fn reorder_workspace(&mut self, from_idx: usize, to_idx: usize) -> bool {
        if from_idx < self.workspaces.len() && to_idx < self.workspaces.len() && from_idx != to_idx
        {
            let ws = self.workspaces.remove(from_idx);
            self.workspaces.insert(to_idx, ws);
            true
        } else {
            false
        }
    }

    /// Switch to workspace `id`, suspending all tabs of the previous workspace and restoring the
    /// target workspace's tabs, active tab, and split state. If the target workspace has no tabs,
    /// a blank `"shoji://newtab"` tab is created.
    pub fn switch_workspace(&mut self, id: u32, current_time_secs: u64) -> bool {
        self.ensure_workspace();
        if !self.workspaces.iter().any(|w| w.id == id) {
            return false;
        }
        if id == self.active_workspace_id {
            if self.tabs.is_empty() {
                self.create_tab("shoji://newtab", current_time_secs);
            }
            return true;
        }

        // Save outgoing workspace's active tab and suspend its tabs into `inactive_tabs`.
        self.sync_active_workspace_tab();
        let outgoing_ws = self.active_workspace_id;
        for mut tab in self.tabs.drain(..) {
            tab.workspace = outgoing_ws;
            if tab.status != TabStatus::Crashed {
                tab.status = TabStatus::Suspended;
            }
            self.inactive_tabs.push(tab);
        }

        // Activate incoming workspace and pull its tabs into `self.tabs`.
        self.active_workspace_id = id;
        let mut incoming = Vec::new();
        let mut remaining = Vec::with_capacity(self.inactive_tabs.len());
        for tab in self.inactive_tabs.drain(..) {
            if tab.workspace == id {
                incoming.push(tab);
            } else {
                remaining.push(tab);
            }
        }
        self.inactive_tabs = remaining;
        Self::pinned_first(&mut incoming);
        self.tabs = incoming;

        if self.tabs.is_empty() {
            self.active_tab_id = None;
            self.create_tab("shoji://newtab", current_time_secs);
            return true;
        }

        // Validate split panes and determine restored active tab.
        let saved_active = self
            .workspaces
            .iter()
            .find(|w| w.id == id)
            .and_then(|w| w.active_tab);
        let restored_id = saved_active
            .filter(|&aid| self.tabs.iter().any(|t| t.id == aid))
            .unwrap_or(self.tabs[0].id);

        if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == id) {
            let tabs = &self.tabs;
            ws.splits.retain(|split| {
                split.panes.len() >= 2
                    && split
                        .panes
                        .iter()
                        .enumerate()
                        .all(|(i, p)| !split.panes[..i].contains(p))
                    && split.panes.iter().all(|&p| tabs.iter().any(|t| t.id == p))
            });
            ws.active_tab = Some(restored_id);
        }

        let split_panes = self
            .workspaces
            .iter()
            .find(|w| w.id == id)
            .and_then(|w| w.splits.iter().find(|s| s.panes.contains(&restored_id)))
            .map(|s| s.panes.clone());

        for tab in &mut self.tabs {
            if tab.id == restored_id {
                tab.status = TabStatus::Active;
                tab.last_active_timestamp_secs = current_time_secs;
            } else if split_panes.as_ref().is_some_and(|p| p.contains(&tab.id)) {
                if tab.status != TabStatus::Crashed {
                    tab.status = TabStatus::Inactive;
                }
            } else if tab.status == TabStatus::Active {
                tab.status = TabStatus::Inactive;
            }
        }
        self.active_tab_id = Some(restored_id);
        true
    }

    /// Delete workspace `id` and close its tabs. Refuses to delete the last remaining workspace.
    pub fn delete_workspace(&mut self, id: u32, current_time_secs: u64) -> bool {
        self.ensure_workspace();
        if self.workspaces.len() <= 1 {
            return false;
        }
        let Some(pos) = self.workspaces.iter().position(|w| w.id == id) else {
            return false;
        };

        if self.active_workspace_id == id {
            let next_pos = if pos + 1 < self.workspaces.len() {
                pos + 1
            } else {
                pos - 1
            };
            let next_ws_id = self.workspaces[next_pos].id;
            let removed_tabs: Vec<TabState> = std::mem::take(&mut self.tabs);
            for t in removed_tabs {
                self.record_closed(t.url, t.title);
            }
            self.workspaces.remove(pos);
            self.active_workspace_id = next_ws_id;
            let mut incoming = Vec::new();
            let mut remaining = Vec::new();
            for tab in self.inactive_tabs.drain(..) {
                if tab.workspace == next_ws_id {
                    incoming.push(tab);
                } else {
                    remaining.push(tab);
                }
            }
            self.inactive_tabs = remaining;
            Self::pinned_first(&mut incoming);
            self.tabs = incoming;
            self.active_tab_id = None;
            if self.tabs.is_empty() {
                self.create_tab("shoji://newtab", current_time_secs);
            } else {
                let saved_active = self
                    .workspaces
                    .iter()
                    .find(|w| w.id == next_ws_id)
                    .and_then(|w| w.active_tab);
                let restored_id = saved_active
                    .filter(|&aid| self.tabs.iter().any(|t| t.id == aid))
                    .unwrap_or(self.tabs[0].id);
                self.switch_tab(restored_id, current_time_secs);
                if let Some(panes) = self.split_panes() {
                    for tab in &mut self.tabs {
                        if panes.contains(&tab.id)
                            && tab.id != restored_id
                            && tab.status != TabStatus::Crashed
                        {
                            tab.status = TabStatus::Inactive;
                        }
                    }
                }
            }
        } else {
            self.workspaces.remove(pos);
            let mut remaining = Vec::with_capacity(self.inactive_tabs.len());
            let mut closed = Vec::new();
            for tab in self.inactive_tabs.drain(..) {
                if tab.workspace == id {
                    closed.push((tab.url, tab.title));
                } else {
                    remaining.push(tab);
                }
            }
            self.inactive_tabs = remaining;
            for (url, title) in closed {
                self.record_closed(url, title);
            }
        }
        self.prune_groups();
        true
    }

    /// Move `tab_id` into `target_workspace_id`. Leaves the source workspace with a blank tab if
    /// `tab_id` was the last tab of the active workspace.
    pub fn move_tab_to_workspace(
        &mut self,
        tab_id: TabId,
        target_workspace_id: u32,
        current_time_secs: u64,
    ) -> bool {
        self.ensure_workspace();
        if !self.workspaces.iter().any(|w| w.id == target_workspace_id) {
            return false;
        }

        if let Some(pos) = self.tabs.iter().position(|t| t.id == tab_id) {
            if target_workspace_id == self.active_workspace_id {
                return true;
            }
            let mut moved = self.tabs.remove(pos);
            moved.workspace = target_workspace_id;
            if moved.status != TabStatus::Crashed {
                moved.status = TabStatus::Suspended;
            }

            // Dissolve the split group `tab_id` was part of; other groups survive.
            let surviving_pane = self.leave_splits(tab_id);

            // Update target workspace's active_tab if it doesn't have one yet.
            if let Some(target_ws) = self
                .workspaces
                .iter_mut()
                .find(|w| w.id == target_workspace_id)
            {
                if target_ws.active_tab.is_none() {
                    target_ws.active_tab = Some(tab_id);
                }
            }
            self.inactive_tabs.push(moved);

            // Pick next active tab in active workspace, or create a blank tab if now empty.
            if self.tabs.is_empty() {
                self.active_tab_id = None;
                self.create_tab("shoji://newtab", current_time_secs);
            } else if self.active_tab_id == Some(tab_id) {
                let next_id = surviving_pane
                    .filter(|&p| self.tabs.iter().any(|t| t.id == p))
                    .unwrap_or_else(|| {
                        let next_pos = pos.min(self.tabs.len() - 1);
                        self.tabs[next_pos].id
                    });
                if let Some(t) = self.tabs.iter_mut().find(|t| t.id == next_id) {
                    t.status = TabStatus::Active;
                    t.last_active_timestamp_secs = current_time_secs;
                }
                self.active_tab_id = Some(next_id);
                self.sync_active_workspace_tab();
            }
            self.prune_groups();
            return true;
        }

        if let Some(pos) = self.inactive_tabs.iter().position(|t| t.id == tab_id) {
            let src_ws_id = self.inactive_tabs[pos].workspace;
            if src_ws_id == target_workspace_id {
                return true;
            }
            let mut moved = self.inactive_tabs.remove(pos);
            moved.workspace = target_workspace_id;

            // Update source inactive workspace split & active_tab.
            let surviving_pane = self.leave_splits(tab_id);
            if let Some(src_ws) = self.workspaces.iter_mut().find(|w| w.id == src_ws_id) {
                if src_ws.active_tab == Some(tab_id) {
                    src_ws.active_tab = surviving_pane.or_else(|| {
                        self.inactive_tabs
                            .iter()
                            .find(|t| t.workspace == src_ws_id)
                            .map(|t| t.id)
                    });
                }
            }

            if target_workspace_id == self.active_workspace_id {
                if moved.status != TabStatus::Crashed {
                    moved.status = if self.active_tab_id.is_none() {
                        TabStatus::Active
                    } else {
                        TabStatus::Inactive
                    };
                }
                self.tabs.push(moved);
                if self.active_tab_id.is_none() {
                    self.active_tab_id = Some(tab_id);
                    self.sync_active_workspace_tab();
                }
            } else {
                if moved.status != TabStatus::Crashed {
                    moved.status = TabStatus::Suspended;
                }
                if let Some(target_ws) = self
                    .workspaces
                    .iter_mut()
                    .find(|w| w.id == target_workspace_id)
                {
                    if target_ws.active_tab.is_none() {
                        target_ws.active_tab = Some(tab_id);
                    }
                }
                self.inactive_tabs.push(moved);
            }
            self.prune_groups();
            return true;
        }

        false
    }

    /* ----- split view ----- */

    /// Take `id` out of whatever split group holds it. A group left with fewer than two tabs
    /// is dissolved. Returns the neighbour that should take over as the active tab.
    fn leave_splits(&mut self, id: TabId) -> Option<TabId> {
        let mut surviving = None;
        for ws in &mut self.workspaces {
            ws.splits.retain_mut(|s| {
                let Some(at) = s.panes.iter().position(|&p| p == id) else {
                    return true;
                };
                if surviving.is_none() {
                    surviving = s
                        .panes
                        .get(at + 1)
                        .or_else(|| at.checked_sub(1).and_then(|i| s.panes.get(i)))
                        .copied();
                }
                s.panes.remove(at);
                if s.panes.len() < 2 {
                    return false;
                }
                s.weights.clear();
                s.fix_weights();
                true
            });
        }
        surviving
    }

    fn visible_split_mut(&mut self) -> Option<&mut SplitState> {
        let active = self.active_tab_id?;
        let ws_id = self.active_workspace_id;
        self.workspaces
            .iter_mut()
            .find(|w| w.id == ws_id)?
            .splits
            .iter_mut()
            .find(|s| s.panes.contains(&active))
    }

    fn split_mut(&mut self, id: u32) -> Option<&mut SplitState> {
        self.workspaces
            .iter_mut()
            .flat_map(|w| w.splits.iter_mut())
            .find(|s| s.id == id)
    }

    /// All split groups of the active workspace.
    pub fn splits(&self) -> &[SplitState] {
        self.active_workspace()
            .map(|w| w.splits.as_slice())
            .unwrap_or(&[])
    }

    /// The split group `tab` belongs to in the active workspace, if any.
    pub fn split_of(&self, tab: TabId) -> Option<&SplitState> {
        self.splits().iter().find(|s| s.panes.contains(&tab))
    }

    /// The split group currently visible in the viewport (the group of the active tab).
    pub fn visible_split(&self) -> Option<&SplitState> {
        self.split_of(self.active_tab_id?)
    }

    /// The pane ids, in screen order, when split view is currently visible.
    pub fn split_panes(&self) -> Option<Vec<TabId>> {
        self.visible_split().map(|s| s.panes.clone())
    }

    pub fn is_in_split(&self, id: TabId) -> bool {
        self.split_of(id).is_some()
    }

    pub fn is_tab_visible(&self, id: TabId) -> bool {
        self.active_tab_id == Some(id)
            || self.visible_split().is_some_and(|s| s.panes.contains(&id))
    }

    /// The pane after `id` in its group (wrapping round), if it is in one.
    pub fn other_split_pane(&self, id: TabId) -> Option<TabId> {
        let split = self.split_of(id)?;
        let at = split.panes.iter().position(|&p| p == id)?;
        split.panes.get((at + 1) % split.panes.len()).copied()
    }

    /// Keep a group's tabs together in the strip, in order, starting where the first one is. All
    /// must be in the same workspace and either all pinned or none. Returns false if any is
    /// missing or they differ.
    pub fn gather_panes(&mut self, panes: &[TabId]) -> bool {
        let Some((&first, rest)) = panes.split_first() else {
            return false;
        };
        if rest.contains(&first) {
            return false;
        }
        for list in [&mut self.tabs, &mut self.inactive_tabs] {
            let Some(p1) = list.iter().position(|t| t.id == first) else {
                continue;
            };
            let key = (list[p1].workspace, list[p1].pinned);
            let same = rest.iter().all(|id| {
                list.iter()
                    .find(|t| t.id == *id)
                    .is_some_and(|t| (t.workspace, t.pinned) == key)
            });
            if !same {
                return false;
            }
            let mut taken = Vec::new();
            for id in rest {
                if let Some(p) = list.iter().position(|t| t.id == *id) {
                    taken.push(list.remove(p));
                }
            }
            let at = list.iter().position(|t| t.id == first).unwrap_or(0);
            for (k, tab) in taken.into_iter().enumerate() {
                list.insert((at + 1 + k).min(list.len()), tab);
            }
            return true;
        }
        false
    }

    /// Keep two tabs adjacent in the strip: `second` moves to just after `first`.
    pub fn gather_split(&mut self, first: TabId, second: TabId) -> bool {
        first != second && self.gather_panes(&[first, second])
    }

    /// Create a split group of `first` and `second` named "Split N" in the default colour.
    pub fn open_split(&mut self, first: TabId, second: TabId) -> Option<u32> {
        self.open_split_with(first, second, "", "blue")
    }

    /// Create a new split group. Any group either tab already belongs to is dissolved first.
    /// Refuses (None) when the tabs are equal, missing, in different workspaces or pinned. An
    /// empty `name` becomes "Split {id}". The two panes are gathered next to each other.
    pub fn open_split_with(
        &mut self,
        first: TabId,
        second: TabId,
        name: &str,
        color: &str,
    ) -> Option<u32> {
        self.ensure_workspace();
        if first == second {
            return None;
        }
        let (ws_id, pinned_a) = self.get_tab_by_id(first).map(|t| (t.workspace, t.pinned))?;
        let (ws_b, pinned_b) = self
            .get_tab_by_id(second)
            .map(|t| (t.workspace, t.pinned))?;
        if ws_id != ws_b || pinned_a || pinned_b {
            return None;
        }
        self.leave_splits(first);
        self.leave_splits(second);
        for t in self.tabs.iter_mut().filter(|t| t.id == second) {
            if t.status == TabStatus::Suspended {
                t.status = TabStatus::Inactive;
            }
        }
        self.next_split_id = self.next_split_id.max(1);
        let id = self.next_split_id;
        self.next_split_id += 1;
        let name: String = name.trim().chars().take(40).collect();
        let color = color.trim();
        let mut state = SplitState::new(first, second);
        state.id = id;
        state.name = if name.is_empty() {
            format!("Split {id}")
        } else {
            name
        };
        if !color.is_empty() {
            state.color = color.to_string();
        }
        self.workspaces
            .iter_mut()
            .find(|w| w.id == ws_id)?
            .splits
            .push(state);
        self.gather_split(first, second);
        Some(id)
    }

    /// Add `tab` to group `split`: at `index` (0 = first pane), or at the end. The tab leaves any
    /// group it was in. Every pane then gets an equal share. Refuses (false) for a pinned tab, a
    /// tab of another workspace, a full group or an unknown group.
    pub fn add_to_split(&mut self, split: u32, tab: TabId, index: Option<usize>) -> bool {
        let Some(ws_id) = self
            .get_tab_by_id(tab)
            .filter(|t| !t.pinned)
            .map(|t| t.workspace)
        else {
            return false;
        };
        let Some(group) = self
            .workspaces
            .iter()
            .find(|w| w.id == ws_id)
            .and_then(|w| w.splits.iter().find(|s| s.id == split))
        else {
            return false;
        };
        if group.panes.contains(&tab) {
            return false;
        }
        if group.panes.len() >= MAX_SPLIT_PANES {
            return false;
        }
        self.leave_splits(tab);
        for t in self.tabs.iter_mut().filter(|t| t.id == tab) {
            if t.status == TabStatus::Suspended {
                t.status = TabStatus::Inactive;
            }
        }
        let Some(group) = self
            .workspaces
            .iter_mut()
            .find(|w| w.id == ws_id)
            .and_then(|w| w.splits.iter_mut().find(|s| s.id == split))
        else {
            return false;
        };
        let at = index.unwrap_or(group.panes.len()).min(group.panes.len());
        group.panes.insert(at, tab);
        group.equalize();
        let panes = group.panes.clone();
        self.gather_panes(&panes);
        true
    }

    /// Take `tab` out of its group (the group dissolves if one tab would be left).
    pub fn remove_from_split(&mut self, tab: TabId) -> bool {
        let was = self.split_of(tab).is_some();
        self.leave_splits(tab);
        was
    }

    /// Dissolve split group `id`. The tabs stay open.
    pub fn close_split(&mut self, id: u32) -> bool {
        let mut removed = false;
        for ws in &mut self.workspaces {
            let before = ws.splits.len();
            ws.splits.retain(|s| s.id != id);
            removed |= ws.splits.len() != before;
        }
        removed
    }

    /// Dissolve the group of the active tab, keeping the focused pane as the active tab.
    pub fn close_visible_split(&mut self) -> bool {
        match self.visible_split().map(|s| s.id) {
            Some(id) => self.close_split(id),
            None => false,
        }
    }

    pub fn rename_split(&mut self, id: u32, name: &str) -> bool {
        let name: String = name.trim().chars().take(40).collect();
        if name.is_empty() {
            return false;
        }
        match self.split_mut(id) {
            Some(s) => {
                s.name = name;
                true
            }
            None => false,
        }
    }

    pub fn set_split_color(&mut self, id: u32, color: &str) -> bool {
        let color = color.trim();
        if color.is_empty() {
            return false;
        }
        match self.split_mut(id) {
            Some(s) => {
                s.color = color.to_string();
                true
            }
            None => false,
        }
    }

    /// Swap which pane is first (left or top) in group `id`.
    pub fn swap_split_panes(&mut self, id: u32) -> bool {
        let panes = match self.split_mut(id) {
            Some(s) => {
                s.panes.reverse();
                s.weights.reverse();
                s.panes.clone()
            }
            None => return false,
        };
        // The strip shows the panes in the order they sit on screen.
        self.gather_panes(&panes);
        true
    }

    /// Swap pane `index` with the one after it.
    pub fn swap_split_pair(&mut self, id: u32, index: usize) -> bool {
        let panes = match self.split_mut(id) {
            Some(s) if index + 1 < s.panes.len() => {
                s.panes.swap(index, index + 1);
                s.weights.swap(index, index + 1);
                s.panes.clone()
            }
            _ => return false,
        };
        self.gather_panes(&panes);
        true
    }

    /// Toggle split view via keyboard shortcut: dissolves the active tab's group and returns
    /// true. When the active tab is in no group nothing happens (false); groups are only ever
    /// created explicitly.
    pub fn toggle_split(&mut self) -> bool {
        self.close_visible_split()
    }

    /// Move keyboard/omnibox focus to the other pane of the visible split.
    pub fn switch_split_pane(&mut self, current_time_secs: u64) -> Option<TabId> {
        let active = self.active_tab_id?;
        let other = self.other_split_pane(active)?;
        if self.switch_tab(other, current_time_secs) {
            Some(other)
        } else {
            None
        }
    }

    /// Set the first pane's share of a two-pane split.
    pub fn set_split_ratio(&mut self, ratio: f64) -> bool {
        let clamped = SplitState::clamp_ratio(ratio, None, 0.0);
        if let Some(split) = self.visible_split_mut() {
            if split.panes.len() == 2 {
                split.weights = vec![clamped, 1.0 - clamped];
                return true;
            }
        }
        false
    }

    /// Give every pane of the visible split an equal share.
    pub fn reset_split_ratio(&mut self) -> bool {
        match self.visible_split_mut() {
            Some(split) => {
                split.equalize();
                true
            }
            None => false,
        }
    }

    /// Drag divider `index` of the visible split to `cursor` (see [`SplitState::drag_divider`]).
    pub fn drag_split_divider(
        &mut self,
        index: usize,
        bounds: (f64, f64, f64, f64),
        gap_px: f64,
        cursor: f64,
    ) -> bool {
        self.visible_split_mut()
            .is_some_and(|s| s.drag_divider(index, bounds, gap_px, cursor))
    }

    pub fn set_split_orientation(&mut self, orientation: SplitOrientation) -> bool {
        if let Some(split) = self.visible_split_mut() {
            split.orientation = orientation;
            return true;
        }
        false
    }

    pub fn toggle_split_orientation(&mut self) -> Option<SplitOrientation> {
        let split = self.visible_split_mut()?;
        split.orientation = match split.orientation {
            SplitOrientation::Vertical => SplitOrientation::Horizontal,
            SplitOrientation::Horizontal => SplitOrientation::Vertical,
        };
        Some(split.orientation)
    }

    /* ----- tab groups ----- */

    pub fn groups(&self) -> &[TabGroup] {
        &self.groups
    }

    /// Create a group and return its id. Names are trimmed and capped at 40 characters.
    pub fn create_group(&mut self, name: &str, color: &str) -> u32 {
        self.next_group_id = self.next_group_id.max(1);
        let id = self.next_group_id;
        self.next_group_id += 1;
        let name: String = name.trim().chars().take(40).collect();
        self.groups.push(TabGroup {
            id,
            name: if name.is_empty() {
                "Group".to_string()
            } else {
                name
            },
            color: color.to_string(),
        });
        id
    }

    /// Put a tab in a group (or take it out with `None`). Unknown groups are ignored.
    pub fn set_tab_group(&mut self, tab: TabId, group: Option<u32>) -> bool {
        if group.is_some_and(|g| !self.groups.iter().any(|x| x.id == g)) {
            return false;
        }
        let Some(t) = self.find_tab_mut(tab) else {
            return false;
        };
        t.group = group;
        self.prune_groups();
        true
    }

    pub fn rename_group(&mut self, id: u32, name: &str) {
        let name: String = name.trim().chars().take(40).collect();
        if name.is_empty() {
            return;
        }
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == id) {
            g.name = name;
        }
    }

    pub fn set_group_color(&mut self, id: u32, color: &str) {
        if let Some(g) = self.groups.iter_mut().find(|g| g.id == id) {
            g.color = color.to_string();
        }
    }

    /// Replace the group list (session restore); tabs keep their `group` ids.
    pub fn restore_groups(&mut self, groups: Vec<TabGroup>) {
        self.next_group_id = groups.iter().map(|g| g.id).max().unwrap_or(0) + 1;
        // Not pruned here: the tabs that belong to these groups are assigned afterwards, then
        // `prune_groups` drops any group that ended up empty.
        self.groups = groups;
    }

    pub fn prune_groups(&mut self) {
        let tabs = &self.tabs;
        let inactive = &self.inactive_tabs;
        self.groups.retain(|g| {
            tabs.iter().any(|t| t.group == Some(g.id))
                || inactive.iter().any(|t| t.group == Some(g.id))
        });
    }

    /// Keep the pinned block at the front of a workspace's tabs (order within each part kept).
    fn pinned_first(tabs: &mut [TabState]) {
        tabs.sort_by_key(|t| !t.pinned);
    }

    /// How many tabs are pinned. Pinned tabs are always the first ones.
    pub fn pinned_count(&self) -> usize {
        self.tabs.iter().take_while(|t| t.pinned).count()
    }

    /// Pin or unpin a tab. A pinned tab moves to the end of the pinned block, an unpinned one
    /// to just after it, so pinned tabs stay together at the left.
    pub fn set_pinned(&mut self, id: TabId, pinned: bool) -> bool {
        let Some(pos) = self.tabs.iter().position(|t| t.id == id) else {
            return false;
        };
        if self.tabs[pos].pinned == pinned {
            return false;
        }
        if pinned && self.is_in_split(id) {
            return false;
        }
        let mut tab = self.tabs.remove(pos);
        tab.pinned = pinned;
        if pinned {
            tab.group = None;
        }
        let block = self.pinned_count();
        self.tabs.insert(block, tab);
        self.prune_groups();
        true
    }

    /// Move a tab. A pinned tab stays inside the pinned block and an unpinned one outside it.
    pub fn reorder_tab(&mut self, from_idx: usize, to_idx: usize) -> bool {
        if from_idx < self.tabs.len() && to_idx < self.tabs.len() && from_idx != to_idx {
            let block = self.pinned_count();
            if self.tabs[from_idx].pinned != (to_idx < block) {
                return false;
            }
            let tab = self.tabs.remove(from_idx);
            self.tabs.insert(to_idx, tab);
            true
        } else {
            false
        }
    }

    /// Move `count` neighbouring tabs starting at `from_idx` so the block starts at `to_idx` (an
    /// index in the list once the block is out). Used to move a whole split group in the strip.
    pub fn reorder_block(&mut self, from_idx: usize, count: usize, to_idx: usize) -> bool {
        let n = self.tabs.len();
        if count == 0 || from_idx + count > n || to_idx + count > n || from_idx == to_idx {
            return false;
        }
        let block = self.pinned_count();
        let pinned = self.tabs[from_idx].pinned;
        if self.tabs[from_idx..from_idx + count]
            .iter()
            .any(|t| t.pinned != pinned)
            || pinned != (to_idx < block)
        {
            return false;
        }
        let moved: Vec<TabState> = self.tabs.drain(from_idx..from_idx + count).collect();
        for (k, t) in moved.into_iter().enumerate() {
            self.tabs.insert(to_idx + k, t);
        }
        true
    }

    pub fn get_tab_by_id(&self, id: TabId) -> Option<&TabState> {
        self.tabs
            .iter()
            .find(|t| t.id == id)
            .or_else(|| self.inactive_tabs.iter().find(|t| t.id == id))
    }

    /// Extract a tab from this manager (for moving to another window).
    pub fn extract_tab(&mut self, id: TabId) -> Option<TabState> {
        self.ensure_workspace();
        if let Some(pos) = self.tabs.iter().position(|t| t.id == id) {
            let tab = self.tabs.remove(pos);
            let surviving_pane = self.leave_splits(id);
            if self.active_tab_id == Some(id) {
                if self.tabs.is_empty() {
                    self.active_tab_id = None;
                } else if let Some(other) =
                    surviving_pane.filter(|&p| self.tabs.iter().any(|t| t.id == p))
                {
                    if let Some(t) = self.tabs.iter_mut().find(|t| t.id == other) {
                        t.status = TabStatus::Active;
                    }
                    self.active_tab_id = Some(other);
                } else {
                    let next_pos = if pos >= self.tabs.len() {
                        self.tabs.len() - 1
                    } else {
                        pos
                    };
                    let next_id = self.tabs[next_pos].id;
                    self.tabs[next_pos].status = TabStatus::Active;
                    self.active_tab_id = Some(next_id);
                }
            }
            self.sync_active_workspace_tab();
            self.prune_groups();
            Some(tab)
        } else {
            None
        }
    }

    /// Insert an existing tab state into this manager (e.g. from another window).
    pub fn insert_tab(&mut self, mut tab: TabState, at_index: Option<usize>) -> TabId {
        self.ensure_workspace();
        let id = if tab.id.0 > 0 {
            self.note_id(tab.id.0);
            tab.id
        } else {
            let new_id = self.alloc_id();
            tab.id = new_id;
            new_id
        };
        tab.workspace = self.active_workspace_id;
        tab.status = TabStatus::Active;

        if let Some(prev_active) = self.active_tab_id {
            if let Some(prev) = self.tabs.iter_mut().find(|t| t.id == prev_active) {
                prev.status = TabStatus::Inactive;
            }
        }

        let block = self.pinned_count();
        let mut idx = at_index.unwrap_or(self.tabs.len()).min(self.tabs.len());
        idx = if tab.pinned {
            idx.min(block)
        } else {
            idx.max(block)
        };
        self.tabs.insert(idx, tab);
        self.active_tab_id = Some(id);
        self.sync_active_workspace_tab();
        id
    }

    pub fn mark_tab_crashed(&mut self, id: TabId) {
        if let Some(tab) = self.find_tab_mut(id) {
            tab.status = TabStatus::Crashed;
        }
    }

    pub fn snapshot(&self) -> Vec<TabState> {
        self.tabs.clone()
    }

    pub fn restore_from_snapshot(&mut self, tabs: Vec<TabState>) {
        self.ensure_workspace();
        let active_ws = self.active_workspace_id;
        self.tabs = tabs
            .into_iter()
            .map(|mut t| {
                if t.workspace == 0 {
                    t.workspace = active_ws;
                }
                t
            })
            .collect();
        if let Some(active) = self.tabs.iter().find(|t| t.status == TabStatus::Active) {
            self.active_tab_id = Some(active.id);
        } else if let Some(first) = self.tabs.first() {
            self.active_tab_id = Some(first.id);
        }
        if let Some(max_id) = self
            .tabs
            .iter()
            .chain(self.inactive_tabs.iter())
            .map(|t| t.id.0)
            .max()
        {
            self.note_id(max_id);
        }
        self.sync_active_workspace_tab();
    }

    /// Serialize this window's workspaces, tabs, active tabs, groups, and split state into a
    /// [`crate::userdata::SessionWindow`]. Returns `None` if the window only has the default
    /// workspace with no web pages.
    pub fn to_session_window(&self) -> Option<crate::userdata::SessionWindow> {
        use crate::userdata::{SessionSplit, SessionTab, SessionWindow, SessionWorkspace};

        let mut tabs = Vec::new();
        let mut workspaces = Vec::new();
        let mut window_active = 0;

        for ws in &self.workspaces {
            let ws_tabs = self.tabs_for_workspace(ws.id);
            let ws_active_id = if ws.id == self.active_workspace_id {
                self.active_tab_id.or(ws.active_tab)
            } else {
                ws.active_tab
            };

            let mut kept_ids = Vec::new();
            let mut ws_active_idx = 0;

            for t in &ws_tabs {
                let is_web = t.url.starts_with("http://") || t.url.starts_with("https://");
                let in_split = ws.splits.iter().any(|s| s.panes.contains(&t.id));
                if !is_web && !in_split {
                    continue;
                }
                let local_idx = kept_ids.len();
                let global_idx = tabs.len();
                if Some(t.id) == ws_active_id {
                    ws_active_idx = local_idx;
                }
                if ws.id == self.active_workspace_id && Some(t.id) == self.active_tab_id {
                    window_active = global_idx;
                }
                kept_ids.push(t.id);
                tabs.push(SessionTab {
                    url: t.url.clone(),
                    title: t.title.clone(),
                    group: t.group,
                    pinned: t.pinned,
                    workspace: ws.id,
                });
            }

            let splits: Vec<SessionSplit> = ws
                .splits
                .iter()
                .filter_map(|s| {
                    let panes: Vec<usize> = s
                        .panes
                        .iter()
                        .map(|p| kept_ids.iter().position(|id| id == p))
                        .collect::<Option<_>>()?;
                    (panes.len() >= 2).then(|| SessionSplit {
                        id: s.id,
                        name: s.name.clone(),
                        color: s.color.clone(),
                        panes,
                        orientation: s.orientation,
                        ratio: s.ratio(),
                        weights: s.weights.clone(),
                    })
                })
                .collect();

            workspaces.push(SessionWorkspace {
                id: ws.id,
                name: ws.name.clone(),
                color: ws.color.clone(),
                active: ws_active_idx,
                splits,
                split: None,
            });
        }

        let has_custom_workspaces = workspaces.len() > 1
            || workspaces
                .first()
                .is_some_and(|w| w.name != "Main" || w.color != "blue");

        if tabs.is_empty() && !has_custom_workspaces {
            return None;
        }

        Some(SessionWindow {
            tabs,
            active: window_active,
            groups: self.groups.clone(),
            workspaces,
            active_workspace: self.active_workspace_id,
        })
    }

    /// Restore workspaces, tabs, active tabs, groups, and split state from a saved
    /// [`crate::userdata::SessionWindow`], migrating pre-workspace session files into the default
    /// `"Main"` workspace.
    pub fn restore_session_window(
        &mut self,
        saved: &crate::userdata::SessionWindow,
        current_time_secs: u64,
    ) {
        self.tabs.clear();
        self.inactive_tabs.clear();
        self.workspaces.clear();
        self.active_tab_id = None;
        self.restore_groups(saved.groups.clone());

        // Legacy session file without workspaces: migrate all tabs into the default "Main" workspace.
        if saved.workspaces.is_empty() {
            self.workspaces.push(Workspace::default_main());
            self.active_workspace_id = 1;
            self.next_workspace_id = 2;

            for st in &saved.tabs {
                let id = self.create_tab(&st.url, current_time_secs);
                if !st.title.is_empty() {
                    self.update_title(id, st.title.clone());
                }
                if st.group.is_some() {
                    self.set_tab_group(id, st.group);
                }
            }
            if self.tabs.is_empty() {
                self.create_tab("shoji://newtab", current_time_secs);
            } else {
                let target_idx = saved.active.min(self.tabs.len() - 1);
                let target_id = self.tabs[target_idx].id;
                self.switch_tab(target_id, current_time_secs);
            }
            self.prune_groups();
            return;
        }

        let active_ws_id = if saved
            .workspaces
            .iter()
            .any(|w| w.id == saved.active_workspace)
        {
            saved.active_workspace
        } else {
            saved.workspaces[0].id
        };
        self.active_workspace_id = active_ws_id;
        self.next_workspace_id = saved.workspaces.iter().map(|w| w.id).max().unwrap_or(1) + 1;

        for (ws_idx, sw) in saved.workspaces.iter().enumerate() {
            let mut ws_tabs = Vec::new();
            let mut ws_ids = Vec::new();

            for st in &saved.tabs {
                let belongs = st.workspace == sw.id || (ws_idx == 0 && st.workspace == 0);
                if !belongs {
                    continue;
                }
                let id = self.alloc_id();
                ws_ids.push(id);
                ws_tabs.push(TabState {
                    id,
                    url: st.url.clone(),
                    title: if st.title.is_empty() {
                        "New Tab".to_string()
                    } else {
                        st.title.clone()
                    },
                    favicon_uri: None,
                    is_loading: false,
                    can_go_back: false,
                    can_go_forward: false,
                    is_audio_playing: false,
                    notice: false,
                    is_muted: false,
                    status: TabStatus::Suspended,
                    last_active_timestamp_secs: current_time_secs,
                    group: st.group,
                    pinned: st.pinned,
                    workspace: sw.id,
                });
            }

            if ws_tabs.is_empty() {
                let id = self.alloc_id();
                ws_ids.push(id);
                ws_tabs.push(TabState {
                    id,
                    url: "shoji://newtab".to_string(),
                    title: "New Tab".to_string(),
                    favicon_uri: None,
                    is_loading: false,
                    can_go_back: false,
                    can_go_forward: false,
                    is_audio_playing: false,
                    notice: false,
                    is_muted: false,
                    status: TabStatus::Suspended,
                    last_active_timestamp_secs: current_time_secs,
                    group: None,
                    pinned: false,
                    workspace: sw.id,
                });
            }

            let ws_active_id = ws_ids.get(sw.active).or_else(|| ws_ids.first()).copied();

            // Legacy single `split` is migrated as one more group (id 0 = assign below).
            let mut splits: Vec<SplitState> = Vec::new();
            for sp in sw.splits.iter().chain(sw.split.as_ref()) {
                let Some(panes) = sp
                    .panes
                    .iter()
                    .map(|&i| ws_ids.get(i).copied())
                    .collect::<Option<Vec<TabId>>>()
                else {
                    continue;
                };
                let pinned = |id: TabId| ws_tabs.iter().any(|t| t.id == id && t.pinned);
                if panes.len() < 2
                    || panes.len() > MAX_SPLIT_PANES
                    || panes
                        .iter()
                        .enumerate()
                        .any(|(i, p)| panes[..i].contains(p) || pinned(*p))
                    || splits
                        .iter()
                        .any(|x| x.panes.iter().any(|p| panes.contains(p)))
                {
                    continue;
                }
                let mut state = SplitState::with_panes(panes).with_legacy_ratio(sp.ratio);
                if sp.weights.len() == state.panes.len() {
                    state.weights = sp.weights.clone();
                    state.fix_weights();
                }
                state.id = sp.id;
                state.name = sp.name.clone();
                state.color = if sp.color.trim().is_empty() {
                    default_split_color()
                } else {
                    sp.color.clone()
                };
                state.orientation = sp.orientation;
                splits.push(state);
            }

            self.workspaces.push(Workspace {
                id: sw.id,
                name: if sw.name.trim().is_empty() {
                    "Workspace".to_string()
                } else {
                    sw.name.trim().chars().take(40).collect()
                },
                color: if sw.color.trim().is_empty() {
                    "blue".to_string()
                } else {
                    sw.color.trim().to_string()
                },
                active_tab: ws_active_id,
                splits,
                split: None,
            });

            if sw.id == active_ws_id {
                for tab in &mut ws_tabs {
                    if Some(tab.id) == ws_active_id {
                        tab.status = TabStatus::Active;
                    } else {
                        tab.status = TabStatus::Inactive;
                    }
                }
                Self::pinned_first(&mut ws_tabs);
                self.tabs = ws_tabs;
                self.active_tab_id = ws_active_id;
            } else {
                self.inactive_tabs.extend(ws_tabs);
            }
        }

        self.prune_groups();
        self.migrate_splits();
    }
}

/// Normalize an omnibox input string into a valid HTTP/HTTPS URL or search engine query.
pub fn normalize_url(input: &str, search_template: &str) -> Result<String, String> {
    let trimmed = input.trim();
    let lower = trimmed.to_ascii_lowercase();
    if trimmed.is_empty() || lower == "about:home" || lower == "about:newtab" {
        return Ok("shoji://newtab".to_string());
    }

    if lower == "about:blank" {
        return Ok("about:blank".to_string());
    }

    if lower == "about:settings" {
        return Ok("shoji://settings".to_string());
    }

    // Internal browser scheme
    if lower.starts_with(crate::INTERNAL_SCHEME) {
        return Ok(trimmed.to_string());
    }

    // Explicit valid scheme
    if lower.starts_with("https://") || lower.starts_with("http://") {
        return Ok(trimmed.to_string());
    }

    // A page of an installed extension (its popup or options page)
    if let Some(rest) = lower.strip_prefix("chrome-extension://") {
        let id = rest.split(['/', '?', '#']).next().unwrap_or("");
        if id.len() == 32 && id.bytes().all(|b| (b'a'..=b'p').contains(&b)) {
            return Ok(trimmed.to_string());
        }
    }

    // Page source of a web page
    if lower.starts_with("view-source:https://") || lower.starts_with("view-source:http://") {
        return Ok(trimmed.to_string());
    }

    // Disallowed schemes (security check - case-insensitive)
    if lower.starts_with("javascript:")
        || lower.starts_with("blob:")
        || lower.starts_with("mailto:")
        || lower.starts_with("ftp:")
        || lower.starts_with("view-source:")
        || lower.starts_with("file:")
        || lower.starts_with("data:")
        || lower.starts_with("vbscript:")
        || lower.starts_with("about:")
    {
        return Err(format!("Navigation to scheme prohibited: {}", trimmed));
    }

    // Machines on this computer or network ("localhost:3000", "192.168.1.1/admin", "[::1]:8080")
    // rarely have a certificate, so they are tried over plain HTTP.
    if !trimmed.contains(char::is_whitespace) {
        let authority = trimmed.split(['/', '?', '#']).next().unwrap_or("");
        let host = authority.rsplit('@').next().unwrap_or("");
        let host = match host.strip_prefix('[') {
            Some(v6) => v6.split(']').next().unwrap_or(""),
            None => host.split(':').next().unwrap_or(""),
        };
        let port_ok = authority.rsplit_once(':').is_none_or(|(_, p)| {
            p.is_empty() || p.bytes().all(|b| b.is_ascii_digit()) || p.ends_with(']')
        });
        let is_ip = host.contains(':')
            || (host.split('.').count() == 4
                && host
                    .split('.')
                    .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())));
        if port_ok && (host.eq_ignore_ascii_case("localhost") || is_ip) {
            return Ok(format!("http://{trimmed}"));
        }
    }

    // Hostname check: contains dot and no spaces
    if trimmed.contains('.') && !trimmed.contains(' ') {
        return Ok(format!("https://{}", trimmed));
    }

    // Fallback: search query
    let encoded_query = url_encode(trimmed);
    Ok(search_template.replace("%s", &encoded_query))
}

/// Hosts that are never upgraded to HTTPS: local machines and networks, which rarely have
/// certificates (localhost, single-label names, `.local`/`.test`/`.internal`, IP addresses).
pub fn is_local_host(host: &str) -> bool {
    let host = host
        .trim_matches(|c| c == '[' || c == ']')
        .to_ascii_lowercase();
    host.is_empty()
        || !host.contains('.')
        || host.contains(':')
        || host
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        || ["localhost", "local", "test", "internal", "lan", "home.arpa"]
            .iter()
            .any(|s| host == *s || host.ends_with(&format!(".{s}")))
}

/// The https:// version of a plain-HTTP web address, or `None` if it isn't HTTP or is a local host.
pub fn upgrade_to_https(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("HTTP://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    // A literal port means the site expects plain HTTP on that port.
    if host.contains(':') && !host.starts_with('[') {
        return None;
    }
    let hostname = host.split(':').next()?;
    (!is_local_host(hostname)).then(|| format!("https://{rest}"))
}

/// Simple percent encoder for search queries
pub fn url_encode(input: &str) -> String {
    let mut result = String::new();
    for byte in input.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push('+'),
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}
