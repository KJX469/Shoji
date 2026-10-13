//! HTML for every browser-drawn surface: the chrome strip, the side panel, and the
//! internal pages (new tab, welcome, settings, crash and certificate notices).
//!
//! Templates live in `ui/*.html` and are filled in a single pass. All of them share the
//! theme stylesheet from `shoji-brand`, so the whole browser restyles by flipping one
//! `data-accent` attribute per document.

use shoji_brand::{theme, theme::TypeProfile, Accent};
use shoji_core::settings::Settings;
use std::sync::LazyLock;

const CHROME: &str = include_str!("../ui/chrome.html");
const SIDEBAR: &str = include_str!("../ui/sidebar.html");
const HOME: &str = include_str!("../ui/home.html");
const WELCOME: &str = include_str!("../ui/welcome.html");
const SETTINGS: &str = include_str!("../ui/settings.html");
const NOTICE: &str = include_str!("../ui/notice.html");
const BYOBU: &str = include_str!("../ui/byobu.html");
const NOTES: &str = include_str!("../ui/notes.html");
const OVERLAY: &str = include_str!("../ui/overlay.html");
const READER: &str = include_str!("../ui/reader.html");
/// Extracts the article from the current page (see the header of the file).
pub const READER_SCRIPT: &str = include_str!("../ui/reader.js");
/// Dark Reader (modified for Shoji; see third_party/darkreader/SHOJI.md), minified.
const DARK_READER: &str = include_str!("../ui/vendor/darkreader.min.js");

static THEME_CSS: LazyLock<String> = LazyLock::new(theme::css);

/// Replace every `{{KEY}}` in `template` in one pass. Unknown keys are left untouched.
fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + THEME_CSS.len() + 1024);
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let key = &after[..end];
                match vars.iter().find(|(k, _)| *k == key) {
                    Some((_, v)) => out.push_str(v),
                    None => out.push_str(&rest[start..start + 4 + end]),
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A JSON value safe to embed inside a `<script>` block.
fn script_json(value: serde_json::Result<String>) -> String {
    value
        .unwrap_or_else(|_| "null".into())
        .replace("</", "<\\/")
}

/// Toolbar behaviour flags read by the chrome strip.
pub fn prefs_json(s: &Settings) -> String {
    serde_json::json!({
        "zoomBadge": s.appearance.show_zoom_badge,
        "downloadsRing": s.downloads.show_progress_toolbar,
        "reorder": s.tabs.enable_tab_reordering,
        "tearoff": s.tabs.enable_tab_tearoff,
        "darkWebsites": s.appearance.dark_websites == Some(true),
        "darkExclude": s.appearance.dark_websites_exclude,
        "compactAddress": s.appearance.compact_address,
        "tintToolbar": s.appearance.tint_toolbar,
        "searchSuggestions": s.appearance.search_suggestions,
        "suggestUrl": suggest_url(s),
    })
    .to_string()
}

/// Where the chrome strip may fetch search suggestions from, or `null` when the option is off.
/// `%s` is the percent-encoded query; every endpoint answers with the OpenSearch JSON format.
fn suggest_url(s: &Settings) -> Option<&'static str> {
    if !s.appearance.search_suggestions {
        return None;
    }
    Some(match s.search_engine.to_lowercase().as_str() {
        "google" => "https://suggestqueries.google.com/complete/search?client=firefox&q=%s",
        "bing" => "https://api.bing.com/osjson.aspx?query=%s",
        "brave" => "https://search.brave.com/api/suggest?q=%s",
        "ecosia" => "https://ac.ecosia.org/autocomplete?type=list&q=%s",
        _ => "https://duckduckgo.com/ac/?type=list&q=%s",
    })
}

