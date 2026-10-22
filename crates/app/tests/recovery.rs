//! A page that stops responding, and an engine that goes away.

#![cfg(windows)]

mod common;

use common::*;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetWindowThreadProcessId, PostMessageW,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
};

/// The page areas (render windows) inside the windows of process `pid`.
fn render_windows(pid: u32) -> Vec<HWND> {
    struct Find {
        pid: u32,
        found: Vec<HWND>,
    }
    unsafe extern "system" fn child(hwnd: HWND, l: LPARAM) -> windows::core::BOOL {
        let f = unsafe { &mut *(l.0 as *mut Find) };
        let mut name = [0u16; 64];
        let n = unsafe { GetClassNameW(hwnd, &mut name) } as usize;
        if String::from_utf16_lossy(&name[..n]) == "Chrome_RenderWidgetHostHWND" {
            f.found.push(hwnd);
        }
        true.into()
    }
    unsafe extern "system" fn top(hwnd: HWND, l: LPARAM) -> windows::core::BOOL {
        let f = unsafe { &mut *(l.0 as *mut Find) };
        let mut owner = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
        if owner == f.pid {
            let _ = unsafe { EnumChildWindows(Some(hwnd), Some(child), l) };
        }
        true.into()
    }
    let mut f = Find {
        pid,
        found: Vec::new(),
    };
    let _ = unsafe { EnumWindows(Some(top), LPARAM(&mut f as *mut Find as isize)) };
    f.found
}

#[test]
fn a_hung_page_offers_wait_or_close() {
    if !can_run() {
        return;
    }
    let server = Server::start(|hit| match hit.path() {
        "/" => Reply::html(
            "<title>Busy page</title><p>busy</p>\
             <script>setTimeout(()=>{for(;;){}},1500)</script>",
        ),
        _ => Reply::empty(),
    });
    let url = server.url("busy.test", "/");
    let shoji = Shoji::start("hung", &url, Options::default());
    let _ = shoji.page("busy.test");
    std::thread::sleep(Duration::from_secs(3));
    let mut chrome = shoji.chrome();
    // The engine notices a hang when the page doesn't answer input; give it some, without
    // moving the real pointer.
    let end = Instant::now() + Duration::from_secs(90);
    let mut offered = false;
    while Instant::now() < end {
        for hwnd in render_windows(shoji.pid()) {
            let at = LPARAM((40 << 16) | 40);
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_MOUSEMOVE, WPARAM(0), at);
                let _ = PostMessageW(Some(hwnd), WM_LBUTTONDOWN, WPARAM(1), at);
                let _ = PostMessageW(Some(hwnd), WM_LBUTTONUP, WPARAM(0), at);
            }
        }
        let title = chrome.eval("(document.getElementById('toastTitle')||{}).textContent||''");
        if title.as_str().unwrap_or("").contains("isn't responding") {
            offered = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    assert!(offered, "no wait-or-close card for the hung page");
    let buttons = chrome.eval(
        "[...document.querySelectorAll('#toastActs button')].map(b=>b.textContent).join('|')",
    );
    assert_eq!(buttons, "Wait|Close page");
    // Close page closes the tab; the window keeps going.
    chrome.eval("[...document.querySelectorAll('#toastActs button')][1].click()");
    let gone = Instant::now() + Duration::from_secs(10);
    while Instant::now() < gone
        && shoji
            .targets()
            .iter()
            .any(|t| t["url"].as_str().unwrap_or("").contains("busy.test"))
    {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        !shoji
            .targets()
            .iter()
            .any(|t| t["url"].as_str().unwrap_or("").contains("busy.test")),
        "the hung tab is still open"
    );
    // It was the window's only tab, so the window may close with it; nothing crashed.
    let crashes = std::fs::read_dir(shoji.data_dir().join("crashes"))
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(
        crashes,
        0,
        "Shoji crashed
{}",
        shoji.report()
    );
}

/// The engine's browser process: the `msedgewebview2.exe` that Shoji started itself.
fn engine_pid(shoji: u32) -> Option<u32> {
    let query = format!(
        "(Get-CimInstance Win32_Process -Filter \"ParentProcessId={shoji} AND Name='msedgewebview2.exe'\" | Select-Object -First 1).ProcessId"
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &query])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

#[test]
fn tabs_come_back_when_the_engine_goes_away() {
    if !can_run() {
        return;
    }
    let server = Server::start(|hit| match hit.path() {
        "/" => Reply::html("<title>Kept page</title><p>kept</p>"),
        _ => Reply::empty(),
    });
    let url = server.url("kept.test", "/story?n=1");
    let shoji = Shoji::start("engine-exit", &url, Options::default());
    let _ = shoji.page("kept.test");
    let engine = engine_pid(shoji.pid()).expect("engine process");
    let _ = std::process::Command::new("taskkill")
        .args(["/T", "/F", "/PID", &engine.to_string()])
        .output();
    // A new engine starts and the tab opens again where it was.
    let end = Instant::now() + Duration::from_secs(60);
    let mut back = false;
    while Instant::now() < end {
        if engine_pid(shoji.pid()).is_some_and(|p| p != engine)
            && shoji
                .targets()
                .iter()
                .any(|t| t["url"].as_str().unwrap_or("").contains("kept.test:"))
        {
            back = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    assert!(back, "the tab didn't come back\n{}", shoji.report());
    assert!(shoji.is_running());
}
