//! Shoji's own interface for screen readers and keyboards: every control in the toolbar strip and
//! Shoji's pages has a name a screen reader can read out, can be reached with the keyboard, and
//! keeps its text in High Contrast (forced colours).

#![cfg(windows)]

mod common;

use common::*;
use serde_json::{json, Value};
use std::time::Duration;

/// Controls without an accessible name, from the page's accessibility tree.
fn unnamed(page: &mut Page) -> Vec<String> {
    let tree = page.call("Accessibility.getFullAXTree", json!({}));
    let roles = [
        "button",
        "link",
        "textbox",
        "checkbox",
        "combobox",
        "switch",
        "menuitem",
        "tab",
        "searchbox",
        "slider",
        "radio",
        "listbox",
        "option",
    ];
    tree["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|n| n["ignored"] != true)
        .filter(|n| roles.contains(&n["role"]["value"].as_str().unwrap_or("")))
        .filter(|n| n["name"]["value"].as_str().unwrap_or("").trim().is_empty())
        .map(|n| {
            let dom = n["backendDOMNodeId"].as_i64().unwrap_or(0);
            let html = page.call("DOM.describeNode", json!({ "backendNodeId": dom }));
            let node: &Value = &html["node"];
            let attrs = node["attributes"].as_array().cloned().unwrap_or_default();
            let pairs: Vec<String> = attrs
                .chunks(2)
                .map(|p| {
                    format!(
                        "{}={}",
                        p[0].as_str().unwrap_or(""),
                        p.get(1).and_then(|v| v.as_str()).unwrap_or("")
                    )
                })
                .filter(|p| {
                    p.starts_with("id=") || p.starts_with("class=") || p.starts_with("title=")
                })
                .collect();
            format!(
                "{} <{}{}>",
                n["role"]["value"].as_str().unwrap_or(""),
                node["localName"].as_str().unwrap_or(""),
                if pairs.is_empty() {
                    String::new()
                } else {
                    format!(" {}", pairs.join(" "))
                }
            )
        })
        .collect()
}

/// Visible controls the keyboard can't reach, and images with no alternative text.
const KEYBOARD: &str = "(()=>{const vis=e=>{const r=e.getBoundingClientRect();return r.width>0&&r.height>0&&getComputedStyle(e).visibility!=='hidden'};\
const out=[];for(const e of document.querySelectorAll('button,a[href],input,select,textarea,[role=button],[role=switch],[role=tab],[onclick]')){\
if(!vis(e)||e.disabled)continue;const t=e.tabIndex;if(t<0&&!e.closest('[role=tablist],[role=menu],[role=listbox]'))out.push('no keyboard: '+e.tagName.toLowerCase()+(e.id?'#'+e.id:'')+(e.className&&typeof e.className==='string'?'.'+e.className.split(' ')[0]:''))}\
for(const i of document.querySelectorAll('img')){if(vis(i)&&!i.hasAttribute('alt'))out.push('no alt: img'+(i.id?'#'+i.id:'')+(i.className?'.'+String(i.className).split(' ')[0]:''))}\
return out})()";

/// Text that disappears in forced colours (the same colour as its background).
const FORCED: &str = "(()=>{const out=[];for(const e of document.querySelectorAll('button,a,span,b,label,p,h1,h2,h3,div')){\
if(!e.childNodes.length||![...e.childNodes].some(n=>n.nodeType===3&&n.textContent.trim()))continue;const r=e.getBoundingClientRect();if(!r.width||!r.height)continue;\
const s=getComputedStyle(e);if(s.color===s.backgroundColor&&s.backgroundColor!=='rgba(0, 0, 0, 0)')out.push((e.id||e.tagName)+': '+s.color)}return out.slice(0,20)})()";

fn audit(page: &mut Page, what: &str) -> Vec<String> {
    let mut problems: Vec<String> = unnamed(page)
        .into_iter()
        .map(|p| format!("{what}: no name: {p}"))
        .collect();
    let keys = page.eval(KEYBOARD);
    for k in keys.as_array().cloned().unwrap_or_default() {
        problems.push(format!("{what}: {}", k.as_str().unwrap_or("")));
    }
    page.call(
        "Emulation.setEmulatedMedia",
        json!({ "features": [{ "name": "forced-colors", "value": "active" }] }),
    );
    std::thread::sleep(Duration::from_millis(300));
    for f in page.eval(FORCED).as_array().cloned().unwrap_or_default() {
        problems.push(format!(
            "{what}: invisible in High Contrast: {}",
            f.as_str().unwrap_or("")
        ));
    }
    page.call("Emulation.setEmulatedMedia", json!({ "features": [] }));
    problems
}

#[test]
fn shojis_own_interface_is_usable_without_a_mouse_or_eyes() {
    if !can_run() {
        return;
    }
    let server = Server::start(|_| Reply::html("<title>Page</title><p>page</p>"));
    let shoji = Shoji::start("a11y", &server.url("page.test", "/"), Options::default());
    let mut page = shoji.page("page.test");
    let mut problems = Vec::new();
    let mut strip = shoji.chrome();
    problems.extend(audit(&mut strip, "toolbar"));
    for name in [
        "newtab",
        "settings",
        "byobu",
        "history",
        "downloads",
        "bookmarks",
        "passwords",
    ] {
        page.goto(&format!("http://shoji.{name}/"));
        std::thread::sleep(Duration::from_millis(800));
        problems.extend(audit(&mut page, name));
    }
    // 200% display scaling: a normal window is then about 640 logical pixels wide.
    let narrow = |p: &mut Page, what: &str, problems: &mut Vec<String>| {
        p.call(
            "Emulation.setDeviceMetricsOverride",
            json!({ "width": 640, "height": 480, "deviceScaleFactor": 2, "mobile": false }),
        );
        std::thread::sleep(Duration::from_millis(400));
        let over = p.eval("document.documentElement.scrollWidth - innerWidth");
        if over.as_i64().unwrap_or(0) > 2 {
            problems.push(format!("{what}: {over} px wider than the window at 200%"));
        }
        p.call("Emulation.clearDeviceMetricsOverride", json!({}));
    };
    let mut strip = shoji.chrome();
    narrow(&mut strip, "toolbar", &mut problems);
    strip.call(
        "Emulation.setDeviceMetricsOverride",
        json!({ "width": 640, "height": 84, "deviceScaleFactor": 2, "mobile": false }),
    );
    std::thread::sleep(Duration::from_millis(400));
    let off = strip.eval(
        "['btnMenu','omnibox'].filter(id=>{const e=document.getElementById(id);if(!e)return true;const r=e.getBoundingClientRect();return r.right>innerWidth+1||r.width<1})",
    );
    for id in off.as_array().cloned().unwrap_or_default() {
        problems.push(format!("toolbar: {id} off screen at 200%"));
    }
    strip.call("Emulation.clearDeviceMetricsOverride", json!({}));
    for name in ["newtab", "settings", "byobu", "history"] {
        page.goto(&format!("http://shoji.{name}/"));
        std::thread::sleep(Duration::from_millis(600));
        narrow(&mut page, name, &mut problems);
    }
    for p in &problems {
        eprintln!("{p}");
    }
    assert!(
        problems.is_empty(),
        "{} accessibility problems",
        problems.len()
    );
}