/// Script injected into every page: reports its colour and whether it holds a video, so the
/// toolbar can take on the page's tint and offer picture-in-picture. Only these two facts leave.
pub const PAGE_INFO_SCRIPT: &str = r#"(() => {
  if (window.top !== window || !window.ipc) return;
  let last = '', timer;
  const send = () => {
    let color = null;
    const m = document.querySelector('meta[name="theme-color"]');
    if (m && m.content) color = m.content;
    if (!color) {
      const el = document.body || document.documentElement;
      const bg = el ? getComputedStyle(el).backgroundColor : '';
      if (bg && !/rgba\(\s*0,\s*0,\s*0,\s*0\s*\)|transparent/.test(bg)) color = bg;
    }
    const has_video = !!document.querySelector('video');
    const key = color + '|' + has_video;
    if (key === last) return;
    last = key;
    window.ipc.postMessage(JSON.stringify({ action: 'PageInfo', payload: { color, has_video } }));
  };
  const later = () => { clearTimeout(timer); timer = setTimeout(send, 250); };
  addEventListener('DOMContentLoaded', later);
  addEventListener('load', () => { later(); setTimeout(send, 1500); });
  addEventListener('pageshow', later);
  addEventListener('play', later, true);
  document.addEventListener('visibilitychange', later);
})();"#;

fn dark_config(s: &Settings) -> String {
    serde_json::json!({
        "on": s.appearance.dark_websites == Some(true),
        "exclude": s.appearance.dark_websites_exclude,
    })
    .to_string()
}

/// Script registered on every tab while dark websites is on: Dark Reader plus its settings.
pub fn dark_document_script(s: &Settings) -> Option<String> {
    (s.appearance.dark_websites == Some(true)).then(|| {
        format!(
            "{DARK_READER}
;DarkReader.shoji({});",
            dark_config(s)
        )
    })
}

/// Script that re-applies the current dark-website settings to a page that is already open,
/// loading Dark Reader into it first if needed.
pub fn dark_apply_js(s: &Settings) -> String {
    let config = dark_config(s);
    if s.appearance.dark_websites == Some(true) {
        format!(
            "if (!window.DarkReader) {{ {DARK_READER}
 }};DarkReader.shoji({config});"
        )
    } else {
        format!("window.DarkReader && DarkReader.shoji({config});")
    }
}

pub fn notes(s: &Settings, notes_json: &str) -> String {
    fill(
        NOTES,
        &[
            ("THEME_CSS", &THEME_CSS),
            ("ACCENT", s.accent().id()),
            ("MODE", &s.appearance.theme),
            ("FONTS", s.fonts().id),
            ("NOTES_JSON", &script_json(Ok(notes_json.to_string()))),
        ],
    )
}

fn swatches() -> String {
    Accent::ALL
        .iter()
        .map(|a| {
            format!(
                "<button class=\"opt\" role=\"radio\" data-id=\"{id}\" aria-checked=\"false\" onclick=\"pick('{id}')\">{}<span>{}</span></button>",
                theme::crane_swatch(*a),
                a.name(),
                id = a.id()
            )
        })
        .collect()
}

fn type_options() -> String {
    TypeProfile::ALL
        .iter()
        .map(|p| {
            format!(
                "<button class=\"type-opt\" role=\"radio\" data-fonts=\"{id}\" aria-checked=\"false\" onclick=\"setFonts('{id}')\" style=\"{vars}\"><span class=\"type-aa\">Aa</span><b>{name}</b><small>{note}</small></button>",
                id = p.id,
                vars = html_escape(&p.css_vars()),
                name = p.name,
                note = p.note,
            )
        })
        .collect()
}

/// JSON array of `{id, name, note, vars}` for scripts that render type pickers.
fn profiles_json() -> String {
    let items: Vec<_> = TypeProfile::ALL
        .iter()
        .map(|p| serde_json::json!({ "id": p.id, "name": p.name, "note": p.note, "vars": p.css_vars() }))
        .collect();
    script_json(serde_json::to_string(&items))
}

/// A bundled font file for `http://shoji.fonts/<file>`.
pub fn font_file(file: &str) -> Option<&'static [u8]> {
    Some(match file {
        "InstrumentSans.woff2" => include_bytes!("../../../assets/fonts/InstrumentSans.woff2"),
        "Newsreader.woff2" => include_bytes!("../../../assets/fonts/Newsreader.woff2"),
        "Fraunces.woff2" => include_bytes!("../../../assets/fonts/Fraunces.woff2"),
        "Literata.woff2" => include_bytes!("../../../assets/fonts/Literata.woff2"),
        _ => return None,
    })
}

pub fn chrome(s: &Settings) -> String {
    fill(
        CHROME,
        &[
            ("THEME_CSS", &THEME_CSS),
            ("ACCENT", s.accent().id()),
            ("MODE", &s.appearance.theme),
            ("FONTS", s.fonts().id),
            ("SEARCH_ENGINE_NAME", s.search_engine_display_name()),
            ("PREFS_JSON", &prefs_json(s)),
            ("ACCENTS_JSON", &theme::accents_json()),
            (
                "FROST",
                if s.appearance.frosted_toolbar {
                    "1"
                } else {
                    "0"
                },
            ),
        ],
    )
}

