//! Shared harness for the browser integration tests: a local web server, an isolated copy of
//! Shoji with its own profile, and a DevTools-protocol client for its pages.
//!
//! Every test host is mapped to the local server with `--host-resolver-rules`, so test pages can
//! use real ad and tracker host names without anything leaving the machine. Shoji's own list
//! downloads go through its updater, not the engine, and are unaffected.

#![allow(dead_code)]

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/* ---------------------------------------------------------------- server */

/// One request as the server saw it.
#[derive(Debug, Clone)]
pub struct Hit {
    pub method: String,
    /// Host header without the port
    pub host: String,
    /// Path and query
    pub target: String,
    /// Header names in lower case
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Hit {
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or("")
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

/// What a route answers.
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn new(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), content_type.into())],
            body: body.into(),
        }
    }

    pub fn html(body: impl Into<String>) -> Self {
        Self::new(200, "text/html; charset=utf-8", body.into())
    }

    pub fn js(body: impl Into<String>) -> Self {
        Self::new(200, "text/javascript", body.into())
    }

    pub fn empty() -> Self {
        Self::new(200, "text/plain", Vec::new())
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

type Router = dyn Fn(&Hit) -> Reply + Send + Sync;

/// A plain HTTP/1.1 server on a free loopback port that records every request.
pub struct Server {
    pub port: u16,
    hits: Arc<Mutex<Vec<Hit>>>,
}

impl Server {
    pub fn start(router: impl Fn(&Hit) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let port = listener.local_addr().unwrap().port();
        let hits: Arc<Mutex<Vec<Hit>>> = Arc::default();
        let router: Arc<Router> = Arc::new(router);
        {
            let hits = hits.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let (hits, router) = (hits.clone(), router.clone());
                    std::thread::spawn(move || serve(stream, &hits, &*router));
                }
            });
        }
        Self { port, hits }
    }

    /// `http://<host>:<port><path>`
    pub fn url(&self, host: &str, path: &str) -> String {
        format!("http://{host}:{}{path}", self.port)
    }

    pub fn hits(&self) -> Vec<Hit> {
        self.hits.lock().unwrap().clone()
    }

    pub fn clear(&self) {
        self.hits.lock().unwrap().clear();
    }

    /// Requests to `host` whose path starts with `path`.
    pub fn hits_for(&self, host: &str, path: &str) -> Vec<Hit> {
        self.hits()
            .into_iter()
            .filter(|h| h.host == host && h.path().starts_with(path))
            .collect()
    }

    pub fn saw(&self, host: &str, path: &str) -> bool {
        !self.hits_for(host, path).is_empty()
    }

    /// Wait until a request for `host` + `path` arrives.
    pub fn wait_for(&self, host: &str, path: &str, timeout: Duration) -> Option<Hit> {
        let end = Instant::now() + timeout;
        loop {
            if let Some(h) = self.hits_for(host, path).pop() {
                return Some(h);
            }
            if Instant::now() > end {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn serve(stream: TcpStream, hits: &Mutex<Vec<Hit>>, router: &Router) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    // The engine upgrades hosts on the HSTS preload list to https. Such a request reached the
    // server even though it can't be answered: record its host from the TLS greeting.
    let mut first = [0u8; 1];
    if stream.peek(&mut first).ok() == Some(1) && first[0] == 0x16 {
        let mut hello = vec![0u8; 4096];
        let n = (&stream).read(&mut hello).unwrap_or(0);
        if let Some(host) = sni(&hello[..n]) {
            hits.lock().unwrap().push(Hit {
                method: "TLS".into(),
                host,
                target: "/".into(),
                headers: HashMap::new(),
                body: Vec::new(),
            });
        }
        return;
    }
    let mut reader = BufReader::new(stream);
    // Keep-alive: answer requests on this connection until the client closes it.
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let mut parts = line.split_whitespace();
        let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
            return;
        };
        let (method, target) = (method.to_string(), target.to_string());
        let mut headers = HashMap::new();
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).unwrap_or(0) == 0 {
                return;
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
            }
        }
        let len: usize = headers
            .get("content-length")
            .and_then(|l| l.parse().ok())
            .unwrap_or(0);
        let mut body = vec![0; len.min(1 << 20)];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let host = headers
            .get("host")
            .map(|h| h.split(':').next().unwrap_or("").to_ascii_lowercase())
            .unwrap_or_default();
        let hit = Hit {
            method,
            host,
            target,
            headers,
            body,
        };
        hits.lock().unwrap().push(hit.clone());
        let reply = router(&hit);
        let mut out = format!(
            "HTTP/1.1 {} X\r\nContent-Length: {}\r\nCache-Control: no-store\r\n",
            reply.status,
            reply.body.len()
        );
        for (k, v) in &reply.headers {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
        out.push_str("\r\n");
        let stream = reader.get_mut();
        if stream.write_all(out.as_bytes()).is_err() || stream.write_all(&reply.body).is_err() {
            return;
        }
    }
}

