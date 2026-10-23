//! The compatibility list, live, with Shoji's defaults (Byōbu, fingerprinting, the lot):
//! `cargo test -p shoji --test compat -- --ignored --nocapture`.
//! Each site must load and show what it is for, with no errors from scripts Shoji injected.
//! Signing in itself needs accounts and is checked by hand.

#![cfg(windows)]

mod common;

use common::*;
use serde_json::json;
use std::time::Duration;

/// (area, address, a selector that shows the page works)
const ONLY: Option<&str> = option_env!("COMPAT_ONLY");

const SITES: &[(&str, &str, &str)] = &[
    (
        "sign-in",
        "https://accounts.google.com/signin",
        "#identifierId,input[name=identifier]",
    ),
    (
        "sign-in",
        "https://login.live.com/",
        "input[type=email],input[name=loginfmt]",
    ),
    (
        "sign-in",
        "https://account.apple.com/sign-in",
        "iframe,input",
    ),
    ("sign-in", "https://github.com/login", "input#login_field"),
    ("sign-in", "https://discord.com/login", "input[name=email]"),
    ("video", "https://www.twitch.tv/", "video,[data-a-target]"),
    ("video", "https://www.netflix.com/", "a[href*=login],form"),
    ("video", "https://www.disneyplus.com/", "a,button"),
    ("video", "https://open.spotify.com/", "[data-testid],button"),
    (
        "video",
        "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
        "video",
    ),
    (
        "payments",
        "https://checkout.stripe.dev/preview",
        "button,a[href*=demo]",
    ),
    (
        "payments",
        "https://www.paypal.com/signin",
        "input[name=login_email],#email",
    ),
    ("payments", "https://3dsecure.io/", "body"),
    (
        "captcha",
        "https://www.google.com/recaptcha/api2/demo",
        "iframe[src*=recaptcha]",
    ),
    (
        "captcha",
        "https://accounts.hcaptcha.com/demo",
        "iframe[src*=hcaptcha]",
    ),
    (
        "captcha",
        "https://2captcha.com/demo/cloudflare-turnstile",
        "iframe[src*=challenges],.cf-turnstile",
    ),
    (
        "apps",
        "https://docs.google.com/document/u/0/",
        "input,form,[role=main]",
    ),
    ("apps", "https://www.office.com/", "a,button"),
    (
        "apps",
        "https://www.figma.com/",
        "a[href*=signup],a[href*=login]",
    ),
    ("apps", "https://www.notion.so/", "a,button"),
    ("apps", "https://web.whatsapp.com/", "canvas,[data-ref]"),
];

#[test]
#[ignore = "live sites; run on demand"]
fn compatibility_list() {
    if !can_run() {
        return;
    }
    let shoji = Shoji::start(
        "compat",
        "https://example.com/",
        Options {
            real_lists: true,
            local_hosts: false,
            ..Default::default()
        },
    );
    let mut page = shoji.page("example.com");
    page.call("Runtime.enable", json!({}));
    std::thread::sleep(Duration::from_secs(20));

    // Passkeys and DRM: what the engine offers pages.
    let passkeys = page.eval(
        "(async()=>window.PublicKeyCredential?{platform:await PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable(),conditional:!!(PublicKeyCredential.isConditionalMediationAvailable&&await PublicKeyCredential.isConditionalMediationAvailable())}:null)()",
    );
    let drm = page.eval(
        "(async()=>{const out={};for(const k of ['com.widevine.alpha','com.microsoft.playready.recommendation','com.microsoft.playready','org.w3.clearkey']){try{await navigator.requestMediaKeySystemAccess(k,[{initDataTypes:['cenc'],videoCapabilities:[{contentType:'video/mp4; codecs=\"avc1.42E01E\"'}]}]);out[k]=true}catch(e){out[k]=false}}return out})()",
    );
    eprintln!("passkeys: {passkeys}\nDRM: {drm}");

    let mut failures = Vec::new();
    let only = std::env::var("COMPAT_ONLY").ok().or(ONLY.map(String::from));
    for (area, url, ready) in SITES
        .iter()
        .filter(|s| only.as_deref().is_none_or(|o| s.1.contains(o)))
    {
        page.events.clear();
        page.call("Page.enable", json!({}));
        page.call("Page.navigate", json!({ "url": url }));
        let shown = page.wait_for(
            &format!(
                "!!document.querySelector({})",
                serde_json::to_string(ready).unwrap()
            ),
            Duration::from_secs(20),
        );
        std::thread::sleep(Duration::from_secs(2));
        let ours: Vec<String> = page
            .events
            .iter()
            .filter(|e| e["method"] == "Runtime.exceptionThrown")
            .filter_map(|e| {
                let d = &e["params"]["exceptionDetails"];
                let frames = d["stackTrace"]["callFrames"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let injected = d["url"].as_str().unwrap_or("").is_empty()
                    && frames
                        .iter()
                        .all(|f| f["url"].as_str().unwrap_or("").is_empty());
                injected.then(|| {
                    d["exception"]["description"]
                        .as_str()
                        .unwrap_or("")
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(120)
                        .collect()
                })
            })
            .collect();
        let title: String = page
            .eval("document.title")
            .as_str()
            .unwrap_or("")
            .chars()
            .take(50)
            .collect();
        let ok = shown && ours.is_empty();
        eprintln!(
            "{} {area:9} {url:55} {title}{}",
            if ok { "ok  " } else { "FAIL" },
            if ours.is_empty() {
                String::new()
            } else {
                format!("  Shoji errors: {ours:?}")
            }
        );
        if !ok {
            failures.push(*url);
            let shot = page.call("Page.captureScreenshot", json!({ "format": "png" }));
            if let Some(data) = shot["data"].as_str() {
                use base64::Engine as _;
                let host = url.split('/').nth(2).unwrap_or("page");
                let file = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
                    .join(format!("compat-{host}.png"));
                let _ = std::fs::write(
                    &file,
                    base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .unwrap_or_default(),
                );
                eprintln!("     screenshot {}", file.display());
            }
            let text = page.eval(
                r"document.body ? document.body.innerText.slice(0, 200).replace(/\s+/g, ' ') : ''",
            );
            eprintln!("     text: {text}");
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
