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
    shoji_core::env::install_directory()
        .unwrap_or_else(|| PathBuf::from(".").join("Programs").join(APP_NAME))
}

/// The engine folder for this window's own WebView2. It goes in the temp folder, never beside
/// the executable (WebView2's default), so setup leaves no `ShojiSetup.exe.WebView2` in the
/// Downloads folder and the uninstaller holds nothing open in the folders it deletes.
fn ui_data_dir() -> PathBuf {
    std::env::temp_dir().join(format!("shoji-setup-ui-{}", std::process::id()))
}

/// The uninstaller runs from a copy in the temp folder: a program cannot delete the folder it
/// is running from. Returns true when this process should just exit (the copy took over).
fn relaunch_from_temp() -> bool {
    let Ok(me) = std::env::current_exe() else {
        return false;
    };
    let inside = shoji_core::env::footprint_dirs()
        .iter()
        .any(|d| shoji_core::env::is_within(&me, d));
    if !inside {
        return false;
    }
    let copy = std::env::temp_dir().join(format!("shoji-uninstall-{}.exe", std::process::id()));
    if std::fs::copy(&me, &copy).is_err() {
        return false; // uninstall from here; the clean-up script finishes the folder
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    Command::new(&copy)
        .args(&args)
        .current_dir(std::env::temp_dir())
        .spawn()
        .is_ok()
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

/// End every program still running from `dir` (the browser in the tray, an old copy that was
/// moved aside during an update), and wait for each to go. Programs of the same name elsewhere,
/// such as a build run from a source folder, are left alone.
#[cfg(windows)]
fn stop_programs_in(dir: &Path) {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
        PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        PROCESS_TERMINATE,
    };
    let me = std::process::id();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut more = Process32FirstW(snap, &mut entry).is_ok();
        while more {
            let pid = entry.th32ProcessID;
            if pid != me && pid != 0 {
                let access =
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE;
                if let Ok(h) = OpenProcess(access, false, pid) {
                    let mut buf = [0u16; 1024];
                    let mut len = buf.len() as u32;
                    let path = QueryFullProcessImageNameW(
                        h,
                        PROCESS_NAME_WIN32,
                        windows::core::PWSTR(buf.as_mut_ptr()),
                        &mut len,
                    )
                    .ok()
                    .map(|_| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])));
                    if path.is_some_and(|p| shoji_core::env::is_within(&p, dir)) {
                        let _ = TerminateProcess(h, 0);
                        let _ = WaitForSingleObject(h, 5000);
                    }
                    let _ = CloseHandle(h);
                }
            }
            more = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(HANDLE(snap.0));
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
    web_context: wry::WebContext,
    busy: Arc<AtomicBool>,
}

/// Set once an uninstall has run: on exit, whatever was still in use is cleaned up.
static UNINSTALLED: AtomicBool = AtomicBool::new(false);

impl ApplicationHandler<InstallerEvent> for InstallerApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let icon = Icon::from_rgba(
            shoji_brand::render_kage_rgba(Default::default(), 64),
            64,
            64,
        )
        .ok();
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
        let webview = WebViewBuilder::new_with_web_context(&mut self.web_context)
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
    let result = install_files(desktop, start_menu, false, true, &|pct, text| {
        let _ = proxy.send_event(InstallerEvent::Progress(pct, text));
    });
    let _ = proxy.send_event(match result {
        Ok(()) => InstallerEvent::InstallComplete,
        Err(text) => InstallerEvent::InstallFailed(text),
    });
}

/// `ShojiSetup.exe --stage`: the browser starts this right after downloading a newer installer,
/// while it is running. No window and nothing is closed: the new files are put in place beside
/// the running ones (a running program can be renamed, just not overwritten), so the next start
/// is already the new version.
fn run_stage() -> ! {
    let ok = install_files(false, false, true, false, &|_, _| {}).is_ok();
    std::process::exit(if ok { 0 } else { 1 });
}

/// `ShojiSetup.exe --update`: the browser starts this (from its tray) once no window is open.
/// No window. Quits the running copy, finishes putting the new version in place (nothing to
/// copy when it was staged already), leaves shortcuts alone, and starts it in the tray. Shoji is
/// started again even if replacing failed. With `--reopen` ("Restart and update" in the browser)
/// it starts with a window and the tabs that were open, instead of in the tray.
fn run_update() -> ! {
    let ok = install_files(false, false, true, true, &|_, _| {}).is_ok();
    let exe = install_dir().join(EXE_NAME);
    if exe.exists() {
        let mut cmd = Command::new(exe);
        let reopen = std::env::args().any(|a| a == "--reopen");
        cmd.arg(if reopen {
            "--restore-session"
        } else {
            "--background"
        });
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        let _ = cmd.spawn();
    }
    std::process::exit(if ok { 0 } else { 1 });
}

/// Put the browser's executable in place without needing the running copy to exit. A running
/// program cannot be overwritten but it can be renamed, so the old file moves aside and the new
/// one takes its name. The old file is deleted later, once nothing is running from it.
fn swap_in_exe(dir: &Path) -> Result<(), String> {
    let exe = dir.join(EXE_NAME);
    if std::fs::read(&exe).is_ok_and(|b| b == BROWSER_PAYLOAD) {
        return Ok(()); // staged already
    }
    let new = dir.join("shoji.exe.new");
    std::fs::write(&new, BROWSER_PAYLOAD)
        .map_err(|e| format!("Couldn't write the new {EXE_NAME} ({e})"))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let aside = dir.join(format!("shoji.{stamp}.old"));
    if exe.exists() {
        std::fs::rename(&exe, &aside).map_err(|e| {
            let _ = std::fs::remove_file(&new);
            format!("Couldn't move the old {EXE_NAME} aside ({e})")
        })?;
    }
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&aside, &exe);
        let _ = std::fs::remove_file(&new);
        return Err(format!("Couldn't put the new {EXE_NAME} in place ({e})"));
    }
    Ok(())
}