pub fn overlay(s: &Settings) -> String {
    fill(
        OVERLAY,
        &[
            ("THEME_CSS", &THEME_CSS),
            ("ACCENT", s.accent().id()),
            ("MODE", &s.appearance.theme),
            ("FONTS", s.fonts().id),
        ],
    )
}

/// The reader view of an article that the page's own extraction script produced.
/// `html` has already been rebuilt from a tag whitelist in the page; the page's CSP forbids
/// scripts regardless.
pub fn reader(
    s: &Settings,
    title: &str,
    byline: &str,
    site: &str,
    html: &str,
    original_url: &str,
) -> String {
    fill(
        READER,
        &[
            ("THEME_CSS", &THEME_CSS),
            ("ACCENT", s.accent().id()),
            ("MODE", &s.appearance.theme),
            ("FONTS", s.fonts().id),
            ("TITLE", &html_escape(title)),
            ("BYLINE", &html_escape(byline)),
            ("SITE", &html_escape(site)),
            ("ORIGINAL_URL", &html_escape(original_url)),
            ("CONTENT", html),
        ],
    )
}

/// What the new tab page shows besides the search box.
pub struct HomeData<'a> {
    pub favourites: &'a [shoji_core::userdata::Favourite],
    pub recent: &'a [shoji_core::tabs::ClosedTab],
    /// Tabs from a crashed session waiting to be restored (0 = nothing to offer)
    pub restore_tabs: usize,
}

/// The new tab page's data as JSON (embedded in the page and pushed to open ones).
pub fn home_json(d: &HomeData) -> String {
    script_json(serde_json::to_string(&serde_json::json!({
        "favourites": d.favourites,
        "recent": d.recent.iter().take(6).collect::<Vec<_>>(),
        "restoreTabs": d.restore_tabs,
    })))
}

/// Shown when a site could not be reached over HTTPS in HTTPS-only mode.
pub fn insecure_fallback(s: &Settings, tab_id: u64, host: &str, http_url: &str) -> String {
    let host_html = html_escape(host);
    let payload = html_escape(
        &serde_json::json!({ "tab_id": tab_id, "host": host, "url": http_url }).to_string(),
    );
    let body = format!(
        r#"<div class="mark danger">{ICON_WARN}</div>
<h1>This site doesn't offer a secure connection</h1>
<p>Shoji is set to use HTTPS only, and <strong>{host_html}</strong> couldn't be reached securely. If you continue, anything you send or receive could be read by others on the network.</p>
<div class="actions"><button class="primary" onclick="history.length > 1 ? history.back() : post('Navigate', {{ url: '' }})">Go back</button>
<button class="link" onclick="post('ContinueInsecure', {payload})">Continue to {host_html} over HTTP</button></div>"#
    );
    fill(
        NOTICE,
        &[
            ("THEME_CSS", &THEME_CSS),
            ("ACCENT", s.accent().id()),
            ("MODE", &s.appearance.theme),
            ("FONTS", s.fonts().id),
            ("TITLE", "No secure connection"),
            ("BODY", &body),
        ],
    )
}

pub fn sidebar(s: &Settings) -> String {
    fill(
        SIDEBAR,
        &[
            ("THEME_CSS", &THEME_CSS),
            ("ACCENT", s.accent().id()),
            ("MODE", &s.appearance.theme),
            ("FONTS", s.fonts().id),
            ("CRANE", &theme::crane_svg("c", "")),
        ],
    )
}

/// The shield's logger page (`shoji://byobu`).
pub fn byobu(s: &Settings) -> String {
    fill(
        BYOBU,
        &[
            ("THEME_CSS", &THEME_CSS),
            ("ACCENT", s.accent().id()),
            ("MODE", &s.appearance.theme),
            ("FONTS", s.fonts().id),
            ("CRANE", &theme::crane_svg("c", "")),
        ],
    )
}

