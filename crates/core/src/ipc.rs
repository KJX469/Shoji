use crate::tabs::{TabGroup, TabId, TabState};
use serde::{Deserialize, Serialize};

/// Strongly-typed commands sent from the browser UI (JS) to the Rust host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", content = "payload")]
pub enum UiToHostMessage {
    /// The chrome strip finished loading and is ready for initial state
    ChromeReady,
    CreateTab {
        url: Option<String>,
    },
    OpenNewWindow,
    SwitchTab {
        id: TabId,
    },
    CloseTab {
        id: TabId,
    },
    /// Navigate the active tab to a URL or search query
    Navigate {
        url: String,
    },
    GoBack,
    GoForward,
    Reload,
    ReloadTab {
        tab_id: TabId,
    },
    Stop,
    OpenDevTools,
    /// Window frame (custom title bar)
    MinimizeWindow,
    ToggleMaximize,
    CloseWindow,
    /// Resize from the top edge (the side and bottom edges are native; dragging the window
    /// is handled natively through `app-region: drag`)
    StartWindowResize,
    /// Dark websites: the global switch, and a per-site exception for `host`
    ToggleDarkWebsites,
    SetDarkWebsites {
        on: bool,
    },
    ToggleDarkSite {
        host: String,
    },
    /// Notes
    ToggleNotes,
    CloseNotes,
    /// Grow the notepad into a wide desk-sized sheet, or back
    /// The notepad page has loaded and run its first open
    NotesReady,
    SetNotesExpanded {
        expanded: bool,
    },
    SaveNote {
        id: String,
        created: u64,
        text: String,
    },
    DeleteNote {
        id: String,
    },
    /// Side panel
    ToggleMenuPanel,
    ToggleSecurityPanel,
    /// Open the Byobu (shield) tab of the side panel
    ToggleShieldPanel,
    /// Start the "hide an element" picker in the active tab
    ShieldPickElement,
    /// A Byobu surface asks for fresh state (lists, counters, optionally the request log);
    /// `refresh` also re-checks every filter list now
    ShieldState {
        #[serde(default)]
        refresh: bool,
        #[serde(default)]
        log: bool,
    },
    ToggleDownloadsSidebar,
    CloseSidebar,
    /// Open the native Windows certificate dialog for the given host
    OpenCertificateDialog {
        host: String,
    },
    /// Tab strip gestures
    ReorderTab {
        from_index: usize,
        to_index: usize,
    },
    DetachTabToNewWindow {
        tab_id: TabId,
        #[serde(default)]
        screen_x: Option<f64>,
        #[serde(default)]
        screen_y: Option<f64>,
    },
    /// Zoom
    ZoomIn,
    ZoomOut,
    ZoomReset,
    /// Find in page
    ToggleFindBar,
    OpenFindInPage,
    FindInPage {
        query: String,
        forward: bool,
    },
    CloseFindInPage,
    /// Result reported by the find script running in the active page
    FindResult {
        current: usize,
        total: usize,
    },
    /// Downloads
    CancelDownload {
        download_id: u64,
    },
    ChangeDownloadFolder,
    /// Site permissions
    OpenPermissionPrompt,
    ClosePermissionPrompt,
    PermissionResponse {
        permission_id: u64,
        allow: bool,
    },
    /// Settings & appearance
    OpenSettings,
    /// Partial settings object, merged into the current settings (see `Settings::apply_patch`)
    SaveSettings {
        patch: serde_json::Value,
    },
    /// Change the crane colour (applies live: UI, window and taskbar icon, shortcuts)
    SetAccent {
        accent: String,
    },
    /// Change the colour mode: "system", "light" or "dark"
    SetTheme {
        theme: String,
    },
    /// Close the first-run welcome page and start browsing
    FinishWelcome,
    /// Favourites
    ToggleFavourite,
    AddFavourite {
        url: String,
        title: String,
    },
    RemoveFavourite {
        url: String,
    },
    /// Address-bar suggestions, built by the chrome strip: the rows to show and where the box
    /// sits (logical px)
    SuggestShow {
        items: Vec<OverlayItem>,
        x: f64,
        width: f64,
    },
    /// Move the highlighted suggestion (index into the last list, -1 for none)
    SuggestHighlight {
        index: i32,
    },
    SuggestClose,
    /// Command palette (Ctrl+K)
    OpenPalette,
    /// The user picked something in the overlay (suggestion, palette entry, menu item or prompt text)
    OverlayChoose {
        kind: String,
        value: String,
    },
    OverlayClose,
    /// The overlay page has loaded and can be shown
    OverlayReady,
    /// Right-click menu for a tab
    TabMenu {
        tab_id: TabId,
        x: f64,
        y: f64,
    },
    /// Reader mode for the active tab (toggles)
    ToggleReader,
    /// Picture-in-picture for the active tab's video
    TogglePictureInPicture,
    /// Split view: show `tab_id` beside the active tab, or leave split view when `None`
    SplitWith {
        tab_id: Option<TabId>,
    },
    /// Session restore
    RestoreSession,
    DismissSession,
    /// Forget remembered per-site choices (permissions and zoom)
    ClearSiteData,
    /// Delete saved logins and autofill entries from the profile
    ClearSavedLogins,
    /// Page facts reported by the page-info script that runs in every tab
    PageInfo {
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        has_video: bool,
    },
    /// Continue to a site over plain HTTP after HTTPS-only mode could not reach it over HTTPS
    ContinueInsecure {
        tab_id: TabId,
        host: String,
        url: String,
    },
    /// Explicit user bypass of an untrusted TLS certificate (interstitial page only)
    BypassCertificateError {
        tab_id: TabId,
        host: String,
        url: String,
    },
}

impl UiToHostMessage {
    /// Messages that ordinary web content is allowed to send. Everything else is accepted
    /// only from the browser's own surfaces (chrome strip, side panel, internal pages).
    pub fn allowed_from_web_content(&self) -> bool {
        matches!(
            self,
            UiToHostMessage::FindResult { .. } | UiToHostMessage::PageInfo { .. }
        )
    }
}

/// One row of the floating overlay (suggestions, command palette, menus).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverlayItem {
    /// What picking it does: "url", "tab", "search", "action"
    pub kind: String,
    pub value: String,
    pub label: String,
    #[serde(default)]
    pub sub: String,
    /// Optional colour chip (a crane tone hex)
    #[serde(default)]
    pub color: Option<String>,
}

/// Security and certificate status for the active origin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SecurityInfo {
    pub host: String,
    pub is_secure: bool,
    pub is_internal: bool,
    /// Whether this site keeps cookies across sessions (persistent whitelist)
    pub is_persistent: bool,
    /// True once live certificate details have been fetched
    pub has_certificate: bool,
    pub subject: String,
    pub issuer: String,
    pub valid_from: String,
    pub valid_to: String,
    pub thumbprint: String,
    pub serial_number: String,
    pub signature_algorithm: String,
}

/// Strongly-typed state events sent from the Rust host to the UI (JS).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "data")]
pub enum HostToUiMessage {
    /// Full tab state update
    TabStateSync {
        tabs: Vec<TabState>,
        active_tab_id: Option<TabId>,
        #[serde(default)]
        groups: Vec<TabGroup>,
        /// The two tabs shown side by side, if split view is on
        #[serde(default)]
        split: Option<[TabId; 2]>,
    },
    /// Side panel state
    SidebarStateSync {
        open: bool,
        mode: String,
        security_info: Option<Box<SecurityInfo>>,
    },
}
