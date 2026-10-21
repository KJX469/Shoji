//! Dark Reader's per-site fixes (`dynamic-theme-fixes.config`): the selectors to leave alone and
//! the CSS that makes sites such as YouTube, Google or GitHub come out right. The config is
//! parsed once, on first use, and only the fix for the page being opened is sent to it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::NavigationStartingEventHandler;
use windows::core::{PCWSTR, PWSTR};
use wry::{WebView, WebViewExtWindows};

const CONFIG: &str = include_str!("../ui/vendor/dark-fixes.config");

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Whether pages are being darkened (nothing is looked up or injected while it is off).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Should this address be shown dark by Shoji (so the page is asked for its light version)?
pub type DarkFor = Arc<dyn Fn(&str) -> bool + Send + Sync>;

const LIGHT: &str = r#"{"features":[{"name":"prefers-color-scheme","value":"light"}]}"#;
const NO_EMULATION: &str = r#"{"features":[]}"#;

fn emulate_core(core: &ICoreWebView2, light: bool) {
    let method: Vec<u16> = "Emulation.setEmulatedMedia"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let params: Vec<u16> = (if light { LIGHT } else { NO_EMULATION })
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let _ = unsafe {
        core.CallDevToolsProtocolMethod(PCWSTR(method.as_ptr()), PCWSTR(params.as_ptr()), None)
    };
}

/// Whether a tab is currently told to report a light colour scheme. Shared by [`emulate`] and
/// the navigation handler from [`attach`], so neither acts on a stale idea of the other's work.
pub type Emulated = Arc<AtomicBool>;

/// Make an open tab report a light colour scheme (or the system's again). Websites that follow
/// the system theme (YouTube, GitHub...) then serve their light look, which Dark Reader darkens
/// completely and with the site's fixes, instead of a native dark look it can only tint.
pub fn emulate(view: &WebView, state: &Emulated, light: bool) {
    if let Ok(core) = unsafe { view.controller().CoreWebView2() } {
        state.store(light, Ordering::Relaxed);
        emulate_core(&core, light);
    }
}

/// Keep a tab's reported colour scheme right as it navigates: light for pages that are
/// darkened, the system's for everything else (including Shoji's own pages).
pub fn attach(view: &WebView, dark_for: DarkFor) -> Emulated {
    let state = Emulated::default();
    let Ok(core) = (unsafe { view.controller().CoreWebView2() }) else {
        return state;
    };
    let core_nav = core.clone();
    let last = state.clone();
    let handler = NavigationStartingEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let mut uri = PWSTR::null();
        unsafe { args.Uri(&mut uri)? };
        let url = unsafe { uri.to_string().unwrap_or_default() };
        unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(uri.0 as *const _)) };
        let web = (url.starts_with("http://") || url.starts_with("https://"))
            && !url.starts_with("http://shoji.");
        let want = ENABLED.load(Ordering::Relaxed) && web && dark_for(&url);
        if want != last.swap(want, Ordering::Relaxed) {
            emulate_core(&core_nav, want);
        }
        Ok(())
    }));
    let mut token = Default::default();
    let _ = unsafe { core.add_NavigationStarting(&handler, &mut token) };
    state
}

#[derive(Default)]
struct Fix {
    urls: Vec<&'static str>,
    invert: Vec<&'static str>,
    css: &'static str,
    ignore_inline: Vec<&'static str>,
    ignore_image: Vec<&'static str>,
    ignore_css_url: Vec<&'static str>,
}

fn is_command(line: &str) -> bool {
    let words: Vec<&str> = line.split(' ').collect();
    (1..=3).contains(&words.len())
        && words
            .iter()
            .all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_uppercase()))
}

fn lines_of(text: &'static str) -> Vec<&'static str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect()
}

fn parse() -> Vec<Fix> {
    let mut fixes = Vec::new();
    let mut block_start = 0;
    let mut blocks: Vec<&'static str> = Vec::new();
    // Blocks are separated by lines of "=" characters.
    let mut offset = 0;
    for line in CONFIG.split_inclusive('\n') {
        let t = line.trim();
        if t.len() >= 2 && t.bytes().all(|b| b == b'=') {
            blocks.push(&CONFIG[block_start..offset]);
            block_start = offset + line.len();
        }
        offset += line.len();
    }
    blocks.push(&CONFIG[block_start..]);
    for block in blocks {
        // (start of the line, start of the line after it, command)
        let mut commands: Vec<(usize, usize, &str)> = Vec::new();
        let mut at = 0;
        for line in block.split_inclusive('\n') {
            let t = line.trim_end_matches(['\r', '\n']);
            if is_command(t) {
                commands.push((at, at + line.len(), t));
            }
            at += line.len();
        }
        let Some(first) = commands.first() else {
            continue;
        };
        let mut fix = Fix {
            urls: lines_of(&block[..first.0]),
            ..Fix::default()
        };
        for (i, (_, value_start, name)) in commands.iter().enumerate() {
            let end = commands.get(i + 1).map_or(block.len(), |c| c.0);
            let value = &block[*value_start..end];
            match *name {
                "INVERT" => fix.invert = lines_of(value),
                "CSS" => fix.css = value.trim(),
                "IGNORE INLINE STYLE" => fix.ignore_inline = lines_of(value),
                "IGNORE IMAGE ANALYSIS" => fix.ignore_image = lines_of(value),
                "IGNORE CSS URL" => fix.ignore_css_url = lines_of(value),
                _ => {}
            }
        }
        if !fix.urls.is_empty() {
            fixes.push(fix);
        }
    }
    fixes
}

