//! Live smoke test against the real upstream lists: `cargo run -p shoji-byobu --release --example live`.

use shoji_byobu::{Mode, RequestKind, Shield, Updater};
use std::time::Instant;

fn main() {
    let dir = std::env::temp_dir().join("byobu-live");
    let updater = Updater::new(&dir);
    let t = Instant::now();
    let (sources, r) = updater.refresh(false);
    println!("refresh: {} sources, {:?}, {:.1}s", sources.len(), r, t.elapsed().as_secs_f32());
    let shield = Shield::new();
    let t = Instant::now();
    let n = shield.rebuild(&updater, &sources);
    println!("engine: {n} lists, {:.1}s", t.elapsed().as_secs_f32());
    for url in [
        "https://securepubads.g.doubleclick.net/tag/js/gpt.js",
        "https://www.google-analytics.com/analytics.js",
        "https://example.org/style.css",
    ] {
        let d = shield.check(url, "https://news.test/", RequestKind::Script, Mode::Standard);
        println!("{url} -> {d:?}");
    }
    let c = shield.cosmetic("https://www.youtube.com/", Mode::Standard);
    println!("youtube cosmetic: {} hide, {} procedural, {} script bytes", c.hide.len(), c.procedural.len(), c.script.len());
    println!("youtube script mentions adPlacements: {}", c.script.contains("adPlacements"));
    println!("next due in {}s", updater.next_due_in(&sources));
}
