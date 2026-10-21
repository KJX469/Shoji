//! Cookie notices answered with "Reject all" (see `ui/consent.js`). Every frame gets the
//! notice detectors once its document has loaded; a frame that finds a showing notice asks for
//! that notice's steps, and is answered with only those.

use shoji_core::tabs::TabId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::*;
use windows::core::{Interface, PCWSTR, PWSTR};
use wry::{WebView, WebViewExtWindows};

const ENGINE: &str = include_str!("../ui/consent.js");

/// Whether notices are answered on the page at an address (the user's settings, live).
pub type OnFor = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// The rules file as last read, split for the page: the detectors go to every frame, a notice's
/// steps only to the frame that shows it.
struct Rules {
    modified: Option<SystemTime>,
    script: Arc<String>,
    steps: HashMap<String, String>,
}

static PATH: OnceLock<PathBuf> = OnceLock::new();
static RULES: Mutex<Option<Rules>> = Mutex::new(None);

/// Where the updater keeps the rules (`Updater::consent_rules_path`).
pub fn init(path: PathBuf) {
    let _ = PATH.set(path);
}

/// Split Consent-O-Matic's rules (`{name: {detectors, methods}}`) into the detector script and
/// each notice's steps.
fn split(text: &str) -> Option<(String, HashMap<String, String>)> {
    let serde_json::Value::Object(all) = serde_json::from_str(text).ok()? else {
        return None;
    };
    let mut detectors = serde_json::Map::new();
    let mut steps = HashMap::new();
    for (name, rule) in all {
        let (Some(d), Some(m)) = (rule.get("detectors"), rule.get("methods")) else {
            continue;
        };
        if !d.is_array() || !m.is_array() || name.len() > 100 {
            continue;
        }
        detectors.insert(name.clone(), d.clone());
        steps.insert(name, m.to_string());
    }
    if detectors.is_empty() {
        return None;
    }
    let data = serde_json::Value::Object(detectors).to_string();
    // Parsed as JSON, which is quicker than the same data as a script literal.
    let literal = serde_json::to_string(&data).ok()?;
    let script = ENGINE.replace("__CONSENT_DETECTORS__", &format!("JSON.parse({literal})"));
    Some((script, steps))
}

/// The detector script, re-read when the file changes.
fn script() -> Option<Arc<String>> {
    let path: &Path = PATH.get()?;
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let mut rules = RULES.lock().ok()?;
    if rules.as_ref().map(|r| r.modified) != Some(modified) {
        let text = std::fs::read_to_string(path).ok();
        *rules = text
            .as_deref()
            .and_then(split)
            .map(|(script, steps)| Rules {
                modified,
                script: Arc::new(script),
                steps,
            });
        if rules.is_none() && modified.is_some() {
            eprintln!("consent: unreadable rules in {}", path.display());
        }
    }
    rules.as_ref().map(|r| r.script.clone())
}

/// The script that hands a notice's steps to the page.
fn steps_reply(name: &str) -> Option<String> {
    let rules = RULES.lock().ok()?;
    let steps = rules.as_ref()?.steps.get(name)?;
    let name = serde_json::to_string(name).ok()?;
    Some(format!(
        "window.__shojiConsent&&window.__shojiConsent.run({name},{steps})"
    ))
}

fn take(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as _)) };
    s
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn is_web(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://")) && !url.starts_with("http://shoji.")
}

/// What a page message asks for, if it is one of ours.
enum Ask<'a> {
    Steps(&'a str),
    Done,
}

fn ask(msg: &str) -> Option<Ask<'_>> {
    let rest = msg.strip_prefix("shoji:consent:")?;
    match rest.strip_prefix("cmp:") {
        Some(name) => Some(Ask::Steps(name)),
        None => (rest == "done").then_some(Ask::Done),
    }
}

struct Tab {
    core: ICoreWebView2,
    on_for: OnFor,
    tab: u64,
}

impl Tab {
    fn page(&self) -> String {
        let mut src = PWSTR::null();
        let _ = unsafe { self.core.Source(&mut src) };
        take(src)
    }

    /// The detectors, for a document at `url` inside the tab's current page.
    fn detectors_for(&self, url: &str) -> Option<Arc<String>> {
        let page = self.page();
        (is_web(url) && is_web(&page) && (self.on_for)(&page))
            .then(script)
            .flatten()
    }

    fn answer(&self, msg: &str, reply: impl FnOnce(&str)) {
        match ask(msg) {
            Some(Ask::Steps(name)) if (self.on_for)(&self.page()) => {
                if let Some(js) = steps_reply(name) {
                    reply(&js);
                }
            }
            Some(Ask::Done) => crate::shield::hub().note_notice(self.tab),
            _ => {}
        }
    }
}

fn run_in_frame(frame: &ICoreWebView2Frame2, js: &str) {
    let w = wide(js);
    let done = ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())));
    let _ = unsafe { frame.ExecuteScript(PCWSTR(w.as_ptr()), &done) };
}

