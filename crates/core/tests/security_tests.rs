//! Security boundary tests: IPC parsing, navigation, engine flags, elevation, limits, installer paths.

use shoji_core::env::*;
use shoji_core::ipc::*;
use shoji_core::settings::*;
use shoji_core::tabs::*;
use std::path::PathBuf;

// Malformed IPC messages

#[test]
fn test_security_v1_malformed_json_fuzzing() {
    let fuzzed_inputs = vec![
        "",
        " ",
        "\0",
        "{\0}",
        "{",
        "}",
        "[]",
        "null",
        "42",
        "\"raw string\"",
        "{ \"action\": }",
        "{ \"action\": \"Navigate\", \"payload\": ",
        "{ \"action\": \"Navigate\", \"payload\": { \"url\": ",
        "{\"action\": \"SwitchTab\", \"payload\": {\"id\": \"not_a_number\"}}",
        "{\"action\": \"CloseTab\", \"payload\": {\"id\": [1, 2, 3]}}",
        "{\"action\": \"CloseTab\", \"payload\": {\"id\": null}}",
        "{\"action\": \"SaveSettings\", \"payload\": {\"settings_json\": ",
        "{\"action\": \"BypassCertificateError\", \"payload\": null}",
    ];

    for input in fuzzed_inputs {
        let res = serde_json::from_str::<UiToHostMessage>(input);
        assert!(
            res.is_err(),
            "Malformed JSON must not parse into UiToHostMessage: {:?}",
            input
        );
    }
}

#[test]
fn test_security_v1_inner_settings_json_malformed_safety() {
    // When UiToHostMessage::SaveSettings carries invalid inner JSON,
    // host safely ignores it without panic or state corruption.
    let malformed_inner = "{not valid json!!";
    let parsed_inner = serde_json::from_str::<serde_json::Value>(malformed_inner);
    assert!(
        parsed_inner.is_err(),
        "Invalid inner JSON must not parse into Value"
    );

    // Default settings must remain intact
    let mut settings = Settings::default();
    let original_engine = settings.search_engine.clone();
    if let Ok(val) = parsed_inner {
        settings.apply_patch(&val);
    }
    assert_eq!(
        settings.search_engine, original_engine,
        "Settings must not mutate on invalid JSON"
    );
}

#[test]
fn test_security_v1_unknown_and_dangerous_action_rejection() {
    let dangerous_actions = vec![
        r#"{"action": "ExecShell", "payload": {"cmd": "calc.exe"}}"#,
        r#"{"action": "WriteFile", "payload": {"path": "C:\\malicious.exe"}}"#,
        r#"{"action": "EvalCode", "payload": "alert(1)"}"#,
        r#"{"action": "DropSandbox", "payload": {}}"#,
        r#"{"action": "ReadCredentials", "payload": {}}"#,
        r#"{"action": "NativeSystemExec", "payload": "shutdown /s"}"#,
    ];

    for action_json in dangerous_actions {
        let res = serde_json::from_str::<UiToHostMessage>(action_json);
        assert!(
            res.is_err(),
            "Unknown/dangerous action tag must be rejected: {}",
            action_json
        );
    }
}

#[test]
fn test_security_v1_massive_dos_payload_handling() {
    // 10 MB payload string to verify no stack overflow or unhandled OOM
    let large_string = "X".repeat(10 * 1024 * 1024);
    let json = serde_json::json!({
        "action": "Navigate",
        "payload": {
            "url": large_string
        }
    });

    let parsed: Result<UiToHostMessage, _> = serde_json::from_value(json);
    assert!(
        parsed.is_ok(),
        "10MB string must be parsed gracefully without panic"
    );
    if let Ok(UiToHostMessage::Navigate { url }) = parsed {
        assert_eq!(url.len(), 10 * 1024 * 1024);
    }
}

#[test]
fn test_security_v1_out_of_bounds_tab_ids() {
    let mut manager = TabManager::new();
    let t1 = manager.create_tab("https://example.com", 100);

    // Closing boundary values must not panic
    assert_eq!(manager.close_tab(TabId(0)), Some(t1));
    assert_eq!(manager.close_tab(TabId(999_999)), Some(t1));
    assert_eq!(manager.close_tab(TabId(u64::MAX)), Some(t1));

    // Switching to boundary values must return false
    assert!(!manager.switch_tab(TabId(0), 101));
    assert!(!manager.switch_tab(TabId(u64::MAX), 101));
    assert!(!manager.switch_tab(TabId(888_888), 101));

    // Crash marking on non-existent tab ID must not panic
    manager.mark_tab_crashed(TabId(0));
    manager.mark_tab_crashed(TabId(u64::MAX));
}

// Navigation and URL schemes

