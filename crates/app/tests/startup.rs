//! How long Shoji takes from launch to a loaded first page, by stage (median of several runs):
//! `SHOJI_EXE=<release shoji.exe> cargo test -p shoji --test startup -- --ignored --nocapture`.

#![cfg(windows)]

mod common;

use common::*;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[test]
#[ignore = "timing; run on demand with a release build"]
fn launch_to_first_page() {
    if !can_run() {
        return;
    }
    let server = Server::start(|_| Reply::html("<title>Start</title><p>start</p>"));
    let url = server.url("start.test", "/");
    let runs = 7;
    let mut marks: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for i in 0..runs {
        let shoji = Shoji::start(
            &format!("startup-{i}"),
            &url,
            Options {
                env: vec![("SHOJI_STARTUP_TIMING", "1".into())],
                ..Default::default()
            },
        );
        let end = Instant::now() + Duration::from_secs(30);
        let log = shoji.dir.join("err.log");
        let mut lines = String::new();
        while Instant::now() < end {
            lines = std::fs::read_to_string(&log).unwrap_or_default();
            if lines.contains("first page loaded") {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        for l in lines.lines().filter_map(|l| l.strip_prefix("startup ")) {
            let (ms, label) = l.trim_start().split_once(" ms  ").unwrap_or(("0", ""));
            marks
                .entry(label.to_string())
                .or_default()
                .push(ms.trim().parse().unwrap_or(0));
        }
    }
    let mut rows: Vec<(u64, String)> = marks
        .into_iter()
        .map(|(label, mut v)| {
            v.sort_unstable();
            (v[v.len() / 2], format!("{label} (n={})", v.len()))
        })
        .collect();
    rows.sort();
    for (ms, label) in rows {
        eprintln!("{ms:>6} ms  {label}");
    }
}
