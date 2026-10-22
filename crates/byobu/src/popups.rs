//! `$popup` rules. Brave's engine drops them (Chromium blocks popups before an ad blocker
//! sees them), so Byōbu keeps them in a small matcher of their own: each becomes a page rule
//! (`$document`), and a popup's address is checked as a page opened by the page that opened it.

use adblock::lists::{FilterSet, ParseOptions};
use adblock::request::Request;
use adblock::Engine;
use std::collections::HashMap;

/// The compiled popup rules, and the list line each came from (for the logger).
pub struct Popups {
    engine: Engine,
    lines: HashMap<String, String>,
}

/// Options a popup rule may keep: whose popup it is, and how firmly it applies. A rule with any
/// other behaviour (`redirect=`, `csp=`, `to=`, ...) is left out rather than half applied.
fn keeps(option: &str) -> bool {
    matches!(
        option,
        "third-party"
            | "3p"
            | "~third-party"
            | "first-party"
            | "1p"
            | "~first-party"
            | "important"
            | "match-case"
            | "badfilter"
    ) || option.starts_with("domain=")
        || option.starts_with("from=")
}

/// Request types, which say nothing about popups: a `$script,popup` rule is a popup rule here.
fn is_type(option: &str) -> bool {
    matches!(
        option.trim_start_matches('~'),
        "script"
            | "image"
            | "stylesheet"
            | "css"
            | "xhr"
            | "xmlhttprequest"
            | "subdocument"
            | "frame"
            | "media"
            | "font"
            | "object"
            | "ping"
            | "websocket"
            | "other"
            | "document"
            | "doc"
            | "popunder"
    )
}

/// `line` as a page rule, if it is a popup rule (`$popup`, or `$all`, which includes popups).
pub fn convert(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty()
        || line.starts_with('!')
        || line.starts_with('[')
        || ["##", "#@#", "#?#", "#$#", "#%#"]
            .iter()
            .any(|c| line.contains(c))
    {
        return None;
    }
    let (pattern, options) = line.rsplit_once('$')?;
    let mut popup = false;
    let mut kept = Vec::new();
    for o in options.split(',').map(str::trim) {
        match o {
            "popup" | "all" => popup = true,
            "~popup" => return None,
            _ if keeps(o) => kept.push(o),
            _ if is_type(o) => {}
            _ => return None,
        }
    }
    if !popup {
        return None;
    }
    // `$popup,domain=x` stops every popup from x.
    let pattern = match pattern {
        "" => "*",
        "@@" => "@@*",
        p => p,
    };
    kept.insert(0, "document");
    Some(format!("{pattern}${}", kept.join(",")))
}

impl Popups {
    /// The popup rules among `lists` (`(text, trusted)`), or `None` when there are none.
    pub fn compile(lists: &[(String, bool)]) -> Option<Self> {
        let mut lines = HashMap::new();
        let mut text = String::new();
        for (list, _) in lists {
            // Most lines can't be popup rules; skip them without splitting their options.
            for line in list
                .lines()
                .filter(|l| l.contains("popup") || l.contains("all"))
            {
                if let Some(rule) = convert(line) {
                    text.push_str(&rule);
                    text.push('\n');
                    lines.insert(rule, line.trim().to_string());
                }
            }
        }
        if lines.is_empty() {
            return None;
        }
        let mut set = FilterSet::new(true);
        set.add_filter_list(text, ParseOptions::default());
        Some(Self {
            engine: Engine::new_with_filter_set(set),
            lines,
        })
    }

    /// The rule that stops `url` opening as a popup of the page at `opener`.
    pub fn blocks(&self, url: &str, opener: &str) -> Option<String> {
        let req = Request::new(url, opener, "main_frame", "GET").ok()?;
        let r = self.engine.check_network_request(&req);
        if !r.should_block() || r.exception.is_some() {
            return None;
        }
        let rule = r.filter.and_then(|f| f.raw_line).unwrap_or_default();
        Some(self.lines.get(&rule).cloned().unwrap_or(rule))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_rules_become_page_rules() {
        assert_eq!(
            convert("||adsterra.com^$popup").as_deref(),
            Some("||adsterra.com^$document")
        );
        assert_eq!(
            convert("|https://$popup,third-party,domain=video.test").as_deref(),
            Some("|https://$document,third-party,domain=video.test")
        );
        assert_eq!(
            convert("@@||pay.test^$popup").as_deref(),
            Some("@@||pay.test^$document")
        );
        assert_eq!(
            convert("||bad.test^$script,popup").as_deref(),
            Some("||bad.test^$document")
        );
        assert_eq!(
            convert("$popup,third-party,domain=stream.test").as_deref(),
            Some("*$document,third-party,domain=stream.test")
        );
        assert_eq!(convert("||ads.test^$script"), None);
        assert_eq!(convert("||ads.test^$popup,redirect=noop.js"), None);
        assert_eq!(convert("site.test##.popup$x"), None);
        assert_eq!(convert("! ||ads.test^$popup"), None);
    }

    #[test]
    fn popups_are_matched_against_their_opener() {
        let lists = vec![(
            "||adsterra.test^$popup\n\
             |https://$popup,third-party,domain=video.test\n\
             @@||pay.test^$popup\n\
             $popup,domain=stream.test\n\
             ||tracker.test^\n"
                .to_string(),
            false,
        )];
        let p = Popups::compile(&lists).unwrap();
        let news = "https://news.test/a";
        assert_eq!(
            p.blocks("https://adsterra.test/x", news).as_deref(),
            Some("||adsterra.test^$popup")
        );
        // An ordinary rule doesn't stop a window.
        assert_eq!(p.blocks("https://tracker.test/", news), None);
        // Every third-party popup from the video site, except the payment one.
        let video = "https://video.test/watch";
        assert!(p.blocks("https://anything.test/", video).is_some());
        assert!(p.blocks("https://pay.test/checkout", video).is_none());
        assert!(p.blocks("https://anything.test/", news).is_none());
        assert!(p
            .blocks("https://stream.test/next", "https://stream.test/v")
            .is_some());
    }
}