pub fn home(s: &Settings, data: &HomeData) -> String {
    fill(
        HOME,
        &[
            ("THEME_CSS", &THEME_CSS),
            ("ACCENT", s.accent().id()),
            ("MODE", &s.appearance.theme),
            ("FONTS", s.fonts().id),
            ("CRANE", &theme::crane_svg("c", "")),
            ("SEARCH_ENGINE_NAME", s.search_engine_display_name()),
            ("HOME_JSON", &home_json(data)),
        ],
    )
}

pub fn welcome(s: &Settings) -> String {
    fill(
        WELCOME,
        &[
            ("THEME_CSS", &THEME_CSS),
            ("ACCENT", s.accent().id()),
            ("MODE", &s.appearance.theme),
            ("FONTS", s.fonts().id),
            ("CRANE", &theme::crane_svg("c", "hero")),
            ("SWATCHES", &swatches()),
            (
                "DARK_CHECKED",
                if s.appearance.dark_websites == Some(true) {
                    "checked"
                } else {
                    ""
                },
            ),
            ("PROFILES_JSON", &profiles_json()),
            ("SEARCH_ENGINE", &s.search_engine.to_lowercase()),
            (
                "PRIVATE_MODE",
                if s.privacy.ephemeral_default {
                    "true"
                } else {
                    "false"
                },
            ),
        ],
    )
}

pub fn settings(
    s: &Settings,
    engine_version: &str,
    data_dir: &std::path::Path,
    site_choices: usize,
) -> String {
    fill(
        SETTINGS,
        &[
            ("THEME_CSS", &THEME_CSS),
            ("ACCENT", s.accent().id()),
            ("MODE", &s.appearance.theme),
            ("FONTS", s.fonts().id),
            ("CRANE", &theme::crane_svg("c", "")),
            ("SWATCHES", &swatches()),
            ("TYPE_OPTIONS", &type_options()),
            ("SETTINGS_JSON", &script_json(serde_json::to_string(s))),
            ("APP_VERSION", env!("CARGO_PKG_VERSION")),
            ("ENGINE_VERSION", &html_escape(engine_version)),
            ("DATA_DIR", &html_escape(&data_dir.to_string_lossy())),
            ("SITE_CHOICES", &site_choices.to_string()),
        ],
    )
}

const ICON_WARN: &str = r#"<svg class="i" viewBox="0 0 24 24"><path d="M12 3l9.5 17h-19z"/><path d="M12 10v4M12 17h.01"/></svg>"#;
const ICON_SHIELD: &str = r#"<svg class="i" viewBox="0 0 24 24"><path d="M12 21s7-3.5 7-9.5V5.5L12 3 5 5.5v6c0 6 7 9.5 7 9.5z"/><path d="M12 9v4M12 16h.01"/></svg>"#;

/// Shown inside a tab whose renderer process failed.
pub fn crash(s: &Settings, tab_id: u64) -> String {
    let body = format!(
        r#"<div class="mark">{ICON_WARN}</div>
<h1>This page stopped working</h1>
<p>Its process ended unexpectedly. Your other tabs are fine.</p>
<div class="actions"><button class="primary" onclick="post('ReloadTab', {{ tab_id: {tab_id} }})">Reload</button></div>"#
    );
    fill(
        NOTICE,
        &[
            ("THEME_CSS", &THEME_CSS),
            ("ACCENT", s.accent().id()),
            ("MODE", &s.appearance.theme),
            ("FONTS", s.fonts().id),
            ("TITLE", "Page stopped working"),
            ("BODY", &body),
        ],
    )
}

