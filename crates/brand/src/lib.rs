//! The Shoji origami crane.
//!
//! One vector shape, seven paper colours. This crate is the single source of truth for the
//! crane's geometry and palettes. It renders the mark to RGBA at any size and encodes `.ico`
//! files, so the window/taskbar icon, the executable icon (via build scripts) and the
//! shortcut icons are all generated from the same data — no bitmap assets ship in the repo.

pub mod logos;
pub mod theme;

/// The seven paper colours of the crane. `White` is the default, greyscale theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Accent {
    #[default]
    White,
    Blue,
    Cyan,
    Green,
    Gold,
    Purple,
    Red,
}

/// A two-stop gradient, `[from, to]`, running top-left to bottom-right across a shape.
pub type Gradient = [[u8; 3]; 2];

impl Accent {
    pub const ALL: [Accent; 7] = [
        Accent::White,
        Accent::Blue,
        Accent::Cyan,
        Accent::Green,
        Accent::Gold,
        Accent::Purple,
        Accent::Red,
    ];

    /// Stable lowercase identifier used in settings, CSS (`data-accent`) and IPC.
    pub fn id(self) -> &'static str {
        match self {
            Accent::White => "white",
            Accent::Blue => "blue",
            Accent::Cyan => "cyan",
            Accent::Green => "green",
            Accent::Gold => "gold",
            Accent::Purple => "purple",
            Accent::Red => "red",
        }
    }

    /// Display name shown in the UI.
    pub fn name(self) -> &'static str {
        match self {
            Accent::White => "White",
            Accent::Blue => "Blue",
            Accent::Cyan => "Cyan",
            Accent::Green => "Green",
            Accent::Gold => "Gold",
            Accent::Purple => "Purple",
            Accent::Red => "Red",
        }
    }

    /// Parse an identifier; unknown values fall back to the default (White).
    pub fn from_id(id: &str) -> Accent {
        Accent::ALL
            .into_iter()
            .find(|a| a.id().eq_ignore_ascii_case(id.trim()))
            .unwrap_or_default()
    }

    /// The face of the paper (tail, wing, neck).
    pub fn paper(self) -> Gradient {
        match self {
            Accent::White => [rgb(0xFFFFFF), rgb(0xE9ECF3)],
            Accent::Blue => [rgb(0x8BB4FF), rgb(0x397AFF)],
            Accent::Cyan => [rgb(0x84F2F5), rgb(0x25C7DD)],
            Accent::Green => [rgb(0x78E9A0), rgb(0x21B86E)],
            Accent::Gold => [rgb(0xFFE074), rgb(0xF8B42C)],
            Accent::Purple => [rgb(0xD2A0FF), rgb(0x9D55EF)],
            Accent::Red => [rgb(0xFF7074), rgb(0xED3049)],
        }
    }

    /// The reverse side of the paper, visible under the wing.
    pub fn fold(self) -> Gradient {
        match self {
            Accent::White => [rgb(0xB2BACB), rgb(0x8792A7)],
            Accent::Blue => [rgb(0x2054D3), rgb(0x173A9C)],
            Accent::Cyan => [rgb(0x0797BD), rgb(0x066989)],
            Accent::Green => [rgb(0x12884E), rgb(0x0D603C)],
            Accent::Gold => [rgb(0xDB8016), rgb(0xA65A0C)],
            Accent::Purple => [rgb(0x772DC5), rgb(0x531D95)],
            Accent::Red => [rgb(0xBD183D), rgb(0x89102C)],
        }
    }
}

const fn rgb(hex: u32) -> [u8; 3] {
    [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8]
}

/// Which gradient a shape is filled with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fill {
    Paper,
    Fold,
}

/// The crane, back to front, as SVG path data in the original 1254-unit artboard.
pub const SHAPES: [(Fill, &str); 4] = [
    // Rear wing / tail
    (Fill::Paper, "M282 547 515 653 517 827Z"),
    // Large, upright folded wing
    (Fill::Paper, "M529 334 731 660 529 869Z"),
    // The contrasting reverse side of the paper
    (Fill::Fold, "M529 869 731 660 791 767Z"),
    // Neck and head: an unbroken folded-paper silhouette
    (
        Fill::Paper,
        "M731 660 860 540Q865 535 870 540L962 599 862 586 873 547 865 543 790 766Z",
    ),
];

