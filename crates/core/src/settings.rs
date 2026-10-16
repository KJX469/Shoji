use serde::{Deserialize, Serialize};
use shoji_brand::{theme::TypeProfile, Accent};
use std::path::{Path, PathBuf};

/// Application settings persisted to `%APPDATA%\shoji\settings.json`.
///
/// Every struct is `#[serde(default)]`, so older or partial files load cleanly: missing
/// fields take their defaults and unknown fields are ignored instead of discarding the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Whether the first-run welcome (colour picker) has been completed
    pub first_run_complete: bool,
    pub appearance: AppearanceSettings,
    pub privacy: PrivacySettings,
    pub performance: PerformanceSettings,
    /// Default search engine ("duckduckgo", "google", "bing", "brave", "ecosia")
    pub search_engine: String,
    pub downloads: DownloadSettings,
    pub tabs: TabSettings,
    pub permissions: PermissionSettings,
    pub shield: ShieldSettings,
    pub extensions: ExtensionSettings,
}

/// Browser extensions (unpacked Chromium extensions loaded through the engine). Read when the
/// engine starts, because it changes how the engine is created.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtensionSettings {
    pub enabled: bool,
}

/// Byobu, the ad and tracker shield.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShieldSettings {
    /// "off", "standard" or "aggressive"
    pub mode: String,
    /// Sites the shield leaves alone
    pub paused_sites: Vec<String>,
    /// Ids of upstream filter lists the user turned off
    pub disabled_lists: Vec<String>,
    /// Filter lists the user added, by URL
    pub custom_lists: Vec<String>,
    /// The user's own filters, one per line (element picker rules land here)
    pub custom_rules: String,
    /// Skip tracker redirects and drop tracking parameters from links you open
    pub clean_links: bool,
    /// Tell sites not to sell or share your data (Global Privacy Control)
    pub send_gpc: bool,
}

