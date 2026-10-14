//! Tab groups, HTTPS-only upgrades, favourites, per-site data and the session file.

use shoji_core::tabs::{is_local_host, normalize_url, upgrade_to_https, TabManager};
use shoji_core::userdata::{Favourites, Session, SessionTab, SessionWindow, SiteData};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("shoji-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn tab_groups_assign_prune_and_rename() {
    let mut m = TabManager::new();
    let a = m.create_tab("https://a.com", 0);
    let b = m.create_tab("https://b.com", 0);
    let g = m.create_group("  Reading  ", "blue");
    assert_eq!(m.groups()[0].name, "Reading");

    assert!(m.set_tab_group(a, Some(g)));
    assert!(m.set_tab_group(b, Some(g)));
    // Unknown groups are refused.
    assert!(!m.set_tab_group(a, Some(999)));

    m.rename_group(g, "Later");
    m.rename_group(g, "   "); // blank names are ignored
    assert_eq!(m.groups()[0].name, "Later");

    // A group disappears with its last tab.
    m.set_tab_group(a, None);
    assert_eq!(m.groups().len(), 1);
    m.close_tab(b);
    assert!(m.groups().is_empty());
}

#[test]
fn restored_groups_survive_until_tabs_are_assigned() {
    use shoji_core::tabs::TabGroup;
    let mut m = TabManager::new();
    let a = m.create_tab("https://a.com", 0);
    m.create_tab("https://b.com", 0);
    m.restore_groups(vec![
        TabGroup {
            id: 4,
            name: "Kept".into(),
            color: "gold".into(),
        },
        TabGroup {
            id: 5,
            name: "Empty".into(),
            color: "red".into(),
        },
    ]);
    assert!(m.set_tab_group(a, Some(4)));
    m.prune_groups();
    assert_eq!(m.groups().len(), 1);
    assert_eq!(m.groups()[0].name, "Kept");
    // New groups don't reuse restored ids.
    assert!(m.create_group("New", "blue") > 5);
}

#[test]
fn recently_closed_keeps_titles_newest_first() {
    let mut m = TabManager::new();
    let a = m.create_tab("https://a.com", 0);
    m.update_title(a, "A site".into());
    let b = m.create_tab("https://b.com", 0);
    m.close_tab(a);
    m.close_tab(b);
    let recent = m.recently_closed();
    assert_eq!(recent[0].url, "https://b.com");
    assert_eq!(recent[1].title, "A site");
    assert_eq!(m.pop_last_closed().as_deref(), Some("https://b.com"));
}

#[test]
fn https_only_upgrades_web_hosts_but_not_local_ones() {
    assert_eq!(
        upgrade_to_https("http://example.com/a?b=1").as_deref(),
        Some("https://example.com/a?b=1")
    );
    assert_eq!(upgrade_to_https("https://example.com"), None);
    for local in [
        "http://localhost:3000/",
        "http://127.0.0.1/",
        "http://192.168.1.10/admin",
        "http://printer/",
        "http://nas.local/",
        "http://site.test/",
        "http://[::1]/",
        // An explicit port means the server expects plain HTTP there.
        "http://example.com:8080/",
    ] {
        assert_eq!(upgrade_to_https(local), None, "{local}");
    }
    assert!(is_local_host("localhost") && is_local_host("a.internal") && !is_local_host("a.com"));
}

#[test]
fn engine_pages_are_never_navigated_to() {
    for page in ["edge://flags", "edge://settings/passwords"] {
        let out = normalize_url(page, "https://x/?q=%s").unwrap();
        assert!(out.starts_with("https://x/"), "{page} -> {out}");
    }
}

