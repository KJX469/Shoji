//! Shoji setup and standalone uninstaller.
//!
//! Per-user install into `%LOCALAPPDATA%\Programs\Shoji`, no elevation. Shortcuts and the
//! Add/Remove Programs entry are written through Win32/COM directly — no PowerShell.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod installer_ui;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Icon, Window, WindowAttributes, WindowId};
use wry::WebViewBuilder;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const APP_NAME: &str = shoji_core::APP_NAME;
const PUBLISHER: &str = "Shoji Contributors";
const EXE_NAME: &str = "shoji.exe";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Shoji";
const WIDTH: f64 = 480.0;
const HEIGHT: f64 = 520.0;

const BROWSER_PAYLOAD: &[u8] = include_bytes!("../assets/shoji.exe");

fn install_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Programs")
        .join(APP_NAME)
}

/// Ask a running Shoji to save its tabs and quit. It may be waiting in the tray with no window,
/// so the installed copy is started with `--quit`, which hands the request to the running one.
fn close_running_copy(exe: &Path) {
    if !exe.exists() {
        return;
    }
    let mut cmd = Command::new(exe);
    cmd.arg("--quit");
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    if cmd.status().is_ok() {
        // Give it a moment to exit and release its files.
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
}

fn data_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(shoji_core::DATA_DIR_NAME))
}

#[cfg(windows)]
mod registry {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::*;

    fn wide(s: &str) -> Vec<u8> {
        s.encode_utf16()
            .chain(Some(0))
            .flat_map(|u| u.to_le_bytes())
            .collect()
    }

    pub fn write_uninstall_entry(key: &str, values: &[(&str, &str)]) -> windows::core::Result<()> {
        unsafe {
            let mut hkey = HKEY::default();
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                &HSTRING::from(key),
                None,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut hkey,
                None,
            )
            .ok()?;
            for (name, value) in values {
                let _ = RegSetValueExW(
                    hkey,
                    &HSTRING::from(*name),
                    None,
                    REG_SZ,
                    Some(&wide(value)),
                );
            }
            for name in ["NoModify", "NoRepair"] {
                let _ = RegSetValueExW(
                    hkey,
                    &HSTRING::from(name),
                    None,
                    REG_DWORD,
                    Some(&1u32.to_le_bytes()),
                );
            }
            let _ = RegCloseKey(hkey);
        }
        Ok(())
    }

    pub fn remove_uninstall_entry(key: &str) {
        unsafe {
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key));
        }
    }
}

#[derive(Debug)]
enum InstallerEvent {
    Progress(u32, &'static str),
    InstallComplete,
    InstallFailed(String),
    UninstallComplete,
    Exit,
}

struct InstallerApp {
    is_uninstall: bool,
    /// Remove everything, including settings and notes, without asking again
    purge: bool,
    proxy: EventLoopProxy<InstallerEvent>,
    window: Option<Arc<Window>>,
    webview: Option<wry::WebView>,
    busy: Arc<AtomicBool>,
}

impl ApplicationHandler<InstallerEvent> for InstallerApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let icon = Icon::from_rgba(shoji_brand::render_rgba(Default::default(), 64), 64, 64).ok();
        let attrs = WindowAttributes::default()
            .with_title(if self.is_uninstall {
                "Uninstall Shoji"
            } else {
                "Shoji Setup"
            })
            .with_inner_size(LogicalSize::new(WIDTH, HEIGHT))
            .with_resizable(false)
            .with_window_icon(icon);
        let Ok(window) = event_loop.create_window(attrs).map(Arc::new) else {
            return event_loop.exit();
        };
        if let Some(monitor) = window.current_monitor() {
            let scale = monitor.scale_factor();
            let size = monitor.size().to_logical::<f64>(scale);
            window.set_outer_position(LogicalPosition::new(
                ((size.width - WIDTH) / 2.0).max(0.0),
                ((size.height - HEIGHT) / 2.0).max(0.0),
            ));
        }