/// Square crop around the crane used for icons and inline marks (`x y size`).
pub const VIEW_BOX: (f32, f32, f32) = (262.0, 242.0, 720.0);

/// `viewBox` attribute value matching [`VIEW_BOX`].
pub const VIEW_BOX_ATTR: &str = "262 242 720 720";

/// A standalone SVG of the crane in the given colour (used for docs and exports).
pub fn svg(accent: Accent) -> String {
    let hex = |c: [u8; 3]| format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]);
    let (p, f) = (accent.paper(), accent.fold());
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{VIEW_BOX_ATTR}\" role=\"img\" aria-label=\"Shoji\">\
<defs><linearGradient id=\"p\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"><stop offset=\"0\" stop-color=\"{}\"/><stop offset=\"1\" stop-color=\"{}\"/></linearGradient>\
<linearGradient id=\"f\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"><stop offset=\"0\" stop-color=\"{}\"/><stop offset=\"1\" stop-color=\"{}\"/></linearGradient></defs>",
        hex(p[0]),
        hex(p[1]),
        hex(f[0]),
        hex(f[1])
    );
    for (fill, d) in SHAPES {
        let id = if fill == Fill::Paper { "p" } else { "f" };
        out.push_str(&format!("<path d=\"{d}\" fill=\"url(#{id})\"/>"));
    }
    out.push_str("</svg>");
    out
}

/// A polygon flattened from path data, plus its bounding box for gradient mapping.
struct Polygon {
    fill: Fill,
    points: Vec<(f32, f32)>,
    min: (f32, f32),
    max: (f32, f32),
}

/// Parse the tiny SVG path subset used by [`SHAPES`]: absolute `M`, `L`, `Q` and `Z`,
/// with implicit repeated `L` coordinates. Quadratic curves are flattened.
fn flatten(d: &str) -> Vec<(f32, f32)> {
    let mut nums = Vec::new();
    let mut points: Vec<(f32, f32)> = Vec::new();
    let mut cmd = 'M';
    let flush = |cmd: char, nums: &mut Vec<f32>, points: &mut Vec<(f32, f32)>| {
        match cmd {
            'M' | 'L' => {
                for pair in nums.as_chunks::<2>().0 {
                    points.push((pair[0], pair[1]));
                }
            }
            'Q' => {
                for quad in nums.as_chunks::<4>().0 {
                    let p0 = *points.last().unwrap_or(&(quad[0], quad[1]));
                    let (c, p1) = ((quad[0], quad[1]), (quad[2], quad[3]));
                    const STEPS: usize = 8;
                    for i in 1..=STEPS {
                        let t = i as f32 / STEPS as f32;
                        let u = 1.0 - t;
                        points.push((
                            u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p1.0,
                            u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p1.1,
                        ));
                    }
                }
            }
            _ => {}
        }
        nums.clear();
    };
    let mut token = String::new();
    for ch in d.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_digit() || ch == '.' || (ch == '-' && token.is_empty()) {
            token.push(ch);
            continue;
        }
        if !token.is_empty() {
            nums.push(token.parse().unwrap_or(0.0));
            token.clear();
        }
        if ch.is_ascii_alphabetic() {
            flush(cmd, &mut nums, &mut points);
            cmd = ch.to_ascii_uppercase();
        }
    }
    flush(cmd, &mut nums, &mut points);
    points
}

fn polygons() -> Vec<Polygon> {
    SHAPES
        .iter()
        .map(|&(fill, d)| {
            let points = flatten(d);
            let mut min = (f32::MAX, f32::MAX);
            let mut max = (f32::MIN, f32::MIN);
            for &(x, y) in &points {
                min = (min.0.min(x), min.1.min(y));
                max = (max.0.max(x), max.1.max(y));
            }
            Polygon {
                fill,
                points,
                min,
                max,
            }
        })
        .collect()
}

