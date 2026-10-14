use crate::tabs::{SplitOrientation, SplitState, TabGroup, TabId, TabState, Workspace};
use serde::{Deserialize, Serialize};

/// What the user types on the settings page to confirm uninstalling Shoji and deleting all of
/// its data.
pub const UNINSTALL_CONFIRMATION: &str = "UNINSTALL";

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
    /// Open the Workspaces tab of the side panel
    ToggleWorkspacesSidebar,
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
    /// Workspaces
    CreateWorkspace {
        name: String,
        #[serde(default)]
        color: String,
    },
    SwitchWorkspace {
        id: u32,
    },
    RenameWorkspace {
        id: u32,
        name: String,
    },
    SetWorkspaceColor {
        id: u32,
        color: String,
    },
    ReorderWorkspace {
        from_index: usize,
        to_index: usize,
    },
    DeleteWorkspace {
        id: u32,
    },
    MoveTabToWorkspace {
        tab_id: TabId,
        workspace_id: u32,
    },
    WorkspaceMenu {
        workspace_id: u32,
        x: f64,
        y: f64,
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
    /// Import: the browsers that can be imported from, and which of them are installed
    ListImportSources,
    /// Read data from `source` (a catalogue id). `profile` indexes that source's profiles;
    /// `path` is the file or folder the user picked for sources without a fixed location.
    StartImport {
        source: String,
        #[serde(default)]
        profile: usize,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        kinds: crate::import::Kinds,
    },
    /// Let the user pick a bookmarks file or profile folder for `source`
    PickImportSource {
        source: String,
    },
    /// Is Shoji the default browser? Answered with `__defaultBrowser(bool)` on the page.
    CheckDefaultBrowser,
    /// Open Windows' default apps settings on Shoji
    SetDefaultBrowser,
    /// Address-bar suggestions from history; answered with `__historyResults(seq, items)`
    SearchHistory {
        query: String,
        seq: u64,
    },
    /// Forget all browsing history kept by Shoji
    ClearHistory,
    /* Password manager. The `Vault*` messages come only from the passwords page. */
    /// Ask for the vault's state (answered with `__vaultState`)
    VaultStatus,
    /// The logins matching `query` (answered with `__vaultList`)
    VaultList {
        #[serde(default)]
        query: String,
    },
    /// Send one login's password and note to the page (`__vaultSecret`)
    VaultReveal {
        id: String,
    },
    /// Put "username" or "password" of a login on the clipboard (cleared after a short while)
    VaultCopy {
        id: String,
        field: String,
    },
    /// Add a login, or change the one with `id`
    VaultSave {
        #[serde(default)]
        id: Option<String>,
        url: String,
        #[serde(default)]
        title: String,
        #[serde(default)]
        username: String,
        password: String,
        #[serde(default)]
        note: String,
    },
    VaultDelete {
        id: String,
    },
    VaultDeleteAll,
    VaultUnlock {
        master: String,
    },
    VaultLock,
    VaultSetMaster {
        master: String,
    },
    VaultRemoveMaster,
    /// A random password with these options (answered with `__vaultGenerated`)
    VaultGenerate {
        #[serde(default = "default_length")]
        length: usize,
        #[serde(default = "yes")]
        lower: bool,
        #[serde(default = "yes")]
        upper: bool,
        #[serde(default = "yes")]
        digits: bool,
        #[serde(default = "yes")]
        symbols: bool,
    },
    /// Pick a CSV file and add its logins
    VaultImportCsv,
    /// Choose where to write every login as plain CSV
    VaultExportCsv,
    /// A login form was submitted on the page (reported by the page itself, so the host uses the
    /// tab's real address, never one the page names, and asks the user before saving anything)
    LoginSubmitted {
        #[serde(default)]
        username: String,
        password: String,
    },
    /// The user focused a login field; `x`, `y`, `w`, `h` are its rectangle in the page. The host
    /// offers only the logins saved for the tab's own site, and fills only after a click.
    LoginFieldFocused {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    },
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
    /// Enter or exit split view with the most recent sibling tab
    ToggleSplit,
    /// Flip the split divider between vertical and horizontal
    ToggleSplitOrientation,
    SetSplitOrientation {
        orientation: SplitOrientation,
    },
    /// Set the split divider ratio (clamped to minimum pane bounds)
    SetSplitRatio {
        ratio: f64,
    },
    /// Reset split divider to 50/50
    ResetSplitRatio,
    /// Move focus between the two split panes
    SwitchSplitPane,
    /// A link was dragged and dropped onto a split pane
    LinkDropped {
        url: String,
    },
    /// Mute or unmute a tab's audio output
    ToggleMuteTab {
        tab_id: TabId,
    },
    /// Session restore
    RestoreSession,
    DismissSession,
    /// Forget remembered per-site choices (permissions and zoom)
    ClearSiteData,
    /// Delete saved logins and autofill entries from the profile
    ClearSavedLogins,
    /// Uninstall Shoji and delete everything it stored. Honoured only when `confirm` is
    /// [`UNINSTALL_CONFIRMATION`], which the user has to type on the settings page.
    UninstallShoji {
        confirm: String,
    },
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

fn yes() -> bool {
    true
}

fn default_length() -> usize {
    20
}

impl UiToHostMessage {
    /// Messages that ordinary web content is allowed to send. Everything else is accepted
    /// only from the browser's own surfaces (chrome strip, side panel, internal pages).
    pub fn allowed_from_web_content(&self) -> bool {
        matches!(
            self,
            UiToHostMessage::FindResult { .. }
                | UiToHostMessage::PageInfo { .. }
                | UiToHostMessage::LoginSubmitted { .. }
                | UiToHostMessage::LoginFieldFocused { .. }
                | UiToHostMessage::LinkDropped { .. }
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
        /// Detailed split state (orientation, ratio, panes) for the active workspace
        #[serde(default)]
        split_state: Option<SplitState>,
        /// All workspaces in this window, in user order
        #[serde(default)]
        workspaces: Vec<Workspace>,
        /// Active workspace id in this window
        #[serde(default)]
        active_workspace_id: u32,
        /// Tabs in inactive workspaces (for the workspaces sidebar)
        #[serde(default)]
        inactive_tabs: Vec<TabState>,
    },
    /// Side panel state
    SidebarStateSync {
        open: bool,
        mode: String,
        security_info: Option<Box<SecurityInfo>>,
    },
}
