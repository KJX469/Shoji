//! Link cleaning for top-level navigations: tracker redirects are skipped ("debouncing") and
//! well-known tracking parameters are dropped, so the page you land on never learns where you
//! clicked from. Pure functions; the browser decides when to apply them.

use url::Url;

/// Query parameters that exist only to identify a click or a person. Exact names.
const TRACKING_PARAMS: &[&str] = &[
    "fbclid",
    "gclid",
    "gclsrc",
    "dclid",
    "gbraid",
    "wbraid",
    "msclkid",
    "twclid",
    "ttclid",
    "li_fat_id",
    "yclid",
    "igshid",
    "mc_eid",
    "mc_cid",
    "_hsenc",
    "_hsmi",
    "__hssc",
    "__hstc",
    "__hsfp",
    "hsctatracking",
    "oly_anon_id",
    "oly_enc_id",
    "vero_id",
    "vero_conv",
    "_openstat",
    "wickedid",
    "rb_clickid",
    "s_cid",
    "ml_subscriber",
    "ml_subscriber_hash",
    "mkt_tok",
    "ck_subscriber_id",
    "irclickid",
    "spm",
    "_ga",
    "_gl",
];

/// Parameters matched by prefix (`utm_source`, `pk_campaign`, ...).
const TRACKING_PREFIXES: &[&str] = &["utm_", "pk_", "mtm_", "matomo_", "piwik_", "hsa_", "stm_"];

fn is_tracking(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    TRACKING_PARAMS.contains(&n.as_str()) || TRACKING_PREFIXES.iter().any(|p| n.starts_with(p))
}

/// Known redirectors: (host, path, parameter holding the real destination).
const REDIRECTORS: &[(&str, &str, &str)] = &[
    ("www.google.com", "/url", "q"),
    ("www.google.com", "/url", "url"),
    ("l.facebook.com", "/l.php", "u"),
    ("lm.facebook.com", "/l.php", "u"),
    ("out.reddit.com", "/", "url"),
    ("t.umblr.com", "/redirect", "z"),
    ("href.li", "/", ""),
    ("steamcommunity.com", "/linkfilter/", "url"),
    ("www.youtube.com", "/redirect", "q"),
    ("slack-redir.net", "/link", "url"),
    ("l.instagram.com", "/", "u"),
    ("away.vk.com", "/away.php", "to"),
    ("exit.sc", "/", "url"),
];

/// If `url` is a known tracker redirect, the page it points to.
pub fn debounce(url: &str) -> Option<String> {
    let u = Url::parse(url).ok()?;
    if u.scheme() != "https" && u.scheme() != "http" {
        return None;
    }
    let host = u.host_str()?;
    for (h, path, param) in REDIRECTORS {
        if host != *h || !(u.path() == *path || (*path != "/" && u.path().starts_with(path))) {
            continue;
        }
        let target = if param.is_empty() {
            // href.li/https://example.com
            let rest = url.splitn(4, '/').nth(3)?.to_string();
            rest.strip_prefix('?').unwrap_or(&rest).to_string()
        } else {
            u.query_pairs().find(|(k, _)| k == param)?.1.into_owned()
        };
        let t = Url::parse(&target).ok()?;
        if (t.scheme() == "https" || t.scheme() == "http") && t.host_str() != Some(host) {
            return Some(t.to_string());
        }
    }
    None
}

/// `url` without tracking parameters, if it had any.
pub fn strip_tracking(url: &str) -> Option<String> {
    let mut u = Url::parse(url).ok()?;
    if u.scheme() != "https" && u.scheme() != "http" {
        return None;
    }
    u.query()?;
    let all: Vec<(String, String)> = u
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let kept: Vec<&(String, String)> = all.iter().filter(|(k, _)| !is_tracking(k)).collect();
    if kept.len() == all.len() {
        return None;
    }
    if kept.is_empty() {
        u.set_query(None);
    } else {
        u.query_pairs_mut()
            .clear()
            .extend_pairs(kept.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }
    Some(u.to_string())
}

/// The cleaned destination for a navigation: redirects skipped first (up to a few hops),
/// then tracking parameters removed. `None` when nothing changes.
pub fn clean(url: &str) -> Option<String> {
    let mut cur = url.to_string();
    for _ in 0..3 {
        match debounce(&cur) {
            Some(next) => cur = next,
            None => break,
        }
    }
    if let Some(next) = strip_tracking(&cur) {
        cur = next;
    }
    (cur != url).then_some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_only_tracking_params() {
        assert_eq!(
            strip_tracking("https://a.com/p?id=3&utm_source=x&fbclid=abc&q=hi").as_deref(),
            Some("https://a.com/p?id=3&q=hi")
        );
        assert_eq!(
            strip_tracking("https://a.com/p?utm_medium=x").as_deref(),
            Some("https://a.com/p")
        );
        assert_eq!(strip_tracking("https://a.com/p?id=3"), None);
        assert_eq!(strip_tracking("https://a.com/p"), None);
        assert_eq!(strip_tracking("shoji://settings?utm_x=1"), None);
    }

    #[test]
    fn keeps_fragment_and_lookalikes() {
        assert_eq!(
            strip_tracking("https://a.com/?gclid=1#top").as_deref(),
            Some("https://a.com/#top")
        );
        assert_eq!(strip_tracking("https://a.com/?utmost=1&mc_eidx=2"), None);
    }

    #[test]
    fn debounces_known_redirectors() {
        assert_eq!(
            debounce(
                "https://www.google.com/url?sa=t&q=https%3A%2F%2Fexample.org%2Fx%3Fa%3D1&ved=0"
            )
            .as_deref(),
            Some("https://example.org/x?a=1")
        );
        assert_eq!(
            debounce("https://l.facebook.com/l.php?u=https%3A%2F%2Fexample.org%2F&h=AT0")
                .as_deref(),
            Some("https://example.org/")
        );
        assert_eq!(
            debounce("https://www.google.com/search?q=https%3A%2F%2Fexample.org"),
            None
        );
        // Redirect back to the same host is not a tracker hop.
        assert_eq!(
            debounce("https://www.google.com/url?q=https%3A%2F%2Fwww.google.com%2Fx"),
            None
        );
        // Destinations must be web URLs.
        assert_eq!(
            debounce("https://www.google.com/url?q=javascript%3Aalert(1)"),
            None
        );
    }

    #[test]
    fn clean_chains_both() {
        assert_eq!(
            clean("https://www.google.com/url?q=https%3A%2F%2Fexample.org%2F%3Futm_source%3Dg%26k%3D1").as_deref(),
            Some("https://example.org/?k=1")
        );
        assert_eq!(clean("https://example.org/"), None);
    }
}
