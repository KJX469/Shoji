//! The shared Shoji UI theme.
//!
//! Two quiet base modes — *Washi* (warm paper, light) and *Sumi* (ink, dark) — selected by
//! `prefers-color-scheme` unless `data-mode="light|dark"` on `<html>` forces one, seven pastel accents selected by `data-accent` on `<html>`, and three type profiles selected by
//! `data-fonts`.
//! Every browser surface (chrome strip, side panel, internal pages, installer) inlines the
//! same stylesheet, so a colour change is one attribute flip per document.

use crate::{Accent, Gradient, SHAPES, VIEW_BOX_ATTR};

/// Pastel accent tones: `(tone, deep)`. `tone` is the soft fill and the accent on dark
/// surfaces; `deep` is the readable accent for text and strokes on light paper.
pub fn tones(accent: Accent) -> (&'static str, &'static str) {
    match accent {
        Accent::White => ("#cfcbc3", "#5f5a52"),
        Accent::Blue => ("#a9c2ec", "#3f5f98"),
        Accent::Cyan => ("#a3d7da", "#2e7980"),
        Accent::Green => ("#b4d3a6", "#4a7840"),
        Accent::Gold => ("#ead39a", "#87681f"),
        Accent::Purple => ("#c6b9e6", "#63519a"),
        Accent::Red => ("#eab0b3", "#9a434b"),
    }
}

/// Window chrome colours as `0xRRGGBB`, for native frame tinting: `(light, dark)`.
pub const FRAME_LIGHT: u32 = 0xe9e5dc;
pub const FRAME_DARK: u32 = 0x151413;
/// Page background colours as `0xRRGGBB`, used to pre-paint webviews: `(light, dark)`.
pub const PAGE_LIGHT: u32 = 0xf2efe8;
pub const PAGE_DARK: u32 = 0x1b1a18;

/// A set of typefaces for the interface. Every profile fills the same four roles:
/// controls (`--font`), headings (`--font-display`), reading text (`--font-text`) and
/// small uppercase labels (`--font-label`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeProfile {
    /// Stable identifier used in settings, CSS (`data-fonts`) and IPC.
    pub id: &'static str,
    pub name: &'static str,
    /// One line for the settings picker.
    pub note: &'static str,
    pub ui: &'static str,
    pub display: &'static str,
    pub text: &'static str,
    pub label: &'static str,
}

const SANS_FALLBACK: &str = "\"Segoe UI Variable Text\", \"Segoe UI\", system-ui, sans-serif";
const SERIF_FALLBACK: &str = "Georgia, \"Times New Roman\", serif";

/// The bundled faces, served to every Shoji page from `http://shoji.fonts/<file>`.
/// `(family, file, weight range)`; files are built by `scripts/build-fonts.py`.
pub const FONT_FILES: [(&str, &str, &str); 4] = [
    ("Instrument Sans", "InstrumentSans.woff2", "400 700"),
    ("Newsreader", "Newsreader.woff2", "300 700"),
    ("Fraunces", "Fraunces.woff2", "300 600"),
    ("Literata", "Literata.woff2", "300 700"),
];

impl TypeProfile {
    pub const ALL: [TypeProfile; 3] = [
        // An editorial text serif with a crisp grotesque for controls.
        TypeProfile {
            id: "shinbun",
            name: "Shinbun",
            note: "Newsreader with Instrument Sans. Crisp and editorial.",
            ui: "Instrument Sans",
            display: "Newsreader",
            text: "Newsreader",
            label: "Instrument Sans",
        },
        // Soft display serif over a sturdy book serif; the sans keeps small controls legible.
        TypeProfile {
            id: "kinari",
            name: "Kinari",
            note: "Soft Fraunces headings, Literata to read. Warm, like unbleached paper.",
            ui: "Instrument Sans",
            display: "Fraunces",
            text: "Literata",
            label: "Instrument Sans",
        },
        // One optically sized book face throughout, with sans only for tiny caps labels.
        TypeProfile {
            id: "hon",
            name: "Hon",
            note: "Literata everywhere. Quiet, like a printed page.",
            ui: "Literata",
            display: "Literata",
            text: "Literata",
            label: "Instrument Sans",
        },
    ];

