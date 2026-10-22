//! "Report a broken site": a GitHub issue form filled in with what a fix needs. Opening it sends
//! nothing; the user reads it, adds what they saw and decides whether to submit.

use crate::settings::Settings;
use crate::tabs::url_encode;

const NEW_ISSUE: &str = "https://github.com/KJX469/Shoji/issues/new";

/// What the report says about the browser beside the settings.
pub struct Facts<'a> {
    pub shoji: &'a str,
    pub runtime: &'a str,
    pub windows: &'a str,
    /// Requests Byōbu blocked and scripts it replaced on the page
    pub blocked: u32,
    pub replaced: u32,
}

/// The page address without its query and fragment, which can hold personal details.
pub fn page_address(url: &str) -> String {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    url[..end].to_string()
}

/// Which protections were on for `url`, one per line.
pub fn protections(settings: &Settings, url: &str, facts: &Facts) -> String {
    let shield = match settings.shield_mode_for(url) {
        "off" if settings.shield.mode != "off" => "paused on this site".to_string(),
        mode => mode.to_string(),
    };
    let on = |b: bool| if b { "on" } else { "off" };
    let lists = if settings.shield.disabled_lists.is_empty() {
        "all default lists".to_string()
    } else {
        format!("{} default lists off", settings.shield.disabled_lists.len())
    };
    let own = settings
        .shield
        .custom_rules
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('!'))
        .count();
    [
        format!(
            "Byōbu: {shield} ({} blocked, {} replaced on the page)",
            facts.blocked, facts.replaced
        ),
        format!(
            "Lists: {lists}, {} added, {own} own rules",
            settings.shield.custom_lists.len()
        ),
        format!(
            "Fingerprinting: {}",
            settings.fingerprint_mode_for(&extract_host(url))
        ),
        format!(
            "Third-party cookies blocked: {}",
            on(settings.privacy.block_third_party_cookies)
        ),
        format!(
            "Strict tracking prevention: {}",
            on(settings.privacy.strict_tracking_prevention)
        ),
        format!("HTTPS only: {}", on(settings.privacy.https_only)),
        format!("Clean links: {}", on(settings.shield.clean_links)),
        format!("Private mode: {}", on(settings.privacy.ephemeral_default)),
    ]
    .join("\n")
}

fn extract_host(url: &str) -> String {
    crate::settings::extract_domain(url)
}

/// The prefilled "Broken site" issue form for `url`.
pub fn broken_site_url(settings: &Settings, url: &str, facts: &Facts) -> String {
    let page = page_address(url);
    let host = extract_host(&page);
    let version = format!(
        "Shoji {}, WebView2 {}, {}",
        facts.shoji, facts.runtime, facts.windows
    );
    format!(
        "{NEW_ISSUE}?template=broken_site.yml&title={}&url={}&protections={}&version={}",
        url_encode(&format!("Broken site: {host}")),
        url_encode(&page),
        url_encode(&protections(settings, url, facts)),
        url_encode(&version),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts<'static> {
        Facts {
            shoji: "0.1.7",
            runtime: "141.0.3537.71",
            windows: "Windows 11 24H2 (26100.4061)",
            blocked: 12,
            replaced: 1,
        }
    }

    #[test]
    fn the_report_names_the_page_and_protections_but_not_its_query() {
        let mut s = Settings::default();
        s.shield.paused_sites = vec!["news.example".into()];
        s.shield.custom_rules = "! a comment\nexample.org##.ad\n\n||t.example^\n".into();
        let url = "https://news.example/story/1?session=secret#comments";
        assert_eq!(page_address(url), "https://news.example/story/1");
        let text = protections(&s, url, &facts());
        assert!(text.contains("Byōbu: paused on this site (12 blocked, 1 replaced"));
        assert!(text.contains("2 own rules"));
        assert!(text.contains("Fingerprinting: off"), "{text}");
        let link = broken_site_url(&s, url, &facts());
        assert!(link
            .starts_with("https://github.com/KJX469/Shoji/issues/new?template=broken_site.yml&"));
        assert!(!link.contains("secret") && !link.contains("comments"));
        assert!(link.contains("news.example"));
        assert!(link.contains("141.0.3537.71"));
        assert!(!link.contains(' '));

        let other = protections(&Settings::default(), "https://shop.example/", &facts());
        assert!(other.starts_with("Byōbu: standard"), "{other}");
        assert!(other.contains("Fingerprinting: standard"));
    }
}