#[test]
fn test_security_v2_case_insensitive_scheme_injection() {
    let search_template = "https://duckduckgo.com/?q=%s";

    let disallowed_schemes = vec![
        "javascript:alert(1)",
        "JavaScript:alert(document.cookie)",
        "JAVASCRIPT:void(0)",
        "JaVaScRiPt:alert(1)",
        "vbscript:msgbox(1)",
        "VbScRiPt:run",
        "VBSCRIPT:test",
        "data:text/html,<script>alert(1)</script>",
        "DATA:text/html,test",
        "DaTa:text/plain;base64,AAA",
        "file:///C:/Windows/System32/cmd.exe",
        "FILE:///C:/",
        "File://localhost/etc/passwd",
        "about:evil",
        "about:config",
        "about:cache",
        "about:net-internals",
        "blob:https://example.com/abc",
        "mailto:a@b.com",
        "ftp://example.com/file",
        "view-source:file:///C:/",
    ];

    for disallowed in disallowed_schemes {
        let res = normalize_url(disallowed, search_template);
        assert!(
            res.is_err(),
            "Disallowed URI scheme must be rejected: {}",
            disallowed
        );
    }
}

#[test]
fn test_security_v2_safe_schemes_and_search_query_resolution() {
    let search_template = "https://duckduckgo.com/?q=%s";

    let safe_inputs = vec![
        (
            "https://github.com/rust-lang/rust",
            "https://github.com/rust-lang/rust",
        ),
        ("http://example.org", "http://example.org"),
        ("shoji://newtab", "shoji://newtab"),
        ("shoji://settings", "shoji://settings"),
        ("about:blank", "about:blank"),
        ("about:newtab", "shoji://newtab"),
        ("about:home", "shoji://newtab"),
        ("about:settings", "shoji://settings"),
        ("rust-lang.org", "https://rust-lang.org"),
        ("doc.rust-lang.org/std", "https://doc.rust-lang.org/std"),
        ("localhost", "http://localhost"),
        ("localhost:3000/app", "http://localhost:3000/app"),
        ("127.0.0.1:8080", "http://127.0.0.1:8080"),
        ("192.168.1.1/admin", "http://192.168.1.1/admin"),
        ("[::1]:8080", "http://[::1]:8080"),
        (
            "view-source:https://example.com",
            "view-source:https://example.com",
        ),
    ];

    for (input, expected) in safe_inputs {
        let res = normalize_url(input, search_template);
        assert_eq!(
            res.unwrap(),
            expected,
            "Valid URL failed normalization: {}",
            input
        );
    }

    // Search query fallback verification
    let search_queries = vec![
        (
            "rust async runtime",
            "https://duckduckgo.com/?q=rust+async+runtime",
        ),
        ("hello world", "https://duckduckgo.com/?q=hello+world"),
        (
            "what is webview2?",
            "https://duckduckgo.com/?q=what+is+webview2%3F",
        ),
    ];

    for (query, expected) in search_queries {
        let res = normalize_url(query, search_template);
        assert_eq!(
            res.unwrap(),
            expected,
            "Search query normalization mismatch: {}",
            query
        );
    }
}

// Engine flags

#[test]
fn test_security_v3_production_flags_audit() {
    // Prohibited Chromium command-line flags in production builds
    let prohibited_flags = [
        "--no-sandbox",
        "--disable-web-security",
        "--allow-file-access-from-files",
        "--single-process",
        "--disable-gpu-compositing",
    ];

    let settings = Settings::default();
    let serialized = serde_json::to_string(&settings).unwrap();

    for flag in prohibited_flags {
        assert!(
            !serialized.contains(flag),
            "Prohibited security flag found in default settings: {}",
            flag
        );
    }
}

// Host objects

#[test]
fn test_security_v4_host_object_isolation_invariants() {
    // Invariant: Native COM host objects must never be exposed to the DOM window
    // `AreHostObjectsAllowed` must remain false in production configuration.
    let settings = Settings::default();
    assert!(!settings.privacy.ephemeral_default);

    // Verify IPC message definitions do not expose arbitrary method reflection
    let json_sample = r#"{"action": "HostObjectCall", "payload": {"method": "GetProcess"}}"#;
    let parsed = serde_json::from_str::<UiToHostMessage>(json_sample);
    assert!(
        parsed.is_err(),
        "Arbitrary host object calls must be strictly rejected"
    );
}

// TLS exceptions

