//! Browser keyboard shortcuts.
//!
//! One table maps a virtual-key code plus modifiers to a [`Command`]. It is used both by the
//! WebView2 accelerator hook (keys pressed inside a webview) and by the winit window handler
//! (keys pressed while the frame itself has focus), so the two paths can never disagree.
//! Escape is deliberately *not* captured: web pages need it for dialogs and fullscreen.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    NewTab,
    ReopenClosedTab,
    NewWindow,
    CloseTab,
    FocusAddress,
    Find,
    Downloads,
    Print,
    Reload,
    HardReload,
    Back,
    Forward,
    NextTab,
    PrevTab,
    /// Ctrl+1..8 select a tab by position; Ctrl+9 selects the last tab (`usize::MAX`).
    SelectTab(usize),
    ZoomIn,
    ZoomOut,
    ZoomReset,
    DevTools,
    /// Ctrl+K: jump to any tab, favourite, setting or action
    Palette,
    /// Ctrl+D: add or remove the current page from favourites
    ToggleFavourite,
    /// F9: reader mode
    Reader,
    /// Ctrl+H: the history page
    History,
    /// Ctrl+Shift+Del: clear browsing data
    ClearData,
    /// Ctrl+Shift+O / Ctrl+Shift+B: favourites, through the palette
    Favourites,
    /// Ctrl+U: the page's source
    ViewSource,
    /// Ctrl+S: save the page as a file
    SavePage,
    /// F11: full screen
    FullScreen,
    /// Ctrl+, : settings
    Settings,
    /// Ctrl+Shift+A: search across tabs (the palette)
    TabSearch,
    /// Ctrl+PgDn / Ctrl+PgUp: next and previous tab
    NextTabPage,
    PrevTabPage,
    /// Alt+Home: the new tab page
    Home,
    /// F3 / Ctrl+G, with Shift for backwards: next find match
    FindNext,
    FindPrev,
    /// F6: focus the address bar
    FocusAddressF6,
}

const VK_TAB: u32 = 0x09;
const VK_LEFT: u32 = 0x25;
const VK_RIGHT: u32 = 0x27;
const VK_F5: u32 = 0x74;
const VK_F9: u32 = 0x78;
const VK_F12: u32 = 0x7B;
const VK_F3: u32 = 0x72;
const VK_F6: u32 = 0x75;
const VK_F11: u32 = 0x7A;
const VK_DELETE: u32 = 0x2E;
const VK_HOME: u32 = 0x24;
const VK_PRIOR: u32 = 0x21;
const VK_NEXT: u32 = 0x22;
const VK_OEM_COMMA: u32 = 0xBC;
const VK_ADD: u32 = 0x6B;
const VK_SUBTRACT: u32 = 0x6D;
const VK_NUMPAD0: u32 = 0x60;
const VK_OEM_PLUS: u32 = 0xBB;
const VK_OEM_MINUS: u32 = 0xBD;

/// Resolve a key press to a browser command.
pub fn command_for(vkey: u32, ctrl: bool, shift: bool, alt: bool) -> Option<Command> {
    use Command::*;
    let ch = char::from_u32(vkey).filter(|c| c.is_ascii_alphanumeric());
    match (ctrl, alt) {
        (true, false) => Some(match (ch, vkey) {
            (_, VK_DELETE) if shift => ClearData,
            (Some('H'), _) => History,
            (Some('O' | 'B'), _) if shift => Favourites,
            (Some('A'), _) if shift => TabSearch,
            (Some('U'), _) => ViewSource,
            (_, VK_OEM_COMMA) => Settings,
            (_, VK_NEXT) => NextTabPage,
            (_, VK_PRIOR) => PrevTabPage,
            (Some('T'), _) if shift => ReopenClosedTab,
            (Some('T'), _) => NewTab,
            (Some('N'), _) => NewWindow,
            (Some('W'), _) => CloseTab,
            (Some('L' | 'E'), _) => FocusAddress,
            (Some('K'), _) => Palette,
            (Some('D'), _) => ToggleFavourite,
            (Some('F'), _) => Find,
            (Some('J'), _) => Downloads,
            (Some('P'), _) => Print,
            (Some('R'), _) if shift => HardReload,
            (Some('R'), _) => Reload,
            (Some('0'), _) | (_, VK_NUMPAD0) => ZoomReset,
            (Some(d @ '1'..='8'), _) => SelectTab(d as usize - '1' as usize),
            (Some('9'), _) => SelectTab(usize::MAX),
            (_, VK_TAB) if shift => PrevTab,
            (_, VK_TAB) => NextTab,
            (_, VK_OEM_PLUS | VK_ADD) => ZoomIn,
            (_, VK_OEM_MINUS | VK_SUBTRACT) => ZoomOut,
            (_, VK_F5) => HardReload,
            _ => return None,
        }),
        (false, true) => match vkey {
            VK_LEFT => Some(Back),
            VK_RIGHT => Some(Forward),
            VK_HOME => Some(Home),
            _ => None,
        },
        (false, false) => match vkey {
            VK_F5 if shift => Some(HardReload),
            VK_F5 => Some(Reload),
            VK_F9 => Some(Reader),
            VK_F3 if shift => Some(FindPrev),
            VK_F3 => Some(FindNext),
            VK_F6 => Some(FocusAddressF6),
            VK_F11 => Some(FullScreen),
            VK_F12 => Some(DevTools),
            _ => None,
        },
        (true, true) => None,
    }
}

