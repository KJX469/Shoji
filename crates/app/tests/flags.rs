//! Autoplay, WebRTC and DNS over HTTPS, and the engine switches for them, tested for what they
//! actually do in WebView2. The autoplay tests are local; the others need the network:
//! `cargo test -p shoji --test flags -- --include-ignored --nocapture`.

#![cfg(windows)]

mod common;

use common::*;
use serde_json::{json, Value};
use std::time::Duration;

/// A second of a 440 Hz tone as a WAV file.
fn tone() -> Vec<u8> {
    let rate = 8000u32;
    let samples: Vec<i16> = (0..rate)
        .map(|i| {
            ((i as f64 * 440.0 * 2.0 * std::f64::consts::PI / rate as f64).sin() * 8000.0) as i16
        })
        .collect();
    let data_len = samples.len() as u32 * 2;
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

/// A page with media that tries to start on its own: one with sound, one muted.
fn media_site() -> Server {
    let wav = tone();
    Server::start(move |hit| {
        match hit.path() {
        "/" => Reply::html(
            "<button id=go style='width:200px;height:80px'>play</button>\
             <audio id=loud src=/tone.wav loop autoplay></audio>\
             <video id=quiet src=/tone.wav loop autoplay muted></video>\
             <script>document.getElementById('go').onclick=()=>document.getElementById('loud').play()</script>",
        ),
        "/tone.wav" => Reply::new(200, "audio/wav", wav.clone()),
        _ => Reply::empty(),
    }
    })
}

/// Whether each element is playing.
fn playing(page: &mut Page) -> (bool, bool) {
    let v = page.eval(
        "[!document.getElementById('loud').paused, !document.getElementById('quiet').paused]",
    );
    (v[0] == true, v[1] == true)
}

fn open_media(name: &str, settings: Value) -> (Server, Shoji, Page) {
    let server = media_site();
    let url = server.url("media.test", "/");
    let shoji = Shoji::start(
        name,
        &url,
        Options {
            settings,
            ..Default::default()
        },
    );
    let mut page = shoji.page("media.test");
    page.wait_for(
        "(document.getElementById('loud')||{}).readyState>=3",
        Duration::from_secs(10),
    );
    std::thread::sleep(Duration::from_secs(2));
    (server, shoji, page)
}

#[test]
fn sound_waits_for_a_click_and_muted_video_plays() {
    if !can_run() {
        return;
    }
    let (_server, _shoji, mut page) = open_media("autoplay", json!({}));
    assert_eq!(playing(&mut page), (false, true), "sound started by itself");
    // A script calling play() is refused too, until the person clicks.
    let refused = page.eval("document.getElementById('loud').play().then(()=>'played',e=>e.name)");
    assert_eq!(refused, "NotAllowedError");
    page.click_on("#go");
    assert!(
        page.wait_for(
            "!document.getElementById('loud').paused",
            Duration::from_secs(5)
        ),
        "a click didn't let the page play sound"
    );
}

#[test]
fn an_allowed_site_and_the_switch_let_sound_play() {
    if !can_run() {
        return;
    }
    let (_s, _shoji, mut page) = open_media(
        "autoplay-site",
        json!({ "privacy": { "autoplay_sites": ["media.test"] } }),
    );
    assert_eq!(
        playing(&mut page),
        (true, true),
        "the allowed site was held back"
    );
    drop(page);
    drop(_shoji);
    let (_s, _shoji, mut page) = open_media(
        "autoplay-off",
        json!({ "privacy": { "block_autoplay": false } }),
    );
    assert_eq!(
        playing(&mut page),
        (true, true),
        "sound was held back with the switch off"
    );
}

/// The engine's own policy, for the record: WebView2 counts its navigations as user activity,
/// so `document-user-activation-required` holds nothing back (the page reports activation).
#[test]
#[ignore = "documents an engine limit"]
fn the_engine_autoplay_switch_does_nothing() {
    if !can_run() {
        return;
    }
    let server = media_site();
    let url = server.url("media.test", "/");
    let shoji = Shoji::start(
        "autoplay-switch",
        &url,
        Options {
            engine_args: "--autoplay-policy=document-user-activation-required".into(),
            settings: json!({ "privacy": { "block_autoplay": false } }),
            ..Default::default()
        },
    );
    let mut page = shoji.page("media.test");
    page.wait_for(
        "(document.getElementById('loud')||{}).readyState>=3",
        Duration::from_secs(10),
    );
    std::thread::sleep(Duration::from_secs(2));
    let activation = page.eval("navigator.userActivation.hasBeenActive");
    let state = playing(&mut page);
    eprintln!("switch on: playing {state:?}, page activation {activation}");
    assert!(state.0);
}

/// ICE candidates a page can gather (each `[type, address]`). With `media`, the page first
/// gets a (fake) microphone, which turns off the engine's own hiding of local addresses.
fn candidates(name: &str, engine_args: &str, media: bool) -> Vec<(String, String)> {
    let server = Server::start(|_| Reply::html("<p>rtc</p>"));
    let url = server.url("rtc.test", "/");
    let shoji = Shoji::start(
        name,
        &url,
        Options {
            engine_args: format!(
                "{engine_args} --use-fake-device-for-media-stream --host-resolver-rules=\"MAP rtc.test 127.0.0.1\" --unsafely-treat-insecure-origin-as-secure=http://rtc.test:{}",
                server.port
            ),
            // The STUN server is on the real network.
            local_hosts: false,
            settings: json!({ "permissions": { "microphone": "allow", "camera": "allow" } }),
            ..Default::default()
        },
    );
    let mut page = shoji.page("rtc.test");
    if media {
        let ok = page
            .eval("navigator.mediaDevices.getUserMedia({audio:true}).then(()=>true,e=>String(e))");
        assert_eq!(ok, true, "no microphone for the page");
    }
    let got: Value = page.eval(
        "new Promise(done=>{const out=[];const pc=new RTCPeerConnection({iceServers:[{urls:'stun:stun.l.google.com:19302'}]});\
         pc.createDataChannel('x');pc.onicecandidate=e=>{if(e.candidate&&e.candidate.candidate){const p=e.candidate.candidate.split(' ');out.push([p[7],p[4]])}};\
         pc.createOffer().then(o=>pc.setLocalDescription(o));setTimeout(()=>done(out),4000)})",
    );
    got.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|c| {
            (
                c[0].as_str().unwrap_or("").to_string(),
                c[1].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn is_private(addr: &str) -> bool {
    addr.starts_with("10.")
        || addr.starts_with("192.168.")
        || (addr.starts_with("172.")
            && addr
                .split('.')
                .nth(1)
                .and_then(|o| o.parse::<u8>().ok())
                .is_some_and(|o| (16..32).contains(&o)))
        || addr.starts_with("fe80")
        || addr.starts_with("fd")
}

/// For the record: WebView2 hides local addresses behind mDNS names until a page gets a camera
/// or microphone; then they show, and `--force-webrtc-ip-handling-policy` changes nothing.
#[test]
#[ignore = "needs a STUN server on the network; documents an engine limit"]
fn webrtc_local_addresses_with_and_without_the_switch() {
    if !can_run() {
        return;
    }
    let switch = "--force-webrtc-ip-handling-policy=default_public_interface_only";
    let off = candidates("rtc-default", "", false);
    let on = candidates("rtc-switch", switch, false);
    let off_media = candidates("rtc-default-media", "", true);
    let on_media = candidates("rtc-switch-media", switch, true);
    eprintln!("default: {off:?}\nswitch:  {on:?}");
    eprintln!(
        "default, with a microphone: {off_media:?}\nswitch, with a microphone:  {on_media:?}"
    );
    assert!(
        on_media
            .iter()
            .any(|(kind, addr)| kind == "host" && is_private(addr)),
        "the switch now hides local addresses: worth turning on ({on_media:?})"
    );
    let _ = (off, on, off_media);
}

#[test]
#[ignore = "needs the real network"]
fn dns_over_https_setting() {
    if !can_run() {
        return;
    }
    // test.nextdns.io answers with how its own name was looked up: over DoH through NextDNS,
    // or "unconfigured" when the lookup went through the system's resolver.
    let probe = |name: &str, args: &str, settings: Value| -> String {
        let shoji = Shoji::start(
            name,
            "https://test.nextdns.io/",
            Options {
                engine_args: args.into(),
                local_hosts: false,
                settings,
                ..Default::default()
            },
        );
        let mut page = shoji.page("test.nextdns.io");
        page.wait_for(
            "document.body && document.body.innerText.includes('status')",
            Duration::from_secs(30),
        );
        page.eval("document.body.innerText")
            .as_str()
            .unwrap_or("")
            .replace(['\n', '\t'], " ")
    };
    let off = probe(
        "doh-off",
        "",
        json!({ "privacy": { "dns_over_https": "off" } }),
    );
    let nextdns = probe(
        "doh-nextdns",
        "",
        json!({ "privacy": { "dns_over_https": "nextdns" } }),
    );
    // For the record: Chromium's DoH feature switch does nothing in WebView2.
    let feature = probe(
        "doh-feature",
        "--enable-features=DnsOverHttps<DoHTrial --force-fieldtrials=DoHTrial/Group \
         --force-fieldtrial-params=DoHTrial.Group:Fallback/false/Templates/https%3A%2F%2Fdns.nextdns.io%2Fdns-query",
        json!({ "privacy": { "dns_over_https": "off" } }),
    );
    eprintln!("off: {off}\nnextdns: {nextdns}\nfeature switch: {feature}");
    assert!(
        nextdns.contains("\"DOH\""),
        "lookups didn't go over DoH: {nextdns}"
    );
    assert!(off.contains("unconfigured"), "off still used DoH: {off}");
}