#[test]
fn favourites_persist_and_reject_internal_pages() {
    let dir = temp_dir("fav");
    let path = dir.join("favourites.json");
    let mut f = Favourites::load(path.clone());
    assert!(f.add("https://example.com/", "Example"));
    assert!(
        !f.add("https://example.com", "again"),
        "trailing slash is the same page"
    );
    assert!(!f.add("shoji://settings", "Settings"));
    assert!(!f.add("javascript:alert(1)", "x"));
    assert!(f.contains("https://example.com"));

    let reloaded = Favourites::load(path);
    assert_eq!(reloaded.items().len(), 1);
    assert_eq!(reloaded.items()[0].title, "Example");

    let mut f = reloaded;
    assert!(
        !f.toggle("https://example.com/", ""),
        "toggle removes an existing favourite"
    );
    assert!(f.items().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn site_data_remembers_zoom_and_permissions() {
    let dir = temp_dir("site");
    let path = dir.join("sitedata.json");
    let mut d = SiteData::load(path.clone());
    d.set_zoom("example.com", Some(1.25));
    d.set_permission("example.com", "Camera", false);
    d.set_permission("example.com", "Microphone", true);
    assert_eq!(d.count(), 3);

    let mut d = SiteData::load(path);
    assert_eq!(d.zoom_for("example.com"), Some(1.25));
    assert_eq!(d.permission("example.com", "Camera"), Some(false));
    assert_eq!(d.permission("example.com", "Microphone"), Some(true));
    assert_eq!(d.permission("example.com", "Geolocation"), None);
    assert_eq!(d.permission("other.com", "Camera"), None);

    d.set_zoom("example.com", None);
    assert_eq!(d.zoom_for("example.com"), None);
    d.clear();
    assert_eq!(d.count(), 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn session_file_roundtrips_and_tolerates_garbage() {
    let dir = temp_dir("session");
    let path = dir.join("session.json");

    // Missing or corrupt files read as "nothing to restore, exited cleanly".
    assert!(Session::load(&path).clean_exit);
    std::fs::write(&path, "{ not json").unwrap();
    let s = Session::load(&path);
    assert!(s.clean_exit && !s.has_pages());

    let session = Session {
        clean_exit: false,
        windows: vec![SessionWindow {
            tabs: vec![
                SessionTab {
                    url: "https://a.com".into(),
                    title: "A".into(),
                    group: Some(1),
                    workspace: 1,
                },
                SessionTab {
                    url: "https://b.com".into(),
                    title: "B".into(),
                    group: None,
                    workspace: 1,
                },
            ],
            active: 1,
            groups: vec![],
            workspaces: vec![],
            active_workspace: 0,
        }],
    };
    session.save(&path);
    let back = Session::load(&path);
    assert_eq!(back, session);
    assert!(!back.clean_exit && back.has_pages());
    assert_eq!(back.tab_count(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn workspaces_and_splits_persist_roundtrip_migrate_and_respect_private_mode() {
    use shoji_core::settings::PrivacySettings;
    use shoji_core::tabs::{SplitOrientation, TabManager};

    let dir = temp_dir("ws_session");
    let path = dir.join("session.json");

    // 1. Round-trip workspaces, tabs, active tab per workspace, order, and split state.
    let mut tm = TabManager::new();
    let t1 = tm.create_tab("https://alpha.example", 10);
    let t2 = tm.create_tab("https://beta.example", 20);
    tm.update_title(t1, "Alpha".into());
    tm.update_title(t2, "Beta".into());
    tm.switch_tab(t1, 25);
    assert!(tm.open_split(t2));
    assert!(tm.set_split_orientation(SplitOrientation::Horizontal));
    assert!(tm.set_split_ratio(0.6));

    let ws2 = tm.create_workspace("Research", "green");
    tm.switch_workspace(ws2, 30);
    let w2_blank = tm.active_tab_id().unwrap();
    tm.update_url(w2_blank, "https://papers.example".into());
    tm.update_title(w2_blank, "Papers".into());
    let t4 = tm.create_tab("https://notes.example", 40);
    tm.update_title(t4, "Notes".into());
    tm.switch_tab(w2_blank, 45);

    // Reorder workspaces so "Research" comes first.
    assert!(tm.reorder_workspace(1, 0));

    let normal_privacy = PrivacySettings {
        ephemeral_default: false,
        restore_session: "always".into(),
        ..Default::default()
    };

    let session = Session::from_managers(&[&tm], true, &normal_privacy).unwrap();
    assert!(session.save_respecting_privacy(&path, &normal_privacy));

    let loaded = Session::load_respecting_privacy(&path, &normal_privacy);
    assert_eq!(loaded.windows.len(), 1);
    let mut restored = TabManager::new();
    restored.restore_session_window(&loaded.windows[0], 100);

    assert_eq!(restored.workspaces().len(), 2);
    assert_eq!(restored.workspaces()[0].name, "Research");
    assert_eq!(restored.workspaces()[0].color, "green");
    assert_eq!(restored.workspaces()[1].name, "Main");
    assert_eq!(restored.active_workspace_id(), ws2);
    assert_eq!(restored.active_tab().unwrap().url, "https://papers.example");

    // Switch back to Main workspace and verify its split and active tab survived.
    assert!(restored.switch_workspace(1, 110));
    assert_eq!(restored.active_tab().unwrap().url, "https://alpha.example");
    let sp = restored.split().expect("split restored on Main");
    assert_eq!(sp.orientation, SplitOrientation::Horizontal);
    assert!((sp.ratio - 0.6).abs() < 1e-6);

    // 2. Migration from legacy session format (no `workspaces` or `active_workspace` fields).
    let legacy_json = r#"{
        "clean_exit": true,
        "windows": [{
            "tabs": [
                { "url": "https://legacy-one.example", "title": "One" },
                { "url": "https://legacy-two.example", "title": "Two" }
            ],
            "active": 1
        }]
    }"#;
    std::fs::write(&path, legacy_json).unwrap();
    let legacy_session = Session::load(&path);
    let mut migrated = TabManager::new();
    migrated.restore_session_window(&legacy_session.windows[0], 200);
    assert_eq!(migrated.workspaces().len(), 1);
    assert_eq!(migrated.workspaces()[0].name, "Main");
    assert_eq!(migrated.tabs().len(), 2);
    assert_eq!(
        migrated.active_tab().unwrap().url,
        "https://legacy-two.example"
    );

    // 3. Private mode must never persist workspaces or their tabs, and clears any existing file.
    let private_privacy = PrivacySettings {
        ephemeral_default: true,
        ..Default::default()
    };
    assert!(Session::from_managers(&[&tm], true, &private_privacy).is_none());
    assert!(!session.save_respecting_privacy(&path, &private_privacy));
    assert!(!path.exists());
    let after_private = Session::load_respecting_privacy(&path, &private_privacy);
    assert!(!after_private.has_pages());
    assert!(after_private.windows.is_empty());

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn import_and_default_browser_messages_stay_off_web_content() {
    use shoji_core::ipc::UiToHostMessage as M;
    let start: M = serde_json::from_str(
        r#"{"action":"StartImport","payload":{"source":"chrome","kinds":{"favourites":true,"history":false,"cookies":false}}}"#,
    )
    .unwrap();
    match &start {
        M::StartImport {
            source,
            profile,
            path,
            kinds,
        } => {
            assert_eq!(
                (source.as_str(), *profile, path.is_none()),
                ("chrome", 0, true)
            );
            assert!(kinds.favourites && !kinds.history && !kinds.cookies);
        }
        other => panic!("{other:?}"),
    }
    for m in [
        start,
        M::ListImportSources,
        M::PickImportSource {
            source: "safari".into(),
        },
        M::CheckDefaultBrowser,
        M::SetDefaultBrowser,
        M::SearchHistory {
            query: "x".into(),
            seq: 1,
        },
        M::ClearHistory,
    ] {
        assert!(!m.allowed_from_web_content(), "{m:?}");
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<M>(&json).unwrap(), m);
    }
}

#[test]
fn password_manager_messages_are_page_reports_or_internal_only() {
    use shoji_core::ipc::UiToHostMessage as M;
    // Web pages may only report what is on their own login form.
    for m in [
        M::LoginSubmitted {
            username: "me".into(),
            password: "pw".into(),
        },
        M::LoginFieldFocused {
            x: 1.0,
            y: 2.0,
            w: 3.0,
            h: 4.0,
        },
    ] {
        assert!(m.allowed_from_web_content(), "{m:?}");
    }
    // Everything that reads or changes the vault stays with Shoji's own pages.
    let internal = [
        M::VaultStatus,
        M::VaultList {
            query: String::new(),
        },
        M::VaultReveal { id: "x".into() },
        M::VaultCopy {
            id: "x".into(),
            field: "password".into(),
        },
        M::VaultSave {
            id: None,
            url: "https://a.com".into(),
            title: String::new(),
            username: String::new(),
            password: "p".into(),
            note: String::new(),
        },
        M::VaultDelete { id: "x".into() },
        M::VaultDeleteAll,
        M::VaultUnlock { master: "m".into() },
        M::VaultLock,
        M::VaultSetMaster { master: "m".into() },
        M::VaultRemoveMaster,
        M::VaultGenerate {
            length: 20,
            lower: true,
            upper: true,
            digits: true,
            symbols: true,
        },
        M::VaultImportCsv,
        M::VaultExportCsv,
    ];
    for m in internal {
        assert!(!m.allowed_from_web_content(), "{m:?}");
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<M>(&json).unwrap(), m);
    }
    // Defaults let the page omit options.
    let g: M = serde_json::from_str(r#"{"action":"VaultGenerate","payload":{}}"#).unwrap();
    assert_eq!(
        g,
        M::VaultGenerate {
            length: 20,
            lower: true,
            upper: true,
            digits: true,
            symbols: true
        }
    );
}