/// Translate a winit key into the virtual-key code understood by [`command_for`].
pub fn vkey_from_winit(key: &winit::keyboard::Key) -> Option<u32> {
    use winit::keyboard::{Key, NamedKey};
    match key {
        Key::Character(s) => match s.as_str() {
            "+" | "=" => Some(VK_OEM_PLUS),
            "-" | "_" => Some(VK_OEM_MINUS),
            "," | "<" => Some(VK_OEM_COMMA),
            s => {
                let c = s.chars().next()?.to_ascii_uppercase();
                c.is_ascii_alphanumeric().then_some(c as u32)
            }
        },
        Key::Named(NamedKey::Tab) => Some(VK_TAB),
        Key::Named(NamedKey::ArrowLeft) => Some(VK_LEFT),
        Key::Named(NamedKey::ArrowRight) => Some(VK_RIGHT),
        Key::Named(NamedKey::F5) => Some(VK_F5),
        Key::Named(NamedKey::F9) => Some(VK_F9),
        Key::Named(NamedKey::F3) => Some(VK_F3),
        Key::Named(NamedKey::F6) => Some(VK_F6),
        Key::Named(NamedKey::F11) => Some(VK_F11),
        Key::Named(NamedKey::Delete) => Some(VK_DELETE),
        Key::Named(NamedKey::Home) => Some(VK_HOME),
        Key::Named(NamedKey::PageUp) => Some(VK_PRIOR),
        Key::Named(NamedKey::PageDown) => Some(VK_NEXT),
        Key::Named(NamedKey::F12) => Some(VK_F12),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_shortcuts() {
        assert_eq!(
            command_for('T' as u32, true, false, false),
            Some(Command::NewTab)
        );
        assert_eq!(
            command_for('T' as u32, true, true, false),
            Some(Command::ReopenClosedTab)
        );
        assert_eq!(
            command_for('3' as u32, true, false, false),
            Some(Command::SelectTab(2))
        );
        assert_eq!(
            command_for('9' as u32, true, false, false),
            Some(Command::SelectTab(usize::MAX))
        );
        assert_eq!(
            command_for(VK_TAB, true, true, false),
            Some(Command::PrevTab)
        );
        assert_eq!(
            command_for(VK_LEFT, false, false, true),
            Some(Command::Back)
        );
        assert_eq!(
            command_for(VK_F12, false, false, false),
            Some(Command::DevTools)
        );
        assert_eq!(
            command_for('K' as u32, true, false, false),
            Some(Command::Palette)
        );
        assert_eq!(
            command_for('L' as u32, true, false, false),
            Some(Command::FocusAddress)
        );
        assert_eq!(
            command_for('D' as u32, true, false, false),
            Some(Command::ToggleFavourite)
        );
        assert_eq!(
            command_for(VK_F9, false, false, false),
            Some(Command::Reader)
        );
        assert_eq!(
            command_for('H' as u32, true, false, false),
            Some(Command::History)
        );
        assert_eq!(
            command_for(VK_DELETE, true, true, false),
            Some(Command::ClearData)
        );
        assert_eq!(
            command_for('U' as u32, true, false, false),
            Some(Command::ViewSource)
        );
        // Web apps own Ctrl+S and Ctrl+G; they reach the page.
        assert_eq!(command_for('S' as u32, true, false, false), None);
        assert_eq!(command_for('G' as u32, true, false, false), None);
        assert_eq!(
            command_for(VK_F11, false, false, false),
            Some(Command::FullScreen)
        );
        assert_eq!(
            command_for(VK_F3, false, true, false),
            Some(Command::FindPrev)
        );
        assert_eq!(
            command_for(VK_NEXT, true, false, false),
            Some(Command::NextTabPage)
        );
        assert_eq!(
            command_for(VK_OEM_COMMA, true, false, false),
            Some(Command::Settings)
        );
        assert_eq!(
            command_for(VK_HOME, false, false, true),
            Some(Command::Home)
        );
        // Plain typing and Escape pass through to the page.
        assert_eq!(command_for('T' as u32, false, false, false), None);
        assert_eq!(command_for(0x1B, false, false, false), None);
    }
}
