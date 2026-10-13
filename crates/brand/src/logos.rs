//! Small recognisable marks for the browsers Shoji can import from, drawn here as simple 24×24
//! SVGs in each browser's colours (shapes of our own, not the vendors' artwork). Browsers
//! without a custom mark get a rounded tile in their brand colour with their initial, so every
//! entry always has an icon.

/// A circle clipped to the 24×24 box, used by several marks.
fn clip(id: &str) -> String {
    format!(r#"<clipPath id="{id}"><circle cx="12" cy="12" r="11"/></clipPath>"#)
}

fn wrap(inner: &str) -> String {
    format!(r#"<svg viewBox="0 0 24 24" aria-hidden="true">{inner}</svg>"#)
}

fn chrome(center: &str) -> String {
    wrap(&format!(
        r##"<defs>{}</defs><g clip-path="url(#lc)"><path d="M12 12L-0.1 5.0A14 14 0 0 1 24.1 5.0Z" fill="#ea4335"/><path d="M12 12L24.1 5.0A14 14 0 0 1 12 26Z" fill="#fbbc04"/><path d="M12 12L12 26A14 14 0 0 1 -0.1 5.0Z" fill="#34a853"/></g><circle cx="12" cy="12" r="5.4" fill="#fff"/><circle cx="12" cy="12" r="4" fill="{center}"/>"##,
        clip("lc")
    ))
}

fn firefoxish(a: &str, b: &str) -> String {
    wrap(&format!(
        r##"<defs><linearGradient id="lf" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="{a}"/><stop offset="1" stop-color="{b}"/></linearGradient></defs><circle cx="12" cy="12" r="11" fill="url(#lf)"/><path d="M6 10c1-4 5-6 8-4-2 0-3 1-3 3 2-1 5 0 6 3 1 4-2 8-6 8-5 0-8-4-5-10z" fill="#fff" opacity=".28"/><circle cx="12" cy="13.5" r="3.6" fill="#fff" opacity=".9"/>"##
    ))
}

fn tile(color: &str, name: &str) -> String {
    let initial: String = name
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".into());
    wrap(&format!(
        r##"<rect width="24" height="24" rx="6" fill="{color}"/><text x="12" y="16.8" text-anchor="middle" font-family="Segoe UI,system-ui,sans-serif" font-size="13" font-weight="700" fill="#fff">{initial}</text>"##
    ))
}

/// The mark for catalogue entry `id`. `color` and `name` are the entry's brand colour and
/// display name, used for the fallback tile.
pub fn svg(id: &str, color: &str, name: &str) -> String {
    match id {
        "chrome" => chrome("#4285f4"),
        "chrome-beta" => chrome("#e8710a"),
        "chrome-dev" => chrome("#188038"),
        "chrome-canary" => chrome("#f9ab00"),
        "chromium" => wrap(&format!(
            r##"<defs>{}</defs><g clip-path="url(#lc)"><path d="M12 12L-0.1 5.0A14 14 0 0 1 24.1 5.0Z" fill="#4a86e8"/><path d="M12 12L24.1 5.0A14 14 0 0 1 12 26Z" fill="#6fa8dc"/><path d="M12 12L12 26A14 14 0 0 1 -0.1 5.0Z" fill="#3d6cc6"/></g><circle cx="12" cy="12" r="5.4" fill="#fff"/><circle cx="12" cy="12" r="4" fill="#2a5bd7"/>"##,
            clip("lc")
        )),
        "edge" | "edge-beta" | "edge-dev" => {
            let (a, b) = match id {
                "edge-beta" => ("#1f8a70", "#7bd88f"),
                "edge-dev" => ("#3a7bd5", "#9bd3f5"),
                _ => ("#0c59a4", "#3ecf8e"),
            };
            wrap(&format!(
                r##"<defs><linearGradient id="le" x1="0" y1="1" x2="1" y2="0"><stop offset="0" stop-color="{a}"/><stop offset="1" stop-color="{b}"/></linearGradient></defs><circle cx="12" cy="12" r="11" fill="url(#le)"/><path d="M5 13c0-4 3-7 7-7 3.5 0 6 2.4 6 5.5H9.2c.1 2.2 1.8 3.5 4.3 3.5 1.5 0 2.8-.4 4-1.2-1 2.7-3.6 4.2-6.5 4.200C7.6 18 5 16 5 13z" fill="#fff" opacity=".92"/>"##
            ))
        }
        "brave" | "brave-beta" | "brave-nightly" => {
            let c = match id {
                "brave-beta" => "#8a5cf6",
                "brave-nightly" => "#6a3cd0",
                _ => "#fb542b",
            };
            wrap(&format!(
                r##"<path d="M12 1.5l5.2 1.4 3.3 1-.8 4.4.9 3c0 4-3 7-8.6 10.200C6.4 18.3 3.4 15.3 3.4 11.300l.9-3L3.5 3.900l3.3-1z" fill="{c}"/><path d="M12 7.200l3 1.6-.4 1.6-1.6.9 1.6 1.3-2.6 3.1-2.6-3.1 1.6-1.3-1.6-.9-.4-1.600z" fill="#fff"/>"##
            ))
        }
        "firefox" => firefoxish("#ffb000", "#e31587"),
        "librewolf" => firefoxish("#00acff", "#0060df"),
        "waterfox" => firefoxish("#00b4ff", "#0f4cc9"),
        "floorp" => firefoxish("#3aa1ff", "#6e4bd8"),
        "zen" => firefoxish("#f76f53", "#c2410c"),
        "opera" => wrap(
            r##"<circle cx="12" cy="12" r="11" fill="#ff1b2d"/><ellipse cx="12" cy="12" rx="3.6" ry="6.6" fill="none" stroke="#fff" stroke-width="2.6"/>"##,
        ),
        "opera-gx" => wrap(
            r##"<defs><linearGradient id="lg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#fa1e4e"/><stop offset="1" stop-color="#6c2cf0"/></linearGradient></defs><circle cx="12" cy="12" r="11" fill="url(#lg)"/><ellipse cx="12" cy="12" rx="3.6" ry="6.6" fill="none" stroke="#fff" stroke-width="2.6"/>"##,
        ),
        "vivaldi" => wrap(
            r##"<rect width="24" height="24" rx="7" fill="#ef3939"/><path d="M6.5 7.500L12 17l5.5-9.5" fill="none" stroke="#fff" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"/>"##,
        ),
        "safari" => wrap(
            r##"<circle cx="12" cy="12" r="11" fill="#1c9bef"/><circle cx="12" cy="12" r="8.8" fill="none" stroke="#fff" stroke-width=".8" opacity=".7"/><path d="M12 12L9 15l1.5-4.500L15 9l-1.5 4.500z" fill="#fff"/><path d="M12 12l3-3-1.5 4.500z" fill="#ff3b30"/>"##,
        ),
        "ie" => wrap(
            r##"<ellipse cx="12" cy="12.5" rx="11" ry="4.2" transform="rotate(-25 12 12)" fill="none" stroke="#f7b500" stroke-width="1.6"/><path d="M5.5 13c0-4 3-7 6.5-7 3.3 0 5.8 2 5.8 5.200H9.400c.1 2 1.6 3.2 3.8 3.2 1.4 0 2.6-.4 3.6-1.1-.9 2.4-3.2 3.7-5.8 3.700C7.7 17 5.5 15.6 5.5 13z" fill="#1ebbee"/>"##,
        ),
        "tor" => wrap(
            r##"<circle cx="12" cy="12" r="11" fill="#7d4698"/><circle cx="12" cy="12" r="7.2" fill="none" stroke="#fff" stroke-width="1.4"/><circle cx="12" cy="12" r="3.4" fill="none" stroke="#fff" stroke-width="1.4"/><circle cx="12" cy="12" r="1" fill="#fff"/>"##,
        ),
        "yandex" => wrap(
            r##"<circle cx="12" cy="12" r="11" fill="#fc3f1d"/><path d="M13.3 18.500V13.300L16.5 5.500h-2.200L12 11.3 9.7 5.500H7.500l3.2 7.800v5.200z" fill="#fff"/>"##,
        ),
        "arc" => wrap(
            r##"<defs><linearGradient id="la" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#ff5a7a"/><stop offset="1" stop-color="#5c6cff"/></linearGradient></defs><rect width="24" height="24" rx="7" fill="url(#la)"/><path d="M6.5 17.500L12 5.500l5.5 12" fill="none" stroke="#fff" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/>"##,
        ),
        "file" => wrap(
            r##"<rect x="4.5" y="2.5" width="15" height="19" rx="2.5" fill="#868e96"/><path d="M8 7h8M8 11h8M8 15h5" stroke="#fff" stroke-width="1.6" stroke-linecap="round"/>"##,
        ),
        _ => tile(color, name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_are_self_contained_svgs() {
        for id in ["chrome", "edge", "brave", "firefox", "opera", "vivaldi", "safari", "ie", "tor", "arc", "file"] {
            let s = svg(id, "#000000", "X");
            assert!(s.starts_with("<svg") && s.ends_with("</svg>"), "{id}");
            assert_eq!(s.matches("<svg").count(), 1);
        }
    }

    #[test]
    fn unknown_browsers_get_an_initial_tile() {
        let s = svg("zzz", "#123456", "ølaf");
        assert!(s.contains("#123456") && s.contains(">Ø<"));
    }
}