/// Answer cookie notices in a tab's pages and their frames.
pub fn attach(webview: &WebView, tab_id: TabId, on_for: OnFor) {
    let Ok(core) = (unsafe { webview.controller().CoreWebView2() }) else {
        return;
    };
    let tab = Rc::new(Tab {
        core: core.clone(),
        on_for,
        tab: tab_id.0,
    });
    let mut token = Default::default();
    if let Ok(core2) = core.cast::<ICoreWebView2_2>() {
        let t = tab.clone();
        let handler = DOMContentLoadedEventHandler::create(Box::new(move |_, _| {
            let _doing = crate::crash::doing("consent page loaded");
            if let Some(js) = t.detectors_for(&t.page()) {
                let w = wide(&js);
                let done = ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())));
                let _ = unsafe { t.core.ExecuteScript(PCWSTR(w.as_ptr()), &done) };
            }
            Ok(())
        }));
        let _ = unsafe { core2.add_DOMContentLoaded(&handler, &mut token) };
    }
    {
        let t = tab.clone();
        let handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut msg = PWSTR::null();
            if unsafe { args.TryGetWebMessageAsString(&mut msg) }.is_err() {
                return Ok(());
            }
            let msg = take(msg);
            t.answer(&msg, |js| {
                let w = wide(js);
                let done = ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())));
                let _ = unsafe { t.core.ExecuteScript(PCWSTR(w.as_ptr()), &done) };
            });
            Ok(())
        }));
        let _ = unsafe { core.add_WebMessageReceived(&handler, &mut token) };
    }
    if let Ok(core4) = core.cast::<ICoreWebView2_4>() {
        let t = tab.clone();
        let handler = FrameCreatedEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                hook_frame(unsafe { args.Frame()? }, &t);
            }
            Ok(())
        }));
        let _ = unsafe { core4.add_FrameCreated(&handler, &mut token) };
    }
}

/// Notices shown in iframes (several consent platforms draw theirs in one).
fn hook_frame(frame: ICoreWebView2Frame, tab: &Rc<Tab>) {
    let Ok(frame2) = frame.cast::<ICoreWebView2Frame2>() else {
        return;
    };
    let mut token = Default::default();
    let url: Rc<RefCell<String>> = Rc::default();
    {
        let url = url.clone();
        let handler = FrameNavigationStartingEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                let mut uri = PWSTR::null();
                unsafe { args.Uri(&mut uri)? };
                *url.borrow_mut() = take(uri);
            }
            Ok(())
        }));
        let _ = unsafe { frame2.add_NavigationStarting(&handler, &mut token) };
    }
    {
        let (t, url) = (tab.clone(), url.clone());
        let handler = FrameDOMContentLoadedEventHandler::create(Box::new(move |sender, _| {
            let _doing = crate::crash::doing("consent frame loaded");
            let Some(sender) = sender else { return Ok(()) };
            let js = t.detectors_for(&url.borrow());
            if let (Some(js), Ok(f2)) = (js, sender.cast::<ICoreWebView2Frame2>()) {
                run_in_frame(&f2, &js);
            }
            Ok(())
        }));
        let _ = unsafe { frame2.add_DOMContentLoaded(&handler, &mut token) };
    }
    {
        let t = tab.clone();
        let handler = FrameWebMessageReceivedEventHandler::create(Box::new(move |sender, args| {
            let (Some(sender), Some(args)) = (sender, args) else {
                return Ok(());
            };
            let mut msg = PWSTR::null();
            if unsafe { args.TryGetWebMessageAsString(&mut msg) }.is_err() {
                return Ok(());
            }
            let msg = take(msg);
            t.answer(&msg, |js| {
                if let Ok(f2) = sender.cast::<ICoreWebView2Frame2>() {
                    run_in_frame(&f2, js);
                }
            });
            Ok(())
        }));
        let _ = unsafe { frame2.add_WebMessageReceived(&handler, &mut token) };
    }
    if let Ok(frame7) = frame.cast::<ICoreWebView2Frame7>() {
        let t = tab.clone();
        let handler = FrameChildFrameCreatedEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                hook_frame(unsafe { args.Frame()? }, &t);
            }
            Ok(())
        }));
        let _ = unsafe { frame7.add_FrameCreated(&handler, &mut token) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_split_into_detectors_and_steps() {
        let text = r##"{
            "$schema": "x",
            "acme": {"detectors": [{"presentMatcher": {"type": "css", "target": {"selector": "#acme"}}}],
                     "methods": [{"name": "SAVE_CONSENT", "action": {"type": "click", "target": {"selector": "#no"}}}]},
            "broken": {"detectors": {}}
        }"##;
        let (script, steps) = split(text).unwrap();
        assert!(script.contains("JSON.parse("));
        assert!(script.contains("#acme"));
        assert!(
            !script.contains("#no"),
            "steps only go to a frame that shows the notice"
        );
        assert_eq!(steps.len(), 1);
        assert!(steps["acme"].contains("SAVE_CONSENT"));
        assert!(split("[]").is_none());
        assert!(matches!(
            ask("shoji:consent:cmp:acme"),
            Some(Ask::Steps("acme"))
        ));
        assert!(matches!(ask("shoji:consent:done"), Some(Ask::Done)));
        assert!(ask("byobu:fp:canvas").is_none());
    }
}