/// The server name in a TLS ClientHello.
fn sni(b: &[u8]) -> Option<String> {
    // record header (5), handshake header (4), version (2), random (32)
    let mut i = 5 + 4 + 2 + 32;
    let skip = |i: &mut usize, len_bytes: usize| -> Option<()> {
        let mut n = 0usize;
        for k in 0..len_bytes {
            n = (n << 8) | *b.get(*i + k)? as usize;
        }
        *i += len_bytes + n;
        Some(())
    };
    skip(&mut i, 1)?; // session id
    skip(&mut i, 2)?; // cipher suites
    skip(&mut i, 1)?; // compression
    let ext_end = i + 2 + ((*b.get(i)? as usize) << 8 | *b.get(i + 1)? as usize);
    i += 2;
    while i + 4 <= ext_end.min(b.len()) {
        let kind = (b[i] as usize) << 8 | b[i + 1] as usize;
        let len = (b[i + 2] as usize) << 8 | b[i + 3] as usize;
        if kind == 0 {
            // list length (2), name type (1), name length (2), name
            let n = (*b.get(i + 7)? as usize) << 8 | *b.get(i + 8)? as usize;
            let name = b.get(i + 9..i + 9 + n)?;
            return Some(String::from_utf8_lossy(name).to_ascii_lowercase());
        }
        i += 4 + len;
    }
    None
}

/* ---------------------------------------------------------------- browser */

/// One test at a time drives a browser: they share the screen, and some share list caches.
static ONE_BROWSER: Mutex<()> = Mutex::new(());

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn tmp_root() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("shoji-it")
}

/// Where downloaded filter lists are kept between test runs (the updater revalidates them).
fn list_cache() -> PathBuf {
    tmp_root().join("byobu-cache")
}

fn copy_dir(from: &Path, to: &Path) {
    let _ = std::fs::create_dir_all(to);
    for e in std::fs::read_dir(from).into_iter().flatten().flatten() {
        let target = to.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &target);
        } else {
            let _ = std::fs::copy(e.path(), target);
        }
    }
}

/// How an isolated instance is set up.
pub struct Options {
    /// Merged over the test defaults (first run done, no updates, no session restore)
    pub settings: Value,
    /// Byobu's real lists: fetched (and cached between runs). Off: only the test's own rules.
    pub real_lists: bool,
    /// Extra engine switches
    pub engine_args: String,
    /// Send every host to the local server (off: the real web)
    pub local_hosts: bool,
    /// Extra environment for the browser process
    pub env: Vec<(&'static str, String)>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            settings: json!({}),
            real_lists: false,
            engine_args: String::new(),
            local_hosts: true,
            env: Vec::new(),
        }
    }
}

fn merge(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                merge(b.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (b, o) => *b = o.clone(),
    }
}

/// An isolated, running copy of Shoji (its own exe copy, profile and DevTools port).
pub struct Shoji {
    child: Mutex<Child>,
    pub cdp: u16,
    pub dir: PathBuf,
    real_lists: bool,
    _turn: MutexGuard<'static, ()>,
}

/// Skip (pass) when this machine can't run the browser at all.
pub fn can_run() -> bool {
    if std::env::var_os("SHOJI_SKIP_BROWSER").is_some() {
        eprintln!("skipped: SHOJI_SKIP_BROWSER is set");
        return false;
    }
    if shoji_core::env::detect_webview2_runtime().is_none() {
        // CI must run them; elsewhere a machine without the runtime just skips them.
        assert!(
            std::env::var_os("CI").is_none(),
            "no WebView2 runtime on the CI runner"
        );
        eprintln!("skipped: no WebView2 runtime on this machine");
        return false;
    }
    true
}

