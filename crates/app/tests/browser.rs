//! Byōbu and privacy features in a running browser: an isolated Shoji, test pages on a local
//! server, checked over the DevTools protocol and by what the server receives.

#![cfg(windows)]

mod common;

use common::*;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// Rules every shield test starts with; `probe.test` tells when they are live.
fn rules(extra: &[&str]) -> String {
    let mut r = vec!["||probe.test^"];
    r.extend_from_slice(extra);
    r.join("\n")
}

fn shield_settings(extra: &[&str]) -> serde_json::Value {
    json!({ "shield": { "custom_rules": rules(extra) } })
}

const PROBE: &str = "<img src=\"/probe-img\" hidden>";

/// A server whose pages are given as (path, html); `{P}` in a page is the server port, and every
/// page carries the shield probe.
fn site(pages: Vec<(&'static str, String)>) -> Arc<Server> {
    let pages: Vec<(String, String)> = pages.into_iter().map(|(p, h)| (p.into(), h)).collect();
    let port = Arc::new(std::sync::OnceLock::<u16>::new());
    let p2 = port.clone();
    let server = Arc::new(Server::start(move |hit| {
        let port = *p2.get().unwrap_or(&0);
        if hit.path() == "/probe-img" {
            return Reply::new(302, "text/plain", "")
                .header("Location", &format!("http://probe.test:{port}/p.gif"));
        }
        match pages.iter().find(|(p, _)| *p == hit.path()) {
            Some((path, html)) => {
                let body = html.replace("{P}", &port.to_string());
                if path.ends_with(".js") {
                    Reply::js(body)
                } else {
                    Reply::html(format!("{body}{PROBE}"))
                }
            }
            None => Reply::empty(),
        }
    }));
    let _ = port.set(server.port);
    server
}

#[test]
fn shield_blocks_inside_cross_origin_frames_and_workers() {
    if !can_run() {
        return;
    }
    let server = site(vec![
        (
            "/",
            "<h1>top</h1><iframe id=f src=\"http://media-cdn.test:{P}/frame.html\"></iframe>"
                .into(),
        ),
        (
            "/frame.html",
            "<div class=ad-banner id=generic>g</div><div class=frame-only id=specific>s</div>\
             <div class=keep id=keep>k</div>\
             <img src=\"http://px-tracker.test:{P}/pixel.gif?from=frame\">\
             <script src=\"http://media-cdn.test:{P}/ad.js\"></script>\
             <script>try{new SharedWorker('/worker.js')}catch(e){}</script>"
                .into(),
        ),
        ("/ad.js", "window.adRan=true;".into()),
        (
            "/worker.js",
            "fetch('/track?from=sharedworker').catch(()=>{});".into(),
        ),
    ]);
    let top = server.url("top-site.test", "/");
    let shoji = Shoji::start(
        "frames",
        &top,
        Options {
            settings: shield_settings(&[
                "||px-tracker.test^",
                "||media-cdn.test^*/ad.js",
                "/track?from=sharedworker",
                "##.ad-banner",
                "media-cdn.test##.frame-only",
            ]),
            ..Default::default()
        },
    );
    let mut page = shoji.page("top-site.test");
    wait_for_shield(&server, &mut page, &top, ("probe.test", "/p.gif"));
    page.goto(&top);
    // The frame loaded, and its kept element exists, before anything is judged.
    assert!(server
        .wait_for("media-cdn.test", "/frame.html", Duration::from_secs(10))
        .is_some());
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        !server.saw("px-tracker.test", "/pixel.gif"),
        "frame pixel reached the server"
    );
    assert!(
        !server.saw("media-cdn.test", "/ad.js"),
        "frame ad script reached the server"
    );
    assert!(
        !server.saw("media-cdn.test", "/track"),
        "shared worker's tracker reached the server"
    );

    // Cosmetic rules inside the frame (a generic class rule and a host rule).
    let mut frame = shoji.page("media-cdn.test");
    let shown = |p: &mut Page, id: &str| {
        p.eval(&format!(
            "getComputedStyle(document.getElementById('{id}')).display!=='none'"
        ))
    };
    assert!(frame.wait_for(
        "getComputedStyle(document.getElementById('generic')).display==='none'",
        Duration::from_secs(10)
    ));
    assert_eq!(shown(&mut frame, "specific"), false);
    assert_eq!(shown(&mut frame, "keep"), true);
}