        let proxy = self.proxy.clone();
        let busy = self.busy.clone();
        let uninstall_mode = self.is_uninstall;
        let webview = WebViewBuilder::new()
            .with_html(installer_ui::installer_html(self.is_uninstall, self.purge))
            .with_incognito(true)
            .with_ipc_handler(move |req: wry::http::Request<String>| {
                let Ok(val) = serde_json::from_str::<serde_json::Value>(req.body()) else {
                    return;
                };
                match val["action"].as_str().unwrap_or("") {
                    "cancel" => {
                        let _ = proxy.send_event(InstallerEvent::Exit);
                    }
                    "accept" if !uninstall_mode && !busy.swap(true, Ordering::SeqCst) => {
                        let desktop = val["desktop"].as_bool().unwrap_or(true);
                        let start_menu = val["start_menu"].as_bool().unwrap_or(true);
                        let proxy = proxy.clone();
                        std::thread::spawn(move || install(desktop, start_menu, proxy));
                    }
                    "start_uninstall" if uninstall_mode && !busy.swap(true, Ordering::SeqCst) => {
                        let remove_data = val["remove_data"].as_bool().unwrap_or(false);
                        let proxy = proxy.clone();
                        std::thread::spawn(move || uninstall(remove_data, proxy));
                    }
                    "finish" => {
                        let exe = install_dir().join(EXE_NAME);
                        if val["launch"].as_bool().unwrap_or(false) && exe.exists() {
                            let _ = Command::new(exe).spawn();
                        }
                        let _ = proxy.send_event(InstallerEvent::Exit);
                    }
                    _ => {}
                }
            })
            .build(window.as_ref());
        match webview {
            Ok(wv) => {
                self.webview = Some(wv);
                self.window = Some(window);
            }
            Err(e) => {
                eprintln!("Failed to start setup UI: {e}");
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: InstallerEvent) {
        if !matches!(event, InstallerEvent::Progress(..)) {
            self.busy.store(false, Ordering::SeqCst);
        }
        let script = match event {
            InstallerEvent::Progress(pct, text) => {
                format!("window.updateProgress({pct}, {text:?})")
            }
            InstallerEvent::InstallComplete => "window.installComplete()".into(),
            InstallerEvent::InstallFailed(text) => format!(
                "window.installFailed({})",
                serde_json::to_string(&text).unwrap_or_default()
            ),
            InstallerEvent::UninstallComplete => "window.uninstallComplete()".into(),
            InstallerEvent::Exit => return event_loop.exit(),
        };
        if let Some(wv) = &self.webview {
            let _ = wv.evaluate_script(&script);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let WindowEvent::CloseRequested = event {
            if !self.busy.load(Ordering::SeqCst) {
                event_loop.exit();
            }
        }
    }
}

fn install(desktop: bool, start_menu: bool, proxy: EventLoopProxy<InstallerEvent>) {
    let result = install_files(desktop, start_menu, false, &|pct, text| {
        let _ = proxy.send_event(InstallerEvent::Progress(pct, text));
    });
    let _ = proxy.send_event(match result {
        Ok(()) => InstallerEvent::InstallComplete,
        Err(text) => InstallerEvent::InstallFailed(text),
    });
}

/// `ShojiSetup.exe --update`: the browser starts this (from its tray) after downloading a newer
/// installer. No window. Replaces the files and registration, leaves shortcuts alone, and starts
/// the new version in the tray. Shoji is started again even if replacing failed.
fn run_update() -> ! {
    let ok = install_files(false, false, true, &|_, _| {}).is_ok();
    let exe = install_dir().join(EXE_NAME);
    if exe.exists() {
        let mut cmd = Command::new(exe);
        cmd.arg("--background");
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        let _ = cmd.spawn();
    }
    std::process::exit(if ok { 0 } else { 1 });
}

/// Copy Shoji into place and register it. With `update`, existing shortcuts are left as they
/// are (they already point at the install folder).
fn install_files(
    desktop: bool,
    start_menu: bool,
    update: bool,
    progress: &dyn Fn(u32, &'static str),
) -> Result<(), String> {
    let dir = install_dir();
    progress(5, "Closing Shoji");
    close_running_copy(&dir.join(EXE_NAME));
    progress(10, "Copying files");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let exe = dir.join(EXE_NAME);
    // The old copy may take a moment to exit and release the file.
    let mut last = Ok(());
    for _ in 0..20 {
        last = std::fs::write(&exe, BROWSER_PAYLOAD);
        if last.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    if let Err(e) = last {
        return Err(format!(
            "Couldn't write {EXE_NAME} ({e}). If Shoji is open, close it and try again."
        ));
    }
    let uninstaller = dir.join("uninstall.exe");
    if let Ok(me) = std::env::current_exe() {
        let _ = std::fs::copy(me, &uninstaller);
    }

    progress(60, "Adding shortcuts");
    #[cfg(windows)]
    {
        shoji_core::shell::init_com();
        let wanted = [
            (desktop && !update, shoji_core::shell::desktop_shortcut()),
            (
                start_menu && !update,
                shoji_core::shell::start_menu_shortcut(),
            ),
        ];
        for (_, lnk) in wanted.iter().filter(|(on, _)| *on) {
            if let Some(lnk) = lnk {
                let _ = shoji_core::shell::write_shortcut(lnk, &exe, None);
            }
        }
        shoji_core::shell::notify_icons_changed();

        progress(85, "Registering with Windows");
        let uninstall_cmd = format!("\"{}\" --uninstall", uninstaller.display());
        let _ = registry::write_uninstall_entry(
            UNINSTALL_KEY,
            &[
                ("DisplayName", APP_NAME),
                ("DisplayVersion", env!("CARGO_PKG_VERSION")),
                ("Publisher", PUBLISHER),
                ("DisplayIcon", &exe.to_string_lossy()),
                ("UninstallString", &uninstall_cmd),
                ("InstallLocation", &dir.to_string_lossy()),
            ],
        );
        // Lists Shoji among the web browsers in Settings → Default apps.
        let _ = shoji_core::shell::register_as_browser(&exe);
    }
    progress(100, "Done");
    Ok(())
}

/// Delete a folder, retrying briefly: the engine's helper processes can outlive the browser
/// by a moment and keep profile files open.
fn remove_dir_retrying(dir: &std::path::Path) {
    for _ in 0..8 {
        if !dir.exists() || std::fs::remove_dir_all(dir).is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
}

fn uninstall(remove_data: bool, proxy: EventLoopProxy<InstallerEvent>) {
    let progress = |pct, text| {
        let _ = proxy.send_event(InstallerEvent::Progress(pct, text));
    };
    let dir = install_dir();
    let exe = dir.join(EXE_NAME);

    progress(15, "Closing Shoji");
    close_running_copy(&exe);
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", EXE_NAME])
            .creation_flags(CREATE_NO_WINDOW)
            .output();

        progress(40, "Removing shortcuts");
        for lnk in shoji_core::shell::all_shortcuts() {
            let _ = std::fs::remove_file(lnk);
        }
        shoji_core::shell::notify_icons_changed();
        registry::remove_uninstall_entry(UNINSTALL_KEY);
        shoji_core::shell::unregister_browser();
    }

    progress(70, "Removing files");
    // Settings and notes under %APPDATA%, and the browsing profile (logins, cache, history)
    // under %LOCALAPPDATA%.
    let mut wipe: Vec<PathBuf> = Vec::new();
    if remove_data {
        wipe.extend(data_dir());
        wipe.extend(shoji_core::env::local_app_directory());
        for d in &wipe {
            remove_dir_retrying(d);
        }
    }
    let _ = std::fs::remove_file(exe);

    // This process is uninstall.exe inside the install folder: delete the folder once we exit,
    // and sweep anything the engine was still holding open a moment ago.
    #[cfg(windows)]
    {
        wipe.push(dir);
        if let Some(script) = shoji_core::env::delayed_purge_script(&wipe, &[]) {
            let _ = Command::new("cmd")
                .args(["/c", &script])
                .creation_flags(CREATE_NO_WINDOW)
                .spawn();
        }
    }
    progress(100, "Done");
    let _ = proxy.send_event(InstallerEvent::UninstallComplete);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let exe_name = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    let is_uninstall =
        std::env::args().any(|a| a == "--uninstall") || exe_name.contains("uninstall");

    if !is_uninstall && shoji_core::env::detect_webview2_runtime().is_none() {
        #[cfg(windows)]
        unsafe {
            use windows::core::w;
            use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
            MessageBoxW(
                None,
                w!("Shoji needs the Microsoft Edge WebView2 Runtime, which wasn't found.\n\nInstall it from https://developer.microsoft.com/microsoft-edge/webview2/ and run setup again."),
                w!("Shoji Setup"),
                MB_OK | MB_ICONERROR,
            );
        }
        std::process::exit(1);
    }

    if !is_uninstall && std::env::args().any(|a| a == "--update") {
        run_update();
    }

    let event_loop = EventLoop::<InstallerEvent>::with_user_event().build()?;
    let mut app = InstallerApp {
        is_uninstall,
        // Started from the browser's settings page, where the user already confirmed.
        purge: is_uninstall && std::env::args().any(|a| a == "--purge"),
        proxy: event_loop.create_proxy(),
        window: None,
        webview: None,
        busy: Arc::new(AtomicBool::new(false)),
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