/// Even-odd point-in-polygon test.
fn contains(points: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (xi, yi) = points[i];
        let (xj, yj) = points[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Render the crane to straight-alpha RGBA (`size * size * 4` bytes) on a transparent background.
pub fn render_rgba(accent: Accent, size: u32) -> Vec<u8> {
    const SS: u32 = 5; // 5x5 supersampling per pixel
    let polys = polygons();
    let (paper, fold) = (accent.paper(), accent.fold());
    let (vx, vy, vs) = VIEW_BOX;
    let scale = vs / size as f32;
    let mut out = vec![0u8; (size * size * 4) as usize];

    for py in 0..size {
        for px in 0..size {
            let mut acc = [0u32; 3];
            let mut hits = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = vx + (px as f32 + (sx as f32 + 0.5) / SS as f32) * scale;
                    let y = vy + (py as f32 + (sy as f32 + 0.5) / SS as f32) * scale;
                    // Painter's order: the last shape containing the sample wins.
                    let top = polys.iter().rev().find(|p| {
                        x >= p.min.0
                            && x <= p.max.0
                            && y >= p.min.1
                            && y <= p.max.1
                            && contains(&p.points, x, y)
                    });
                    if let Some(p) = top {
                        // objectBoundingBox gradient from (0,0) to (1,1): t = (u + v) / 2
                        let u = (x - p.min.0) / (p.max.0 - p.min.0).max(1.0);
                        let v = (y - p.min.1) / (p.max.1 - p.min.1).max(1.0);
                        let t = ((u + v) * 0.5).clamp(0.0, 1.0);
                        let g = if p.fill == Fill::Paper { paper } else { fold };
                        for c in 0..3 {
                            acc[c] +=
                                (g[0][c] as f32 + (g[1][c] as f32 - g[0][c] as f32) * t) as u32;
                        }
                        hits += 1;
                    }
                }
            }
            if hits == 0 {
                continue;
            }
            {
                let i = ((py * size + px) * 4) as usize;
                out[i] = (acc[0] / hits) as u8;
                out[i + 1] = (acc[1] / hits) as u8;
                out[i + 2] = (acc[2] / hits) as u8;
                out[i + 3] = (hits * 255 / (SS * SS)) as u8;
            }
        }
    }
    out
}

/* ------------------------------------------------------------------ kage */

