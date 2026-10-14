//! The marks of the browsers Shoji can import from, as `data:` URIs for `<img>` tags (so the
//! gradient ids inside each SVG can never clash with another logo on the same page).
//!
//! The artwork is the browsers' own logos: SVGs and 64 px PNGs from the MIT-licensed
//! [browser-logos](https://github.com/alrra/browser-logos) collection, and single-colour marks
//! from [Simple Icons](https://simpleicons.org) (CC0), filled in each brand's colour. The logos
//! remain trademarks of their owners and are used here only to identify the browser to import from.
//! Browsers with no published logo get a rounded tile in their brand colour with their initial.

/// (catalogue id, file bytes, is a PNG)
static FILES: &[(&str, &[u8], bool)] = &[
    ("arc", include_bytes!("../assets/logos/arc.svg"), false),
    ("avast", include_bytes!("../assets/logos/avast.svg"), false),
    ("brave-beta", include_bytes!("../assets/logos/brave-beta.svg"), false),
    ("brave-nightly", include_bytes!("../assets/logos/brave-nightly.png"), true),
    ("brave", include_bytes!("../assets/logos/brave.svg"), false),
    ("ccleaner", include_bytes!("../assets/logos/ccleaner.svg"), false),
    ("chrome-beta", include_bytes!("../assets/logos/chrome-beta.svg"), false),
    ("chrome-canary", include_bytes!("../assets/logos/chrome-canary.svg"), false),
    ("chrome-dev", include_bytes!("../assets/logos/chrome-dev.svg"), false),
    ("chrome", include_bytes!("../assets/logos/chrome.svg"), false),
    ("chromium", include_bytes!("../assets/logos/chromium.svg"), false),
    ("coccoc", include_bytes!("../assets/logos/coccoc.png"), true),
    ("comet", include_bytes!("../assets/logos/comet.svg"), false),
    ("edge-beta", include_bytes!("../assets/logos/edge-beta.png"), true),
    ("edge-dev", include_bytes!("../assets/logos/edge-dev.png"), true),
    ("edge", include_bytes!("../assets/logos/edge.svg"), false),
    ("epic", include_bytes!("../assets/logos/epic.png"), true),
    ("firefox", include_bytes!("../assets/logos/firefox.svg"), false),
    ("floorp", include_bytes!("../assets/logos/floorp.svg"), false),
    ("ie", include_bytes!("../assets/logos/ie.svg"), false),
    ("librewolf", include_bytes!("../assets/logos/librewolf.svg"), false),
    ("maxthon", include_bytes!("../assets/logos/maxthon.png"), true),
    ("mullvad", include_bytes!("../assets/logos/mullvad.svg"), false),
    ("opera-gx", include_bytes!("../assets/logos/opera-gx.svg"), false),
    ("opera", include_bytes!("../assets/logos/opera.svg"), false),
    ("palemoon", include_bytes!("../assets/logos/palemoon.png"), true),
    ("qq", include_bytes!("../assets/logos/qq.svg"), false),
    ("safari", include_bytes!("../assets/logos/safari.svg"), false),
    ("seamonkey", include_bytes!("../assets/logos/seamonkey.png"), true),
    ("tor", include_bytes!("../assets/logos/tor.png"), true),
    ("vivaldi", include_bytes!("../assets/logos/vivaldi.svg"), false),
    ("whale", include_bytes!("../assets/logos/whale.svg"), false),
    ("yandex", include_bytes!("../assets/logos/yandex.png"), true),
    ("zen", include_bytes!("../assets/logos/zen.svg"), false),
];

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn tile(color: &str, name: &str) -> String {
    let initial: String = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".into());
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><rect width="24" height="24" rx="6" fill="{color}"/><text x="12" y="16.8" text-anchor="middle" font-family="Segoe UI,system-ui,sans-serif" font-size="13" font-weight="700" fill="#fff">{initial}</text></svg>"##
    )
}

/// A `data:` URI for the logo of catalogue entry `id`. `color` and `name` are the entry's brand
/// colour and display name, used for the fallback tile.
pub fn data_uri(id: &str, color: &str, name: &str) -> String {
    match FILES.iter().find(|(k, _, _)| *k == id) {
        Some((_, bytes, png)) => format!(
            "data:{};base64,{}",
            if *png { "image/png" } else { "image/svg+xml" },
            base64(bytes)
        ),
        None => format!("data:image/svg+xml;base64,{}", base64(tile(color, name).as_bytes())),
    }
}

/// Whether `id` has a real logo (rather than the fallback tile).
pub fn has_logo(id: &str) -> bool {
    FILES.iter().any(|(k, _, _)| *k == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn every_file_is_a_real_image() {
        for (id, bytes, png) in FILES {
            if *png {
                assert!(bytes.starts_with(b"\x89PNG"), "{id}");
            } else {
                let text = std::str::from_utf8(bytes).unwrap();
                assert!(text.contains("<svg"), "{id}");
                assert!(!text.contains("<script"), "{id}");
            }
        }
        assert!(has_logo("chrome") && has_logo("firefox") && has_logo("edge"));
    }

    #[test]
    fn unknown_browsers_get_an_initial_tile() {
        let uri = data_uri("zzz", "#123456", "waterfox");
        assert!(uri.starts_with("data:image/svg+xml;base64,"));
        assert!(!has_logo("zzz"));
    }
}
