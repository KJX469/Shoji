//! Byōbu's blocking score: a fixed page that loads well-known ad and tracker requests and the
//! ordinary resources a news site needs, all served locally, judged with the real filter lists.
//! The score may not fall below `score.json`; raise the baseline when it improves.

#![cfg(windows)]

mod common;

use common::*;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// How a resource is requested.
#[derive(Clone, Copy)]
enum As {
    Script,
    Image,
    Frame,
    Fetch,
    Style,
}

/// Requests a page full of ads and trackers makes. All of them should be stopped.
const ADS: &[(As, &str)] = &[
    // First-party, so only Byōbu's lists stop these (the first one tells they are loaded).
    (As::Script, "news.test/js/prebid.js"),
    (As::Script, "news.test/gpt.js"),
    (As::Image, "news.test/pixel.gif?ev=1"),
    (As::Image, "news.test/banner_ads/leader.gif"),
    (As::Script, "news.test/openx/www/delivery/ajs.php?zoneid=1"),
    (As::Script, "news.test/adserver.js"),
    (As::Image, "news.test/beacon.gif?u=1"),
    (As::Script, "news.test/piwik.js"),
    (As::Script, "news.test/matomo.js"),
    (As::Script, "news.test/gtm.js?id=GTM-X"),
    (As::Script, "news.test/fbevents.js"),
    (As::Image, "news.test/collect?v=1&tid=UA-1"),
    (As::Script, "news.test/clicktale.js"),
    (As::Script, "news.test/adobe-analytics.js"),
    (As::Script, "news.test/pagead/conversion.js"),
    (
        As::Script,
        "pagead2.googlesyndication.com/pagead/js/adsbygoogle.js",
    ),
    (As::Script, "securepubads.g.doubleclick.net/tag/js/gpt.js"),
    (As::Script, "www.googletagservices.com/tag/js/gpt.js"),
    (As::Script, "c.amazon-adsystem.com/aax2/apstag.js"),
    (As::Script, "cdn.taboola.com/libtrc/newsco/loader.js"),
    (As::Script, "widgets.outbrain.com/outbrain.js"),
    (As::Script, "static.criteo.net/js/ld/publishertag.js"),
    (As::Script, "acdn.adnxs.com/ast/ast.js"),
    (As::Script, "ads.pubmatic.com/AdServer/js/pwt/1/2/pwt.js"),
    (As::Script, "www.google-analytics.com/analytics.js"),
    (As::Script, "www.googletagmanager.com/gtag/js?id=G-TEST1"),
    (As::Script, "connect.facebook.net/en_US/fbevents.js"),
    (As::Script, "sb.scorecardresearch.com/beacon.js"),
    (As::Script, "static.hotjar.com/c/hotjar-1.js"),
    (
        As::Script,
        "cdn.segment.com/analytics.js/v1/abc/analytics.min.js",
    ),
    (As::Script, "js.hs-analytics.net/analytics/1/1.js"),
    (As::Script, "bat.bing.com/bat.js"),
    (As::Script, "snap.licdn.com/li.lms-analytics/insight.min.js"),
    (As::Script, "static.ads-twitter.com/uwt.js"),
    (As::Script, "www.clarity.ms/tag/abc123"),
    (As::Script, "cdn.mxpnl.com/libs/mixpanel-2-latest.min.js"),
    (As::Script, "script.crazyegg.com/pages/scripts/0000/0000.js"),
    (As::Script, "cdn.heapanalytics.com/js/heap-1.js"),
    (As::Script, "s.pinimg.com/ct/core.js"),
    (As::Script, "analytics.tiktok.com/i18n/pixel/events.js"),
    (As::Script, "js-agent.newrelic.com/nr-1.js"),
    (As::Script, "cdn.optimizely.com/js/1.js"),
    (As::Script, "static.doubleclick.net/instream/ad_status.js"),
    (As::Script, "www.googleadservices.com/pagead/conversion.js"),
    (As::Script, "cdn.adsafeprotected.com/iasPET.1.js"),
    (As::Script, "z.moatads.com/x/moatad.js"),
    (As::Script, "c.statcounter.com/counter/counter.js"),
    (As::Script, "s7.addthis.com/js/300/addthis_widget.js"),
    (As::Script, "cdn.permutive.com/x-web.js"),
    (As::Script, "ced.sascdn.com/tag/1/smart.js"),
    (As::Image, "www.facebook.com/tr?id=1&ev=PageView"),
    (As::Image, "px.ads.linkedin.com/collect?pid=1"),
    (As::Image, "t.co/i/adsct?txn_id=1"),
    (As::Image, "bat.bing.com/action/0?ti=1"),
    (As::Image, "pixel.quantserve.com/pixel/p-x.gif"),
    (As::Image, "b.scorecardresearch.com/p?c1=2&c2=1"),
    (As::Image, "ib.adnxs.com/getuid?x"),
    (As::Image, "pixel.rubiconproject.com/tap.php?v=1"),
    (As::Image, "ad.doubleclick.net/ddm/activity/src=1"),
    (As::Image, "www.google-analytics.com/collect?v=1&t=pageview"),
    (As::Image, "stats.g.doubleclick.net/r/collect?v=1"),
    (As::Image, "secure.adnxs.com/px?id=1"),
    (As::Image, "ct.pinterest.com/v3/?tid=1"),
    (As::Image, "analytics.twitter.com/i/adsct?p_id=1"),
    (
        As::Image,
        "googleads.g.doubleclick.net/pagead/viewthroughconversion/1/",
    ),
    (As::Image, "sync.outbrain.com/cookie-sync?p=1"),
    (As::Image, "match.adsrvr.org/track/cmf/generic"),
    (As::Image, "sync.search.spotxchange.com/partner?x"),
    (As::Image, "cm.g.doubleclick.net/pixel?google_nid=1"),
    (
        As::Frame,
        "tpc.googlesyndication.com/safeframe/1-0-40/html/container.html",
    ),
    (
        As::Frame,
        "googleads.g.doubleclick.net/pagead/ads?client=ca-pub-1",
    ),
    (As::Frame, "s.amazon-adsystem.com/iu3?d=1"),
    (
        As::Fetch,
        "www.google-analytics.com/g/collect?v=2&tid=G-TEST1",
    ),
    (As::Fetch, "api.segment.io/v1/t"),
    (As::Fetch, "in.hotjar.com/api/v2/client/sites/1/visit-data"),
    (As::Fetch, "api-js.mixpanel.com/track/?ip=1"),
    (As::Fetch, "rs.fullstory.com/rec/bundle"),
    (As::Fetch, "c.bing.com/c.gif"),
    (As::Fetch, "prebid.adnxs.com/pbs/v1/openrtb2/auction"),
    (As::Fetch, "hbopenbid.pubmatic.com/translator?source=prebid"),
];

