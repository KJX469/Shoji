//! Load time and Byōbu's cost on the UI thread over 24 real sites:
//! `SHOJI_EXE=<release shoji.exe> cargo test -p shoji --test sweep -- --ignored --nocapture`.

#![cfg(windows)]

mod common;

use common::*;
use serde_json::json;
use std::time::{Duration, Instant};

pub const SITES: &[&str] = &[
    "https://www.wikipedia.org/",
    "https://en.wikipedia.org/wiki/Origami",
    "https://www.bbc.com/news",
    "https://www.theguardian.com/international",
    "https://www.cnn.com/",
    "https://www.nytimes.com/",
    "https://www.dailymail.co.uk/home/index.html",
    "https://www.reddit.com/r/origami/",
    "https://github.com/KJX469/Shoji",
    "https://www.youtube.com/",
    "https://www.twitch.tv/",
    "https://open.spotify.com/",
    "https://docs.google.com/",
    "https://outlook.live.com/",
    "https://www.google.com/maps",
    "https://www.google.com/recaptcha/api2/demo",
    "https://accounts.hcaptcha.com/demo",
    "https://2captcha.com/demo/cloudflare-turnstile",
    "https://checkout.stripe.dev/",
    "https://www.paypal.com/",
    "https://www.amazon.com/",
    "https://www.figma.com/",
    "https://www.notion.so/",
    "https://web.whatsapp.com/",
];

#[test]
#[ignore = "live sites; run on demand"]
fn sweep() {
    if !can_run() {
        return;
    }
    let shoji = Shoji::start(
        "sweep",
        "https://example.com/",
        Options {
            real_lists: true,
            local_hosts: false,
            env: vec![("SHOJI_BYOBU_TIMING", "1".into())],
            ..Default::default()
        },
    );
    let mut page = shoji.page("example.com");
    page.call("Network.enable", json!({}));
    std::thread::sleep(Duration::from_secs(20));
    let mut loads = Vec::new();
    let (mut total_blocked, mut total_reqs) = (0, 0);
    for site in SITES {
        page.events.clear();
        page.call("Page.enable", json!({}));
        let t = Instant::now();
        page.call("Page.navigate", json!({ "url": site }));
        let loaded = page
            .wait_event("Page.loadEventFired", Duration::from_secs(20))
            .is_some();
        let ms = t.elapsed().as_millis();
        std::thread::sleep(Duration::from_secs(4));
        page.eval("0");
        let reqs = page
            .events
            .iter()
            .filter(|e| e["method"] == "Network.requestWillBeSent")
            .count();
        let blocked = page
            .events
            .iter()
            .filter(|e| {
                e["method"] == "Network.responseReceived"
                    && e["params"]["response"]["statusText"] == "Blocked"
            })
            .count();
        total_blocked += blocked;
        total_reqs += reqs;
        eprintln!(
            "{site:48} load {:>6} ms{}  reqs {reqs:>5}  blocked {blocked:>4}",
            ms,
            if loaded { "" } else { "+" }
        );
        loads.push(ms);
    }
    loads.sort_unstable();
    eprintln!(
        "load median {} ms, p90 {} ms; {total_reqs} requests, {total_blocked} blocked by Byōbu",
        loads[loads.len() / 2],
        loads[loads.len() * 9 / 10]
    );
    let err = std::fs::read_to_string(shoji.dir.join("err.log")).unwrap_or_default();
    if let Some(line) = err.lines().filter(|l| l.starts_with("byobu timing")).last() {
        eprintln!("{line}");
    }
    let stalls = std::fs::read_to_string(shoji.data_dir().join("logs").join("ui-stalls.log"))
        .unwrap_or_default();
    eprintln!(
        "UI stalls: {}",
        if stalls.is_empty() {
            "none".into()
        } else {
            stalls
        }
    );
}