/// The app icon, "kage" (影, shadow): the crane's shadow falling on a lit shoji screen. A
/// rounded tile of warm paper in a thin wooden frame, crossed by one kumiko bar each way, with
/// the crane behind it as a soft shadow in the chosen colour. The installer's icon; the browser
/// itself uses the plain crane.
pub fn render_kage_rgba(accent: Accent, size: u32) -> Vec<u8> {
    const SS: u32 = 4;
    let n = size as usize;
    let s = size as f32;
    let polys = polygons();
    let (vx, vy, vs) = VIEW_BOX;
    // Tile geometry in unit coordinates.
    let inset = (0.6 / s).max(0.03);
    let radius = 0.21;
    let frame = (1.1 / s).max(0.03);
    let bar = (0.7 / s).max(0.02);
    let inner = (inset + frame, 1.0 - inset - frame);
    // The crane sits a little low and right of centre, the way a lamp up and to the left casts it.
    let (cx0, cy0, cw) = (0.13, 0.14, 0.78);

    let mut tile = vec![0f32; n * n];
    let mut ring = vec![0f32; n * n];
    let mut bars = vec![0f32; n * n];
    let mut crane = vec![0f32; n * n];
    let per = 1.0 / (SS * SS) as f32;
    for py in 0..n {
        for px in 0..n {
            let i = py * n + px;
            for sy in 0..SS {
                for sx in 0..SS {
                    let u = (px as f32 + (sx as f32 + 0.5) / SS as f32) / s;
                    let v = (py as f32 + (sy as f32 + 0.5) / SS as f32) / s;
                    if !in_round_rect(u, v, inset, 1.0 - inset, radius) {
                        continue;
                    }
                    tile[i] += per;
                    if !in_round_rect(u, v, inner.0, inner.1, (radius - frame).max(0.02)) {
                        ring[i] += per;
                        continue;
                    }
                    // Tiny icons keep only the frame: a bar across the crane would hide it.
                    if size > 24 && ((u - 0.5).abs() < bar / 2.0 || (v - 0.5).abs() < bar / 2.0) {
                        bars[i] += per;
                    }
                    let x = vx + (u - cx0) / cw * vs;
                    let y = vy + (v - cy0) / cw * vs;
                    if polys.iter().any(|p| {
                        x >= p.min.0
                            && x <= p.max.0
                            && y >= p.min.1
                            && y <= p.max.1
                            && contains(&p.points, x, y)
                    }) {
                        crane[i] += per;
                    }
                }
            }
        }
    }
    // A shadow on paper is soft at the edges.
    let blur = (s * 0.016).round() as usize;
    if blur > 0 {
        crane = box_blur(&crane, n, blur);
    }

    let [p0, p1] = accent.paper();
    let tint = |c: [f32; 3]| -> [f32; 3] {
        if accent == Accent::White {
            return c;
        }
        let t = 0.07;
        [
            c[0] + (p1[0] as f32 - c[0]) * t,
            c[1] + (p1[1] as f32 - c[1]) * t,
            c[2] + (p1[2] as f32 - c[2]) * t,
        ]
    };
    let _ = p0;
    let paper_lit = tint([253.0, 250.0, 241.0]);
    let paper_dim = tint([236.0, 226.0, 203.0]);
    let wood = [150.0, 126.0, 88.0];
    let wood_dark = [118.0, 96.0, 64.0];
    let kumiko = [196.0, 174.0, 134.0];
    let shade = kage_shadow(accent);
    let depth = if size <= 24 { 0.82 } else { 0.68 };

    let mut out = vec![0u8; n * n * 4];
    for py in 0..n {
        for px in 0..n {
            let i = py * n + px;
            if tile[i] <= 0.0 {
                continue;
            }
            let u = (px as f32 + 0.5) / s;
            let v = (py as f32 + 0.5) / s;
            // Lamp light from the upper left, falling off across the paper.
            let d = ((u - 0.28).powi(2) + (v - 0.24).powi(2)).sqrt();
            let lit = (1.0 - d / 1.05).clamp(0.0, 1.0);
            let mut c = mix(paper_dim, paper_lit, lit);
            let inside = tile[i] - ring[i];
            if inside > 0.0 {
                // The shadow falls on the paper; the kumiko bars stand in front of it.
                c = mix(c, shade, (crane[i] / inside).min(1.0) * depth);
                c = mix(c, kumiko, (bars[i] / inside).min(1.0) * 0.9);
            }
            let r = (ring[i] / tile[i]).min(1.0);
            c = mix(c, mix(wood, wood_dark, (u + v) / 2.0), r);
            let o = i * 4;
            out[o] = c[0].round() as u8;
            out[o + 1] = c[1].round() as u8;
            out[o + 2] = c[2].round() as u8;
            out[o + 3] = (tile[i] * 255.0).round().min(255.0) as u8;
        }
    }
    out
}

/// The colour of the crane's shadow on the paper: the deep side of its colour, softened.
fn kage_shadow(accent: Accent) -> [f32; 3] {
    if accent == Accent::White {
        return [78.0, 88.0, 114.0];
    }
    let f = accent.fold()[1];
    let ink = [52.0, 52.0, 58.0];
    mix([f[0] as f32, f[1] as f32, f[2] as f32], ink, 0.2)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Is (u, v) inside the square [lo, hi]² with corners rounded by `r`?
fn in_round_rect(u: f32, v: f32, lo: f32, hi: f32, r: f32) -> bool {
    if u < lo || u > hi || v < lo || v > hi {
        return false;
    }
    let cx = u.clamp(lo + r, hi - r);
    let cy = v.clamp(lo + r, hi - r);
    (u - cx).powi(2) + (v - cy).powi(2) <= r * r
}

/// A separable box blur of radius `r` over an `n`×`n` coverage map.
fn box_blur(src: &[f32], n: usize, r: usize) -> Vec<f32> {
    let pass = |src: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0f32; n * n];
        for a in 0..n {
            for b in 0..n {
                let (mut sum, mut cnt) = (0.0, 0.0);
                for k in b.saturating_sub(r)..(b + r + 1).min(n) {
                    sum += if horizontal {
                        src[a * n + k]
                    } else {
                        src[k * n + a]
                    };
                    cnt += 1.0;
                }
                let i = if horizontal { a * n + b } else { b * n + a };
                out[i] = sum / cnt;
            }
        }
        out
    };
    pass(&pass(src, true), false)
}

/// Icon sizes embedded in generated `.ico` files.
pub const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