#[test]
fn page_rules_hide_elements_and_run_before_page_scripts() {
    if !can_run() {
        return;
    }
    let server = site(vec![(
        "/",
        "<div class=sponsored id=a>ad</div><div id=b>story</div>\
         <div class=promo-box id=c><span>Sponsored</span></div>\
         <img id=px src=\"http://stats.tracker.test:{P}/collect.gif\">"
            .into(),
    )]);
    let url = server.url("news.test", "/");
    let shoji = Shoji::start(
        "cosmetic",
        &url,
        Options {
            settings: shield_settings(&[
                "news.test##.sponsored",
                "news.test##.promo-box:has-text(Sponsored)",
                "||tracker.test^$image",
            ]),
            ..Default::default()
        },
    );
    let mut page = shoji.page("news.test");
    wait_for_shield(&server, &mut page, &url, ("probe.test", "/p.gif"));
    page.goto(&url);
    assert!(page.wait_for(
        "getComputedStyle(document.getElementById('a')).display==='none'",
        Duration::from_secs(10)
    ));
    assert!(page.wait_for(
        "getComputedStyle(document.getElementById('c')).display==='none'",
        Duration::from_secs(10)
    ));
    assert_eq!(
        page.eval("getComputedStyle(document.getElementById('b')).display"),
        "block"
    );
    assert!(!server.saw("stats.tracker.test", "/collect.gif"));
}

#[test]
fn a_document_rule_stops_the_page_with_a_warning() {
    if !can_run() {
        return;
    }
    let server = site(vec![
        ("/", "<p>start</p>".into()),
        ("/bad", "<p>bad</p>".into()),
    ]);
    let url = server.url("start.test", "/");
    let shoji = Shoji::start(
        "document",
        &url,
        Options {
            settings: shield_settings(&["||malware-host.test^$document"]),
            ..Default::default()
        },
    );
    let mut page = shoji.page("start.test");
    wait_for_shield(&server, &mut page, &url, ("probe.test", "/p.gif"));
    page.call(
        "Page.navigate",
        json!({ "url": server.url("malware-host.test", "/bad") }),
    );
    let mut notice = shoji.page("shoji.notice-");
    assert!(notice.wait_for(
        "document.body && document.body.innerText.length>0",
        Duration::from_secs(10)
    ));
    assert!(
        !server.saw("malware-host.test", "/bad"),
        "the blocked page was fetched"
    );
}

#[test]
fn global_privacy_control_is_sent_and_visible() {
    if !can_run() {
        return;
    }
    let server = site(vec![("/", "<p>gpc</p>".into())]);
    let url = server.url("gpc.test", "/");
    let shoji = Shoji::start("gpc", &url, Options::default());
    let mut page = shoji.page("gpc.test");
    page.goto(&url);
    let hit = server
        .wait_for("gpc.test", "/", Duration::from_secs(10))
        .unwrap();
    assert_eq!(hit.header("sec-gpc"), Some("1"));
    assert_eq!(page.eval("navigator.globalPrivacyControl"), true);
}

#[test]
fn opened_links_lose_tracking_parameters() {
    if !can_run() {
        return;
    }
    let server = site(vec![
        ("/", "<p>start</p>".into()),
        ("/story", "<p>story</p>".into()),
    ]);
    let url = server.url("start.test", "/");
    let shoji = Shoji::start("cleanlinks", &url, Options::default());
    let mut page = shoji.page("start.test");
    page.goto(&url);
    server.clear();
    page.goto(&server.url("news.test", "/story?id=7&utm_source=mail&fbclid=abc"));
    let hit = server
        .wait_for("news.test", "/story", Duration::from_secs(10))
        .unwrap();
    assert_eq!(hit.target, "/story?id=7");
}