impl Shoji {
    pub fn start(name: &str, url: &str, opts: Options) -> Self {
        let turn = ONE_BROWSER.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tmp_root().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        let data = dir.join("appdata").join("Shoji");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(dir.join("local")).unwrap();
        let exe = dir.join("shoji.exe");
        // `SHOJI_EXE` runs another build (a release build, for timings) instead of this one.
        let source = std::env::var_os("SHOJI_EXE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_shoji")));
        std::fs::copy(&source, &exe).expect("copy shoji.exe");

        let mut settings = json!({
            "first_run_complete": true,
            "performance": { "auto_update": false, "run_in_background": false },
            "privacy": { "restore_session": "never", "mode_chosen": true, "https_only": false },
        });
        merge(&mut settings, &opts.settings);
        std::fs::write(data.join("settings.json"), settings.to_string()).unwrap();
        if opts.real_lists {
            copy_dir(&list_cache(), &data.join("byobu"));
        }

        // Port 0: the engine picks a free port and writes it to `DevToolsActivePort`.
        let cdp = 0;
        let mut args = format!(
            "--remote-debugging-port={cdp} --no-first-run --enable-logging --log-file=\"{}\"",
            dir.join("engine-debug.log").display()
        );
        if opts.local_hosts {
            args.push_str(" --host-resolver-rules=\"MAP * 127.0.0.1\"");
        }
        if !opts.engine_args.is_empty() {
            args.push(' ');
            args.push_str(&opts.engine_args);
        }
        let mut cmd = Command::new(&exe);
        cmd.arg(url)
            .env("APPDATA", dir.join("appdata"))
            .env("LOCALAPPDATA", dir.join("local"))
            .env("SHOJI_UPDATE_API", "http://127.0.0.1:9/none")
            .env("SHOJI_ALLOW_ELEVATED", "1")
            .env("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", args)
            .env_remove("WEBVIEW2_USER_DATA_FOLDER")
            .stdout(std::fs::File::create(dir.join("out.log")).unwrap())
            .stderr(std::fs::File::create(dir.join("err.log")).unwrap());
        for (k, v) in &opts.env {
            cmd.env(k, v);
        }
        if !opts.real_lists {
            // The updater honours the proxy variables; a dead proxy keeps it to the test's rules.
            cmd.env("HTTPS_PROXY", "http://127.0.0.1:9")
                .env("HTTP_PROXY", "http://127.0.0.1:9")
                .env("ALL_PROXY", "http://127.0.0.1:9");
        }
        let child = cmd.spawn().expect("start shoji.exe");
        Self {
            child: Mutex::new(child),
            cdp,
            dir,
            real_lists: opts.real_lists,
            _turn: turn,
        }
    }

    pub fn data_dir(&self) -> PathBuf {
        self.dir.join("appdata").join("Shoji")
    }

    /// Every DevTools target (each Shoji webview is one).
    pub fn targets(&self) -> Vec<Value> {
        self.try_targets().unwrap_or_default()
    }

    /// The DevTools port the engine actually listens on (it picks one and writes it down).
    fn devtools_port(&self) -> Option<u16> {
        let file = self
            .dir
            .join("local/Shoji/User Data/EBWebView/DevToolsActivePort");
        std::fs::read_to_string(file)
            .ok()?
            .lines()
            .next()?
            .trim()
            .parse()
            .ok()
    }

    fn try_targets(&self) -> Result<Vec<Value>, String> {
        let port = self.devtools_port().ok_or("no DevToolsActivePort yet")?;
        let url = format!("http://127.0.0.1:{port}/json");
        let text = ureq::get(&url)
            .call()
            .map_err(|e| format!("{url}: {e}"))?
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("{url}: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("{url}: {e}: {text:.200}"))
    }

    /// The first page target whose URL satisfies `pick`, once it exists.
    pub fn page_where(&self, pick: impl Fn(&str) -> bool, timeout: Duration) -> Option<Page> {
        let end = Instant::now() + timeout;
        loop {
            let found = self.targets().into_iter().find(|t| {
                (t["type"] == "page" || t["type"] == "iframe")
                    && pick(t["url"].as_str().unwrap_or(""))
                    && t["webSocketDebuggerUrl"].is_string()
            });
            if let Some(t) = found {
                if let Some(p) = Page::connect(t["webSocketDebuggerUrl"].as_str().unwrap()) {
                    return Some(p);
                }
            }
            if Instant::now() > end {
                return None;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// The tab showing a URL that contains `part`.
    pub fn page(&self, part: &str) -> Page {
        self.page_where(|u| u.contains(part), Duration::from_secs(60))
            .unwrap_or_else(|| {
                panic!(
                    "no page with {part:?}; targets: {:?}\n{}",
                    self.targets()
                        .iter()
                        .map(|t| t["url"].as_str().unwrap_or("").to_string())
                        .collect::<Vec<_>>(),
                    self.report()
                )
            })
    }

    /// The window's toolbar strip (the chrome page with tabs in it).
    pub fn chrome(&self) -> Page {
        // Menus and panels are data pages too; the strip is the one with the toast in it.
        let end = Instant::now() + Duration::from_secs(30);
        loop {
            for t in self.targets() {
                let url = t["url"].as_str().unwrap_or("");
                if t["type"] != "page"
                    || !(url.starts_with("data:") || url.starts_with("about:blank"))
                {
                    continue;
                }
                let Some(ws) = t["webSocketDebuggerUrl"].as_str() else {
                    continue;
                };
                if let Some(mut p) = Page::connect(ws) {
                    let r = p.call(
                        "Runtime.evaluate",
                        json!({ "expression": "!!document.getElementById('toastTitle')", "returnByValue": true }),
                    );
                    if r["result"]["value"] == true {
                        return p;
                    }
                }
            }
            assert!(Instant::now() < end, "chrome strip");
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.child.lock().unwrap().try_wait(), Ok(None))
    }

    /// What there is to know when the browser didn't come up: whether it runs, its output, and
    /// the logs and files it wrote.
    pub fn report(&self) -> String {
        let mut out = format!(
            "process: running {}\ndevtools: {:?}\n",
            self.is_running(),
            self.try_targets().map(|t| t.len())
        );
        let engines = Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq msedgewebview2.exe"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        out.push_str(&format!("engine processes: {engines}\n"));
        let pid = self.child.lock().unwrap().id();
        let query = format!(
            "Get-CimInstance Win32_Process -Filter \"ParentProcessId={pid}\" | ForEach-Object {{ $_.CommandLine }}"
        );
        let lines = Command::new("powershell")
            .args(["-NoProfile", "-Command", &query])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        out.push_str(&format!("engine command line: {lines}\n"));
        let debug = std::fs::read_to_string(self.dir.join("engine-debug.log")).unwrap_or_default();
        let tail: Vec<&str> = debug.lines().rev().take(40).collect();
        out.push_str(&format!("engine log, newest first:\n{}\n", tail.join("\n")));
        for f in ["out.log", "err.log"] {
            let text = std::fs::read_to_string(self.dir.join(f)).unwrap_or_default();
            out.push_str(&format!("{f}: {text}\n"));
        }
        let logs = self.data_dir().join("logs");
        for e in std::fs::read_dir(logs).into_iter().flatten().flatten() {
            let text = std::fs::read_to_string(e.path()).unwrap_or_default();
            out.push_str(&format!("{}: {text}\n", e.file_name().to_string_lossy()));
        }
        fn walk(d: &Path, depth: usize, files: &mut Vec<String>) {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                files.push(e.path().display().to_string());
                if depth < 6 && e.path().is_dir() {
                    walk(&e.path(), depth + 1, files);
                }
            }
        }
        let mut files = Vec::new();
        walk(&self.dir, 0, &mut files);
        out.push_str(&format!("files ({}):\n", files.len()));
        for f in files.iter().take(120) {
            out.push_str(&format!("  {f}\n"));
        }
        for f in files
            .iter()
            .filter(|f| f.ends_with(".log") && f.contains("EBWebView"))
        {
            let text = std::fs::read_to_string(f).unwrap_or_default();
            let tail: Vec<&str> = text.lines().rev().take(30).collect();
            out.push_str(&format!("{f}, newest first:\n{}\n", tail.join("\n")));
        }
        out
    }
}

impl Drop for Shoji {
    fn drop(&mut self) {
        let pid = self.child.lock().unwrap().id();
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .output();
        let _ = self.child.lock().unwrap().wait();
        if self.real_lists {
            // Keep what was downloaded for the next run.
            let lists = self.data_dir().join("byobu");
            let _ = std::fs::remove_dir_all(list_cache());
            copy_dir(&lists, &list_cache());
            let _ = std::fs::remove_file(list_cache().join("stats.json"));
        }
    }
}

/* ---------------------------------------------------------------- devtools */

/// A DevTools-protocol connection to one page.
pub struct Page {
    ws: tungstenite::WebSocket<TcpStream>,
    next: u64,
    /// Events received while waiting for replies
    pub events: Vec<Value>,
}

impl Page {
    fn connect(ws_url: &str) -> Option<Self> {
        let addr = ws_url.strip_prefix("ws://")?.split('/').next()?.to_string();
        let stream = TcpStream::connect(addr).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
            .ok()?;
        let (ws, _) = tungstenite::client(ws_url, stream).ok()?;
        Some(Self {
            ws,
            next: 0,
            events: Vec::new(),
        })
    }

    /// Call a protocol method and wait for its result.
    pub fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        let msg = json!({ "id": id, "method": method, "params": params }).to_string();
        self.ws
            .send(tungstenite::Message::Text(msg.into()))
            .expect("devtools send");
        loop {
            let m = self.ws.read().expect("devtools read");
            let Ok(text) = m.to_text() else { continue };
            let Ok(v) = serde_json::from_str::<Value>(text) else {
                continue;
            };
            if v["id"] == id {
                if !v["error"].is_null() {
                    panic!("{method} failed: {}", v["error"]);
                }
                return v["result"].clone();
            }
            if v["method"].is_string() {
                self.events.push(v);
            }
        }
    }

    /// Evaluate `expr` in the page (promises are awaited) and return its value.
    pub fn eval(&mut self, expr: &str) -> Value {
        let r = self.call(
            "Runtime.evaluate",
            json!({ "expression": expr, "awaitPromise": true, "returnByValue": true }),
        );
        if let Some(ex) = r.get("exceptionDetails") {
            panic!("script failed: {expr}\n{ex}");
        }
        r["result"]["value"].clone()
    }

    /// Load `url` and wait for its load event (or `timeout`).
    pub fn goto(&mut self, url: &str) {
        self.call("Page.enable", json!({}));
        self.events.clear();
        self.call("Page.navigate", json!({ "url": url }));
        self.wait_event("Page.loadEventFired", Duration::from_secs(30));
        // An event from the page being left can arrive first; wait for the new document itself.
        let want = serde_json::to_string(url.split('#').next().unwrap_or(url)).unwrap();
        self.wait_for(
            &format!(
                "document.readyState==='complete'&&location.href.startsWith({want}.split('?')[0])"
            ),
            Duration::from_secs(30),
        );
    }

    /// Wait for an event named `method`.
    pub fn wait_event(&mut self, method: &str, timeout: Duration) -> Option<Value> {
        let end = Instant::now() + timeout;
        loop {
            if let Some(i) = self.events.iter().position(|e| e["method"] == method) {
                return Some(self.events.remove(i));
            }
            if Instant::now() > end {
                return None;
            }
            // A cheap round trip collects whatever events arrived meanwhile.
            self.call("Runtime.evaluate", json!({ "expression": "0" }));
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Poll `expr` until it is truthy.
    pub fn wait_for(&mut self, expr: &str, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        loop {
            let v = self.eval(expr);
            if !(v.is_null() || v == false || v == 0 || v == "") {
                return true;
            }
            if Instant::now() > end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// A real (trusted) click at page coordinates, which counts as a user gesture.
    pub fn click(&mut self, x: f64, y: f64) {
        for kind in ["mousePressed", "mouseReleased"] {
            self.call(
                "Input.dispatchMouseEvent",
                json!({ "type": kind, "x": x, "y": y, "button": "left", "clickCount": 1 }),
            );
        }
    }

    /// Click the centre of the element matching `selector`.
    pub fn click_on(&mut self, selector: &str) {
        let at = self.eval(&format!(
            "(()=>{{const r=document.querySelector({}).getBoundingClientRect();return [r.x+r.width/2,r.y+r.height/2]}})()",
            serde_json::to_string(selector).unwrap()
        ));
        self.click(at[0].as_f64().unwrap(), at[1].as_f64().unwrap());
    }
}

/* ---------------------------------------------------------------- shield */

/// Wait until Byōbu blocks `probe_url` (a rule the test added, or one from the real lists):
/// reload `page_url` until the server stops seeing the probe.
pub fn wait_for_shield(server: &Server, page: &mut Page, page_url: &str, probe: (&str, &str)) {
    let end = Instant::now() + Duration::from_secs(240);
    loop {
        server.clear();
        page.goto(page_url);
        std::thread::sleep(Duration::from_millis(400));
        if server.saw(page_host(page_url), "/") && !server.saw(probe.0, probe.1) {
            server.clear();
            return;
        }
        if Instant::now() > end {
            panic!("Byōbu never blocked the probe {}{}", probe.0, probe.1);
        }
        std::thread::sleep(Duration::from_millis(1500));
    }
}

fn page_host(url: &str) -> &str {
    url.split("//")
        .nth(1)
        .and_then(|r| r.split([':', '/']).next())
        .unwrap_or("")
}