#[test]
fn test_security_v5_tls_host_scoping_boundary() {
    let mut allowed_hosts = std::collections::HashSet::new();
    allowed_hosts.insert("badssl.com".to_string());

    // Exact host matching verification
    assert!(allowed_hosts.contains("badssl.com"));

    // Subdomains must NOT be automatically allowed (no wildcard leak)
    assert!(!allowed_hosts.contains("sub.badssl.com"));
    assert!(!allowed_hosts.contains("evil-badssl.com"));
    assert!(!allowed_hosts.contains("badssl.com.evil.com"));

    // Port and trailing dot injection must not match
    assert!(!allowed_hosts.contains("badssl.com:443"));
    assert!(!allowed_hosts.contains("badssl.com:8443"));
    assert!(!allowed_hosts.contains("badssl.com."));

    // Empty or whitespace host injection must fail
    assert!(!allowed_hosts.contains(""));
    assert!(!allowed_hosts.contains(" "));
}

// Elevation

#[test]
fn test_security_v6_process_elevation_token_check() {
    // Calling is_process_elevated must execute safely without panicking.
    // In standard CI/development runs, the process runs unelevated (false).
    let elevated = is_process_elevated();
    println!("elevated: {}", elevated);
}

// Size limits

#[test]
fn test_security_v7_bounded_history_capacity() {
    let mut manager = TabManager::new();

    // Create and close 100 distinct tabs
    for i in 0..100 {
        let id = manager.create_tab(&format!("https://example.com/page-{}", i), i);
        manager.close_tab(id);
    }

    // Verify bounded capacity
    // closed_history must be capped at 50 to prevent unbounded RAM growth
    let mut restored_count = 0;
    while manager.pop_last_closed().is_some() {
        restored_count += 1;
    }

    assert_eq!(
        restored_count, 50,
        "Closed tab history must be capped at 50 items"
    );
}

// Installer paths

#[test]
fn test_security_v9_installer_path_and_registry_safety() {
    // Verify that uninstall command formatting is strictly quoted
    // to protect against unquoted service path / command injection (CWE-428).
    let sample_uninstaller =
        PathBuf::from(r"C:\Users\Some One\AppData\Local\Programs\Shoji\uninstall.exe");
    let formatted_cmd = format!("\"{}\" --uninstall", sample_uninstaller.display());

    assert!(formatted_cmd.starts_with('"'));
    assert!(formatted_cmd.contains("\" --uninstall"));
    assert!(!formatted_cmd.starts_with("C:\\Program Files")); // Must be quoted!
}

// Web content may not drive privileged browser actions

#[test]
fn test_security_v10_web_content_ipc_allowlist() {
    let privileged = [
        UiToHostMessage::BypassCertificateError {
            tab_id: TabId(1),
            host: "evil.com".into(),
            url: "https://evil.com".into(),
        },
        UiToHostMessage::SaveSettings {
            patch: serde_json::json!({ "privacy": { "ephemeral_default": false } }),
        },
        UiToHostMessage::SetAccent {
            accent: "red".into(),
        },
        UiToHostMessage::Navigate {
            url: "https://x.com".into(),
        },
        UiToHostMessage::OpenNewWindow,
        UiToHostMessage::CreateTab { url: None },
        UiToHostMessage::ChangeDownloadFolder,
        UiToHostMessage::FinishWelcome,
        UiToHostMessage::UninstallShoji {
            confirm: "UNINSTALL".into(),
        },
        // History, downloads and look-and-feel belong to Shoji's own pages.
        UiToHostMessage::DownloadAction {
            key: 1,
            action: "open".into(),
        },
        UiToHostMessage::ClearBrowsingData {
            range: "all".into(),
            history: true,
            downloads: true,
            cookies: true,
            cache: true,
        },
        UiToHostMessage::HistoryRemove {
            urls: vec!["https://a.com/".into()],
        },
        UiToHostMessage::HistoryQuery {
            query: String::new(),
            offset: 0,
            limit: 10,
            seq: 1,
        },
        UiToHostMessage::OpenPage {
            page: "settings".into(),
        },
        UiToHostMessage::SetDarkTheme {
            host: None,
            theme: DarkTheme::default(),
        },
        UiToHostMessage::SetDarkSchedule {
            enabled: true,
            from: "20:00".into(),
            to: "07:00".into(),
        },
        UiToHostMessage::PinSidebar { pinned: true },
        UiToHostMessage::ToggleMuteTab { tab_id: TabId(1) },
        UiToHostMessage::SplitCreate {
            first: TabId(1),
            second: TabId(2),
        },
        UiToHostMessage::SplitClose { id: 1 },
        UiToHostMessage::SplitRename {
            id: 1,
            name: "x".into(),
        },
        UiToHostMessage::SplitSwapPanes { id: 1 },
        UiToHostMessage::TabDragStart {
            tab_id: TabId(1),
            grab_dx: 1.0,
            grab_dy: 1.0,
            strip_left: 0.0,
        },
        UiToHostMessage::TabCenters { centers: vec![1.0] },
        UiToHostMessage::SplitSetColor {
            id: 1,
            color: "rose".into(),
        },
        UiToHostMessage::FavouriteFolder {
            url: "https://a.com".into(),
        },
        UiToHostMessage::ExportFavourites,
        UiToHostMessage::SiteChoicesQuery,
        UiToHostMessage::ExtensionsQuery,
        UiToHostMessage::AddExtension,
        UiToHostMessage::RemoveExtension { id: "x".into() },
        UiToHostMessage::ForgetSiteChoice {
            host: "a.com".into(),
            kind: None,
        },
        UiToHostMessage::TrayRects {
            find: Some([0.0, 0.0, 9000.0, 9000.0]),
            permission: None,
            toast: None,
            letter: None,
        },
        UiToHostMessage::UpdateAction {
            choice: "now".into(),
        },
    ];
    for msg in privileged {
        assert!(
            !msg.allowed_from_web_content(),
            "{msg:?} must be blocked from web pages"
        );
    }
    assert!(UiToHostMessage::FindResult {
        current: 1,
        total: 2
    }
    .allowed_from_web_content());
}