    /// Parse an identifier; unknown values fall back to the first profile.
    pub fn from_id(id: &str) -> TypeProfile {
        TypeProfile::ALL
            .into_iter()
            .find(|p| p.id.eq_ignore_ascii_case(id.trim()))
            .unwrap_or(TypeProfile::ALL[0])
    }

    fn stack(family: &str) -> String {
        let fallback = if family == "Instrument Sans" {
            SANS_FALLBACK
        } else {
            SERIF_FALLBACK
        };
        format!("\"{family}\", {fallback}")
    }

    /// CSS declarations for this profile's font roles.
    pub fn css_vars(&self) -> String {
        format!(
            "--font:{};--font-display:{};--font-text:{};--font-label:{}",
            Self::stack(self.ui),
            Self::stack(self.display),
            Self::stack(self.text),
            Self::stack(self.label)
        )
    }
}

impl Default for TypeProfile {
    fn default() -> Self {
        TypeProfile::ALL[0]
    }
}

const DARK_TOKENS: &str = "--bg: #1b1a18; --bg-sunk: #151413; --surface: #22211f; --surface-2: #2a2926; --line: rgba(236, 229, 216, .09); --line-strong: rgba(236, 229, 216, .17); --ink: #ebe6dc; --ink-2: #a59f94; --ink-3: #78736a; --hover: rgba(236, 229, 216, .06); --press: rgba(236, 229, 216, .10); --ok: #9cc29a; --warn: #d8b46e; --danger: #df8f87; --shadow: 0 1px 2px rgba(0, 0, 0, .3), 0 14px 36px -14px rgba(0, 0, 0, .65); --crane-shadow: drop-shadow(0 10px 24px rgba(0, 0, 0, .35)); --accent: var(--tone);";

const BASE_CSS: &str = r#"
:root {
  color-scheme: light dark;
  --font: "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif;
  --font-display: "Segoe UI Variable Display", "Segoe UI", system-ui, sans-serif;
  --font-text: var(--font);
  --font-label: var(--font);
  --mono: "Cascadia Mono", Consolas, monospace;
  --bg: #f2efe8;
  --bg-sunk: #e9e5dc;
  --surface: #faf8f3;
  --surface-2: #f0ece4;
  --line: rgba(58, 50, 38, .12);
  --line-strong: rgba(58, 50, 38, .22);
  --ink: #2b2924;
  --ink-2: #6b665c;
  --ink-3: #9b958a;
  --hover: rgba(58, 50, 38, .06);
  --press: rgba(58, 50, 38, .11);
  --ok: #5b8560;
  --warn: #a8772c;
  --danger: #b05049;
  --shadow: 0 1px 2px rgba(58, 50, 38, .06), 0 12px 32px -14px rgba(58, 50, 38, .22);
  --crane-shadow: drop-shadow(0 .5px .6px rgba(58, 50, 38, .35)) drop-shadow(0 8px 18px rgba(58, 50, 38, .10));
  --on-tone: #2b2924;
  --accent: var(--deep);
  --tint: color-mix(in srgb, var(--tone) 34%, transparent);
  --tint-soft: color-mix(in srgb, var(--tone) 16%, transparent);
  --ease: cubic-bezier(.2, .7, .2, 1);
}
* { box-sizing: border-box; margin: 0; padding: 0; }
html { background: var(--bg); }
* { scrollbar-width: none; }
::-webkit-scrollbar { display: none; }
body { font-family: var(--font); color: var(--ink); background: var(--bg); -webkit-font-smoothing: antialiased; }
svg.i { width: 16px; height: 16px; stroke: currentColor; stroke-width: 1.6; stroke-linecap: round; stroke-linejoin: round; fill: none; flex-shrink: 0; }
.crane { display: block; filter: var(--crane-shadow); }
.crane path { transition: fill .35s var(--ease); }
:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
::selection { background: var(--tint); }
.label { font-family: var(--font-label); font-size: 11px; font-weight: 600; letter-spacing: .14em; text-transform: uppercase; color: var(--ink-3); }
"#;

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn crane_vars(paper: Gradient, fold: Gradient) -> String {
    format!(
        "--c1:{};--c2:{};--c3:{};--c4:{}",
        hex(paper[0]),
        hex(paper[1]),
        hex(fold[0]),
        hex(fold[1])
    )
}