/// Interstitial for an untrusted TLS certificate.
pub fn certificate_error(s: &Settings, tab_id: u64, host: &str, uri: &str) -> String {
    let host_html = html_escape(host);
    let uri_html = html_escape(uri);
    // JSON in a double-quoted attribute: HTML-escape it so quotes in the URL can't break out.
    let payload =
        html_escape(&serde_json::json!({ "tab_id": tab_id, "host": host, "url": uri }).to_string());
    let body = format!(
        r#"<div class="mark danger">{ICON_SHIELD}</div>
<h1>This connection isn't private</h1>
<p>Shoji stopped loading <strong>{host_html}</strong> because its security certificate isn't trusted. Someone may be trying to impersonate the site or read what you send.</p>
<div class="uri">{uri_html}</div>
<div class="actions"><button class="primary" onclick="history.length > 1 ? history.back() : post('Navigate', {{ url: '' }})">Go back</button></div>
<details><summary>Details</summary>
<p>The server could not prove it is {host_html}. This can be caused by a misconfigured or expired certificate, or by an attacker intercepting the connection.</p>
<button class="link" onclick="post('BypassCertificateError', {payload})">Continue to {host_html} (unsafe)</button></details>"#
    );
    fill(
        NOTICE,
        &[
            ("THEME_CSS", &THEME_CSS),
            ("ACCENT", s.accent().id()),
            ("MODE", &s.appearance.theme),
            ("FONTS", s.fonts().id),
            ("TITLE", "Connection isn't private"),
            ("BODY", &body),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_replaces_known_and_keeps_unknown() {
        assert_eq!(fill("a{{X}}b{{Y}}c", &[("X", "1")]), "a1b{{Y}}c");
        assert_eq!(fill("{{X}}", &[("X", "{{X}}")]), "{{X}}");
        assert_eq!(fill("open {{", &[]), "open {{");
    }

    #[test]
    fn pages_render_without_leftover_placeholders() {
        let s = Settings::default();
        for html in [
            chrome(&s),
            sidebar(&s),
            home(
                &s,
                &HomeData {
                    favourites: &[],
                    recent: &[],
                    restore_tabs: 2,
                },
            ),
            welcome(&s),
            settings(&s, "1.0", std::path::Path::new("C:\\data"), 3),
            overlay(&s),
            reader(&s, "T", "B", "site", "<p>x</p>", "https://a.com/"),
            insecure_fallback(&s, 2, "a.com", "http://a.com/"),
            crash(&s, 3),
            notes(&s, "[]"),
            certificate_error(&s, 4, "a.com", "https://a.com/<x>"),
        ] {
            assert!(!html.contains("{{"), "unfilled placeholder");
            assert!(
                html.contains("data-fonts=\"shinbun\""),
                "missing type profile"
            );
        }
    }

    #[test]
    fn every_bundled_face_is_served() {
        for (_, file, _) in theme::FONT_FILES {
            assert!(
                font_file(file).is_some_and(|b| b.starts_with(b"wOF2")),
                "{file}"
            );
        }
        assert!(font_file("../settings.json").is_none());
    }

    #[test]
    fn certificate_page_escapes_hostile_input() {
        let html = certificate_error(
            &Settings::default(),
            1,
            "a.com",
            "https://a.com/'</script><x>",
        );
        assert!(!html.contains("</script><x>"));
    }
}

/// `SHOJI_DUMP=<dir> cargo test -p shoji dump_pages -- --ignored` writes every page for review.
#[cfg(test)]
#[test]
#[ignore]
fn dump_pages() {
    let dir = std::path::PathBuf::from(std::env::var("SHOJI_DUMP").expect("SHOJI_DUMP"));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Settings::default();
    s.privacy.persistent_sites = vec!["github.com".into()];
    for (name, html) in [
        ("chrome", chrome(&s)),
        (
            "notes",
            notes(
                &s,
                r#"[{"id":"a","created":1791800000000,"updated":1791800000000,"text":"Ideas for the weekend
Walk by the river"}]"#,
            ),
        ),
        ("sidebar", sidebar(&s)),
        (
            "home",
            home(
                &s,
                &HomeData {
                    favourites: &[],
                    recent: &[],
                    restore_tabs: 0,
                },
            ),
        ),
        ("overlay", overlay(&s)),
        (
            "reader",
            reader(
                &s,
                "A quiet article",
                "By Someone",
                "example.com",
                "<p>Hello.</p>",
                "https://example.com/",
            ),
        ),
        ("welcome", welcome(&s)),
        (
            "settings",
            settings(
                &s,
                "141.0.3537.71",
                std::path::Path::new(r"C:\Users\you\AppData\Roaming\shoji"),
                0,
            ),
        ),
        ("crash", crash(&s, 1)),
        (
            "cert",
            certificate_error(&s, 1, "expired.badssl.com", "https://expired.badssl.com/"),
        ),
    ] {
        std::fs::write(dir.join(format!("{name}.html")), html).unwrap();
    }
}

/// Script that re-themes an open Shoji page (accent, colour mode and type profile).
pub fn apply_theme_js(s: &Settings) -> String {
    format!(
        "window.__applyTheme && window.__applyTheme({{accent:'{}',mode:'{}',fonts:'{}'}})",
        s.accent().id(),
        s.appearance.theme,
        s.fonts().id
    )
}
