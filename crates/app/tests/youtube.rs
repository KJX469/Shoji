//! Live YouTube with Byōbu's real lists, signed out:
//! `cargo test -p shoji --test youtube -- --ignored --nocapture`.
//! Pre-roll and mid-roll ads, the anti-adblock wall, and ad slots on the home page and in Shorts.

#![cfg(windows)]

mod common;

use common::*;
use std::time::{Duration, Instant};

/// Long, popular videos (they carry ads when nothing blocks them).
const VIDEOS: &[&str] = &[
    "https://www.youtube.com/watch?v=hT_nvWreIhg",
    "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
    "https://www.youtube.com/watch?v=kJQP7kiw5Fk",
    "https://www.youtube.com/watch?v=9bZkp7q19f0",
    "https://www.youtube.com/watch?v=JGwWNGJdvx8",
];

/// What the player shows: an ad is playing, the anti-adblock wall is up, the video plays.
const PLAYER_STATE: &str = "(()=>{const p=document.querySelector('#movie_player');\
  const v=document.querySelector('video.html5-main-video');\
  return JSON.stringify({ad:!!(p&&p.classList.contains('ad-showing')),\
  wall:!!document.querySelector('ytd-enforcement-message-view-model,tp-yt-paper-dialog ytd-enforcement-message-view-model'),\
  playing:!!(v&&!v.paused&&v.currentTime>0),t:v?Math.round(v.currentTime):-1,\
  dur:v?Math.round(v.duration||0):-1})})()";

fn state(page: &mut Page) -> serde_json::Value {
    let s = page.eval(PLAYER_STATE);
    serde_json::from_str(s.as_str().unwrap_or("{}")).unwrap_or_default()
}

#[test]
#[ignore = "live YouTube; run on demand"]
fn youtube_signed_out() {
    if !can_run() {
        return;
    }
    let shoji = Shoji::start(
        "youtube",
        "https://www.youtube.com/",
        Options {
            real_lists: true,
            local_hosts: false,
            // Sound may play on its own here, as a video page opened by the user would.
            settings: serde_json::json!({ "privacy": { "autoplay_sites": ["youtube.com"] } }),
            ..Default::default()
        },
    );
    let mut page = shoji.page("youtube.com");
    // Lists first.
    std::thread::sleep(Duration::from_secs(25));
    page.goto("https://www.youtube.com/");
    std::thread::sleep(Duration::from_secs(6));
    let feed_ads = page.eval(
        "document.querySelectorAll('ytd-ad-slot-renderer,ytd-in-feed-ad-layout-renderer,ytd-display-ad-renderer').length",
    );
    let feed_visible = page.eval(
        "[...document.querySelectorAll('ytd-ad-slot-renderer,ytd-in-feed-ad-layout-renderer,ytd-display-ad-renderer')].filter(e=>e.offsetParent!==null).length",
    );
    eprintln!("home: {feed_ads} ad slots in the page, {feed_visible} visible");
    let mut failures = Vec::new();
    for v in VIDEOS {
        page.goto(v);
        let start = Instant::now();
        let (mut ad, mut wall, mut played) = (false, false, false);
        while start.elapsed() < Duration::from_secs(20) {
            let s = state(&mut page);
            ad |= s["ad"] == true;
            wall |= s["wall"] == true;
            played |= s["playing"] == true;
            std::thread::sleep(Duration::from_millis(500));
        }
        // Mid-roll: jump to the middle and watch again.
        let dur = state(&mut page)["dur"].as_i64().unwrap_or(0);
        let mut mid_ad = false;
        if dur > 120 {
            page.eval(&format!(
                "document.querySelector('video.html5-main-video').currentTime={}",
                dur / 2
            ));
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(12) {
                mid_ad |= state(&mut page)["ad"] == true;
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        eprintln!("{v}: pre-roll ad {ad}, mid-roll ad {mid_ad}, anti-adblock wall {wall}, played {played}");
        if ad || mid_ad || wall || !played {
            failures.push(*v);
        }
    }
    page.goto("https://www.youtube.com/shorts/");
    let start = Instant::now();
    let mut shorts_ads = 0;
    for _ in 0..8 {
        let n = page.eval("document.querySelectorAll('ytd-ad-slot-renderer,ytd-reel-video-renderer[is-ad],ytd-reel-video-renderer ytd-ad-slot-renderer').length");
        shorts_ads = shorts_ads.max(n.as_u64().unwrap_or(0));
        // Next short.
        page.call(
            "Input.dispatchKeyEvent",
            serde_json::json!({ "type": "keyDown", "key": "ArrowDown", "code": "ArrowDown", "windowsVirtualKeyCode": 40 }),
        );
        std::thread::sleep(Duration::from_secs(3));
        if start.elapsed() > Duration::from_secs(60) {
            break;
        }
    }
    eprintln!("shorts: {shorts_ads} ad slots seen over 8 shorts");
    assert!(
        failures.is_empty(),
        "ads, the wall, or no playback on {failures:?}"
    );
    assert_eq!(feed_visible, 0, "ads visible on the home page");
    assert_eq!(shorts_ads, 0, "ads in Shorts");
}
