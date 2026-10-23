//! Real extensions from the Chrome Web Store: `cargo test -p shoji --test extensions_live -- --ignored --nocapture`.

#![cfg(windows)]

mod common;

use common::*;
use std::time::{Duration, Instant};

const EXTENSIONS: &[(&str, &str, &str)] = &[
    (
        "Bitwarden",
        "nngceckbapebfimnlniiiahkandclblb",
        "bitwarden-password-manager",
    ),
    (
        "1Password",
        "aeblfdkhhhdcdjpifhhbdiojplfjncoa",
        "1password-%E2%80%93-password-mana",
    ),
    (
        "Grammarly",
        "kbfnbcaeplbcioakkpcpgfkobkghlhen",
        "grammarly-ai-writing-and-gra",
    ),
];

/// The toolbar strip: the inline page with the tab strip in it.
fn chrome(shoji: &Shoji) -> Page {
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        for t in shoji.targets() {
            let url = t["url"].as_str().unwrap_or("");
            if t["type"] == "page" && (url.starts_with("data:") || url == "about:blank") {
                if let Some(mut p) =
                    Page::connect_to(t["webSocketDebuggerUrl"].as_str().unwrap_or(""))
                {
                    if p.eval("document.querySelectorAll('#tabs .tab').length") != 0 {
                        return p;
                    }
                }
            }
        }
        assert!(Instant::now() < end, "no chrome strip");
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn wait<T>(what: &str, secs: u64, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if Instant::now() > end {
            eprintln!("  timed out: {what}");
            return None;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[test]
#[ignore = "installs real extensions from the Web Store"]
fn real_extensions_install_and_open() {
    if !can_run() {
        return;
    }
    let mut failures = Vec::new();
    let only = std::env::var("ONLY").unwrap_or_default();
    for (name, id, slug) in EXTENSIONS.iter().filter(|e| only.is_empty() || e.0 == only) {
        let url = format!("https://chromewebstore.google.com/detail/{slug}/{id}");
        let shoji = Shoji::start(
            &format!("ext-{id}"),
            &url,
            Options {
                local_hosts: false,
                // No dead proxy: the extension download goes through Shoji's own client.
                real_lists: true,
                ..Default::default()
            },
        );
        let _ = shoji.page("chromewebstore.google.com");
        let mut strip = chrome(&shoji);
        // The toast offering to add it.
        let toast = wait("the Add to Shoji toast", 30, || {
            let t = strip.eval("(document.getElementById('toastTitle')||{}).textContent||''");
            let acts = strip.eval("[...document.querySelectorAll('#toastActs button')].map(b=>b.textContent).join('|')");
            acts.as_str()
                .unwrap_or("")
                .contains("Add to Shoji")
                .then(|| {
                    format!(
                        "{} [{}]",
                        t.as_str().unwrap_or(""),
                        acts.as_str().unwrap_or("")
                    )
                })
        });
        eprintln!("{name}: toast {toast:?}");
        if toast.is_none() {
            let urls: Vec<String> = shoji
                .targets()
                .iter()
                .map(|t| t["url"].as_str().unwrap_or("").chars().take(140).collect())
                .collect();
            eprintln!("  targets: {urls:?}");
            failures.push(format!("{name}: no toast"));
            continue;
        }
        strip.eval("[...document.querySelectorAll('#toastActs button')].find(b=>b.textContent==='Add to Shoji').click()");
        for _ in 0..2 {
            std::thread::sleep(Duration::from_secs(5));
            let t = strip.eval("[document.getElementById('toastTitle').textContent, document.getElementById('toastText').textContent].join(' / ')");
            let files: Vec<String> = std::fs::read_dir(shoji.data_dir().join("Extensions"))
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            eprintln!("  toast: {t}  extensions dir: {files:?}");
        }
        let installing = Instant::now();
        let worker = wait("the service worker", 900, || {
            shoji.targets().into_iter().find(|t| {
                t["type"] == "service_worker"
                    && t["url"]
                        .as_str()
                        .unwrap_or("")
                        .starts_with(&format!("chrome-extension://{id}/"))
            })
        });
        eprintln!("{name}: install took {:?}", installing.elapsed());
        eprintln!(
            "{name}: service worker {}",
            worker.as_ref().map_or("none".into(), |w| w["url"]
                .as_str()
                .unwrap_or("")
                .to_string())
        );
        if worker.is_none() {
            failures.push(format!("{name}: no service worker"));
        }
        // Its toolbar button (new extensions start pinned) and the puzzle menu button.
        let mut strip = chrome(&shoji);
        let buttons = wait("its toolbar button", 20, || {
            let v = strip.eval("JSON.stringify({ext:!document.getElementById('btnExt').hidden,btns:[...document.querySelectorAll('#extBtns button')].map(b=>b.title)})");
            let s = v.as_str().unwrap_or("").to_string();
            s.contains(&format!("\"{}", &name[..3])).then_some(s)
        });
        eprintln!("{name}: toolbar {buttons:?}");
        if buttons.is_none() {
            failures.push(format!("{name}: no toolbar button"));
            continue;
        }
        let at = strip.eval(&format!(
            "(()=>{{const r=[...document.querySelectorAll('#extBtns button')].find(b=>b.title.startsWith({:?})).getBoundingClientRect();return [r.x+r.width/2,r.y+r.height/2]}})()",
            &name[..3]
        ));
        strip.click(at[0].as_f64().unwrap(), at[1].as_f64().unwrap());
        let mut seen = Vec::new();
        for _ in 0..50 {
            for t in shoji.targets() {
                let u = t["url"].as_str().unwrap_or("").to_string();
                if u.starts_with("chrome-extension://") && !seen.contains(&u) && t["type"] == "page"
                {
                    seen.push(u);
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        eprintln!("{name}: extension pages seen in 5 s after the click: {seen:?}");
        let err = std::fs::read_to_string(shoji.dir.join("err.log")).unwrap_or_default();
        for l in err.lines().filter(|l| l.contains("extension popup")) {
            eprintln!("  STDERR {l}");
        }
        let toast = strip.eval("[document.getElementById('toastTitle').textContent, document.getElementById('toastText').textContent].join(' / ')");
        eprintln!("  TOAST {toast}");
        std::thread::sleep(Duration::from_secs(3));
        for t in shoji.targets() {
            eprintln!(
                "  target {} {}",
                t["type"],
                t["url"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(120)
                    .collect::<String>()
            );
        }
        let popup = wait("the popup", 20, || {
            shoji.targets().into_iter().find(|t| {
                t["type"] == "page"
                    && t["url"]
                        .as_str()
                        .unwrap_or("")
                        .starts_with(&format!("chrome-extension://{id}/"))
            })
        });
        match popup {
            Some(p) => {
                let mut page =
                    Page::connect_to(p["webSocketDebuggerUrl"].as_str().unwrap_or("")).unwrap();
                std::thread::sleep(Duration::from_secs(4));
                let info = page.eval("JSON.stringify({x:screenX,y:screenY,w:innerWidth,h:innerHeight,doc:[document.documentElement.scrollWidth,document.documentElement.scrollHeight],text:(document.body&&document.body.innerText||'').slice(0,120).replace(/\\s+/g,' ')})");
                eprintln!(
                    "{name}: popup {} {}",
                    p["url"].as_str().unwrap_or(""),
                    info.as_str().unwrap_or("")
                );
                let shot = page.call(
                    "Page.captureScreenshot",
                    serde_json::json!({ "format": "png" }),
                );
                if let Some(data) = shot["data"].as_str() {
                    use base64::Engine as _;
                    let file = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
                        .join(format!("popup-{name}.png"));
                    let _ = std::fs::write(
                        &file,
                        base64::engine::general_purpose::STANDARD
                            .decode(data)
                            .unwrap_or_default(),
                    );
                    eprintln!("{name}: screenshot {}", file.display());
                }
                let v: serde_json::Value =
                    serde_json::from_str(info.as_str().unwrap_or("{}")).unwrap_or_default();
                let fits = v["doc"][0].as_i64().unwrap_or(9999) <= v["w"].as_i64().unwrap_or(0) + 2
                    && v["doc"][1].as_i64().unwrap_or(9999) <= v["h"].as_i64().unwrap_or(0) + 2;
                if v["text"].as_str().unwrap_or("").trim().is_empty() {
                    failures.push(format!("{name}: empty popup"));
                } else if !fits {
                    failures.push(format!("{name}: popup content doesn't fit ({info})"));
                }
            }
            None => {
                let err = std::fs::read_to_string(shoji.dir.join("err.log")).unwrap_or_default();
                for l in err.lines().filter(|l| l.contains("extension popup")) {
                    eprintln!("  STDERR2 {l}");
                }
                for t in shoji.targets() {
                    eprintln!(
                        "  after click: {} {}",
                        t["type"],
                        t["url"]
                            .as_str()
                            .unwrap_or("")
                            .chars()
                            .take(120)
                            .collect::<String>()
                    );
                }
                let err = std::fs::read_to_string(shoji.dir.join("err.log")).unwrap_or_default();
                eprintln!(
                    "  err.log tail: {}",
                    err.lines().rev().take(15).collect::<Vec<_>>().join(" | ")
                );
                failures.push(format!("{name}: no popup"))
            }
        }
        // Installed now: its store page doesn't offer it again.
        let mut tab = shoji.page("chromewebstore.google.com");
        tab.goto(&url);
        std::thread::sleep(Duration::from_secs(8));
        let mut strip = chrome(&shoji);
        let offered = strip.eval("[...document.querySelectorAll('#toastActs button')].some(b=>b.textContent==='Add to Shoji') && getComputedStyle(document.getElementById('toast')).opacity!=='0'");
        eprintln!("{name}: offered again on its store page: {offered}");
        if offered == true {
            failures.push(format!("{name}: offered again although installed"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The open puzzle menu (the overlay with extension rows in it).
fn puzzle_menu(shoji: &Shoji) -> Option<Page> {
    wait("the puzzle menu", 10, || {
        shoji.targets().into_iter().find_map(|t| {
            let url = t["url"].as_str().unwrap_or("");
            if t["type"] != "page" || !(url.starts_with("data:") || url == "about:blank") {
                return None;
            }
            let mut p = Page::connect_to(t["webSocketDebuggerUrl"].as_str()?)?;
            (p.eval("document.querySelectorAll('li .ico').length") != 0).then_some(p)
        })
    })
}

/// Its row for `name`: the logo loaded, which pin glyph it shows, where the pin button is.
fn menu_row(menu: &mut Page, name: &str) -> serde_json::Value {
    let v = menu.eval(&format!(
        "JSON.stringify((()=>{{const li=[...document.querySelectorAll('li')].find(l=>l.innerText.includes({name:?}));if(!li)return null;\
         const img=li.querySelector('img.ico');const pin=li.querySelector('.tool.pin,.tool.pinned');const r=pin&&pin.getBoundingClientRect();\
         return {{logo:!!(img&&img.complete&&img.naturalWidth>0),pinned:!!li.querySelector('.tool.pinned'),remove:!!li.querySelector('.tool.remove'),at:r&&[r.x+r.width/2,r.y+r.height/2]}}}})())"
    ));
    serde_json::from_str(v.as_str().unwrap_or("null")).unwrap_or_default()
}

fn on_toolbar(strip: &mut Page, name: &str) -> bool {
    strip.eval(&format!(
        "[...document.querySelectorAll('#extBtns button')].some(b=>b.title.startsWith({:?}))",
        &name[..3]
    )) == true
}

#[test]
#[ignore = "installs a real extension from the Web Store"]
fn store_button_logos_and_pins() {
    if !can_run() {
        return;
    }
    let only = std::env::var("ONLY").unwrap_or_else(|_| "1Password".into());
    let (name, id, slug) = *EXTENSIONS
        .iter()
        .find(|e| e.0 == only)
        .expect("a known extension");
    let url = format!("https://chromewebstore.google.com/detail/{slug}/{id}");
    let shoji = Shoji::start(
        "ext-pins",
        &url,
        Options {
            local_hosts: false,
            real_lists: true,
            ..Default::default()
        },
    );
    let mut failures = Vec::new();
    let mut store = shoji.page("chromewebstore.google.com");
    // The store's own button, pressed for real.
    let at = wait("the Add to Chrome button", 30, || {
        let v = store.eval("(()=>{const b=[...document.querySelectorAll('button')].find(b=>/add to chrome/i.test(b.innerText));if(!b)return null;const r=b.getBoundingClientRect();return [r.x+r.width/2,r.y+r.height/2]})()");
        v.is_array().then_some(v)
    })
    .expect("no Add to Chrome button");
    store.click(at[0].as_f64().unwrap(), at[1].as_f64().unwrap());
    let mut strip = chrome(&shoji);
    let mut toasts = Vec::new();
    let added = wait("Added to Shoji", 300, || {
        let t = strip.eval("[document.getElementById('toastTitle').textContent, document.getElementById('toastText').textContent].join(' / ')");
        let t = t.as_str().unwrap_or("").to_string();
        if !toasts.contains(&t) {
            toasts.push(t.clone());
        }
        t.contains("Added to Shoji").then_some(())
    });
    eprintln!("{name}: store button toasts {toasts:?}");
    if added.is_none() {
        failures.push("store button: never added".to_string());
    }
    let worker = wait("the service worker", 60, || {
        shoji.targets().into_iter().find(|t| {
            t["type"] == "service_worker"
                && t["url"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with(&format!("chrome-extension://{id}/"))
        })
    });
    eprintln!(
        "{name}: running after the store button: {}",
        worker.is_some()
    );
    if worker.is_none() {
        failures.push("store button: extension not running".to_string());
    }

    // The puzzle menu: logo, pin and remove; unpinning and pinning act at once and keep the menu open.
    let mut strip = chrome(&shoji);
    if wait("its toolbar button", 20, || {
        on_toolbar(&mut strip, name).then_some(())
    })
    .is_none()
    {
        failures.push("new extension not on the toolbar".to_string());
    }
    strip.click_on("#btnExt");
    let mut menu = puzzle_menu(&shoji).expect("the puzzle menu didn't open");
    let row = menu_row(&mut menu, name);
    eprintln!("{name}: menu row {row}");
    if row["logo"] != true {
        failures.push("menu: no logo".to_string());
    }
    if row["pinned"] != true || row["remove"] != true {
        failures.push(format!("menu: pin/remove tools wrong ({row})"));
    }
    menu.click(
        row["at"][0].as_f64().unwrap_or(0.0),
        row["at"][1].as_f64().unwrap_or(0.0),
    );
    let gone = wait("its toolbar button to go", 3, || {
        (!on_toolbar(&mut strip, name)).then_some(())
    });
    let mut menu = puzzle_menu(&shoji).expect("the menu closed after unpinning");
    let row = menu_row(&mut menu, name);
    eprintln!(
        "{name}: after unpin, toolbar button gone {} menu row {row}",
        gone.is_some()
    );
    if gone.is_none() || row["pinned"] != false {
        failures.push("unpin: didn't act at once".to_string());
    }

    // A restart keeps it unpinned.
    shoji.restart();
    let mut strip = chrome(&shoji);
    std::thread::sleep(Duration::from_secs(10));
    let still_off = !on_toolbar(&mut strip, name);
    eprintln!("{name}: still unpinned after a restart: {still_off}");
    if !still_off {
        failures.push("unpin lost on restart".to_string());
    }
    if wait("the puzzle button after a restart", 30, || {
        (strip.eval("document.getElementById('btnExt').hidden") == false).then_some(())
    })
    .is_none()
    {
        failures.push("puzzle button hidden after a restart".to_string());
    }
    strip.click_on("#btnExt");
    let menu = puzzle_menu(&shoji);
    let mut menu = menu.expect("the puzzle menu didn't open after a restart");
    let row = menu_row(&mut menu, name);
    menu.click(
        row["at"][0].as_f64().unwrap_or(0.0),
        row["at"][1].as_f64().unwrap_or(0.0),
    );
    let back = wait("its toolbar button to come back", 3, || {
        on_toolbar(&mut strip, name).then_some(())
    });
    eprintln!(
        "{name}: pinned again, on the toolbar at once: {}",
        back.is_some()
    );
    if back.is_none() {
        failures.push("pin: didn't act at once".to_string());
    }
    shoji.restart();
    let mut strip = chrome(&shoji);
    let kept = wait("its toolbar button after a restart", 20, || {
        on_toolbar(&mut strip, name).then_some(())
    });
    eprintln!("{name}: still pinned after a restart: {}", kept.is_some());
    if kept.is_none() {
        failures.push("pin lost on restart".to_string());
    }

    // Killed outright, the engine forgets its extensions; Shoji adds them back when it starts.
    shoji.kill_and_start();
    let mut strip = chrome(&shoji);
    let running = wait("the service worker after a kill", 60, || {
        shoji.targets().into_iter().find(|t| {
            t["type"] == "service_worker"
                && t["url"]
                    .as_str()
                    .unwrap_or("")
                    .starts_with(&format!("chrome-extension://{id}/"))
        })
    });
    let button = wait("its toolbar button after a kill", 30, || {
        on_toolbar(&mut strip, name).then_some(())
    });
    eprintln!(
        "{name}: after a kill, running {} on the toolbar {}",
        running.is_some(),
        button.is_some()
    );
    if running.is_none() || button.is_none() {
        failures.push("not back after a kill".to_string());
    }
    if wait("the puzzle button after a kill", 30, || {
        (strip.eval("document.getElementById('btnExt').hidden") == false).then_some(())
    })
    .is_some()
    {
        strip.click_on("#btnExt");
        match puzzle_menu(&shoji) {
            Some(mut menu) => {
                let row = menu_row(&mut menu, name);
                eprintln!("{name}: menu row after a kill {row}");
                if row["logo"] != true {
                    failures.push("no logo in the menu after a kill".to_string());
                }
                menu.call("Input.dispatchKeyEvent", serde_json::json!({ "type": "keyDown", "key": "Escape", "code": "Escape", "windowsVirtualKeyCode": 27 }));
            }
            None => failures.push("puzzle menu empty after a kill".to_string()),
        }
    } else {
        failures.push("puzzle button hidden after a kill".to_string());
    }

    // Settings > Extensions: logo and Pin/Unpin.
    strip.eval("window.ipc.postMessage(JSON.stringify({action:'CreateTab',payload:{url:'shoji://settings#extensions'}}))");
    let mut settings = shoji
        .page_where(|u| u.contains("settings"), Duration::from_secs(15))
        .expect("no settings page");
    let row = wait("its settings row", 15, || {
        let v = settings.eval(&format!(
            "JSON.stringify((()=>{{const r=[...document.querySelectorAll('.siterow')].find(r=>r.innerText.includes({name:?}));if(!r)return null;const img=r.querySelector('img.extlogo');\
             return {{logo:!!(img&&img.complete&&img.naturalWidth>0),buttons:[...r.querySelectorAll('button')].map(b=>b.textContent)}}}})())"
        ));
        let v: serde_json::Value =
            serde_json::from_str(v.as_str().unwrap_or("null")).unwrap_or_default();
        (!v.is_null()).then_some(v)
    });
    eprintln!("{name}: settings row {row:?}");
    match row {
        Some(r) if r["logo"] == true && r["buttons"].to_string().contains("Unpin") => {}
        r => failures.push(format!("settings: {r:?}")),
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
