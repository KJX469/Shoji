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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TabManager {
    tabs: Vec<TabState>,
    active_tab_id: Option<TabId>,
    next_id: u64,
    closed_history: Vec<ClosedTab>, // closed tabs for Ctrl+Shift+T (in-memory only)
    #[serde(default)]
    groups: Vec<TabGroup>,
    #[serde(default)]
    next_group_id: u32,
}

impl TabManager {
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            active_tab_id: None,
            next_id: 1,
            closed_history: Vec::new(),
            groups: Vec::new(),
            next_group_id: 1,
        }
    }

    pub fn create_tab(&mut self, url: &str, current_time_secs: u64) -> TabId {
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
        };

        // If another tab was active, mark it inactive
        if let Some(prev_active_id) = self.active_tab_id {
            if let Some(prev) = self.tabs.iter_mut().find(|t| t.id == prev_active_id) {
                prev.status = TabStatus::Inactive;
            }
        }

        self.tabs.push(new_tab);
        self.active_tab_id = Some(id);
        id
    }

    pub fn close_tab(&mut self, id: TabId) -> Option<TabId> {
        if let Some(pos) = self.tabs.iter().position(|t| t.id == id) {
            let removed = self.tabs.remove(pos);
            if !removed.url.is_empty() && removed.url != "about:blank" {
                self.closed_history.push(ClosedTab {
                    url: removed.url,
                    title: removed.title,
                });
                if self.closed_history.len() > 50 {
                    self.closed_history.remove(0);
                }
            }

            if self.active_tab_id == Some(id) {
                if self.tabs.is_empty() {
                    self.active_tab_id = None;
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
            self.prune_groups();
        }
        self.active_tab_id
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

    pub fn update_title(&mut self, id: TabId, title: String) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            tab.title = title;
        }
    }

    pub fn update_url(&mut self, id: TabId, url: String) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            tab.url = url;
        }
    }

    pub fn update_favicon(&mut self, id: TabId, favicon_uri: Option<String>) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            tab.favicon_uri = favicon_uri;
        }
    }

    pub fn update_history_state(&mut self, id: TabId, can_go_back: bool, can_go_forward: bool) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            tab.can_go_back = can_go_back;
            tab.can_go_forward = can_go_forward;
        }
    }

    pub fn set_tab_suspended(&mut self, id: TabId, suspended: bool) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            tab.status = if suspended {
                TabStatus::Suspended
            } else {
                TabStatus::Inactive
            };
        }
    }

    pub fn set_audio_playing(&mut self, id: TabId, playing: bool) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            tab.is_audio_playing = playing;
        }
    }

    pub fn pop_last_closed(&mut self) -> Option<String> {
        self.closed_history.pop().map(|c| c.url)
    }

    /// Recently closed tabs, newest first.
    pub fn recently_closed(&self) -> Vec<ClosedTab> {
        self.closed_history.iter().rev().cloned().collect()
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
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab) else {
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
        self.groups
            .retain(|g| tabs.iter().any(|t| t.group == Some(g.id)));
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
        self.tabs.iter().find(|t| t.id == id)
    }

    /// Extract a tab from this manager (for moving to another window).
    pub fn extract_tab(&mut self, id: TabId) -> Option<TabState> {
        if let Some(pos) = self.tabs.iter().position(|t| t.id == id) {
            let tab = self.tabs.remove(pos);
            if self.active_tab_id == Some(id) {
                if self.tabs.is_empty() {
                    self.active_tab_id = None;
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
            self.prune_groups();
            Some(tab)
        } else {
            None
        }
    }

    /// Insert an existing tab state into this manager (e.g. from another window).
    pub fn insert_tab(&mut self, mut tab: TabState, at_index: Option<usize>) -> TabId {
        let id = if tab.id.0 > 0 {
            self.next_id = self.next_id.max(tab.id.0 + 1);
            tab.id
        } else {
            let new_id = TabId(self.next_id);
            self.next_id += 1;
            tab.id = new_id;
            new_id
        };
        tab.status = TabStatus::Active;

        if let Some(prev_active) = self.active_tab_id {
            if let Some(prev) = self.tabs.iter_mut().find(|t| t.id == prev_active) {
                prev.status = TabStatus::Inactive;
            }
        }

        let idx = at_index.unwrap_or(self.tabs.len()).min(self.tabs.len());
        self.tabs.insert(idx, tab);
        self.active_tab_id = Some(id);
        id
    }

    pub fn mark_tab_crashed(&mut self, id: TabId) {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
            tab.status = TabStatus::Crashed;
        }
    }

    pub fn snapshot(&self) -> Vec<TabState> {
        self.tabs.clone()
    }

    pub fn restore_from_snapshot(&mut self, tabs: Vec<TabState>) {
        self.tabs = tabs;
        if let Some(active) = self.tabs.iter().find(|t| t.status == TabStatus::Active) {
            self.active_tab_id = Some(active.id);
        } else if let Some(first) = self.tabs.first() {
            self.active_tab_id = Some(first.id);
        }
        if let Some(max_id) = self.tabs.iter().map(|t| t.id.0).max() {
            self.next_id = max_id + 1;
        }
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
