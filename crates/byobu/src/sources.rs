//! Where Byōbu's rules come from. All of it is upstream data, fetched at runtime.

use serde::Deserialize;

/// A filter list to fetch and cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListSource {
    /// File-name-safe id (also the cache file name).
    pub id: String,
    /// Name shown in the UI
    pub title: String,
    pub url: String,
    /// Seconds before the list is re-checked with a conditional GET.
    pub refresh_secs: u64,
    /// Whether scriptlet injection from this list is trusted (uBlock's own lists, like Brave).
    pub trusted: bool,
}

const HOUR: u64 = 3600;

const UASSETS: &str = "https://ublockorigin.github.io/uAssets/filters";

/// Brave's resource bundle (scriptlets and redirect stubs), republished with uBlock Origin's by
/// the `byobu-resources` workflow from a reviewed, pinned commit.
pub const RESOURCES_URL: &str =
    "https://github.com/KJX469/Shoji/releases/download/byobu-data/brave-resources.json";

/// uBlock Origin's scriptlets and redirect stubs, assembled from its source by the
/// `byobu-resources` workflow (uBO ships them as ES modules, so there is no file to download
/// from upstream). Kept out of the browser binary because uBlock Origin is GPLv3.
pub const UBO_RESOURCES_URL: &str =
    "https://github.com/KJX469/Shoji/releases/download/byobu-data/ubo-resources.json";

/// Shoji's own fixes (`filters/shoji-fixes.txt` in the repository), published with the Byōbu
/// data so a broken site or a new ad trick can be fixed for everyone without a release.
pub const SHOJI_FIXES_URL: &str =
    "https://github.com/KJX469/Shoji/releases/download/byobu-data/shoji-fixes.txt";

/// Shoji's fix list. Checked hourly, like uBlock's quick fixes, and trusted for scriptlets: it is
/// reviewed in this repository like the code.
pub fn shoji_fixes() -> ListSource {
    ListSource {
        id: "shoji-fixes".into(),
        title: "Shoji fixes".into(),
        url: SHOJI_FIXES_URL.into(),
        refresh_secs: HOUR,
        trusted: true,
    }
}

/// Brave's list catalog; its default-enabled lists are followed automatically.
pub const CATALOG_URL: &str =
    "https://raw.githubusercontent.com/brave/adblock-resources/master/filter_lists/list_catalog.json";

/// The uBlock Origin default set. `quick-fixes` carries urgent site workarounds (YouTube etc.),
/// so it is polled hourly.
pub fn ublock_lists() -> Vec<ListSource> {
    let l = |name: &str, hours: u64| ListSource {
        id: format!("ubo-{name}"),
        title: match name {
            "filters" => "uBlock filters",
            "quick-fixes" => "uBlock quick fixes",
            "unbreak" => "uBlock unbreak",
            "privacy" => "uBlock privacy",
            _ => "uBlock badware risks",
        }
        .to_string(),
        url: format!("{UASSETS}/{name}.min.txt"),
        refresh_secs: hours * HOUR,
        trusted: true,
    };
    vec![
        l("filters", 6),
        l("quick-fixes", 1),
        l("unbreak", 6),
        l("privacy", 6),
        l("badware", 12),
    ]
}

/// Third-party lists uBlock Origin also enables by default.
pub fn community_lists() -> Vec<ListSource> {
    let l = |id: &str, title: &str, url: &str| ListSource {
        id: id.into(),
        title: title.into(),
        url: url.into(),
        refresh_secs: 12 * HOUR,
        trusted: false,
    };
    vec![
        l("easylist", "EasyList", "https://easylist.to/easylist/easylist.txt"),
        l("easyprivacy", "EasyPrivacy", "https://easylist.to/easylist/easyprivacy.txt"),
        l(
            "peter-lowe",
            "Peter Lowe's ad and tracking servers",
            "https://pgl.yoyo.org/adservers/serverlist.php?hostformat=adblockplus&showintro=0&mimetype=plaintext",
        ),
    ]
}

#[derive(Deserialize)]
struct CatalogEntry {
    #[serde(default)]
    title: String,
    #[serde(default)]
    langs: Vec<String>,
    #[serde(default)]
    default_enabled: bool,
    #[serde(default)]
    sources: Vec<CatalogSource>,
}

#[derive(Deserialize)]
struct CatalogSource {
    url: String,
}

/// Lists Brave enables by default, from its catalog JSON. Regional and iOS-only lists are
/// skipped, and lists we already fetch are not duplicated.
pub fn brave_lists_from_catalog(json: &str, have: &[ListSource]) -> Vec<ListSource> {
    let Ok(entries) = serde_json::from_str::<Vec<CatalogEntry>>(json) else {
        return Vec::new();
    };
    let mut out: Vec<ListSource> = Vec::new();
    for e in &entries {
        if !e.default_enabled || !e.langs.is_empty() || e.title.to_lowercase().contains("ios") {
            continue;
        }
        for s in &e.sources {
            let dup = have.iter().chain(out.iter()).any(|h| h.url == s.url)
                || s.url.contains("/uAssets/")
                || s.url.contains("easylist.to/");
            if dup {
                continue;
            }
            let id = format!("brave-{:016x}", fnv(&s.url));
            let title = if e.title.is_empty() {
                s.url.clone()
            } else {
                e.title.clone()
            };
            out.push(ListSource {
                id,
                title,
                url: s.url.clone(),
                refresh_secs: 6 * HOUR,
                trusted: true,
            });
        }
    }
    out
}

/// A list the user added by URL.
pub fn custom_list(url: &str) -> ListSource {
    let host = url
        .split("//")
        .last()
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url);
    ListSource {
        id: format!("custom-{:016x}", fnv(url)),
        title: host.to_string(),
        url: url.to_string(),
        refresh_secs: 12 * HOUR,
        trusted: false,
    }
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100000001b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_keeps_default_global_lists_only() {
        let json = r#"[
          {"title":"Brave First Party","langs":[],"default_enabled":true,
           "sources":[{"url":"https://example.org/brave-firstparty.txt"}]},
          {"title":"Brave iOS-Specific","langs":[],"default_enabled":true,
           "sources":[{"url":"https://example.org/ios.txt"}]},
          {"title":"Regional","langs":["de"],"default_enabled":true,
           "sources":[{"url":"https://example.org/de.txt"}]},
          {"title":"Optional","langs":[],"default_enabled":false,
           "sources":[{"url":"https://example.org/opt.txt"}]},
          {"title":"Dup","langs":[],"default_enabled":true,
           "sources":[{"url":"https://easylist.to/easylist/easyprivacy.txt"}]}
        ]"#;
        let got = brave_lists_from_catalog(json, &community_lists());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].url, "https://example.org/brave-firstparty.txt");
    }

    #[test]
    fn shoji_fixes_come_from_this_projects_data_release() {
        let f = shoji_fixes();
        assert!(f
            .url
            .starts_with("https://github.com/KJX469/Shoji/releases/download/byobu-data/"));
        assert_eq!(f.refresh_secs, HOUR);
        assert!(f.trusted);
    }

    #[test]
    fn quick_fixes_polls_hourly() {
        let q = ublock_lists()
            .into_iter()
            .find(|l| l.id == "ubo-quick-fixes")
            .unwrap();
        assert_eq!(q.refresh_secs, 3600);
    }
}
