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
    ("duckduckgo.com", "/l/", "uddg"),
];

/// If `url` is a known tracker redirect, the page it points to.
pub fn debounce(url: &str) -> Option<String> {
    let u = Url::parse(url).ok()?;
    if u.scheme() != "https" && u.scheme() != "http" {
        return None;
    }
    let host = u.host_str()?;
    for (h, path, param) in REDIRECTORS {
        // A "/" row covers every path on that host.
        let path_ok = if *path == "/" {
            true
        } else {
            u.path() == *path || u.path().starts_with(path)
        };
        if host != *h || !path_ok {
            continue;
        }
        let target = if param.is_empty() {
            // href.li/https://example.com
            let Some(rest) = url.splitn(4, '/').nth(3) else {
                continue;
            };
            rest.strip_prefix('?').unwrap_or(rest).to_string()
        } else {
            // This row may not apply (no such parameter): try the next one.
            let Some((_, v)) = u.query_pairs().find(|(k, _)| k == param) else {
                continue;
            };
            v.into_owned()
        };
        let Ok(t) = Url::parse(&target) else {
            continue;
        };
        if (t.scheme() == "https" || t.scheme() == "http") && t.host_str() != Some(host) {
            return Some(t.to_string());
        }
    }
    None
}

/// Query parameters that sign or authorise the address: removing or re-encoding anything in
/// such a URL could make it stop working.
fn is_signature(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("x-amz-")
        || n.starts_with("x-goog-")
        || matches!(
            n.as_str(),
            "signature" | "sig" | "token" | "expires" | "hmac"
        )
}

/// `url` without tracking parameters, if it had any. Only the tracking pairs are removed; the
/// bytes of everything else are left exactly as they were.
pub fn strip_tracking(url: &str) -> Option<String> {
    let mut u = Url::parse(url).ok()?;
    if u.scheme() != "https" && u.scheme() != "http" {
        return None;
    }
    let query = u.query()?.to_string();
    let name_of = |pair: &str| -> String {
        let raw = pair.split('=').next().unwrap_or("");
        url::form_urlencoded::parse(raw.as_bytes())
            .next()
            .map(|(k, _)| k.into_owned())
            .unwrap_or_default()
    };
    let pairs: Vec<&str> = query.split('&').collect();
    if pairs.iter().any(|p| is_signature(&name_of(p))) {
        return None;
    }
    let present: Vec<&str> = pairs.iter().copied().filter(|p| !p.is_empty()).collect();
    let kept: Vec<&str> = present
        .iter()
        .copied()
        .filter(|p| !is_tracking(&name_of(p)))
        .collect();
    // Only a removed tracking parameter is worth a rewrite, not a stray `&` or a bare `?`.
    if kept.len() == present.len() {
        return None;
    }
    if kept.is_empty() {
        u.set_query(None);
    } else {
        u.set_query(Some(&kept.join("&")));
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
    fn debounces_the_redirect_shapes_seen_in_the_wild() {
        // Google also uses `url=`; the `q` row must not stop the search.
        assert_eq!(
            debounce("https://www.google.com/url?sa=t&url=https%3A%2F%2Fexample.org%2F&ved=1")
                .as_deref(),
            Some("https://example.org/")
        );
        assert_eq!(
            debounce("https://out.reddit.com/t3_1abcde?url=https%3A%2F%2Fexample.org%2Fa&token=x")
                .as_deref(),
            Some("https://example.org/a")
        );
        assert_eq!(
            debounce("https://href.li/https://example.org/page").as_deref(),
            Some("https://example.org/page")
        );
        assert_eq!(
            debounce("https://duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.org%2F").as_deref(),
            Some("https://example.org/")
        );
    }

    #[test]
    fn stripping_leaves_other_parameters_byte_for_byte() {
        assert_eq!(
            strip_tracking("https://example.org/login?next=/a&utm_campaign=x&q=a+b%2c#frag")
                .as_deref(),
            Some("https://example.org/login?next=/a&q=a+b%2c#frag")
        );
        // A signed link is never touched.
        assert_eq!(
            strip_tracking("https://b.s3.amazonaws.com/f?X-Amz-Signature=abc&utm_source=a"),
            None
        );
    }

    #[test]
    fn stray_separators_alone_are_no_reason_to_rewrite() {
        assert_eq!(strip_tracking("https://example.org/api?a=1&"), None);
        assert_eq!(strip_tracking("https://example.org/api?a=1&&b=2"), None);
        assert_eq!(strip_tracking("https://example.org/x?"), None);
        assert_eq!(
            strip_tracking("https://example.org/api?a=1&&utm_source=x").as_deref(),
            Some("https://example.org/api?a=1")
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