#[test]
fn fingerprinting_noise_is_stable_per_site_and_differs_between_sites() {
    if !can_run() {
        return;
    }
    let probe = "<canvas id=c width=64 height=32></canvas>";
    let server = site(vec![("/", probe.into())]);
    let read = "(()=>{const c=document.getElementById('c'),x=c.getContext('2d');\
                x.fillStyle='#f60';x.fillRect(1,1,40,20);x.font='14px Arial';x.fillStyle='#069';\
                x.fillText('Shoji',2,15);return c.toDataURL()})()";
    let shoji = Shoji::start(
        "fingerprint",
        &server.url("a.test", "/"),
        Options::default(),
    );
    let mut page = shoji.page("a.test");
    // The tab's very first document must be protected too.
    page.wait_for(
        "document.readyState==='complete'&&!!document.getElementById('c')",
        Duration::from_secs(10),
    );
    let first = page.eval(read).as_str().unwrap().to_string();
    let mut sample = |host: &str| {
        page.goto(&server.url(host, "/"));
        page.eval(read).as_str().unwrap().to_string()
    };
    let a1 = sample("a.test");
    let a2 = sample("a.test");
    let sub = sample("sub.a.test");
    let b = sample("b.test");
    assert_eq!(first, a1, "the first page in a new tab gets the same noise");
    assert_eq!(a1, a2, "same site, same noise");
    assert_eq!(a1, sub, "subdomains share their site's noise");
    assert_ne!(a1, b, "other sites get other noise");
}

#[test]
fn other_sites_get_the_origin_as_referrer_and_click_pings_are_blocked() {
    if !can_run() {
        return;
    }
    let server = site(vec![(
        "/story",
        "<meta name=referrer content=unsafe-url>\
         <img src=\"http://cdn.paper.test:{P}/same.gif\">\
         <img src=\"http://other-site.test:{P}/cross.gif\">\
         <a id=l href=\"#read\" ping=\"http://paper.test:{P}/ping\">read</a>"
            .into(),
    )]);
    let url = server.url("paper.test", "/story?id=9");
    let shoji = Shoji::start("referrer", &url, Options::default());
    let mut page = shoji.page("paper.test");
    page.goto(&url);
    let wait = |host: &str, path: &str| {
        server
            .wait_for(host, path, Duration::from_secs(10))
            .unwrap_or_else(|| panic!("{host}{path} never loaded"))
    };
    let port = server.port;
    assert_eq!(
        wait("other-site.test", "/cross.gif").header("referer"),
        Some(format!("http://paper.test:{port}/").as_str())
    );
    assert_eq!(
        wait("cdn.paper.test", "/same.gif").header("referer"),
        Some(url.as_str()),
        "a same-site request keeps the page's own referrer policy"
    );
    page.click_on("#l");
    page.eval("navigator.sendBeacon('/beacon', 'b')");
    wait("paper.test", "/beacon");
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        !server.saw("paper.test", "/ping"),
        "the click ping was sent"
    );
}

#[test]
fn popup_rules_stop_ad_windows_and_popunders() {
    if !can_run() {
        return;
    }
    let server = site(vec![
        (
            "/",
            "<a id=ad href=\"http://popads.test:{P}/direct\" target=_blank>ad</a>\
             <button id=under onclick=\"var w=window.open('');setTimeout(function(){w.location='http://popads.test:{P}/late'},300)\">under</button>\
             <a id=ok href=\"http://fine.test:{P}/page\" target=_blank>ok</a>"
                .into(),
        ),
        ("/page", "<p>fine</p>".into()),
    ]);
    let url = server.url("opener.test", "/");
    let shoji = Shoji::start(
        "popups",
        &url,
        Options {
            settings: shield_settings(&["||popads.test^$popup"]),
            ..Default::default()
        },
    );
    let mut page = shoji.page("opener.test");
    wait_for_shield(&server, &mut page, &url, ("probe.test", "/p.gif"));
    page.goto(&url);
    // As if clicked: the tab that opens may hide this one.
    for id in ["ad", "under", "ok"] {
        page.call(
            "Runtime.evaluate",
            json!({ "expression": format!("document.getElementById('{id}').click()"), "userGesture": true }),
        );
    }
    assert!(
        server
            .wait_for("fine.test", "/page", Duration::from_secs(10))
            .is_some(),
        "an ordinary popup didn't open"
    );
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        !server.saw("popads.test", "/direct"),
        "the ad window opened"
    );
    assert!(!server.saw("popads.test", "/late"), "the popunder opened");
}