/// Delete the files `swap_in_exe` moved aside, as far as nothing is running from them.
fn sweep_old(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.ends_with(".old") || name == "shoji.exe.new" {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Copy Shoji into place and register it. With `update`, existing shortcuts are left as they
/// are (they already point at the install folder). `quit_running` closes the running copy first.
fn install_files(
    desktop: bool,
    start_menu: bool,
    update: bool,
    quit_running: bool,
    progress: &dyn Fn(u32, &'static str),
) -> Result<(), String> {
    let dir = install_dir();
    if quit_running {
        progress(5, "Closing Shoji");
        close_running_copy(&dir.join(EXE_NAME));
    }
    progress(10, "Copying files");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let exe = dir.join(EXE_NAME);
    // Two tries, in case something briefly holds the file.
    let mut last = swap_in_exe(&dir);
    if last.is_err() {
        std::thread::sleep(std::time::Duration::from_millis(500));
        last = swap_in_exe(&dir);
    }
    if let Err(e) = last {
        return Err(format!("{e}. If Shoji is open, close it and try again."));
    }
    sweep_old(&dir);
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
    for _ in 0..20 {
        if !dir.exists() || std::fs::remove_dir_all(dir).is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
}

/// What uninstalling removes: the program, its shortcuts and Windows registrations always,
/// downloaded updates always, and with `remove_data` everything else Shoji stored (settings,
/// bookmarks, history, sign-ins, the engine profile), plus its scratch files in the temp folder.
fn uninstall_targets(remove_data: bool) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let install = install_dir();
    let mut dirs = vec![install.clone()];
    let mut files: Vec<PathBuf> = Vec::new();
    if remove_data {
        dirs.extend(
            shoji_core::env::footprint_dirs()
                .into_iter()
                .filter(|d| !shoji_core::env::is_within(d, &install)),
        );
    } else if let Some(d) = data_dir() {
        dirs.push(d.join("updates"));
    }
    for p in shoji_core::env::temp_leftovers() {
        if p.is_dir() {
            dirs.push(p);
        } else {
            files.push(p);
        }
    }
    #[cfg(windows)]
    files.extend(shoji_core::shell::all_shortcuts());
    (dirs, files)
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
        // Whatever did not quit when asked (only programs running from the install folder).
        stop_programs_in(&dir);

        progress(40, "Removing shortcuts");
        for lnk in shoji_core::shell::all_shortcuts() {
            let _ = std::fs::remove_file(lnk);
        }
        registry::remove_uninstall_entry(UNINSTALL_KEY);
        shoji_core::shell::remove_registry_traces(&dir);
        shoji_core::shell::notify_icons_changed();
    }

    progress(70, "Removing files");
    let (dirs, files) = uninstall_targets(remove_data);
    let own_ui = ui_data_dir();
    for d in dirs.iter().filter(|d| **d != own_ui) {
        remove_dir_retrying(d);
    }
    for f in &files {
        let _ = std::fs::remove_file(f);
    }
    UNINSTALLED.store(true, Ordering::SeqCst);
    // Anything still in use (the engine's helper processes can outlive the browser) goes once
    // it is free; `main` adds this window's own files when it exits.
    #[cfg(windows)]
    shoji_core::env::start_purge(&dirs, &files);
    progress(100, "Done");
    let _ = proxy.send_event(InstallerEvent::UninstallComplete);
}

/// On exit: delete this window's engine folder and, after an uninstall, this temporary copy
/// of the uninstaller and anything that was still in use.
fn clean_up_on_exit() {
    let mut dirs = vec![ui_data_dir()];
    let mut files = Vec::new();
    if UNINSTALLED.load(Ordering::SeqCst) {
        let (more_dirs, more_files) = uninstall_targets(false);
        dirs.extend(more_dirs);
        files.extend(more_files);
        if let Ok(me) = std::env::current_exe() {
            files.push(me);
        }
    }
    #[cfg(windows)]
    shoji_core::env::start_purge(&dirs, &files);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Started by the browser, this inherits its engine folder; never use (and lock) that.
    std::env::remove_var("WEBVIEW2_USER_DATA_FOLDER");
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

    if !is_uninstall && std::env::args().any(|a| a == "--stage") {
        run_stage();
    }
    if !is_uninstall && std::env::args().any(|a| a == "--update") {
        run_update();
    }

    if is_uninstall && relaunch_from_temp() {
        return Ok(());
    }

    let event_loop = EventLoop::<InstallerEvent>::with_user_event().build()?;
    let mut app = InstallerApp {
        is_uninstall,
        // Started from the browser's settings page, where the user already confirmed.
        purge: is_uninstall && std::env::args().any(|a| a == "--purge"),
        proxy: event_loop.create_proxy(),
        window: None,
        webview: None,
        web_context: wry::WebContext::new(Some(ui_data_dir())),
        busy: Arc::new(AtomicBool::new(false)),
    };
    let result = event_loop.run_app(&mut app);
    drop(app);
    clean_up_on_exit();
    result?;
    Ok(())
}