/// What the page needs to work. None of these may be stopped.
const NEEDED: &[(As, &str)] = &[
    (As::Script, "news.test/app.js"),
    (As::Style, "news.test/site.css"),
    (As::Image, "news.test/hero.jpg"),
    (
        As::Script,
        "cdn.jsdelivr.net/npm/jquery@3.7.1/dist/jquery.min.js",
    ),
    (
        As::Script,
        "ajax.googleapis.com/ajax/libs/jquery/3.7.1/jquery.min.js",
    ),
    (As::Script, "code.jquery.com/jquery-3.7.1.min.js"),
    (
        As::Script,
        "cdnjs.cloudflare.com/ajax/libs/lodash.js/4.17.21/lodash.min.js",
    ),
    (
        As::Script,
        "unpkg.com/react@18.3.1/umd/react.production.min.js",
    ),
    (As::Style, "fonts.googleapis.com/css2?family=Roboto"),
    (
        As::Image,
        "upload.wikimedia.org/wikipedia/commons/a/a9/Example.jpg",
    ),
    (As::Image, "i.ytimg.com/vi/abc/hqdefault.jpg"),
    (As::Frame, "www.youtube.com/embed/abc"),
    (As::Frame, "player.vimeo.com/video/1"),
    (As::Script, "js.stripe.com/v3/"),
    (As::Script, "maps.googleapis.com/maps/api/js?key=test"),
    (As::Script, "www.paypal.com/sdk/js?client-id=sb"),
    (As::Fetch, "news.test/api/comments?story=1"),
    (As::Script, "www.google.com/recaptcha/api.js"),
    (As::Script, "challenges.cloudflare.com/turnstile/v0/api.js"),
];