/// The complete shared stylesheet: base tokens plus one rule per accent.
pub fn css() -> String {
    let mut css = String::from(BASE_CSS);
    // Dark tokens apply when Windows is dark (unless forced light) or when forced dark.
    css.push_str(&format!(
        "@media (prefers-color-scheme: dark) {{ :root:not([data-mode=\"light\"]) {{ {DARK_TOKENS} }} }}\n\
         :root[data-mode=\"dark\"] {{ {DARK_TOKENS} color-scheme: dark; }}\n\
         :root[data-mode=\"light\"] {{ color-scheme: light; }}\n"
    ));
    for (family, file, weight) in FONT_FILES {
        css.push_str(&format!(
            "@font-face{{font-family:\"{family}\";src:url(\"http://shoji.fonts/{file}\") format(\"woff2\");font-weight:{weight};font-display:fallback}}\n"
        ));
    }
    for p in TypeProfile::ALL {
        css.push_str(&format!(
            ":root[data-fonts=\"{}\"]{{{}}}\n",
            p.id,
            p.css_vars()
        ));
    }
    for a in Accent::ALL {
        let (tone, deep) = tones(a);
        css.push_str(&format!(
            ":root[data-accent=\"{}\"]{{--tone:{tone};--deep:{deep};{}}}\n",
            a.id(),
            crane_vars(a.paper(), a.fold())
        ));
    }
    css
}

/// Inline crane whose colours follow the document's `data-accent` (CSS variables).
/// `id` keeps gradient ids unique when several cranes share a document.
pub fn crane_svg(id: &str, class: &str) -> String {
    let mut out = format!(
        "<svg class=\"crane {class}\" viewBox=\"{VIEW_BOX_ATTR}\" aria-hidden=\"true\"><defs>\
<linearGradient id=\"{id}p\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"><stop offset=\"0\" style=\"stop-color:var(--c1)\"/><stop offset=\"1\" style=\"stop-color:var(--c2)\"/></linearGradient>\
<linearGradient id=\"{id}f\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"><stop offset=\"0\" style=\"stop-color:var(--c3)\"/><stop offset=\"1\" style=\"stop-color:var(--c4)\"/></linearGradient></defs>"
    );
    for (fill, d) in SHAPES {
        let g = if fill == crate::Fill::Paper { 'p' } else { 'f' };
        out.push_str(&format!("<path d=\"{d}\" fill=\"url(#{id}{g})\"/>"));
    }
    out.push_str("</svg>");
    out
}

/// A crane fixed to one colour (for colour pickers), scoped with inline CSS variables.
pub fn crane_swatch(accent: Accent) -> String {
    format!(
        "<span class=\"swatch-crane\" style=\"{}\">{}</span>",
        crane_vars(accent.paper(), accent.fold()),
        crane_svg(&format!("sw-{}-", accent.id()), "")
    )
}

/// JSON array of `{id, name, tone}` for scripts that render pickers.
pub fn accents_json() -> String {
    let items: Vec<String> = Accent::ALL
        .iter()
        .map(|a| {
            format!(
                "{{\"id\":\"{}\",\"name\":\"{}\",\"tone\":\"{}\"}}",
                a.id(),
                a.name(),
                tones(*a).0
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}