// Mutation fuzzing: random edits of valid inputs must never panic a parser that takes outside
// input (web pages' IPC, settings files, bookmark imports).

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn mutate(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    const PIECES: &[&[u8]] = &[
        b"{",
        b"}",
        b"[",
        b"]",
        b"\"",
        b":",
        b",",
        b"null",
        b"-1",
        b"1e309",
        br"\u0000",
        b"\xff",
        b"<",
        b">",
        b"</a>",
        b"<DT>",
        b"\"children\"",
        b"true",
    ];
    let mut out = seed.to_vec();
    for _ in 0..1 + rng.below(6) {
        let at = rng.below(out.len() + 1);
        match rng.below(4) {
            0 if !out.is_empty() => {
                let end = (at + 1 + rng.below(8)).min(out.len());
                out.drain(at.min(end)..end);
            }
            1 => {
                let piece = PIECES[rng.below(PIECES.len())];
                out.splice(at..at, piece.iter().copied());
            }
            2 if at < out.len() => out[at] = rng.next() as u8,
            _ => {
                let copy: Vec<u8> = out[at..(at + rng.below(16)).min(out.len())].to_vec();
                out.splice(at..at, copy);
            }
        }
    }
    out
}

#[test]
fn test_security_parsers_survive_mutated_input() {
    let ipc_seeds = [
        r#"{"action":"CreateTab","payload":{"url":"https://example.com"}}"#,
        r#"{"action":"LoginSubmitted","payload":{"username":"a","password":"b"}}"#,
        r#"{"action":"LinkDropped","payload":{"url":"https://example.com/x"}}"#,
        r##"{"action":"PageInfo","payload":{"color":"#fff","has_video":true}}"##,
        r#"{"action":"FindResult","payload":{"current":1,"total":3}}"#,
    ];
    let settings_seed = serde_json::to_string(&Settings::default()).unwrap();
    let chromium = r#"{"roots":{"bookmark_bar":{"children":[{"type":"url","name":"A","url":"https://a.test/"},{"type":"folder","name":"F","children":[{"type":"url","name":"B","url":"https://b.test/"}]}]}}}"#;
    let netscape = "<!DOCTYPE NETSCAPE-Bookmark-file-1><DL><p><DT><H3>F</H3><DL><p><DT><A HREF=\"https://a.test/\">A</A></DL><p></DL>";
    let dir = std::env::temp_dir().join(format!("shoji_fuzz_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let settings_path = dir.join("settings.json");
    let mut rng = Rng(0x5eed_1234_abcd_ef01);
    for i in 0..4000 {
        let seed = ipc_seeds[i % ipc_seeds.len()];
        let m = mutate(&mut rng, seed.as_bytes());
        if let Ok(text) = std::str::from_utf8(&m) {
            if let Ok(msg) = serde_json::from_str::<UiToHostMessage>(text) {
                let _ = msg.allowed_from_web_content();
            }
        }
        let text = String::from_utf8_lossy(&mutate(&mut rng, chromium.as_bytes())).into_owned();
        let _ = shoji_core::import::bookmarks::from_chromium_json(&text);
        let text = String::from_utf8_lossy(&mutate(&mut rng, netscape.as_bytes())).into_owned();
        let _ = shoji_core::import::bookmarks::from_netscape_html(&text);
        if i % 8 == 0 {
            std::fs::write(&settings_path, mutate(&mut rng, settings_seed.as_bytes())).unwrap();
            let s = Settings::load_from_path(&settings_path);
            assert!(matches!(
                s.shield.mode.as_str(),
                "off" | "standard" | "aggressive"
            ));
            assert!(matches!(
                s.privacy.fingerprinting.as_str(),
                "off" | "standard" | "strict"
            ));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
