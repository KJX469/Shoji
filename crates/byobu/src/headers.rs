//! What third parties are told in request headers. Pure functions; the browser applies them.

use url::Url;

/// Client hints that identify the device more than the browser does: sent only to sites that
/// ask for them, and kept from every third party. The low-entropy ones (`Sec-CH-UA`,
/// `-Mobile`, `-Platform`) stay, since sites use them to pick a layout.
pub const HIGH_ENTROPY_HINTS: &[&str] = &[
    "Sec-CH-UA-Full-Version-List",
    "Sec-CH-UA-Full-Version",
    "Sec-CH-UA-Platform-Version",
    "Sec-CH-UA-Arch",
    "Sec-CH-UA-Model",
    "Sec-CH-UA-Bitness",
    "Sec-CH-UA-WoW64",
    "Sec-CH-UA-Form-Factors",
];

/// The registrable domain of `url`'s host (`news.example.co.uk` -> `example.co.uk`).
pub fn site(url: &str) -> Option<String> {
    let u = Url::parse(url).ok()?;
    let host = u.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    let site = psl::domain_str(&host).unwrap_or(&host).to_string();
    Some(site)
}

/// Whether a request to `url` from the page at `page` goes to another site. Anything that
/// can't be parsed counts as cross-site.
pub fn cross_site(url: &str, page: &str) -> bool {
    match (site(url), site(page)) {
        (Some(a), Some(b)) => a != b,
        _ => true,
    }
}

/// The `Referer` a cross-site request should carry instead of `referer`: its origin alone, as
/// Brave sends, whatever referrer policy the page asked for. `None` when it is already no more
/// than that.
pub fn trimmed_referer(referer: &str) -> Option<String> {
    let u = Url::parse(referer).ok()?;
    if u.scheme() != "https" && u.scheme() != "http" {
        return None;
    }
    let origin = format!("{}/", u.origin().ascii_serialization());
    (referer != origin).then_some(origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites_compare_by_registrable_domain() {
        assert!(!cross_site(
            "https://cdn.news.example.co.uk/a.js",
            "https://www.example.co.uk/story"
        ));
        assert!(cross_site(
            "https://tracker.example/p.gif",
            "https://news.test/story"
        ));
        assert!(cross_site("https://a.github.io/x", "https://b.github.io/"));
        assert!(cross_site("https://a.test/", "about:blank"));
    }

    #[test]
    fn referer_is_cut_to_the_origin() {
        assert_eq!(
            trimmed_referer("https://news.test/2026/story?id=4#c").as_deref(),
            Some("https://news.test/")
        );
        assert_eq!(
            trimmed_referer("http://news.test:8080/a").as_deref(),
            Some("http://news.test:8080/")
        );
        assert_eq!(trimmed_referer("https://news.test/"), None);
        assert_eq!(trimmed_referer("not a url"), None);
    }
}