impl Default for ShieldSettings {
    fn default() -> Self {
        Self {
            mode: "standard".to_string(),
            paused_sites: Vec::new(),
            disabled_lists: Vec::new(),
            custom_lists: Vec::new(),
            custom_rules: String::new(),
            clean_links: true,
            send_gpc: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    /// Crane / accent colour id: "white" (greyscale default), "blue", "cyan", "green", "gold", "purple", "red"
    pub accent: String,
    /// Colour mode: "system", "light" (Washi) or "dark" (Sumi)
    pub theme: String,
    /// Type profile id: "shinbun", "kinari" or "hon"
    pub fonts: String,
    /// Default page zoom level (1.0 = 100%)
    pub default_zoom_level: f64,
    /// Show the zoom badge in the address bar when zoomed
    pub show_zoom_badge: bool,
    /// Show the destination URL in a status bubble when hovering links
    pub enable_link_preview: bool,
    /// Darken websites (Dark Reader). `None` until first run, which seeds it from Windows.
    pub dark_websites: Option<bool>,
    /// Sites left in their original colours while dark websites is on
    pub dark_websites_exclude: Vec<String>,
    /// How websites are darkened (Dark Reader's brightness, contrast, colours and so on)
    pub dark_theme: DarkTheme,
    /// Sites that use their own look instead of `dark_theme`, by host
    pub dark_site_themes: std::collections::BTreeMap<String, DarkTheme>,
    /// Only darken websites between two times of day
    pub dark_schedule: DarkSchedule,
    /// How the browser's own dark mode looks (same controls as for websites)
    pub browser_theme: DarkTheme,
    /// Seasonal and time-of-day decoration on the new tab page
    pub seasons: bool,
    /// Show only the site's domain in the address bar until it is clicked
    pub compact_address: bool,
    /// Tint the toolbar with a soft version of the page's own colour
    pub tint_toolbar: bool,
    /// Translucent Windows 11 backdrop behind the toolbar (applies after a restart)
    pub frosted_toolbar: bool,
    /// Also ask the search engine for suggestions as you type (sends keystrokes to it)
    pub search_suggestions: bool,
}

/// A look for darkened websites. Mirrors the knobs of the Dark Reader extension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DarkTheme {
    /// "sumi", "midnight", "dusk", "oled", "paper" or "custom"
    pub preset: String,
    /// "dark" (invert light pages) or "light" (dim them but keep them light)
    pub mode: String,
    /// 50..=150 (percent)
    pub brightness: i32,
    /// 50..=150 (percent)
    pub contrast: i32,
    /// 0..=100 (percent)
    pub sepia: i32,
    /// 0..=100 (percent)
    pub grayscale: i32,
    /// 0..=100 (percent of Dark Reader's heaviest stroke)
    pub text_stroke: i32,
    pub use_font: bool,
    pub font_family: String,
    /// Page background and text colours as `#rrggbb`
    pub background: String,
    pub text: String,
}

impl DarkTheme {
    pub const PRESETS: [&'static str; 5] = ["sumi", "midnight", "dusk", "oled", "paper"];

    /// A built-in look by id (unknown ids give Sumi).
    pub fn preset(id: &str) -> Self {
        let base = Self {
            preset: "sumi".into(),
            mode: "dark".into(),
            brightness: 100,
            contrast: 95,
            sepia: 0,
            grayscale: 0,
            text_stroke: 0,
            use_font: false,
            font_family: "Segoe UI".into(),
            background: "#1b1a18".into(),
            text: "#ebe6dc".into(),
        };
        let with = |preset: &str, bg: &str, text: &str, b: i32, c: i32, sepia: i32| Self {
            preset: preset.into(),
            background: bg.into(),
            text: text.into(),
            brightness: b,
            contrast: c,
            sepia,
            ..base.clone()
        };
        match id {
            "midnight" => with("midnight", "#0f1420", "#dfe6f2", 100, 100, 0),
            "dusk" => with("dusk", "#241d18", "#f0e2cf", 95, 100, 12),
            "oled" => with("oled", "#000000", "#e8e8e8", 100, 100, 0),
            "paper" => with("paper", "#2a2926", "#cfcac0", 90, 85, 0),
            _ => base,
        }
    }

    fn normalize(&mut self) {
        if !Self::PRESETS.contains(&self.preset.as_str()) && self.preset != "custom" {
            self.preset = "custom".into();
        }
        if self.mode != "light" {
            self.mode = "dark".into();
        }
        self.brightness = self.brightness.clamp(50, 150);
        self.contrast = self.contrast.clamp(50, 150);
        self.sepia = self.sepia.clamp(0, 100);
        self.grayscale = self.grayscale.clamp(0, 100);
        self.text_stroke = self.text_stroke.clamp(0, 100);
        self.font_family = self
            .font_family
            .chars()
            .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
            .take(60)
            .collect();
        if self.font_family.is_empty() {
            self.font_family = "Segoe UI".into();
        }
        let base = Self::preset("sumi");
        for (value, fallback) in [
            (&mut self.background, base.background),
            (&mut self.text, base.text),
        ] {
            let ok = value.len() == 7
                && value.starts_with('#')
                && value[1..].chars().all(|c| c.is_ascii_hexdigit());
            if !ok {
                *value = fallback;
            }
        }
    }
}

impl Default for DarkTheme {
    fn default() -> Self {
        Self::preset("sumi")
    }
}

/// Darken websites only between `from` and `to` (24-hour `HH:MM`, wrapping past midnight).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DarkSchedule {
    pub enabled: bool,
    pub from: String,
    pub to: String,
}

impl Default for DarkSchedule {
    fn default() -> Self {
        Self {
            enabled: false,
            from: "20:00".into(),
            to: "07:00".into(),
        }
    }
}

impl DarkSchedule {
    fn normalize(&mut self) {
        let valid = |t: &str| {
            t.split_once(':').is_some_and(|(h, m)| {
                h.len() == 2
                    && m.len() == 2
                    && h.parse::<u8>().is_ok_and(|h| h < 24)
                    && m.parse::<u8>().is_ok_and(|m| m < 60)
            })
        };
        let fallback = Self::default();
        if !valid(&self.from) {
            self.from = fallback.from;
        }
        if !valid(&self.to) {
            self.to = fallback.to;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacySettings {
    /// Private mode. When true, tabs run in ephemeral mode (no history, logins or cache are kept)
    /// except on `persistent_sites`. When false (the default), Shoji remembers like a normal browser.
    pub ephemeral_default: bool,
    /// Whether the user has made an explicit choice (setup or Settings). Files written before this
    /// existed carry the old "forget everything" default, which is not treated as a choice.
    pub mode_chosen: bool,
    /// In private mode, domains still allowed to retain cookies/session
    pub persistent_sites: Vec<String>,
    /// Upgrade http:// pages to https://, asking before falling back to plain HTTP
    pub https_only: bool,
    /// Ask the engine for its strictest tracking prevention
    pub strict_tracking_prevention: bool,
    /// Refuse cookies that a page's embedded sites try to set (read when the engine starts)
    pub block_third_party_cookies: bool,
    /// Offer to save and fill passwords (stored by the engine, encrypted for this Windows user)
    pub save_passwords: bool,
    /// What to do with open tabs after a crash or force close: "ask", "always" or "never"
    pub restore_session: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerformanceSettings {
    /// Seconds a background tab may stay idle before it is suspended (0 = never)
    pub sleep_after_secs: u64,
    /// Keep running in the notification area after the last window closes, so the shield's
    /// filter lists keep updating. Ignored in Private mode.
    pub run_in_background: bool,
    /// Download new Shoji releases and install them while no window is open
    pub auto_update: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadSettings {
    /// Prompt for a save destination on each download
    pub ask_where_to_save: bool,
    /// Show the download progress ring in the toolbar
    pub show_progress_toolbar: bool,
    /// Default download folder
    pub default_folder: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TabSettings {
    /// Drag tabs to reorder them
    pub enable_tab_reordering: bool,
    /// Drag a tab out of the strip to open it in a new window
    pub enable_tab_tearoff: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionSettings {
    /// Policy per permission: "ask", "allow" or "block"
    pub location: String,
    pub camera: String,
    pub microphone: String,
    pub notifications: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            first_run_complete: false,
            appearance: AppearanceSettings::default(),
            privacy: PrivacySettings::default(),
            performance: PerformanceSettings::default(),
            search_engine: "duckduckgo".to_string(),
            downloads: DownloadSettings::default(),
            tabs: TabSettings::default(),
            permissions: PermissionSettings::default(),
            shield: ShieldSettings::default(),
            extensions: ExtensionSettings::default(),
        }
    }
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            accent: Accent::default().id().to_string(),
            theme: "system".to_string(),
            fonts: TypeProfile::default().id.to_string(),
            default_zoom_level: 1.0,
            show_zoom_badge: true,
            enable_link_preview: true,
            dark_websites: None,
            dark_websites_exclude: Vec::new(),
            dark_theme: DarkTheme::default(),
            dark_site_themes: std::collections::BTreeMap::new(),
            dark_schedule: DarkSchedule::default(),
            browser_theme: DarkTheme::default(),
            seasons: true,
            compact_address: true,
            tint_toolbar: true,
            frosted_toolbar: false,
            search_suggestions: false,
        }
    }
}

impl Default for PrivacySettings {
    fn default() -> Self {
        Self {
            ephemeral_default: false,
            mode_chosen: false,
            persistent_sites: Vec::new(),
            https_only: true,
            strict_tracking_prevention: true,
            block_third_party_cookies: false,
            save_passwords: true,
            restore_session: "ask".to_string(),
        }
    }
}

impl Default for PerformanceSettings {
    fn default() -> Self {
        Self {
            sleep_after_secs: 300,
            run_in_background: true,
            auto_update: true,
        }
    }
}

impl Default for DownloadSettings {
    fn default() -> Self {
        Self {
            ask_where_to_save: true,
            show_progress_toolbar: true,
            default_folder: dirs_fallback_downloads(),
        }
    }
}

impl Default for TabSettings {
    fn default() -> Self {
        Self {
            enable_tab_reordering: true,
            enable_tab_tearoff: true,
        }
    }
}

impl Default for PermissionSettings {
    fn default() -> Self {
        let ask = || "ask".to_string();
        Self {
            location: ask(),
            camera: ask(),
            microphone: ask(),
            notifications: ask(),
        }
    }
}

impl Settings {
    pub fn search_url_template(&self) -> &'static str {
        match self.search_engine.to_lowercase().as_str() {
            "google" => "https://www.google.com/search?q=%s",
            "bing" => "https://www.bing.com/search?q=%s",
            "brave" => "https://search.brave.com/search?q=%s",
            "ecosia" => "https://www.ecosia.org/search?q=%s",
            _ => "https://duckduckgo.com/?q=%s",
        }
    }

    pub fn search_engine_display_name(&self) -> &'static str {
        match self.search_engine.to_lowercase().as_str() {
            "google" => "Google",
            "bing" => "Bing",
            "brave" => "Brave",
            "ecosia" => "Ecosia",
            _ => "DuckDuckGo",
        }
    }

    /// The selected crane colour.
    pub fn accent(&self) -> Accent {
        Accent::from_id(&self.appearance.accent)
    }

    /// The selected type profile.
    pub fn fonts(&self) -> TypeProfile {
        TypeProfile::from_id(&self.appearance.fonts)
    }

    /// Policy ("ask" / "allow" / "block") for a WebView2 permission kind name.
    pub fn permission_policy(&self, kind: &str) -> &str {
        match kind {
            "Geolocation" => &self.permissions.location,
            "Camera" => &self.permissions.camera,
            "Microphone" => &self.permissions.microphone,
            "Notifications" => &self.permissions.notifications,
            _ => "ask",
        }
    }

    /// Whether websites are darkened on `host` (global switch minus per-site exclusions).
    pub fn dark_on(&self, host: &str) -> bool {
        let host = extract_domain(host);
        self.appearance.dark_websites == Some(true)
            && !self.appearance.dark_websites_exclude.iter().any(|p| {
                let p = p.trim_start_matches("*.");
                host == p || host.strip_suffix(p).is_some_and(|r| r.ends_with('.'))
            })
    }

    /// The shield mode for a page on `host`: its configured mode, or "off" for paused sites.
    pub fn shield_mode_for(&self, host: &str) -> &str {
        let host = extract_domain(host);
        let paused = self.shield.paused_sites.iter().any(|p| {
            let p = p.trim_start_matches("*.");
            host == p || host.strip_suffix(p).is_some_and(|r| r.ends_with('.'))
        });
        if paused {
            "off"
        } else {
            self.shield.mode.as_str()
        }
    }

    /// Returns true if the given URL or domain is whitelisted for persistent session storage.
    pub fn is_site_persistent(&self, url_or_domain: &str) -> bool {
        if !self.privacy.ephemeral_default {
            return true;
        }
        let domain = extract_domain(url_or_domain);
        if domain.is_empty() {
            return false;
        }
        self.privacy.persistent_sites.iter().any(|allowed| {
            let trimmed = allowed.trim().to_lowercase();
            let base_pattern = trimmed.strip_prefix("*.").unwrap_or(&trimmed);
            let pattern_domain = extract_domain(base_pattern);
            !pattern_domain.is_empty()
                && (domain == pattern_domain
                    || domain
                        .strip_suffix(pattern_domain.as_str())
                        .is_some_and(|rest| rest.ends_with('.')))
        })
    }

    /// Apply a partial settings object, shaped like the serialized settings, e.g.
    /// `{"appearance": {"accent": "blue"}}`. Objects merge recursively; other values replace.
    /// Patches that would produce an invalid structure are rejected and leave settings untouched.
    pub fn apply_patch(&mut self, patch: &serde_json::Value) -> bool {
        let Ok(mut current) = serde_json::to_value(&*self) else {
            return false;
        };
        merge(&mut current, patch);
        if patch.pointer("/privacy/ephemeral_default").is_some() {
            current["privacy"]["mode_chosen"] = serde_json::Value::Bool(true);
        }
        match serde_json::from_value::<Settings>(current) {
            Ok(mut next) => {
                next.normalize();
                *self = next;
                true
            }
            Err(_) => false,
        }
    }

    /// Clamp and canonicalize values that arrive from the UI.
    fn normalize(&mut self) {
        self.appearance.accent = self.accent().id().to_string();
        self.appearance.fonts = self.fonts().id.to_string();
        if !matches!(self.appearance.theme.as_str(), "system" | "light" | "dark") {
            self.appearance.theme = "system".to_string();
        }
        self.appearance.default_zoom_level = self.appearance.default_zoom_level.clamp(0.25, 3.0);
        self.appearance.dark_theme.normalize();
        self.appearance.dark_schedule.normalize();
        self.appearance.browser_theme.normalize();
        let mut sites = std::mem::take(&mut self.appearance.dark_site_themes);
        sites.retain(|host, _| !host.trim().is_empty() && host.len() < 254);
        if sites.len() > 500 {
            sites = sites.into_iter().take(500).collect();
        }
        for theme in sites.values_mut() {
            theme.normalize();
        }
        self.appearance.dark_site_themes = sites;
        if !matches!(
            self.privacy.restore_session.as_str(),
            "ask" | "always" | "never"
        ) {
            self.privacy.restore_session = "ask".to_string();
        }
        if !matches!(self.shield.mode.as_str(), "off" | "standard" | "aggressive") {
            self.shield.mode = "standard".to_string();
        }
        self.shield.custom_lists.retain(|u| {
            let u = u.trim();
            (u.starts_with("https://") || u.starts_with("http://")) && u.len() < 1000
        });
        self.shield.custom_lists.sort();
        self.shield.custom_lists.dedup();
        self.shield.disabled_lists.sort();
        self.shield.disabled_lists.dedup();
        if self.shield.custom_rules.len() > 200_000 {
            self.shield.custom_rules.truncate(200_000);
        }
        for policy in [
            &mut self.permissions.location,
            &mut self.permissions.camera,
            &mut self.permissions.microphone,
            &mut self.permissions.notifications,
        ] {
            if !matches!(policy.as_str(), "ask" | "allow" | "block") {
                *policy = "ask".to_string();
            }
        }
        for list in [
            &mut self.privacy.persistent_sites,
            &mut self.appearance.dark_websites_exclude,
            &mut self.shield.paused_sites,
        ] {
            let mut seen = std::collections::HashSet::new();
            *list = list
                .iter()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty() && seen.insert(s.clone()))
                .collect();
        }
    }
}

fn merge(target: &mut serde_json::Value, patch: &serde_json::Value) {
    match (target, patch) {
        (serde_json::Value::Object(t), serde_json::Value::Object(p)) => {
            for (k, v) in p {
                match t.get_mut(k) {
                    Some(existing) => merge(existing, v),
                    None => {
                        t.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (t, p) => *t = p.clone(),
    }
}

fn dirs_fallback_downloads() -> PathBuf {
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        PathBuf::from(userprofile).join("Downloads")
    } else {
        PathBuf::from("Downloads")
    }
}

impl Settings {
    /// Load settings, falling back to defaults when the file is missing or unreadable.
    pub fn load_from_path(path: &Path) -> Self {
        let mut settings = std::fs::read_to_string(path)
            .ok()
            .and_then(|content| serde_json::from_str::<Settings>(&content).ok())
            .unwrap_or_default();
        // Older versions defaulted to forgetting everything and wrote that default to disk.
        // Without an explicit choice, use the current default (remember like a normal browser).
        if !settings.privacy.mode_chosen {
            settings.privacy.ephemeral_default = false;
        }
        settings.normalize();
        settings
    }

    pub fn save_to_path(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        // Write to a sibling file and rename, so a crash mid-write never corrupts settings.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, content)?;
        std::fs::rename(&tmp, path)
    }
}

/// Helper to extract a normalized domain from a URL or raw domain string.
pub fn extract_domain(input: &str) -> String {
    let mut s = input.trim();
    if s.is_empty() {
        return String::new();
    }
    // Reject internal schemes and about:blank
    if s.starts_with(crate::INTERNAL_SCHEME)
        || s.starts_with("about:")
        || s.starts_with("data:")
        || s.starts_with("javascript:")
    {
        return String::new();
    }
    // Strip any scheme
    if let Some(idx) = s.find("://") {
        s = &s[idx + 3..];
    }
    // Strip trailing path/query/fragment
    if let Some(idx) = s.find(['/', '?', '#']) {
        s = &s[..idx];
    }
    // Strip userinfo (user:pass@host)
    if let Some(idx) = s.find('@') {
        s = &s[idx + 1..];
    }
    // Strip port (host:8080)
    if let Some(idx) = s.rfind(':') {
        if !s.starts_with('[') || s.ends_with(']') {
            s = &s[..idx];
        }
    }
    // Strip surrounding brackets for IPv6
    s = s.trim_matches(|c| c == '[' || c == ']');
    let domain = s.to_lowercase();
    match domain.strip_prefix("www.") {
        Some(stripped) => stripped.to_string(),
        None => domain,
    }
}