/// Encode the browser's icon (the plain [crane](render_rgba)) for the given colour as a
/// multi-resolution `.ico` (32-bit BMP entries).
pub fn ico(accent: Accent, sizes: &[u32]) -> Vec<u8> {
    encode_ico(render_rgba, accent, sizes)
}

/// Encode the installer's icon (the [kage](render_kage_rgba) tile) as a multi-resolution `.ico`.
pub fn setup_ico(accent: Accent, sizes: &[u32]) -> Vec<u8> {
    encode_ico(render_kage_rgba, accent, sizes)
}

fn encode_ico(render: fn(Accent, u32) -> Vec<u8>, accent: Accent, sizes: &[u32]) -> Vec<u8> {
    let images: Vec<(u32, Vec<u8>)> = sizes
        .iter()
        .map(|&s| (s, bmp_entry(&render(accent, s), s)))
        .collect();

    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, data) in &images {
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bit count
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in images {
        out.extend_from_slice(&data);
    }
    out
}

/// A BITMAPINFOHEADER + bottom-up BGRA pixels + AND mask, as stored inside an `.ico`.
fn bmp_entry(rgba: &[u8], size: u32) -> Vec<u8> {
    let mask_stride = size.div_ceil(32) * 4;
    let mut out = Vec::with_capacity((40 + size * size * 4 + mask_stride * size) as usize);
    for v in [40u32, size, size * 2] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]); // compression, image size, resolution, palette
    for y in (0..size).rev() {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            out.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    for y in (0..size).rev() {
        let mut row = vec![0u8; mask_stride as usize];
        for x in 0..size {
            if rgba[((y * size + x) * 4 + 3) as usize] == 0 {
                row[(x / 8) as usize] |= 0x80 >> (x % 8);
            }
        }
        out.extend_from_slice(&row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for a in Accent::ALL {
            assert_eq!(Accent::from_id(a.id()), a);
        }
        assert_eq!(Accent::from_id("nonsense"), Accent::White);
    }

    #[test]
    fn path_parsing_matches_svg() {
        assert_eq!(flatten(SHAPES[0].1).len(), 3);
        // M + L + 8 curve steps + L + 4 implicit L
        assert_eq!(flatten(SHAPES[3].1).len(), 2 + 8 + 5);
    }

    #[test]
    fn render_has_opaque_body_and_transparent_corners() {
        let px = render_rgba(Accent::Blue, 64);
        assert_eq!(px.len(), 64 * 64 * 4);
        assert_eq!(px[3], 0, "top-left corner should be transparent");
        assert!(px.chunks(4).filter(|p| p[3] == 255).count() > 300);
    }

    #[test]
    fn kage_is_a_framed_tile_with_a_shadow() {
        for size in [16, 32, 64] {
            let px = render_kage_rgba(Accent::Blue, size);
            assert_eq!(px.len(), (size * size * 4) as usize);
            assert_eq!(px[3], 0, "rounded corner is transparent at {size}");
            let mid = (((size / 2) * size + size / 2) * 4) as usize;
            assert_eq!(px[mid + 3], 255, "the tile is solid at {size}");
        }
        // The shadow is darker than the bare paper beside it, and takes the colour.
        let px = render_kage_rgba(Accent::Blue, 64);
        let at = |x: u32, y: u32| {
            let i = ((y * 64 + x) * 4) as usize;
            (px[i] as u32, px[i + 1] as u32, px[i + 2] as u32)
        };
        let wing = at(30, 26);
        let paper = at(14, 50);
        assert!(
            wing.0 + wing.1 + wing.2 < paper.0 + paper.1 + paper.2 - 150,
            "{wing:?} vs {paper:?}"
        );
        assert!(wing.2 > wing.0, "a blue shadow: {wing:?}");
        // Every colour renders.
        for a in Accent::ALL {
            assert!(render_kage_rgba(a, 24).chunks(4).any(|p| p[3] == 255));
        }
    }

    #[test]
    fn ico_header_is_valid() {
        let ico = ico(Accent::Gold, &[16, 32]);
        assert_eq!(&ico[0..6], &[0, 0, 1, 0, 2, 0]);
        let first_offset = u32::from_le_bytes(ico[18..22].try_into().unwrap());
        assert_eq!(first_offset, 6 + 32);
    }
}