/// Ad containers a news page carries; the generic rules should hide them.
const BOXES: &[&str] = &[
    "<ins class=\"adsbygoogle\" style=\"display:block;height:90px\"></ins>",
    "<div id=\"taboola-below-article-thumbnails\" style=\"height:90px\"></div>",
    "<div class=\"OUTBRAIN\" style=\"height:90px\"></div>",
    "<div id=\"div-gpt-ad-1234567890-0\" style=\"height:90px\"></div>",
    "<div class=\"ad-slot\" style=\"height:90px\"></div>",
    "<div class=\"advertisement\" style=\"height:90px\"></div>",
    "<div id=\"google_ads_iframe_1\" style=\"height:90px\"></div>",
    "<div class=\"sponsored-content\" style=\"height:90px\"></div>",
];

fn split(spec: &str) -> (&str, String) {
    let (host, rest) = spec.split_once('/').unwrap();
    (host, format!("/{rest}"))
}

fn tag(kind: As, url: &str) -> String {
    match kind {
        As::Script => format!("<script src=\"{url}\"></script>"),
        As::Image => format!("<img src=\"{url}\" width=1 height=1>"),
        As::Frame => format!("<iframe src=\"{url}\" width=10 height=10></iframe>"),
        As::Style => format!("<link rel=stylesheet href=\"{url}\">"),
        As::Fetch => format!(
            "<script>fetch({}).catch(()=>{{}})</script>",
            serde_json::to_string(url).unwrap()
        ),
    }
}

