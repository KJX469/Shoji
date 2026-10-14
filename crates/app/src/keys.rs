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
    /// Ctrl+Shift+S: enter or exit split view
    ToggleSplit,
    /// Alt+Shift+S: flip split divider between vertical and horizontal
    ToggleSplitOrientation,
    /// Alt+S: move focus to the other split pane
    SwitchSplitPane,
    /// Alt+W: open or close the workspaces side panel
    Workspaces,
    /// Alt+Shift+N: create a new workspace
    NewWorkspace,
    /// Alt+1..8 select a workspace by position; Alt+9 selects the last workspace (`usize::MAX`).
    SelectWorkspace(usize),
}

const VK_TAB: u32 = 0x09;
const VK_LEFT: u32 = 0x25;
const VK_RIGHT: u32 = 0x27;
const VK_F5: u32 = 0x74;
const VK_F9: u32 = 0x78;
const VK_F12: u32 = 0x7B;
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
            (Some('T'), _) if shift => ReopenClosedTab,
            (Some('T'), _) => NewTab,
            (Some('N'), _) => NewWindow,
            (Some('W'), _) => CloseTab,
            (Some('S'), _) if shift => ToggleSplit,
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
        (false, true) => match (ch, vkey) {
            (_, VK_LEFT) => Some(Back),
            (_, VK_RIGHT) => Some(Forward),
            (Some('S'), _) if shift => Some(ToggleSplitOrientation),
            (Some('S'), _) => Some(SwitchSplitPane),
            (Some('W'), _) if !shift => Some(Workspaces),
            (Some('N'), _) if shift => Some(NewWorkspace),
            (Some(d @ '1'..='8'), _) if !shift => Some(SelectWorkspace(d as usize - '1' as usize)),
            (Some('9'), _) if !shift => Some(SelectWorkspace(usize::MAX)),
            _ => None,
        },
        (false, false) => match vkey {
            VK_F5 if shift => Some(HardReload),
            VK_F5 => Some(Reload),
            VK_F9 => Some(Reader),
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
            command_for('S' as u32, true, true, false),
            Some(Command::ToggleSplit)
        );
        assert_eq!(
            command_for('S' as u32, false, true, true),
            Some(Command::ToggleSplitOrientation)
        );
        assert_eq!(
            command_for('S' as u32, false, false, true),
            Some(Command::SwitchSplitPane)
        );
        assert_eq!(
            command_for('W' as u32, false, false, true),
            Some(Command::Workspaces)
        );
        assert_eq!(
            command_for('N' as u32, false, true, true),
            Some(Command::NewWorkspace)
        );
        assert_eq!(
            command_for('2' as u32, false, false, true),
            Some(Command::SelectWorkspace(1))
        );
        assert_eq!(
            command_for('9' as u32, false, false, true),
            Some(Command::SelectWorkspace(usize::MAX))
        );
        // Plain typing and Escape pass through to the page.
        assert_eq!(command_for('T' as u32, false, false, false), None);
        assert_eq!(command_for(0x1B, false, false, false), None);
    }
}