fn fixes() -> &'static [Fix] {
    static FIXES: OnceLock<Vec<Fix>> = OnceLock::new();
    FIXES.get_or_init(parse)
}

/// `*` matches any run of characters.
fn glob(pattern: &str, text: &str) -> bool {
    let (p, t) = (pattern.as_bytes(), text.as_bytes());
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            mark += 1;
            ti = mark;
            pi = s + 1;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

fn host_matches(host: &str, pattern: &str) -> bool {
    let pattern = pattern.strip_prefix("www.").unwrap_or(pattern);
    if pattern.contains('*') {
        glob(pattern, host)
            || pattern
                .strip_prefix("*.")
                .is_some_and(|rest| glob(rest, host))
    } else {
        host == pattern || host.strip_suffix(pattern).is_some_and(|r| r.ends_with('.'))
    }
}

fn url_matches(host: &str, path: &str, pattern: &str) -> bool {
    if pattern.starts_with('/') {
        return false; // regular expressions are not supported here
    }
    let pattern = pattern
        .strip_prefix("https://")
        .or_else(|| pattern.strip_prefix("http://"))
        .unwrap_or(pattern);
    let (phost, ppath) = pattern.split_once('/').unwrap_or((pattern, ""));
    let phost = phost.split(':').next().unwrap_or(phost);
    if !host_matches(host, phost) {
        return false;
    }
    ppath.is_empty() || glob(&format!("/{ppath}*"), path)
}

/// A script that hands the page its site fix as `window.__shojiFix`, or `None` when darkening is
/// off or the site needs no fix of its own (the page then uses an empty one).
pub fn script_for(url: &str) -> Option<String> {
    if !ENABLED.load(Ordering::Relaxed) {
        return None;
    }
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (authority, path) = rest
        .find(['/', '?', '#'])
        .map_or((rest, ""), |i| rest.split_at(i));
    let host = authority
        .rsplit('@')
        .next()?
        .split(':')
        .next()?
        .to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let path = path.split(['?', '#']).next().unwrap_or("");
    let path = if path.is_empty() { "/" } else { path };
    let all = fixes();
    let common = all.first().filter(|f| f.urls.first() == Some(&"*"))?;
    // The most specific site entry wins (as in Dark Reader, by the length of its first address).
    let site = all[1..]
        .iter()
        .filter(|f| f.urls.iter().any(|p| url_matches(host, path, p)))
        .max_by_key(|f| f.urls[0].len())?;
    let join = |a: &[&str], b: &[&str]| -> Vec<String> {
        a.iter().chain(b).map(|s| s.to_string()).collect()
    };
    let css = [common.css, site.css]
        .iter()
        .filter(|c| !c.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    let fix = serde_json::json!({
        "url": [],
        "invert": join(&common.invert, &site.invert),
        "css": css,
        "ignoreInlineStyle": join(&common.ignore_inline, &site.ignore_inline),
        "ignoreImageAnalysis": join(&common.ignore_image, &site.ignore_image),
        "ignoreCSSUrl": join(&common.ignore_css_url, &site.ignore_css_url),
        "disableStyleSheetsProxy": false,
        "disableCustomElementRegistryProxy": false,
    });
    Some(format!("window.__shojiFix={fix};"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_has_a_fix_and_unknown_sites_use_the_common_one() {
        set_enabled(true);
        let s = script_for("https://www.youtube.com/watch?v=1").unwrap();
        assert!(s.contains("--yt-"));
        assert!(script_for("https://no-such-site.invalid/").is_none());
    }

    #[test]
    fn matching() {
        assert!(url_matches("m.youtube.com", "/", "youtube.com"));
        assert!(!url_matches("notyoutube.com", "/", "youtube.com"));
        assert!(url_matches("google.co.uk", "/maps/x", "google.*/maps"));
        assert!(url_matches("a.euronews.com", "/", "*.euronews.com"));
    }
}
