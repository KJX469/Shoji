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
    /// Where the find and permission cards sit in the chrome strip (x, y, width, height in
    /// logical px), or `None` while one is closed. The strip is clipped to them.
    TrayRects {
        #[serde(default)]
        find: Option<[f64; 4]>,
        #[serde(default)]
        permission: Option<[f64; 4]>,
        #[serde(default)]
        toast: Option<[f64; 4]>,
        #[serde(default)]
        letter: Option<[f64; 4]>,
    },
    /// The update letter and Settings' updates row: "check", "now", "close", "later", "skip",
    /// "unskip", "notes" (open the release page) or "state" (just report)
    UpdateAction {
        choice: String,
    },
    /// One of Shoji's hidden things was found (an id from `userdata::SECRETS`)
    FoundSecret {
        id: String,
    },
    /// The dark-websites dropdown
    ToggleDarkPanel,
    /// Open (or switch to) one of Shoji's own pages: "history", "downloads", "passwords",
    /// "byobu" or "settings"
    OpenPage {
        page: String,
    },
    PrintPage,
    /// Open the history page with its clear-browsing-data card showing
    OpenClearData,
    /// One page of the history list for the history page; answered with `__historyPage`
    HistoryQuery {
        #[serde(default)]
        query: String,
        #[serde(default)]
        offset: usize,
        #[serde(default = "default_page")]
        limit: usize,
        seq: u64,
    },
    /// Take these pages out of the history
    HistoryRemove {
        urls: Vec<String>,
    },
    /// The saved downloads list, answered with `__downloadsList`
    DownloadsQuery,
    /// Do something with a saved download: "open", "show" (in its folder) or "remove" (from
    /// the list; the file stays). `key` is the id from the list.
    DownloadAction {
        key: u64,
        action: String,
    },
    /// Clear browsing data from the last `range` ("hour", "day", "week", "month" or "all")
    ClearBrowsingData {
        range: String,
        #[serde(default)]
        history: bool,
        #[serde(default)]
        downloads: bool,
        #[serde(default)]
        cookies: bool,
        #[serde(default)]
        cache: bool,
    },
    CloseSidebar,
    /// The dropdown lost focus (the user clicked elsewhere)
    SidebarBlur,
    /// The pointer entered (`inside`) or left the dropdown
    SidebarPointer {
        inside: bool,
    },
    /// The dropdown's content wants this many logical pixels of height
    SidebarHeight {
        height: f64,
    },
    /// Pin the dropdown as a side panel that takes page width, or float it again
    PinSidebar {
        pinned: bool,
    },
    /// Horizontal centre of each toolbar button (logical px), so dropdowns hang from them
    ToolbarAnchors {
        items: std::collections::BTreeMap<String, f64>,
    },
    /// Set how websites are darkened: for `host`, or for every site when it is `None`
    SetDarkTheme {
        #[serde(default)]
        host: Option<String>,
        theme: crate::settings::DarkTheme,
    },
    /// Set how the browser's own dark mode looks
    SetBrowserTheme {
        theme: crate::settings::DarkTheme,
    },
    /// Forget a site's own dark look (it follows the look for every site again)
    ResetDarkSite {
        host: String,
    },
    /// How dark mode treats one site: "follow" the global switch, "always" dark, or "never" dark
    SetDarkSiteMode {
        host: String,
        mode: String,
    },
    /// Darken websites only between two times of day
    SetDarkSchedule {
        enabled: bool,
        from: String,
        to: String,
    },
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
    /// The search engine list that hangs from the logo in the address bar
    EngineMenu {
        x: f64,
        y: f64,
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
        /// Answer the overlay (the add-a-pane window) instead of the toolbar
        #[serde(default)]
        overlay: bool,
    },
    /// Ask the search engine for suggestions (the page cannot: the engines send no CORS headers);
    /// answered with `__suggestWords(seq, words)`
    SuggestQuery {
        query: String,
        seq: u64,
        #[serde(default)]
        overlay: bool,
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
    /// Ask for the folder to file a favourite in (opens a name prompt)
    FavouriteFolder {
        url: String,
    },
    /// Save the favourites as a bookmarks file other browsers can import
    ExportFavourites,
    /// Do one of the named actions of the command palette and menus ("save-page", "exit"…)
    RunAction {
        value: String,
    },
    /// The bookmarks (every one, plus the folders) for the bookmarks page, panel or bar; answered
    /// with `__bookmarks`
    BookmarksQuery,
    /// Change a bookmark's name and address
    BookmarkEdit {
        url: String,
        title: String,
        new_url: String,
    },
    /// File a bookmark in `folder` ("" for the top level), just before `before` when given
    BookmarkMove {
        url: String,
        folder: String,
        #[serde(default)]
        before: Option<String>,
    },
    /// Make a folder `name` inside `parent` ("" for the top level)
    BookmarkNewFolder {
        #[serde(default)]
        parent: String,
        name: String,
    },
    /// Rename the folder at path `from` to `name` (its last part)
    BookmarkRenameFolder {
        from: String,
        name: String,
    },
    /// Move a folder under another ("" for the top level)
    BookmarkMoveFolder {
        from: String,
        parent: String,
    },
    BookmarkDeleteFolder {
        folder: String,
        keep_items: bool,
    },
    BookmarkRemove {
        urls: Vec<String>,
    },
    /// Open bookmarks: how is "tab", "window", "private", "group" (all as a tab group) or "here"
    BookmarkOpen {
        urls: Vec<String>,
        how: String,
        #[serde(default)]
        group: String,
    },
    /// Right-click menu for a bookmark (`url`), a folder (`folder`) or neither (the bar), at
    /// x, y in the coordinates of the surface that asked: `from` is "bar", "panel" or "page"
    BookmarkMenu {
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        folder: Option<String>,
        #[serde(default)]
        from: String,
        x: f64,
        y: f64,
    },
    /// A folder on the bookmarks bar was clicked: list what is in it, under it at `x`
    BookmarkFolderMenu {
        folder: String,
        x: f64,
    },
    /// The bar's overflow button: list the top-level items after the first `skip`
    BookmarkOverflow {
        skip: usize,
        x: f64,
    },
    /// Show or hide the bookmarks bar
    ToggleBookmarksBar,
    /// The pointer rests on a bookmarks trigger (the toolbar button, or "All bookmarks" on the bar):
    /// drop the list down. It folds away once the pointer is off both the trigger (`left`..`right`)
    /// and the list, unless something inside was pressed (`PinHover`).
    HoverBookmarks {
        left: f64,
        right: f64,
        #[serde(default)]
        bar: bool,
    },
    /// A press or keystroke inside a dropdown that opened by hovering: it stays until dismissed
    PinHover,
    /// The toolbar's bookmarks button: "off" or "on"
    SetSidebarMode {
        mode: String,
    },
    /// Bookmark the page open in the active tab, in `folder` ("" for the top level)
    BookmarkPage {
        #[serde(default)]
        folder: String,
    },
    /// The recent history and recently closed tabs for the menu; answered with `__menuData`
    MenuDataQuery,
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
    /// The dropdown page has loaded and painted, so the view can be shown
    SidebarReady,
    /// Right-click menu for a tab
    TabMenu {
        tab_id: TabId,
        x: f64,
        y: f64,
    },
    /// Mute or unmute a tab's sound (the speaker mark on the tab)
    ToggleMuteTab {
        tab_id: TabId,
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
    /// Move focus to the next split pane
    SwitchSplitPane,
    /// Focus pane `index` (0 is the first) of the visible split
    SplitFocus {
        index: usize,
    },
    /// Add a tab to an existing split group (`at`: pane position, or the end when absent)
    SplitAdd {
        split_id: u32,
        tab_id: TabId,
        #[serde(default)]
        at: Option<usize>,
    },
    /// Swap pane `index` of a split group with the one after it
    SplitSwapPair {
        id: u32,
        index: usize,
    },
    /// Take a tab out of its split group (the tab stays open)
    SplitRemove {
        tab_id: TabId,
    },
    /// Create a split group from two tabs of the same workspace
    SplitCreate {
        first: TabId,
        second: TabId,
    },
    /// Dissolve a split group (the tabs stay open)
    SplitClose {
        id: u32,
    },
    SplitRename {
        id: u32,
        name: String,
    },
    /// The pointer left the tab strip while dragging a tab: the host takes the drag over. The grab
    /// offset is where the pointer was inside the tab (logical px).
    TabDragStart {
        tab_id: TabId,
        #[serde(default)]
        grab_dx: f64,
        #[serde(default)]
        grab_dy: f64,
        /// Where the first tab of a strip starts (logical px), so the new window can put the
        /// dragged tab under the pointer
        #[serde(default)]
        strip_left: f64,
    },
    /// Horizontal centres (logical px) of the tabs in the strip, in strip order. The host uses them
    /// to turn a dragged tab's position into an insertion index.
    TabCenters {
        centers: Vec<f64>,
    },
    /// Move a block of neighbouring tabs (a split group) in the strip
    ReorderGroup {
        from_index: usize,
        count: usize,
        to_index: usize,
    },
    /// Where each split pill sits in the strip, as `[split id, left, right]` (logical px), so a
    /// tab dragged in from another window can be dropped on one to join it
    SplitZones {
        zones: Vec<[f64; 3]>,
    },
    /// Swap which pane is first in a split group
    SplitSwapPanes {
        id: u32,
    },
    SplitSetColor {
        id: u32,
        color: String,
    },
    /// A link was dragged and dropped onto a split pane
    LinkDropped {
        url: String,
    },
    /// Session restore
    RestoreSession,
    DismissSession,
    /// Forget remembered per-site choices (permissions and zoom)
    ClearSiteData,
    /// How many cookies the page in view is sent; answered with `__siteCookies`
    SiteCookies,
    /// Delete the cookies and stored data of the page in view, then reload it
    ClearSiteCookies,
    /// Settings asks for the installed extensions
    ExtensionsQuery,
    /// Choose an unpacked extension folder and install it
    AddExtension,
    /// Install an extension from the Chrome Web Store, given its link or id
    InstallExtension {
        source: String,
    },
    RemoveExtension {
        id: String,
    },
    /// Put an extension on the toolbar or take it off
    SetExtensionPinned {
        id: String,
        pinned: bool,
    },
    /// Turn an installed extension on or off
    SetExtensionEnabled {
        id: String,
        enabled: bool,
    },
    /// The puzzle button: the list of extensions, to open one or manage them
    ExtensionsMenu {
        x: f64,
        y: f64,
    },
    /// An extension's toolbar button was clicked; `right` is the button's right edge (logical px)
    ToggleExtensionPopup {
        id: String,
        right: f64,
    },
    /// Settings asks for the list of remembered site choices
    SiteChoicesQuery,
    /// Forget one remembered choice: a permission name, "zoom", or `None` for the whole site
    ForgetSiteChoice {
        host: String,
        kind: Option<String>,
    },
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
    /// Open a page Byobu blocked as dangerous anyway (blocked-page notice only)
    ContinueBlockedPage {
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

fn default_page() -> usize {
    100
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
#[allow(clippy::large_enum_variant)]
pub enum HostToUiMessage {
    /// Full tab state update
    TabStateSync {
        tabs: Vec<TabState>,
        active_tab_id: Option<TabId>,
        #[serde(default)]
        groups: Vec<TabGroup>,
        /// The tabs shown side by side, in screen order, if split view is on
        #[serde(default)]
        split: Option<Vec<TabId>>,
        /// Detailed split state (orientation, ratio, panes) for the active workspace
        #[serde(default)]
        split_state: Option<SplitState>,
        /// Every split group of the active workspace (visible or not)
        #[serde(default)]
        splits: Vec<SplitState>,
        /// All workspaces in this window, in user order
        #[serde(default)]
        workspaces: Vec<Workspace>,
        /// Active workspace id in this window
        #[serde(default)]
        active_workspace_id: u32,
        /// Tabs in inactive workspaces (for the workspaces sidebar)
        #[serde(default)]
        inactive_tabs: Vec<TabState>,
        /// Containers tabs can be in, for their colour marks
        #[serde(default)]
        containers: Vec<crate::settings::Container>,
    },
    /// Side panel state
    SidebarStateSync {
        open: bool,
        mode: String,
        security_info: Option<Box<SecurityInfo>>,
    },
}
