//! Brave's maintained navigation rules: tracker redirects to skip (`debounce.json`) and
//! per-site tracking parameters (`clean-urls.json`). Fetched with the filter lists; the
//! built-in tables in [`crate::links`] stay as the fallback.

use base64::Engine as _;
use regex::Regex;
use serde::Deserialize;
use url::Url;

/// A WebExtension match pattern (`*://*.example.com/path*`).
#[derive(Debug, Clone)]
struct Pattern {
    scheme: String,
    host: String,
    path: String,
}

impl Pattern {
    fn parse(p: &str) -> Option<Self> {
        let (scheme, rest) = p.split_once("://")?;
        let (host, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/*"),
        };
        Some(Self {
            scheme: scheme.to_ascii_lowercase(),
            host: host.to_ascii_lowercase(),
            path: path.to_string(),
        })
    }

    fn matches(&self, u: &Url) -> bool {
        let scheme_ok = match self.scheme.as_str() {
            "*" => matches!(u.scheme(), "http" | "https"),
            s => u.scheme() == s,
        };
        let Some(host) = u.host_str() else {
            return false;
        };
        let host_ok = if self.host == "*" {
            true
        } else if let Some(base) = self.host.strip_prefix("*.") {
            host == base || host.ends_with(&format!(".{base}"))
        } else {
            host == self.host
        };
        let mut path = u.path().to_string();
        if let Some(q) = u.query() {
            path.push('?');
            path.push_str(q);
        }
        scheme_ok && host_ok && glob(&self.path, &path)
    }
}

/// `*` matches any run of characters; everything else matches itself.
fn glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || text.len() < first.len() + last.len() {
        return false;
    }
    let mut rest = &text[first.len()..];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

fn any(patterns: &[Pattern], u: &Url) -> bool {
    patterns.iter().any(|p| p.matches(u))
}

#[derive(Deserialize)]
struct RawDebounce {
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    action: String,
    #[serde(default)]
    param: String,
    #[serde(default)]
    prepend_scheme: Option<String>,
    #[serde(default)]
    redirect_url_template: Option<String>,
    #[serde(default)]
    pref: Option<String>,
}

#[derive(Debug)]
enum Action {
    /// The destination is in a query parameter
    Param { name: String, base64: bool },
    /// The destination is captured from the path (and optionally fills a template)
    Path {
        regex: Regex,
        template: Option<String>,
    },
}

#[derive(Debug)]
struct Debounce {
    include: Vec<Pattern>,
    exclude: Vec<Pattern>,
    action: Action,
    prepend_scheme: Option<String>,
    /// An AMP rule (Brave puts them behind its de-AMP setting)
    amp: bool,
}

#[derive(Deserialize)]
struct RawClean {
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    params: Vec<String>,
}

#[derive(Debug)]
struct Clean {
    include: Vec<Pattern>,
    exclude: Vec<Pattern>,
    params: Vec<String>,
}

/// Brave's debounce and URL-cleaning rules, parsed.
#[derive(Debug, Default)]
pub struct NavigationRules {
    debounce: Vec<Debounce>,
    clean: Vec<Clean>,
}

fn patterns(list: &[String]) -> Vec<Pattern> {
    list.iter().filter_map(|p| Pattern::parse(p)).collect()
}

impl NavigationRules {
    /// Parse `debounce.json` and `clean-urls.json`. Rules that can't be understood are skipped.
    pub fn from_json(debounce: Option<&str>, clean: Option<&str>) -> Self {
        let debounce = debounce
            .and_then(|j| serde_json::from_str::<Vec<RawDebounce>>(j).ok())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| {
                let action = match r.action.as_str() {
                    "redirect" => Action::Param {
                        name: r.param,
                        base64: false,
                    },
                    "base64,redirect" => Action::Param {
                        name: r.param,
                        base64: true,
                    },
                    "regex-path" => Action::Path {
                        regex: Regex::new(&r.param).ok()?,
                        template: None,
                    },
                    "regex-path-template" => Action::Path {
                        regex: Regex::new(&r.param).ok()?,
                        template: Some(r.redirect_url_template?),
                    },
                    _ => return None,
                };
                let amp = r.pref.as_deref() == Some("brave.de_amp.enabled");
                // Other prefs gate features Shoji doesn't have.
                if r.pref.is_some() && !amp {
                    return None;
                }
                Some(Debounce {
                    include: patterns(&r.include),
                    exclude: patterns(&r.exclude),
                    action,
                    prepend_scheme: r.prepend_scheme,
                    amp,
                })
            })
            .collect();
        let clean = clean
            .and_then(|j| serde_json::from_str::<Vec<RawClean>>(j).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|r| Clean {
                include: patterns(&r.include),
                exclude: patterns(&r.exclude),
                params: r.params,
            })
            .collect();
        Self { debounce, clean }
    }

    pub fn is_empty(&self) -> bool {
        self.debounce.is_empty() && self.clean.is_empty()
    }

    /// Where a tracker redirect at `url` really goes. `amp`: also skip AMP viewers.
    pub fn debounce(&self, url: &str, amp: bool) -> Option<String> {
        let u = Url::parse(url).ok()?;
        for rule in &self.debounce {
            if (rule.amp && !amp) || !any(&rule.include, &u) || any(&rule.exclude, &u) {
                continue;
            }
            if let Some(t) = rule.destination(&u, url) {
                return Some(t);
            }
        }
        None
    }
}

impl Debounce {
    /// Where the redirect at `u` (`url`) goes, by this rule.
    fn destination(&self, u: &Url, url: &str) -> Option<String> {
        let target = match &self.action {
            Action::Param { name, base64 } => {
                let v = u.query_pairs().find(|(k, _)| k == name.as_str())?.1;
                if *base64 {
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(v.trim_end_matches('='))
                        .or_else(|_| {
                            base64::engine::general_purpose::STANDARD_NO_PAD
                                .decode(v.trim_end_matches('='))
                        })
                        .or_else(|_| {
                            base64::engine::general_purpose::URL_SAFE_NO_PAD
                                .decode(v.trim_end_matches('='))
                        })
                        .ok()?;
                    String::from_utf8(bytes).ok()?
                } else {
                    v.into_owned()
                }
            }
            Action::Path { regex, template } => {
                let caps = regex.captures(u.path())?;
                match template {
                    Some(t) => {
                        let mut out = t.clone();
                        for i in (1..caps.len()).rev() {
                            let c = caps.get(i).map(|m| m.as_str()).unwrap_or("");
                            out = out.replace(&format!("${i}"), c);
                        }
                        out
                    }
                    None => {
                        let joined: String = (1..caps.len())
                            .filter_map(|i| caps.get(i).map(|m| m.as_str()))
                            .collect();
                        percent_decode(&joined)
                    }
                }
            }
        };
        let target = match &self.prepend_scheme {
            Some(s) if !target.contains("://") => format!("{s}://{target}"),
            _ => target,
        };
        let t = Url::parse(&target).ok()?;
        if !matches!(t.scheme(), "http" | "https") {
            return None;
        }
        // Never "debounce" to the same site: that is the site's own navigation.
        if crate::headers::site(t.as_str()) == crate::headers::site(url) {
            return None;
        }
        Some(t.to_string())
    }
}

impl NavigationRules {
    /// The site-specific tracking parameters to drop from `url`.
    pub fn params_for(&self, url: &str) -> Vec<&str> {
        let Ok(u) = Url::parse(url) else {
            return Vec::new();
        };
        self.clean
            .iter()
            .filter(|c| any(&c.include, &u) && !any(&c.exclude, &u))
            .flat_map(|c| c.params.iter().map(String::as_str))
            .collect()
    }
}

fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEBOUNCE: &str = r#"[
      {"include":["*://*.prsm1.com/r?*"],"exclude":[],"action":"redirect","param":"url"},
      {"include":["*://pvn.mediamarkt.de/trck/eclick/*"],"exclude":[],"action":"base64,redirect","param":"url64fb"},
      {"include":["*://click.redditmail.com/*"],"exclude":[],"action":"regex-path","param":"^/CL0/([^/]+)/.*$"},
      {"include":["*://www.google.com/amp/s/*"],"pref":"brave.de_amp.enabled","exclude":[],"prepend_scheme":"https","action":"regex-path","param":"^/amp/s/(.*)$"},
      {"include":["*://y2u.be/*"],"exclude":[],"action":"regex-path-template","param":"^/([^/]+)$","redirect_url_template":"https://www.youtube.com/watch?v=$1"},
      {"include":["*://go.example/*"],"exclude":["*://go.example/keep*"],"action":"redirect","param":"u"},
      {"include":["*://x.example/*"],"pref":"brave.something_else","exclude":[],"action":"redirect","param":"u"}
    ]"#;

    const CLEAN: &str = r#"[
      {"include":["*://*.amazon.com/*"],"exclude":["*://*.amazon.com/ap/*"],"params":["ascsubtag","dib"]}
    ]"#;

    fn rules() -> NavigationRules {
        NavigationRules::from_json(Some(DEBOUNCE), Some(CLEAN))
    }

    #[test]
    fn match_patterns() {
        let p = Pattern::parse("*://*.prsm1.com/r?*").unwrap();
        assert!(p.matches(&Url::parse("https://a.prsm1.com/r?url=x").unwrap()));
        assert!(p.matches(&Url::parse("http://prsm1.com/r?x").unwrap()));
        assert!(!p.matches(&Url::parse("https://prsm1.com/q?x").unwrap()));
        assert!(!p.matches(&Url::parse("https://notprsm1.com/r?x").unwrap()));
        assert!(glob("/a*b*c", "/a-b-c"));
        assert!(!glob("/a*b*c", "/a-c"));
    }

    #[test]
    fn debounce_actions() {
        let r = rules();
        assert_eq!(
            r.debounce(
                "https://go.prsm1.com/r?url=https%3A%2F%2Fshop.test%2Fitem&x=1",
                false
            )
            .as_deref(),
            Some("https://shop.test/item")
        );
        let b64 = base64::engine::general_purpose::STANDARD.encode("https://shop.test/b");
        assert_eq!(
            r.debounce(
                &format!("https://pvn.mediamarkt.de/trck/eclick/1?url64fb={b64}"),
                false
            )
            .as_deref(),
            Some("https://shop.test/b")
        );
        assert_eq!(
            r.debounce(
                "https://click.redditmail.com/CL0/https:%2F%2Fwww.reddit.com%2Fr%2Frust/1/abc",
                false
            )
            .as_deref(),
            Some("https://www.reddit.com/r/rust")
        );
        assert_eq!(
            r.debounce("https://y2u.be/dQw4w9WgXcQ", false).as_deref(),
            Some("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
        );
        assert_eq!(
            r.debounce("https://go.example/keep?u=https://shop.test/", false),
            None
        );
        assert_eq!(
            r.debounce("https://x.example/?u=https://shop.test/", false),
            None
        );
    }

    #[test]
    fn amp_rules_follow_the_amp_switch() {
        let r = rules();
        let amp = "https://www.google.com/amp/s/news.test/2026/story.amp";
        assert_eq!(r.debounce(amp, false), None);
        assert_eq!(
            r.debounce(amp, true).as_deref(),
            Some("https://news.test/2026/story.amp")
        );
    }

    #[test]
    fn site_params() {
        let r = rules();
        assert_eq!(
            r.params_for("https://www.amazon.com/dp/1?ascsubtag=x"),
            vec!["ascsubtag", "dib"]
        );
        assert!(r.params_for("https://www.amazon.com/ap/signin").is_empty());
        assert!(r.params_for("https://shop.test/").is_empty());
    }

    #[test]
    fn bad_json_is_no_rules() {
        assert!(NavigationRules::from_json(Some("{"), None).is_empty());
    }
}
