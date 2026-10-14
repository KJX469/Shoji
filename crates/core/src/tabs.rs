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
    pub status: TabStatus,
    pub last_active_timestamp_secs: u64,
    /// Tab group this tab belongs to, if any
    #[serde(default)]
    pub group: Option<u32>,
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
pub const MIN_SPLIT_PANE_PX: f64 = 200.0;

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

/// Two tabs shown side by side inside a workspace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitState {
    pub panes: [TabId; 2],
    #[serde(default)]
    pub orientation: SplitOrientation,
    #[serde(default = "default_split_ratio")]
    pub ratio: f64,
}

impl SplitState {
    pub fn new(first: TabId, second: TabId) -> Self {
        Self {
            panes: [first, second],
            orientation: SplitOrientation::Vertical,
            ratio: DEFAULT_SPLIT_RATIO,
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

    /// Compute the `[first_pane, second_pane]` rectangles `(x, y, width, height)` within `bounds`.
    pub fn layout(&self, bounds: (f64, f64, f64, f64), gap_px: f64) -> [(f64, f64, f64, f64); 2] {
        let (x, y, w, h) = bounds;
        let gap = gap_px.max(0.0);
        match self.orientation {
            SplitOrientation::Vertical => {
                let avail = (w - gap).max(2.0);
                let r = Self::clamp_ratio(self.ratio, Some(w), gap);
                let w1 = (avail * r).round().max(1.0);
                let w2 = (avail - w1).max(1.0);
                [(x, y, w1, h), (x + w1 + gap, y, w2, h)]
            }
            SplitOrientation::Horizontal => {
                let avail = (h - gap).max(2.0);
                let r = Self::clamp_ratio(self.ratio, Some(h), gap);
                let h1 = (avail * r).round().max(1.0);
                let h2 = (avail - h1).max(1.0);
                [(x, y, w, h1), (x, y + h1 + gap, w, h2)]
            }
        }
    }

    /// Compute the draggable divider rectangle `(x, y, width, height)` within `bounds`.
    pub fn divider_rect(&self, bounds: (f64, f64, f64, f64), gap_px: f64) -> (f64, f64, f64, f64) {
        let [first, _] = self.layout(bounds, gap_px);
        let gap = gap_px.max(0.0);
        match self.orientation {
            SplitOrientation::Vertical => (first.0 + first.2, bounds.1, gap, bounds.3),
            SplitOrientation::Horizontal => (bounds.0, first.1 + first.3, bounds.2, gap),
        }
    }

    /// Convert a cursor position `(cursor_x, cursor_y)` inside `bounds` into a clamped split ratio.
    pub fn ratio_from_cursor(
        &self,
        bounds: (f64, f64, f64, f64),
        gap_px: f64,
        cursor_x: f64,
        cursor_y: f64,
    ) -> f64 {
        let (x, y, w, h) = bounds;
        let gap = gap_px.max(0.0);
        match self.orientation {
            SplitOrientation::Vertical => {
                let avail = (w - gap).max(1.0);
                let rel = (cursor_x - x - gap * 0.5) / avail;
                Self::clamp_ratio(rel, Some(w), gap)
            }
            SplitOrientation::Horizontal => {
                let avail = (h - gap).max(1.0);
                let rel = (cursor_y - y - gap * 0.5) / avail;
                Self::clamp_ratio(rel, Some(h), gap)
            }
        }
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
    #[serde(default)]
    pub split: Option<SplitState>,
}

impl Workspace {
    pub fn default_main() -> Self {
        Self {
            id: 1,
            name: "Main".to_string(),
            color: "blue".to_string(),
            active_tab: None,
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
}

impl Default for TabManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TabManager {
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            inactive_tabs: Vec::new(),
            active_tab_id: None,
            next_id: 1,
            closed_history: Vec::new(),
            groups: Vec::new(),
            next_group_id: 1,
            workspaces: default_workspaces(),
            active_workspace_id: 1,
            next_workspace_id: 2,
        }
    }

    fn ensure_workspace(&mut self) {
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
        let id = TabId(self.next_id);
        self.next_id += 1;

        let new_tab = TabState {
            id,
            url: url.to_string(),
            title: "New Tab".to_string(),
            favicon_uri: None,
            is_loading: false,
            can_go_back: false,
            can_go_forward: false,
            is_audio_playing: false,
            is_muted: false,
            status: TabStatus::Active,
            last_active_timestamp_secs: current_time_secs,
            group: None,
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

            // If this tab was in the active workspace's split, exit the split and prefer the
            // surviving pane as the next active tab.
            let mut surviving_pane: Option<TabId> = None;
            if let Some(ws) = self
                .workspaces
                .iter_mut()
                .find(|w| w.id == self.active_workspace_id)
            {
                if let Some(split) = &ws.split {
                    if split.panes.contains(&id) {
                        surviving_pane = split.panes.into_iter().find(|&p| p != id);
                        ws.split = None;
                    }
                }
            }

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

            let mut surviving_pane: Option<TabId> = None;
            if let Some(ws) = self.workspaces.iter_mut().find(|w| w.id == ws_id) {
                if let Some(split) = &ws.split {
                    if split.panes.contains(&id) {
                        surviving_pane = split.panes.into_iter().find(|&p| p != id);
                        ws.split = None;
                    }
                }
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
                    let blank_id = TabId(self.next_id);
                    self.next_id += 1;
                    self.inactive_tabs.push(TabState {
                        id: blank_id,
                        url: "shoji://newtab".to_string(),
                        title: "New Tab".to_string(),
                        favicon_uri: None,
                        is_loading: false,
                        can_go_back: false,
                        can_go_forward: false,
                        is_audio_playing: false,
                        is_muted: false,
                        status: TabStatus::Suspended,
                        last_active_timestamp_secs: current_time_secs,
                        group: None,
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
            if let Some(split) = &ws.split {
                let valid = split.panes[0] != split.panes[1]
                    && split
                        .panes
                        .iter()
                        .all(|&p| self.tabs.iter().any(|t| t.id == p));
                if !valid {
                    ws.split = None;
                }
            }
            ws.active_tab = Some(restored_id);
        }

        let split_panes = self
            .workspaces
            .iter()
            .find(|w| w.id == id)
            .and_then(|w| w.split.as_ref())
            .filter(|s| s.panes.contains(&restored_id))
            .map(|s| s.panes);

        for tab in &mut self.tabs {
            if tab.id == restored_id {
                tab.status = TabStatus::Active;
                tab.last_active_timestamp_secs = current_time_secs;
            } else if split_panes.is_some_and(|p| p.contains(&tab.id)) {
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
            let removed_tabs: Vec<TabState> = self.tabs.drain(..).collect();
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
                        if panes.contains(&tab.id) && tab.id != restored_id {
                            if tab.status != TabStatus::Crashed {
                                tab.status = TabStatus::Inactive;
                            }
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

            // Exit split in active workspace if `tab_id` was part of it.
            let mut surviving_pane: Option<TabId> = None;
            if let Some(ws) = self
                .workspaces
                .iter_mut()
                .find(|w| w.id == self.active_workspace_id)
            {
                if let Some(split) = &ws.split {
                    if split.panes.contains(&tab_id) {
                        surviving_pane = split.panes.into_iter().find(|&p| p != tab_id);
                        ws.split = None;
                    }
                }
            }

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
            if let Some(src_ws) = self.workspaces.iter_mut().find(|w| w.id == src_ws_id) {
                let mut surviving_pane: Option<TabId> = None;
                if let Some(split) = &src_ws.split {
                    if split.panes.contains(&tab_id) {
                        surviving_pane = split.panes.into_iter().find(|&p| p != tab_id);
                        src_ws.split = None;
                    }
                }
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

    /// The split state configured on the active workspace, if any.
    pub fn split(&self) -> Option<&SplitState> {
        self.active_workspace().and_then(|w| w.split.as_ref())
    }

    /// The split state currently visible in the viewport (when the active tab is one of the split
    /// panes).
    pub fn visible_split(&self) -> Option<&SplitState> {
        let active = self.active_tab_id?;
        self.split().filter(|s| s.panes.contains(&active))
    }

    /// The two pane ids when split view is currently visible.
    pub fn split_panes(&self) -> Option<[TabId; 2]> {
        self.visible_split().map(|s| s.panes)
    }

    pub fn is_in_split(&self, id: TabId) -> bool {
        self.split().is_some_and(|s| s.panes.contains(&id))
    }

    pub fn is_tab_visible(&self, id: TabId) -> bool {
        self.active_tab_id == Some(id)
            || self.visible_split().is_some_and(|s| s.panes.contains(&id))
    }

    pub fn other_split_pane(&self, id: TabId) -> Option<TabId> {
        let split = self.split()?;
        if split.panes[0] == id {
            Some(split.panes[1])
        } else if split.panes[1] == id {
            Some(split.panes[0])
        } else {
            None
        }
    }

    /// Pair `second_tab` with the active tab in split view. Both tabs must belong to the active
    /// workspace and be distinct.
    pub fn open_split(&mut self, second_tab: TabId) -> bool {
        self.ensure_workspace();
        let Some(first) = self.active_tab_id else {
            return false;
        };
        if first == second_tab || !self.tabs.iter().any(|t| t.id == second_tab) {
            return false;
        }
        if let Some(second) = self.tabs.iter_mut().find(|t| t.id == second_tab) {
            if second.status == TabStatus::Suspended {
                second.status = TabStatus::Inactive;
            }
        }
        if let Some(ws) = self
            .workspaces
            .iter_mut()
            .find(|w| w.id == self.active_workspace_id)
        {
            match &mut ws.split {
                Some(existing) => {
                    if existing.panes[0] == first {
                        existing.panes[1] = second_tab;
                    } else if existing.panes[1] == first {
                        existing.panes[0] = second_tab;
                    } else {
                        existing.panes = [first, second_tab];
                    }
                }
                None => {
                    ws.split = Some(SplitState::new(first, second_tab));
                }
            }
            return true;
        }
        false
    }

    /// Exit split view in the active workspace, keeping the focused pane as the active tab.
    pub fn close_split(&mut self) -> bool {
        if let Some(ws) = self
            .workspaces
            .iter_mut()
            .find(|w| w.id == self.active_workspace_id)
        {
            if ws.split.take().is_some() {
                return true;
            }
        }
        false
    }

    /// Toggle split view via keyboard shortcut: exits if active, or opens split view with the most
    /// recently active sibling tab (creating a blank tab if the workspace has only one tab).
    pub fn toggle_split(&mut self, current_time_secs: u64) -> bool {
        if self.visible_split().is_some() {
            self.close_split();
            return false;
        }
        if let Some(split) = self.split().cloned() {
            if self.tabs.iter().any(|t| t.id == split.panes[0])
                && self.tabs.iter().any(|t| t.id == split.panes[1])
            {
                self.switch_tab(split.panes[0], current_time_secs);
                return true;
            }
            self.close_split();
        }
        let Some(first) = self.active_tab_id else {
            return false;
        };
        let partner = self
            .tabs
            .iter()
            .filter(|t| t.id != first)
            .max_by_key(|t| t.last_active_timestamp_secs)
            .map(|t| t.id);
        let second = match partner {
            Some(id) => id,
            None => {
                let new_id = self.create_tab("shoji://newtab", current_time_secs);
                // Keep original tab as pane 0 and newly created tab as pane 1, focused on new_id.
                if let Some(ws) = self
                    .workspaces
                    .iter_mut()
                    .find(|w| w.id == self.active_workspace_id)
                {
                    ws.split = Some(SplitState::new(first, new_id));
                }
                return true;
            }
        };
        self.open_split(second)
    }

    /// Move keyboard/omnibox focus to the other pane of the active split.
    pub fn switch_split_pane(&mut self, current_time_secs: u64) -> Option<TabId> {
        let active = self.active_tab_id?;
        let other = self.other_split_pane(active)?;
        if self.switch_tab(other, current_time_secs) {
            Some(other)
        } else {
            None
        }
    }

    pub fn set_split_ratio(&mut self, ratio: f64) -> bool {
        let clamped = SplitState::clamp_ratio(ratio, None, 0.0);
        if let Some(split) = self
            .workspaces
            .iter_mut()
            .find(|w| w.id == self.active_workspace_id)
            .and_then(|w| w.split.as_mut())
        {
            split.ratio = clamped;
            return true;
        }
        false
    }

    pub fn reset_split_ratio(&mut self) -> bool {
        self.set_split_ratio(DEFAULT_SPLIT_RATIO)
    }

    pub fn set_split_orientation(&mut self, orientation: SplitOrientation) -> bool {
        if let Some(split) = self
            .workspaces
            .iter_mut()
            .find(|w| w.id == self.active_workspace_id)
            .and_then(|w| w.split.as_mut())
        {
            split.orientation = orientation;
            return true;
        }
        false
    }

    pub fn toggle_split_orientation(&mut self) -> Option<SplitOrientation> {
        let split = self
            .workspaces
            .iter_mut()
            .find(|w| w.id == self.active_workspace_id)
            .and_then(|w| w.split.as_mut())?;
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

    pub fn reorder_tab(&mut self, from_idx: usize, to_idx: usize) -> bool {
        if from_idx < self.tabs.len() && to_idx < self.tabs.len() && from_idx != to_idx {
            let tab = self.tabs.remove(from_idx);
            self.tabs.insert(to_idx, tab);
            true
        } else {
            false
        }
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
            let mut surviving_pane: Option<TabId> = None;
            if let Some(ws) = self
                .workspaces
                .iter_mut()
                .find(|w| w.id == self.active_workspace_id)
            {
                if let Some(split) = &ws.split {
                    if split.panes.contains(&id) {
                        surviving_pane = split.panes.into_iter().find(|&p| p != id);
                        ws.split = None;
                    }
                }
            }
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
            self.next_id = self.next_id.max(tab.id.0 + 1);
            tab.id
        } else {
            let new_id = TabId(self.next_id);
            self.next_id += 1;
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

        let idx = at_index.unwrap_or(self.tabs.len()).min(self.tabs.len());
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
            self.next_id = max_id + 1;
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
            let split_panes = ws.split.as_ref().map(|s| s.panes);
            let ws_active_id = if ws.id == self.active_workspace_id {
                self.active_tab_id.or(ws.active_tab)
            } else {
                ws.active_tab
            };

            let mut kept_ids = Vec::new();
            let mut ws_active_idx = 0;

            for t in &ws_tabs {
                let is_web = t.url.starts_with("http://") || t.url.starts_with("https://");
                let in_split = split_panes.is_some_and(|p| p.contains(&t.id));
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
                    workspace: ws.id,
                });
            }

            let split = ws.split.as_ref().and_then(|s| {
                let p0 = kept_ids.iter().position(|&id| id == s.panes[0])?;
                let p1 = kept_ids.iter().position(|&id| id == s.panes[1])?;
                (p0 != p1).then_some(SessionSplit {
                    panes: [p0, p1],
                    orientation: s.orientation,
                    ratio: s.ratio,
                })
            });

            workspaces.push(SessionWorkspace {
                id: ws.id,
                name: ws.name.clone(),
                color: ws.color.clone(),
                active: ws_active_idx,
                split,
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
                let id = TabId(self.next_id);
                self.next_id += 1;
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
                    is_muted: false,
                    status: TabStatus::Suspended,
                    last_active_timestamp_secs: current_time_secs,
                    group: st.group,
                    workspace: sw.id,
                });
            }

            if ws_tabs.is_empty() {
                let id = TabId(self.next_id);
                self.next_id += 1;
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
                    is_muted: false,
                    status: TabStatus::Suspended,
                    last_active_timestamp_secs: current_time_secs,
                    group: None,
                    workspace: sw.id,
                });
            }

            let ws_active_id = ws_ids.get(sw.active).or_else(|| ws_ids.first()).copied();

            let split = sw.split.as_ref().and_then(|sp| {
                let p0 = *ws_ids.get(sp.panes[0])?;
                let p1 = *ws_ids.get(sp.panes[1])?;
                (p0 != p1).then_some(SplitState {
                    panes: [p0, p1],
                    orientation: sp.orientation,
                    ratio: SplitState::clamp_ratio(sp.ratio, None, 0.0),
                })
            });

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
                split: split.clone(),
            });

            if sw.id == active_ws_id {
                let split_panes = split
                    .as_ref()
                    .filter(|s| ws_active_id.is_some_and(|a| s.panes.contains(&a)))
                    .map(|s| s.panes);
                for tab in &mut ws_tabs {
                    if Some(tab.id) == ws_active_id {
                        tab.status = TabStatus::Active;
                    } else if split_panes.is_some_and(|p| p.contains(&tab.id)) {
                        tab.status = TabStatus::Inactive;
                    } else {
                        tab.status = TabStatus::Inactive;
                    }
                }
                self.tabs = ws_tabs;
                self.active_tab_id = ws_active_id;
            } else {
                self.inactive_tabs.extend(ws_tabs);
            }
        }

        self.prune_groups();
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

    // Disallowed schemes (security check - case-insensitive)
    if lower.starts_with("javascript:")
        || lower.starts_with("file:")
        || lower.starts_with("data:")
        || lower.starts_with("vbscript:")
        || lower.starts_with("about:")
    {
        return Err(format!("Navigation to scheme prohibited: {}", trimmed));
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
