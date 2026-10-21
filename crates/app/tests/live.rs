//! Byōbu on real ad-heavy sites: `cargo test -p shoji --test live -- --ignored --nocapture`.
//! Live sites change by the hour, so this reports rather than gates; it fails only when a site
//! doesn't load or nothing at all is blocked on it.

#![cfg(windows)]

mod common;

use common::*;
use serde_json::json;
use std::time::Duration;

const SITES: &[&str] = &[
    "https://www.cnn.com/",
    "https://www.dailymail.co.uk/home/index.html",
    "https://www.theguardian.com/international",
    "https://www.bbc.com/news",
    "https://www.nytimes.com/",
    "https://www.forbes.com/",
    "https://weather.com/",
    "https://www.yahoo.com/",
    "https://www.msn.com/",
    "https://www.reddit.com/r/origami/",
    "https://www.speedtest.net/",
    "https://www.wikihow.com/Fold-a-Paper-Crane",
];

#[test]
#[ignore = "live sites; run on demand"]
fn ad_heavy_sites() {
    if !can_run() {
        return;
    }
    let shoji = Shoji::start(
        "live",
        "https://example.com/",
        Options {
            real_lists: true,
            local_hosts: false,
            ..Default::default()
        },
    );
    let mut page = shoji.page("example.com");
    page.call("Network.enable", json!({}));
    page.call("Runtime.enable", json!({}));
    // Let Byōbu load its lists before judging anything.
    std::thread::sleep(Duration::from_secs(20));
    let mut failures = Vec::new();
    eprintln!(
        "{:48} {:>6} {:>6} {:>6} {:>7}",
        "site", "reqs", "byobu", "engine", "errors"
    );
    for site in SITES {
        page.events.clear();
        page.call("Page.enable", json!({}));
        page.call("Page.navigate", json!({ "url": site }));
        std::thread::sleep(Duration::from_secs(10));
        let title = page
            .eval("document.title")
            .as_str()
            .unwrap_or("")
            .to_string();
        let (mut reqs, mut byobu, mut engine, mut ours) = (0, 0, 0, 0);
        let mut answered = std::collections::HashSet::new();
        for e in &page.events {
            let p = &e["params"];
            match e["method"].as_str().unwrap_or("") {
                "Network.requestWillBeSent" => reqs += 1,
                "Network.responseReceived"
                    if p["response"]["status"] == 403
                        && p["response"]["statusText"] == "Blocked" =>
                {
                    byobu += 1;
                    answered.insert(p["requestId"].to_string());
                }
                "Network.loadingFailed"
                    if p["errorText"] == "net::ERR_BLOCKED_BY_CLIENT"
                        && !answered.contains(&p["requestId"].to_string()) =>
                {
                    engine += 1
                }
                // Exceptions from scripts Shoji injected (no URL anywhere in the stack).
                "Runtime.exceptionThrown" => {
                    let d = &p["exceptionDetails"];
                    let frames = d["stackTrace"]["callFrames"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    if d["url"].as_str().unwrap_or("").is_empty()
                        && frames
                            .iter()
                            .all(|f| f["url"].as_str().unwrap_or("").is_empty())
                    {
                        ours += 1;
                    }
                }
                _ => {}
            }
        }
        eprintln!(
            "{site:48} {reqs:>6} {byobu:>6} {engine:>6} {ours:>7}  {}",
            title.chars().take(40).collect::<String>()
        );
        // A site showing a bot wall makes a handful of requests; that is not a Byōbu failure.
        if title.is_empty() || (reqs >= 20 && byobu + engine == 0) {
            failures.push(*site);
        }
    }
    assert!(
        failures.is_empty(),
        "nothing blocked, or no page, on {failures:?}"
    );
}