#[test]
fn blocking_score_holds() {
    if !can_run() {
        return;
    }
    let port = Arc::new(std::sync::OnceLock::<u16>::new());
    let p2 = port.clone();
    let server = Server::start(move |hit| {
        let port = *p2.get().unwrap();
        if hit.host == "news.test" && hit.path() == "/" {
            let mut html = String::from("<!doctype html><title>Score</title><h1>Story</h1>");
            for (i, b) in BOXES.iter().enumerate() {
                html.push_str(&b.replacen('>', &format!(" data-box={i}>"), 1));
            }
            html.push_str("<p id=story>Story text.</p>");
            for (kind, spec) in ADS.iter().chain(NEEDED) {
                let (host, path) = split(spec);
                html.push_str(&tag(*kind, &format!("http://{host}:{port}{path}")));
            }
            return Reply::html(html).header("X-Test-Server", "1");
        }
        if hit.path().ends_with(".js") || hit.target.contains("/js") {
            return Reply::js("").header("X-Test-Server", "1");
        }
        Reply::empty().header("X-Test-Server", "1")
    });
    let _ = port.set(server.port);
    let url = server.url("news.test", "/");
    let shoji = Shoji::start(
        "score",
        &url,
        Options {
            real_lists: true,
            ..Default::default()
        },
    );
    let mut page = shoji.page("news.test");
    let probe = split(ADS[0].1);
    wait_for_shield(&server, &mut page, &url, (probe.0, &probe.1));
    page.call("Network.enable", json!({}));
    page.goto(&url);
    std::thread::sleep(Duration::from_secs(4));
    page.eval("0");

    // What happened to each request, from the page's own network log. Byōbu answers a blocked
    // request with a 403 "Blocked" or a stand-in (no test-server header); the engine's tracking
    // prevention fails it as blocked by the client. Anything else got onto the network: hosts
    // on the HSTS preload list are upgraded to https and fail against this plain server.
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Fate {
        Byobu,
        Engine,
        Network,
    }
    let strip = |u: &str| {
        u.replace(&format!(":{}", server.port), "")
            .replace("https://", "http://")
    };
    let mut urls = std::collections::HashMap::new();
    let mut fates: std::collections::HashMap<String, Fate> = Default::default();
    for e in &page.events {
        let p = &e["params"];
        let id = p["requestId"].to_string();
        match e["method"].as_str().unwrap_or("") {
            "Network.requestWillBeSent" => {
                urls.insert(id, strip(p["request"]["url"].as_str().unwrap_or("")));
            }
            "Network.responseReceived" => {
                let r = &p["response"];
                let ours = r["headers"]
                    .as_object()
                    .is_some_and(|h| h.keys().any(|k| k.eq_ignore_ascii_case("x-test-server")));
                let fate = if ours { Fate::Network } else { Fate::Byobu };
                if let Some(u) = urls.get(&id) {
                    fates.insert(u.clone(), fate);
                }
            }
            "Network.loadingFailed" => {
                let Some(u) = urls.get(&id) else { continue };
                if fates.contains_key(u) {
                    continue; // already answered (a 403 is then cancelled)
                }
                let fate = if p["errorText"] == "net::ERR_BLOCKED_BY_CLIENT" {
                    Fate::Engine
                } else {
                    Fate::Network
                };
                fates.insert(u.clone(), fate);
            }
            _ => {}
        }
    }
    let fate = |spec: &str| -> Fate {
        let (host, path) = split(spec);
        let p = path.split('?').next().unwrap().to_string();
        if server
            .hits()
            .iter()
            .any(|h| h.host == host && h.path() == p)
        {
            return Fate::Network;
        }
        let want = format!("http://{spec}");
        // Frames in other processes aren't in this log; what the server didn't see was blocked.
        fates.get(&want).copied().unwrap_or(Fate::Byobu)
    };
    let missed: Vec<&str> = ADS
        .iter()
        .map(|a| a.1)
        .filter(|s| fate(s) == Fate::Network)
        .collect();
    let broken: Vec<&str> = NEEDED
        .iter()
        .map(|a| a.1)
        .filter(|s| fate(s) != Fate::Network)
        .collect();
    let by_byobu = ADS.iter().filter(|a| fate(a.1) == Fate::Byobu).count();
    let blocked = ADS.len() - missed.len();
    let hidden = page
        .eval(
            "[...document.querySelectorAll('[data-box]')].filter(e=>getComputedStyle(e).display==='none').length",
        )
        .as_u64()
        .unwrap_or(0);
    eprintln!(
        "Byōbu score: {blocked}/{} requests blocked ({by_byobu} by Byōbu, {} by the engine's tracking prevention), {hidden}/{} ad boxes hidden, {} needed requests stopped",
        ADS.len(),
        blocked - by_byobu,
        BOXES.len(),
        broken.len()
    );
    for m in &missed {
        eprintln!("  missed {m}");
    }
    for b in &broken {
        eprintln!("  broke {b} ({:?})", fate(b));
    }

    let base: serde_json::Value =
        serde_json::from_str(include_str!("score.json")).expect("score.json");
    let at_least = |k: &str| base[k].as_u64().unwrap_or(0);
    assert!(
        blocked as u64 >= at_least("blocked"),
        "blocked {blocked}, baseline {}",
        at_least("blocked")
    );
    assert!(
        by_byobu as u64 >= at_least("by_byobu"),
        "blocked by Byōbu {by_byobu}, baseline {}",
        at_least("by_byobu")
    );
    assert!(
        hidden >= at_least("hidden"),
        "hidden {hidden}, baseline {}",
        at_least("hidden")
    );
    assert!(
        broken.is_empty(),
        "needed requests were stopped: {broken:?}"
    );
}
