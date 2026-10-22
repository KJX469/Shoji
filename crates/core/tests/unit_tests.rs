use shoji_core::env::is_process_elevated;
use shoji_core::ipc::{HostToUiMessage, UiToHostMessage};
use shoji_core::settings::Settings;
use shoji_core::tabs::{TabId, TabManager, TabStatus};

#[test]
fn test_tab_manager_create_and_transitions() {
    let mut manager = TabManager::new();

    // Initial state
    assert!(manager.active_tab().is_none());
    assert_eq!(manager.tabs().len(), 0);

    // Create first tab
    let id1 = manager.create_tab("https://example.com", 100);
    assert_eq!(manager.active_tab().unwrap().id, id1);
    assert_eq!(manager.active_tab().unwrap().status, TabStatus::Active);
    assert_eq!(manager.tabs().len(), 1);

    // Create second tab -> id1 becomes Inactive, id2 becomes Active
    let id2 = manager.create_tab("https://rust-lang.org", 200);
    assert_eq!(manager.active_tab().unwrap().id, id2);
    assert_eq!(manager.active_tab().unwrap().status, TabStatus::Active);

    let tab1 = manager.tabs().iter().find(|t| t.id == id1).unwrap();
    assert_eq!(tab1.status, TabStatus::Inactive);
}

#[test]
fn test_tab_manager_switch_tab() {
    let mut manager = TabManager::new();
    let id1 = manager.create_tab("https://tab1.com", 100);
    let id2 = manager.create_tab("https://tab2.com", 200);

    assert_eq!(manager.active_tab().unwrap().id, id2);

    // Switch back to tab 1
    let success = manager.switch_tab(id1, 300);
    assert!(success);
    assert_eq!(manager.active_tab().unwrap().id, id1);
    assert_eq!(manager.active_tab().unwrap().status, TabStatus::Active);

    // Tab 2 should now be Inactive
    let tab2 = manager.tabs().iter().find(|t| t.id == id2).unwrap();
    assert_eq!(tab2.status, TabStatus::Inactive);
}

#[test]
fn test_tab_manager_close_last_tab_returns_none() {
    let mut manager = TabManager::new();
    let id1 = manager.create_tab("https://example.com", 100);

    let next_active = manager.close_tab(id1);
    assert!(next_active.is_none());
    assert!(manager.active_tab().is_none());
    assert_eq!(manager.tabs().len(), 0);
}

#[test]
fn test_tab_manager_pop_last_closed_lifo_and_skips_blank() {
    let mut manager = TabManager::new();

    let id_blank = manager.create_tab("about:blank", 100);
    let id_site1 = manager.create_tab("https://first.com", 101);
    let id_site2 = manager.create_tab("https://second.com", 102);

    // Close in order: blank, site1, site2
    manager.close_tab(id_blank);
    manager.close_tab(id_site1);
    manager.close_tab(id_site2);

    // Pop should return site2 first (LIFO), then site1, then None (about:blank skipped)
    assert_eq!(
        manager.pop_last_closed(),
        Some("https://second.com".to_string())
    );
    assert_eq!(
        manager.pop_last_closed(),
        Some("https://first.com".to_string())
    );
    assert_eq!(manager.pop_last_closed(), None);
}

#[test]
fn test_settings_default_and_roundtrip() {
    let settings = Settings::default();
    // Remember like a normal browser unless the user opts into private mode.
    assert!(!settings.privacy.ephemeral_default);
    assert!(!settings.privacy.mode_chosen);
    assert_eq!(settings.performance.sleep_after_secs, 30);
    assert_eq!(settings.search_engine, "duckduckgo");
    assert_eq!(
        settings.search_url_template(),
        "https://duckduckgo.com/?q=%s"
    );

    let serialized = serde_json::to_string_pretty(&settings).expect("Serialization failed");
    let deserialized: Settings = serde_json::from_str(&serialized).expect("Deserialization failed");
    assert_eq!(settings, deserialized);
}

#[test]
fn test_search_engine_templates() {
    let mut s = Settings {
        search_engine: "google".to_string(),
        ..Default::default()
    };
    assert_eq!(
        s.search_url_template(),
        "https://www.google.com/search?q=%s"
    );
    assert_eq!(s.search_engine_display_name(), "Google");

    s.search_engine = "bing".to_string();
    assert_eq!(s.search_url_template(), "https://www.bing.com/search?q=%s");

    s.search_engine = "brave".to_string();
    assert_eq!(
        s.search_url_template(),
        "https://search.brave.com/search?q=%s"
    );

    s.search_engine = "ecosia".to_string();
    assert_eq!(
        s.search_url_template(),
        "https://www.ecosia.org/search?q=%s"
    );
}

#[test]
fn test_tab_manager_suspension_and_audio() {
    let mut tm = TabManager::new();
    let tab_id = tm.create_tab("https://example.com", 1000);

    tm.set_audio_playing(tab_id, true);
    assert!(tm.tabs()[0].is_audio_playing);

    tm.set_tab_suspended(tab_id, true);
    assert_eq!(tm.tabs()[0].status, shoji_core::tabs::TabStatus::Suspended);

    tm.set_tab_suspended(tab_id, false);
    assert_eq!(tm.tabs()[0].status, shoji_core::tabs::TabStatus::Inactive);
}

#[test]
fn test_tab_manager_reorder() {
    let mut tm = TabManager::new();
    let id1 = tm.create_tab("https://tab1.com", 100);
    let id2 = tm.create_tab("https://tab2.com", 200);
    let id3 = tm.create_tab("https://tab3.com", 300);

    assert_eq!(tm.tabs()[0].id, id1);
    assert_eq!(tm.tabs()[1].id, id2);
    assert_eq!(tm.tabs()[2].id, id3);

    // Reorder: move tab at index 0 to index 2
    let reordered = tm.reorder_tab(0, 2);
    assert!(reordered);
    assert_eq!(tm.tabs()[0].id, id2);
    assert_eq!(tm.tabs()[1].id, id3);
    assert_eq!(tm.tabs()[2].id, id1);

    // Reorder back: move tab at index 2 to index 0
    let reordered2 = tm.reorder_tab(2, 0);
    assert!(reordered2);
    assert_eq!(tm.tabs()[0].id, id1);
    assert_eq!(tm.tabs()[1].id, id2);
    assert_eq!(tm.tabs()[2].id, id3);

    // Out of bounds returns false
    assert!(!tm.reorder_tab(0, 99));
    assert!(!tm.reorder_tab(99, 0));
    assert!(!tm.reorder_tab(1, 1));
}

#[test]
fn test_is_process_elevated_returns_false_unelevated() {
    // Developer runs are unelevated. GitHub's Windows runners run elevated, so CI only checks
    // that the query works and is stable.
    let elevated = is_process_elevated();
    assert_eq!(elevated, is_process_elevated());
    if std::env::var_os("CI").is_none() {
        assert!(
            !elevated,
            "Process expected to run unelevated in standard environment"
        );
    }
}

#[test]
fn test_tab_manager_extract_and_insert() {
    let mut tm1 = TabManager::new();
    let id1 = tm1.create_tab("https://window1-tab1.com", 100);
    let id2 = tm1.create_tab("https://window1-tab2.com", 200);

    assert_eq!(tm1.tabs().len(), 2);
    assert_eq!(tm1.active_tab_id(), Some(id2));

    // Extract tab 1
    let extracted = tm1.extract_tab(id1).expect("Failed to extract tab");
    assert_eq!(extracted.url, "https://window1-tab1.com");
    assert_eq!(tm1.tabs().len(), 1);
    assert_eq!(tm1.active_tab_id(), Some(id2));

    // Insert into another tab manager
    let mut tm2 = TabManager::new();
    let id_in_tm2 = tm2.insert_tab(extracted, None);
    assert_eq!(tm2.tabs().len(), 1);
    assert_eq!(tm2.tabs()[0].url, "https://window1-tab1.com");
    assert_eq!(tm2.active_tab_id(), Some(id_in_tm2));
}

#[test]
fn test_tab_manager_crash_and_snapshot_restore() {
    let mut tm = TabManager::new();
    let id1 = tm.create_tab("https://alpha.example", 100);
    let _id2 = tm.create_tab("https://beta.example", 200);

    assert_eq!(tm.tabs()[0].status, shoji_core::tabs::TabStatus::Inactive);
    assert_eq!(tm.tabs()[1].status, shoji_core::tabs::TabStatus::Active);

    // Mark tab 1 crashed
    tm.mark_tab_crashed(id1);
    assert_eq!(tm.tabs()[0].status, shoji_core::tabs::TabStatus::Crashed);

    // Snapshot
    let snapshot = tm.snapshot();
    assert_eq!(snapshot.len(), 2);
    assert_eq!(snapshot[0].url, "https://alpha.example");
    assert_eq!(snapshot[0].status, shoji_core::tabs::TabStatus::Crashed);
    assert_eq!(snapshot[1].url, "https://beta.example");

    // Restore into a fresh TabManager
    let mut tm_restored = TabManager::new();
    tm_restored.restore_from_snapshot(snapshot);
    assert_eq!(tm_restored.tabs().len(), 2);
    assert_eq!(tm_restored.tabs()[0].url, "https://alpha.example");
    assert_eq!(tm_restored.tabs()[1].url, "https://beta.example");
}

#[test]
fn test_tab_manager_extract_and_insert_tab() {
    let mut tm1 = TabManager::new();
    let id1 = tm1.create_tab("https://tab1.com", 100);
    let _id2 = tm1.create_tab("https://tab2.com", 200);

    // Extract tab 1 from tm1
    let extracted = tm1.extract_tab(id1);
    assert!(extracted.is_some());
    let tab_state = extracted.unwrap();
    assert_eq!(tab_state.url, "https://tab1.com");
    assert_eq!(tm1.tabs().len(), 1);

    // Insert into tm2
    let mut tm2 = TabManager::new();
    let new_id = tm2.insert_tab(tab_state, None);
    assert_eq!(tm2.tabs().len(), 1);
    assert_eq!(tm2.tabs()[0].id, new_id);
    assert_eq!(tm2.tabs()[0].url, "https://tab1.com");
    assert_eq!(tm2.active_tab_id(), Some(new_id));
}

#[test]
fn test_tab_manager_update_url_title_and_favicon() {
    let mut tm = TabManager::new();
    let id = tm.create_tab("https://initial.com", 100);

    tm.update_url(id, "https://updated.com".to_string());
    tm.update_title(id, "Updated Title".to_string());
    tm.update_favicon(id, Some("https://updated.com/favicon.ico".to_string()));

    let tab = tm.tabs().iter().find(|t| t.id == id).unwrap();
    assert_eq!(tab.url, "https://updated.com");
    assert_eq!(tab.title, "Updated Title");
    assert_eq!(
        tab.favicon_uri,
        Some("https://updated.com/favicon.ico".to_string())
    );
}

#[test]
fn test_settings_search_url_template_fallback() {
    let settings = Settings {
        search_engine: "unknown_engine".to_string(),
        ..Default::default()
    };

    assert_eq!(
        settings.search_url_template(),
        "https://duckduckgo.com/?q=%s"
    );
}

#[test]
fn test_persistent_sites_matching() {
    let mut settings = Settings::default();
    // Remember mode: every site keeps its data.
    assert!(settings.is_site_persistent("https://github.com/login"));

    // Private mode: only the keep list does.
    settings.privacy.ephemeral_default = true;
    assert!(!settings.is_site_persistent("https://github.com/login"));
    assert!(!settings.is_site_persistent("github.com"));

    settings.privacy.persistent_sites = vec![
        "github.com".to_string(),
        "*.google.com".to_string(),
        "login.microsoft.com".to_string(),
    ];

    assert!(settings.is_site_persistent("github.com"));
    assert!(settings.is_site_persistent("https://github.com"));
    assert!(settings.is_site_persistent("https://github.com/login?foo=bar#hash"));
    assert!(settings.is_site_persistent("https://www.github.com"));
    assert!(settings.is_site_persistent("http://github.com:8080/path"));

    assert!(settings.is_site_persistent("https://gist.github.com"));
    assert!(settings.is_site_persistent("https://api.github.com/v3"));
    assert!(settings.is_site_persistent("https://accounts.google.com/signin"));
    assert!(settings.is_site_persistent("google.com"));
    assert!(settings.is_site_persistent("https://login.microsoft.com/oauth2"));

    assert!(!settings.is_site_persistent("https://attacker-github.com"));
    assert!(!settings.is_site_persistent("https://fakegoogle.com"));
    assert!(!settings.is_site_persistent("https://microsoft.com"));
    assert!(!settings.is_site_persistent("https://reddit.com"));
    assert!(!settings.is_site_persistent("shoji://newtab"));
    assert!(!settings.is_site_persistent("about:blank"));
    assert!(!settings.is_site_persistent(""));

    settings.privacy.ephemeral_default = false;
    assert!(settings.is_site_persistent("https://reddit.com"));
}

#[test]
fn test_private_mode_choice_and_legacy_files() {
    let dir = std::env::temp_dir().join(format!("shoji-privacy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");

    // An old file that only carries the previous "forget everything" default is not a choice.
    std::fs::write(
        &path,
        r#"{"privacy":{"ephemeral_default":true,"persistent_sites":[]}}"#,
    )
    .unwrap();
    let loaded = Settings::load_from_path(&path);
    assert!(!loaded.privacy.ephemeral_default);

    // Choosing private mode (setup or Settings) is remembered across loads.
    let mut s = loaded;
    assert!(s.apply_patch(&serde_json::json!({"privacy": {"ephemeral_default": true}})));
    assert!(s.privacy.ephemeral_default && s.privacy.mode_chosen);
    s.save_to_path(&path).unwrap();
    assert!(Settings::load_from_path(&path).privacy.ephemeral_default);

    // Choosing "remember" explicitly is also kept.
    assert!(s.apply_patch(&serde_json::json!({"privacy": {"ephemeral_default": false}})));
    s.save_to_path(&path).unwrap();
    let again = Settings::load_from_path(&path);
    assert!(!again.privacy.ephemeral_default && again.privacy.mode_chosen);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_profile_migration_moves_once_and_never_overwrites() {
    let root = std::env::temp_dir().join(format!("shoji-migrate-{}", std::process::id()));
    let legacy = root.join("roaming").join("webview2_data");
    let target = root.join("local").join("Shoji").join("User Data");
    std::fs::create_dir_all(legacy.join("EBWebView")).unwrap();
    std::fs::write(legacy.join("EBWebView").join("marker"), "login").unwrap();

    let used = shoji_core::env::migrate_profile(&legacy, target.clone());
    assert_eq!(used, target);
    assert!(target.join("EBWebView").join("marker").exists());
    assert!(!legacy.exists());

    // An existing profile at the target is never replaced by a stale legacy folder.
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("other"), "x").unwrap();
    let used = shoji_core::env::migrate_profile(&legacy, target.clone());
    assert_eq!(used, target);
    assert!(target.join("EBWebView").join("marker").exists());
    assert!(legacy.join("other").exists());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn test_ipc_serde_roundtrip() {
    let variants = vec![
        UiToHostMessage::ChromeReady,
        UiToHostMessage::CreateTab {
            url: Some("https://test.com".into()),
        },
        UiToHostMessage::CreateTab { url: None },
        UiToHostMessage::SwitchTab { id: TabId(42) },
        UiToHostMessage::Navigate {
            url: "https://nav.com".into(),
        },
        UiToHostMessage::ReorderTab {
            from_index: 0,
            to_index: 2,
        },
        UiToHostMessage::DetachTabToNewWindow {
            tab_id: TabId(3),
            screen_x: Some(1.0),
            screen_y: None,
        },
        UiToHostMessage::FindInPage {
            query: "q".into(),
            forward: true,
        },
        UiToHostMessage::SaveSettings {
            patch: serde_json::json!({ "search_engine": "bing" }),
        },
        UiToHostMessage::SetAccent {
            accent: "gold".into(),
        },
        UiToHostMessage::SetTheme {
            theme: "dark".into(),
        },
        UiToHostMessage::FinishWelcome,
        UiToHostMessage::BypassCertificateError {
            tab_id: TabId(1),
            host: "a.com".into(),
            url: "https://a.com".into(),
        },
    ];
    for msg in variants {
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(serde_json::from_str::<UiToHostMessage>(&json).unwrap(), msg);
    }
    let sync = HostToUiMessage::TabStateSync {
        tabs: vec![],
        active_tab_id: None,
        groups: vec![],
        split: None,
        split_state: None,
        splits: vec![],
        workspaces: vec![],
        active_workspace_id: 1,
        inactive_tabs: vec![],
        containers: vec![],
    };
    let json = serde_json::to_string(&sync).unwrap();
    assert_eq!(
        serde_json::from_str::<HostToUiMessage>(&json).unwrap(),
        sync
    );
}

#[test]
fn test_settings_apply_patch_merges_nested_objects() {
    let mut s = Settings::default();
    assert!(s.apply_patch(&serde_json::json!({
        "search_engine": "google",
        "downloads": { "ask_where_to_save": false },
        "appearance": { "accent": "PURPLE" }
    })));
    assert_eq!(s.search_engine, "google");
    assert!(!s.downloads.ask_where_to_save);
    assert!(
        s.downloads.show_progress_toolbar,
        "sibling fields untouched"
    );
    assert_eq!(s.appearance.accent, "purple", "accent ids are normalized");
    assert_eq!(s.appearance.default_zoom_level, 1.0);
}

#[test]
fn test_settings_apply_patch_rejects_bad_types_and_clamps() {
    let mut s = Settings::default();
    let before = s.clone();
    assert!(!s.apply_patch(&serde_json::json!({ "downloads": { "ask_where_to_save": "yes" } })));
    assert_eq!(s, before, "invalid patch leaves settings untouched");

    assert!(s.apply_patch(&serde_json::json!({
        "appearance": { "default_zoom_level": 40.0, "theme": "neon", "accent": "nope" },
        "permissions": { "camera": "always" }
    })));
    assert_eq!(s.appearance.default_zoom_level, 3.0);
    assert_eq!(s.appearance.theme, "system");
    assert_eq!(s.appearance.accent, "white");
    assert_eq!(s.permissions.camera, "ask");
}

#[test]
fn test_settings_persistent_sites_are_normalized() {
    let mut s = Settings::default();
    s.apply_patch(&serde_json::json!({
        "privacy": { "persistent_sites": ["GitHub.com", " accounts.google.com ", "", "github.com"] }
    }));
    assert_eq!(
        s.privacy.persistent_sites,
        vec!["github.com", "accounts.google.com"]
    );
    assert!(s.is_site_persistent("https://github.com"));
    assert!(s.is_site_persistent("https://accounts.google.com"));
}

#[test]
fn test_settings_load_tolerates_old_and_partial_files() {
    let dir = std::env::temp_dir().join(format!("shoji_settings_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");

    // An older file with removed sections and missing new fields still loads.
    std::fs::write(&path, r#"{"search_engine":"brave","features":{"enable_find_in_page":true},"engine":{"auto_check":true}}"#).unwrap();
    let s = Settings::load_from_path(&path);
    assert_eq!(s.search_engine, "brave");
    assert_eq!(s.appearance.accent, "white");
    assert!(!s.first_run_complete);

    // Garbage falls back to defaults instead of failing.
    std::fs::write(&path, "{not json").unwrap();
    assert_eq!(Settings::load_from_path(&path), Settings::default());

    // Save is atomic and round-trips.
    let mut s = Settings::default();
    s.appearance.accent = "cyan".into();
    s.save_to_path(&path).unwrap();
    assert_eq!(Settings::load_from_path(&path).appearance.accent, "cyan");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_permission_policy_lookup() {
    let mut s = Settings::default();
    s.permissions.camera = "block".into();
    assert_eq!(s.permission_policy("Camera"), "block");
    assert_eq!(s.permission_policy("Microphone"), "ask");
    assert_eq!(s.permission_policy("Clipboard"), "ask");
}

#[test]
fn test_purge_script_deletes_only_safe_paths() {
    use shoji_core::env::purge_script;
    use std::path::PathBuf;

    let dirs = [
        PathBuf::from(r"C:\Users\a\AppData\Roaming\shoji"),
        PathBuf::from(r"C:\Users\%USERNAME%\evil"),
        PathBuf::from(r#"C:\Users\a\x" & calc"#),
        PathBuf::from(r"C:\"),
    ];
    let files = [PathBuf::from(r"C:\Users\a\Desktop\Shoji.lnk")];
    let script = purge_script(&dirs, &files).unwrap();
    assert!(script.contains(r#"rmdir /s /q "C:\Users\a\AppData\Roaming\shoji""#));
    assert!(script.contains(r#"del /f /q "C:\Users\a\Desktop\Shoji.lnk""#));
    assert!(!script.contains("USERNAME") && !script.contains("calc"));
    assert_eq!(script.matches("rmdir").count(), 1);
    // It keeps trying while something holds the files, then gives up and removes itself.
    assert!(script.contains(r#"if exist "C:\Users\a\AppData\Roaming\shoji" goto wait"#));
    assert!(script.contains("goto again") && script.contains("geq 60"));
    assert!(script.contains(r#"del /f /q "%~f0""#));

    assert!(purge_script(&[PathBuf::from(r"C:\")], &[]).is_none());
}

#[test]
fn test_every_place_shoji_stores_things_is_removed_by_uninstall() {
    use shoji_core::env::*;
    use std::path::{Path, PathBuf};
    // Only meaningful where Windows' folders are known.
    if std::env::var_os("APPDATA").is_none() || std::env::var_os("LOCALAPPDATA").is_none() {
        return;
    }
    let roots = footprint_dirs();
    let covered = |p: &Path| roots.iter().any(|r| is_within(p, r));
    let data = resolve_data_directory(Path::new(r"C:\nowhere"));
    let install = install_directory().unwrap();
    for p in [
        data.clone(),
        data.join("settings.json"),
        data.join("updates"),
        data.join("byobu"),
        data.join("webview2_data"),
        local_app_directory().unwrap().join("User Data"),
        install.join("shoji.exe"),
        install.join("uninstall.exe"),
    ] {
        assert!(covered(&p), "{} is not removed on uninstall", p.display());
    }
    assert!(!covered(&PathBuf::from(r"C:\Windows")));
    assert!(is_within(Path::new(r"C:\A\b\C.txt"), Path::new(r"c:\a\B\")));
    assert!(!is_within(Path::new(r"C:\A\bc"), Path::new(r"C:\A\b")));
}

#[test]
fn test_uninstall_message_round_trips_with_confirmation() {
    let json = r#"{"action":"UninstallShoji","payload":{"confirm":"UNINSTALL"}}"#;
    match serde_json::from_str::<UiToHostMessage>(json).unwrap() {
        UiToHostMessage::UninstallShoji { confirm } => {
            assert_eq!(confirm, shoji_core::ipc::UNINSTALL_CONFIRMATION)
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn test_run_in_background_defaults_on_and_can_be_switched_off() {
    use shoji_core::settings::Settings;

    let mut s = Settings::default();
    assert!(s.performance.run_in_background);

    assert!(s.apply_patch(&serde_json::json!({ "performance": { "run_in_background": false } })));
    assert!(!s.performance.run_in_background);
    // The sibling setting is untouched by the patch.
    assert_eq!(s.performance.sleep_after_secs, 30);

    // Settings files written before the option existed load with it on.
    let old: Settings =
        serde_json::from_str(r#"{ "performance": { "sleep_after_secs": 60 } }"#).unwrap();
    assert!(old.performance.run_in_background);
}

#[test]
fn test_link_privacy_defaults_on_and_old_files_load() {
    let s = Settings::default();
    assert!(s.shield.clean_links && s.shield.send_gpc);
    let old: Settings = serde_json::from_str(r#"{"shield":{"mode":"standard"}}"#).unwrap();
    assert!(old.shield.clean_links && old.shield.send_gpc);
    let off: Settings = serde_json::from_str(r#"{"shield":{"clean_links":false}}"#).unwrap();
    assert!(!off.shield.clean_links && off.shield.send_gpc);
}

#[test]
fn pinned_tabs_stay_together_at_the_left() {
    let mut tm = TabManager::new();
    let a = tm.create_tab("https://a.com", 0);
    let b = tm.create_tab("https://b.com", 0);
    let c = tm.create_tab("https://c.com", 0);

    assert!(tm.set_pinned(c, true));
    assert!(!tm.set_pinned(c, true), "already pinned");
    assert!(tm.set_pinned(b, true));
    let order: Vec<_> = tm.tabs().iter().map(|t| t.id).collect();
    assert_eq!(order, vec![c, b, a]);
    assert_eq!(tm.pinned_count(), 2);

    // Pinned tabs reorder only among themselves, unpinned only outside the block.
    assert!(!tm.reorder_tab(0, 2));
    assert!(!tm.reorder_tab(2, 0));
    assert!(tm.reorder_tab(0, 1));
    assert_eq!(tm.tabs()[0].id, b);

    // Unpinning drops the tab just after the pinned block.
    assert!(tm.set_pinned(b, false));
    let order: Vec<_> = tm.tabs().iter().map(|t| t.id).collect();
    assert_eq!(order, vec![c, b, a]);
    assert_eq!(tm.pinned_count(), 1);
}

#[test]
fn pinning_leaves_a_group_and_inserted_tabs_respect_the_block() {
    let mut tm = TabManager::new();
    let a = tm.create_tab("https://a.com", 0);
    let b = tm.create_tab("https://b.com", 0);
    let g = tm.create_group("Work", "blue");
    assert!(tm.set_tab_group(a, Some(g)));
    assert!(tm.set_pinned(a, true));
    assert_eq!(tm.get_tab_by_id(a).unwrap().group, None);
    assert!(tm.groups().is_empty(), "the emptied group is dropped");

    let mut tab = tm.extract_tab(b).unwrap();
    tab.pinned = false;
    tm.insert_tab(tab, Some(0));
    assert_eq!(
        tm.tabs()[0].id,
        a,
        "an unpinned tab can't land before pinned ones"
    );

    tm.set_tab_muted(b, true);
    assert!(tm.get_tab_by_id(b).unwrap().is_muted);
}

#[test]
fn test_workspace_crud_and_reorder() {
    let mut tm = TabManager::new();
    tm.create_tab("https://main.example", 10);
    assert_eq!(tm.workspaces().len(), 1);
    assert_eq!(tm.active_workspace_id(), 1);

    let ws2 = tm.create_workspace("Work", "green");
    let ws3 = tm.create_workspace("Reading", "amber");
    tm.switch_workspace(ws3, 30);
    assert_eq!(tm.workspaces().len(), 3);
    assert_eq!(tm.active_workspace_id(), ws3);

    tm.rename_workspace(ws2, "Work & Docs");
    tm.set_workspace_color(ws2, "purple");
    let w2 = tm.workspaces().iter().find(|w| w.id == ws2).unwrap();
    assert_eq!(w2.name, "Work & Docs");
    assert_eq!(w2.color, "purple");

    assert!(tm.reorder_workspace(2, 0));
    assert_eq!(tm.workspaces()[0].id, ws3);

    // Deleting active workspace switches to neighbor; last workspace cannot be deleted.
    assert!(tm.delete_workspace(ws3, 40));
    assert_eq!(tm.workspaces().len(), 2);
    assert!(tm.delete_workspace(1, 41));
    assert_eq!(tm.workspaces().len(), 1);
    assert!(!tm.delete_workspace(ws2, 42));
}

#[test]
fn test_workspace_switch_suspends_and_restores_active_tab() {
    let mut tm = TabManager::new();
    let t1 = tm.create_tab("https://one.example", 10);
    let t2 = tm.create_tab("https://two.example", 20);
    tm.switch_tab(t1, 25);
    assert_eq!(tm.active_tab_id(), Some(t1));

    let ws2 = tm.create_workspace("Second", "red");
    assert!(tm.switch_workspace(ws2, 30));
    assert_eq!(tm.active_workspace_id(), ws2);
    // Tabs in the inactive workspace (ws 1) are suspended, not destroyed.
    assert_eq!(tm.inactive_tabs().len(), 2);
    assert!(tm
        .inactive_tabs()
        .iter()
        .all(|t| t.status == TabStatus::Suspended));

    let w2_tab = tm.active_tab_id().unwrap();
    tm.update_url(w2_tab, "https://second.example".into());
    tm.update_title(w2_tab, "Second".into());

    // Switching back to workspace 1 restores its last active tab (t1).
    assert!(tm.switch_workspace(1, 40));
    assert_eq!(tm.active_workspace_id(), 1);
    assert_eq!(tm.active_tab_id(), Some(t1));
    assert_eq!(tm.tabs().len(), 2);
    assert!(tm.tabs().iter().any(|t| t.id == t2));
    assert_eq!(tm.inactive_tabs().len(), 1);
    assert_eq!(tm.inactive_tabs()[0].status, TabStatus::Suspended);
}

#[test]
fn test_workspace_tab_moves_and_last_tab_blank() {
    let mut tm = TabManager::new();
    let t1 = tm.create_tab("https://move.example", 10);
    let ws2 = tm.create_workspace("Target", "cyan");
    assert_eq!(tm.tabs().len(), 1);

    // Moving the only tab out of workspace 1 leaves workspace 1 with a blank tab.
    assert!(tm.move_tab_to_workspace(t1, ws2, 30));
    assert_eq!(tm.tabs().len(), 1);
    assert_ne!(tm.tabs()[0].id, t1);
    assert_eq!(tm.tabs()[0].url, "shoji://newtab");

    // Switching to ws2 shows the moved tab.
    tm.switch_workspace(ws2, 35);
    assert!(tm.tabs().iter().any(|t| t.id == t1));

    // Closing the last tab in a workspace via close_workspace_tab leaves a blank tab.
    let ids: Vec<TabId> = tm.tabs().iter().map(|t| t.id).collect();
    for id in ids {
        tm.close_workspace_tab(id, 40);
    }
    assert_eq!(tm.tabs().len(), 1);
    assert_eq!(tm.tabs()[0].url, "shoji://newtab");
}

#[test]
fn test_split_open_close_focus_and_workspace_survival() {
    use shoji_core::tabs::SplitOrientation;

    let mut tm = TabManager::new();
    let t1 = tm.create_tab("https://left.example", 10);
    let t2 = tm.create_tab("https://right.example", 20);
    let t3 = tm.create_tab("https://third.example", 30);
    tm.switch_tab(t1, 35);

    assert!(tm.open_split(t1, t2).is_some());
    assert_eq!(tm.split_panes(), Some(vec![t1, t2]));
    assert!(tm.is_tab_visible(t1) && tm.is_tab_visible(t2) && !tm.is_tab_visible(t3));

    // Switching focus between split panes keeps the split open and changes active_tab_id.
    assert_eq!(tm.switch_split_pane(40), Some(t2));
    assert_eq!(tm.active_tab_id(), Some(t2));
    assert_eq!(tm.switch_split_pane(41), Some(t1));
    assert_eq!(tm.active_tab_id(), Some(t1));

    // Ratio clamping and reset to 50/50.
    assert!(tm.set_split_ratio(0.95));
    assert!(tm.visible_split().unwrap().ratio() <= 0.85);
    assert!(tm.reset_split_ratio());
    assert!((tm.visible_split().unwrap().ratio() - 0.5).abs() < 1e-6);

    // Orientation toggle.
    assert_eq!(
        tm.toggle_split_orientation(),
        Some(SplitOrientation::Horizontal)
    );

    // Split survives switching to another workspace and back.
    let ws2 = tm.create_workspace("Other", "rose");
    tm.switch_workspace(ws2, 50);
    assert_eq!(tm.active_workspace_id(), ws2);
    assert!(tm.split_panes().is_none());
    tm.switch_workspace(1, 60);
    assert_eq!(tm.split_panes(), Some(vec![t1, t2]));

    // Closing one pane exits the split and focuses the surviving pane.
    tm.close_tab(t1);
    assert!(tm.split_panes().is_none());
    assert_eq!(tm.active_tab_id(), Some(t2));
}

#[test]
fn pinned_tabs_stay_in_the_pinned_block_across_workspaces() {
    let mut tm = TabManager::new();
    let a = tm.create_tab("https://a.com", 0);
    let b = tm.create_tab("https://b.com", 0);
    let ws = tm.create_workspace("Two", "blue");
    assert!(tm.set_pinned(b, true));
    assert_eq!(tm.tabs()[0].id, b);

    assert!(tm.switch_workspace(ws, 1));
    let c = tm.create_tab("https://c.com", 1);
    assert!(tm.set_pinned(c, true));
    assert!(tm.switch_workspace(1, 2));
    let order: Vec<_> = tm.tabs().iter().map(|t| t.id).collect();
    assert_eq!(order, vec![b, a]);
    assert_eq!(tm.pinned_count(), 1);

    // A pinned tab moved elsewhere lands in the pinned block there.
    assert!(tm.move_tab_to_workspace(b, ws, 3));
    assert!(tm.switch_workspace(ws, 4));
    let order: Vec<_> = tm.tabs().iter().map(|t| t.id).collect();
    assert_eq!(order.len(), 3);
    assert_eq!(tm.pinned_count(), 2);
    assert!(tm.tabs()[..2].iter().all(|t| t.pinned));
}

#[test]
fn split_divider_follows_the_cursor_on_the_right_axis() {
    use shoji_core::tabs::{SplitOrientation, SplitState};
    let mut split = SplitState::new(TabId(1), TabId(2));
    let bounds = (0.0, 84.0, 1010.0, 600.0);
    // Vertical: only x matters; the y value must not leak into the sizes.
    assert!(split.drag_divider(0, bounds, 10.0, 255.0));
    let sizes = split.sizes(1010.0, 10.0);
    assert!((sizes[0] - 250.0).abs() <= 1.0, "{sizes:?}");
    assert_eq!(sizes.iter().sum::<f64>(), 1000.0);
    // Extremes clamp so each pane keeps its minimum width.
    split.drag_divider(0, bounds, 10.0, 0.0);
    assert!(split.sizes(1010.0, 10.0)[0] >= 119.0);
    split.drag_divider(0, bounds, 10.0, 5000.0);
    assert!(split.sizes(1010.0, 10.0)[1] >= 119.0);
    // Horizontal: only y matters, measured from the top of the bounds.
    let mut split = SplitState::new(TabId(1), TabId(2));
    split.orientation = SplitOrientation::Horizontal;
    split.drag_divider(0, bounds, 10.0, 84.0 + 300.0);
    let h = split.sizes(600.0, 10.0);
    assert!((h[0] - 295.0).abs() <= 1.0, "{h:?}");
}

#[test]
fn many_panes_share_the_space_and_keep_a_minimum() {
    use shoji_core::tabs::{SplitOrientation, SplitState};
    let panes: Vec<TabId> = (1..=6).map(TabId).collect();
    let mut split = SplitState::with_panes(panes);
    let bounds = (0.0, 100.0, 1850.0, 700.0);
    let rects = split.layout(bounds, 10.0);
    assert_eq!(rects.len(), 6);
    // Equal columns that tile the width exactly (6 panes, 5 gaps).
    assert_eq!(rects.iter().map(|r| r.2).sum::<f64>(), 1800.0);
    for pair in rects.windows(2) {
        assert_eq!(pair[0].0 + pair[0].2 + 10.0, pair[1].0);
    }
    assert_eq!(split.divider_rects(bounds, 10.0).len(), 5);

    // Dragging one divider moves only the two panes beside it.
    let before = split.sizes(1850.0, 10.0);
    assert!(split.drag_divider(2, bounds, 10.0, rects[2].0 + 400.0));
    let after = split.sizes(1850.0, 10.0);
    assert_eq!(before[0], after[0]);
    assert_eq!(before[1], after[1]);
    assert_eq!(before[4], after[4]);
    assert_eq!(before[2] + before[3], after[2] + after[3]);
    assert!(after[2] > before[2]);

    // Too narrow for the minimum: panes share equally instead of overflowing.
    let tight = split.sizes(300.0, 10.0);
    assert_eq!(tight.len(), 6);
    assert_eq!(tight.iter().sum::<f64>(), 250.0);
    assert!(tight.iter().all(|s| *s >= 1.0));

    // Stacked panes lay out along y.
    split.orientation = SplitOrientation::Horizontal;
    split.equalize();
    let rows = split.layout(bounds, 10.0);
    assert!(rows.iter().all(|r| r.2 == 1850.0));
    assert!(rows[1].1 > rows[0].1);
}

#[test]
fn a_split_can_grow_shrink_and_survive_a_session() {
    let mut tm = TabManager::new();
    let ids: Vec<TabId> = (0..5)
        .map(|i| tm.create_tab(&format!("https://{i}.example"), 10 + i))
        .collect();
    let id = tm.open_split(ids[0], ids[1]).expect("split");
    tm.switch_tab(ids[0], 20);
    // Add at the end and at the front; the strip follows the screen order.
    assert!(tm.add_to_split(id, ids[2], None));
    assert!(tm.add_to_split(id, ids[3], Some(0)));
    assert_eq!(tm.split_panes(), Some(vec![ids[3], ids[0], ids[1], ids[2]]));
    let strip: Vec<TabId> = tm.tabs().iter().map(|t| t.id).collect();
    assert_eq!(&strip[..4], &[ids[3], ids[0], ids[1], ids[2]]);
    assert!(!tm.add_to_split(id, ids[2], None), "already a pane");
    let w: f64 = tm.visible_split().unwrap().weights.iter().sum();
    assert!((w - 1.0).abs() < 1e-9);

    // Neighbours swap places.
    assert!(tm.swap_split_pair(id, 0));
    assert_eq!(tm.split_of(ids[0]).unwrap().panes[0], ids[0]);

    // A session keeps the group and its sizes.
    tm.switch_tab(ids[0], 50);
    assert!(tm.set_split_ratio(0.5) || tm.reset_split_ratio());
    let saved = tm.to_session_window().unwrap();
    let json = serde_json::to_string(&saved).unwrap();
    let back: shoji_core::userdata::SessionWindow = serde_json::from_str(&json).unwrap();
    let mut restored = TabManager::new();
    restored.restore_session_window(&back, 70);
    let sp = restored.splits().first().expect("split restored");
    assert_eq!(sp.panes.len(), 4);

    // Taking a pane out keeps the group until fewer than two are left.
    assert!(tm.remove_from_split(ids[3]));
    assert_eq!(tm.split_of(ids[0]).unwrap().panes.len(), 3);
    assert!(tm.remove_from_split(ids[2]));
    assert!(tm.remove_from_split(ids[0]));
    assert!(tm.splits().is_empty(), "one tab left: no group");

    // Closing a pane of a bigger group leaves the rest together.
    let id = tm.open_split(ids[0], ids[1]).expect("split");
    assert!(tm.add_to_split(id, ids[2], None));
    tm.close_tab(ids[1]);
    assert_eq!(tm.split_of(ids[0]).unwrap().panes, vec![ids[0], ids[2]]);

    // Pinned tabs cannot join, and the group is capped.
    assert!(tm.set_pinned(ids[4], true));
    assert!(!tm.add_to_split(id, ids[4], None));
}

fn four_tabs() -> (TabManager, [TabId; 4]) {
    let mut tm = TabManager::new();
    let a = tm.create_tab("https://a.example", 1);
    let b = tm.create_tab("https://b.example", 2);
    let c = tm.create_tab("https://c.example", 3);
    let d = tm.create_tab("https://d.example", 4);
    (tm, [a, b, c, d])
}

#[test]
fn several_split_groups_coexist_and_a_tab_is_in_at_most_one() {
    let (mut tm, [a, b, c, d]) = four_tabs();
    assert!(tm.splits().is_empty());
    let g1 = tm.open_split(a, b).unwrap();
    let g2 = tm.open_split_with(c, d, "Docs", "gold").unwrap();
    assert_ne!(g1, g2);
    assert_eq!(tm.splits().len(), 2);
    assert_eq!(tm.split_of(a).unwrap().name, format!("Split {g1}"));
    assert_eq!(tm.split_of(d).unwrap().name, "Docs");
    assert_eq!(tm.split_of(d).unwrap().color, "gold");
    assert_eq!(tm.other_split_pane(c), Some(d));
    assert!(tm.is_in_split(a) && tm.is_in_split(d));

    // The visible group is the one of the active tab (d is active).
    assert_eq!(tm.visible_split().unwrap().id, g2);
    tm.switch_tab(a, 10);
    assert_eq!(tm.visible_split().unwrap().id, g1);

    // Re-pairing b with c dissolves both old groups, leaving only the new one.
    let g3 = tm.open_split(b, c).unwrap();
    assert_eq!(tm.splits().len(), 1);
    assert_eq!(tm.splits()[0].id, g3);
    assert!(!tm.is_in_split(a) && !tm.is_in_split(d));
}

#[test]
fn open_split_refuses_bad_input_and_pinned_tabs() {
    let (mut tm, [a, b, c, _d]) = four_tabs();
    assert!(tm.open_split(a, a).is_none());
    assert!(tm.open_split(a, TabId(999)).is_none());
    assert!(tm.set_pinned(a, true));
    assert!(tm.open_split(a, b).is_none());
    assert!(tm.open_split(b, a).is_none());
    assert!(tm.splits().is_empty());

    // A pane cannot be pinned.
    assert!(tm.open_split(b, c).is_some());
    assert!(!tm.set_pinned(b, true));
    assert!(!tm.tabs().iter().find(|t| t.id == b).unwrap().pinned);

    // Tabs from different workspaces are refused.
    let ws = tm.create_workspace("Other", "rose");
    assert!(tm.move_tab_to_workspace(c, ws, 20));
    assert!(tm.open_split(c, b).is_none());
}

#[test]
fn closing_a_pane_dissolves_only_its_group() {
    let (mut tm, [a, b, c, d]) = four_tabs();
    tm.open_split(a, b).unwrap();
    let g2 = tm.open_split(c, d).unwrap();
    tm.switch_tab(a, 10);
    tm.close_tab(a);
    assert_eq!(tm.splits().len(), 1);
    assert_eq!(tm.splits()[0].id, g2);
    // The surviving pane of the dissolved group takes focus.
    assert_eq!(tm.active_tab_id(), Some(b));
}

#[test]
fn split_groups_survive_workspace_switch_and_moving_a_pane_dissolves_its_group() {
    let (mut tm, [a, b, c, d]) = four_tabs();
    tm.open_split(a, b).unwrap();
    tm.open_split(c, d).unwrap();
    let ws2 = tm.create_workspace("Other", "rose");
    assert!(tm.switch_workspace(ws2, 20));
    assert!(tm.splits().is_empty());
    assert!(tm.switch_workspace(1, 30));
    assert_eq!(tm.splits().len(), 2);

    // Moving one pane away dissolves just that group.
    assert!(tm.move_tab_to_workspace(a, ws2, 40));
    assert_eq!(tm.splits().len(), 1);
    assert!(tm.is_in_split(c) && tm.is_in_split(d));
    assert!(!tm.is_in_split(b));
}

#[test]
fn toggle_split_never_creates_a_group() {
    let (mut tm, [a, b, ..]) = four_tabs();
    tm.switch_tab(a, 10);
    assert!(!tm.toggle_split());
    assert!(tm.splits().is_empty());
    assert_eq!(tm.tabs().len(), 4);
    tm.open_split(a, b).unwrap();
    assert!(tm.toggle_split());
    assert!(tm.splits().is_empty());

    // A lone tab does not get a blank partner either.
    let mut one = TabManager::new();
    one.create_tab("https://x.example", 1);
    assert!(!one.toggle_split());
    assert_eq!(one.tabs().len(), 1);
}

#[test]
fn split_panes_are_gathered_next_to_each_other() {
    let (mut tm, [a, b, c, d]) = four_tabs();
    tm.open_split(a, d).unwrap();
    let order: Vec<_> = tm.tabs().iter().map(|t| t.id).collect();
    assert_eq!(order, vec![a, d, b, c]);

    // gather_split on its own: move c to follow a.
    assert!(tm.gather_split(a, c));
    let order: Vec<_> = tm.tabs().iter().map(|t| t.id).collect();
    assert_eq!(order, vec![a, c, d, b]);
    assert!(!tm.gather_split(a, a));
    assert!(!tm.gather_split(a, TabId(999)));
}

#[test]
fn split_swap_rename_colour_and_close() {
    let (mut tm, [a, b, ..]) = four_tabs();
    let g = tm.open_split(a, b).unwrap();
    assert!(tm.swap_split_panes(g));
    assert_eq!(tm.split_of(a).unwrap().panes, [b, a]);
    assert!(tm.rename_split(g, "  Research  "));
    assert!(!tm.rename_split(g, "   "));
    assert!(tm.set_split_color(g, "green"));
    assert!(!tm.set_split_color(g, ""));
    let s = tm.split_of(a).unwrap();
    assert_eq!((s.name.as_str(), s.color.as_str()), ("Research", "green"));
    assert!(!tm.rename_split(99, "x") && !tm.swap_split_panes(99) && !tm.close_split(99));
    assert!(tm.close_split(g));
    assert!(tm.splits().is_empty());
    assert_eq!(tm.tabs().len(), 4);
}

#[test]
fn ratio_and_orientation_apply_to_the_visible_group_only() {
    use shoji_core::tabs::SplitOrientation;
    let (mut tm, [a, b, c, d]) = four_tabs();
    tm.open_split(a, b).unwrap();
    tm.open_split(c, d).unwrap();
    tm.switch_tab(a, 10);
    assert!(tm.set_split_ratio(0.3));
    assert_eq!(
        tm.toggle_split_orientation(),
        Some(SplitOrientation::Horizontal)
    );
    assert!((tm.split_of(a).unwrap().ratio() - 0.3).abs() < 1e-9);
    assert_eq!(tm.split_of(c).unwrap().ratio(), 0.5);
    assert_eq!(
        tm.split_of(c).unwrap().orientation,
        SplitOrientation::Vertical
    );
    // With a non-member active there is no visible group to adjust.
    let lone = tm.create_tab("https://e.example", 50);
    assert_eq!(tm.active_tab_id(), Some(lone));
    assert!(!tm.set_split_ratio(0.4));
    assert!(tm.toggle_split_orientation().is_none());
}

#[test]
fn session_roundtrip_keeps_two_groups_with_names_colours_ratios_and_orientation() {
    use shoji_core::tabs::SplitOrientation;
    let (mut tm, [a, b, c, d]) = four_tabs();
    let g1 = tm.open_split_with(a, b, "Left", "rose").unwrap();
    let g2 = tm.open_split_with(c, d, "Right", "gold").unwrap();
    tm.switch_tab(a, 10);
    tm.set_split_ratio(0.3);
    tm.switch_tab(c, 11);
    tm.set_split_orientation(SplitOrientation::Horizontal);
    tm.set_split_ratio(0.65);
    tm.switch_tab(a, 12);

    let saved = tm.to_session_window().unwrap();
    assert_eq!(saved.workspaces[0].splits.len(), 2);
    assert!(saved.workspaces[0].split.is_none());
    let json = serde_json::to_string(&saved).unwrap();
    assert!(
        !json.contains("\"split\":"),
        "legacy field is never written"
    );
    let saved: shoji_core::userdata::SessionWindow = serde_json::from_str(&json).unwrap();

    let mut back = TabManager::new();
    back.restore_session_window(&saved, 100);
    let splits = back.splits().to_vec();
    assert_eq!(splits.len(), 2);
    let by_name = |n: &str| splits.iter().find(|s| s.name == n).unwrap();
    let (l, r) = (by_name("Left"), by_name("Right"));
    assert_eq!((l.id, r.id), (g1, g2));
    assert_eq!((l.color.as_str(), r.color.as_str()), ("rose", "gold"));
    assert!((l.ratio() - 0.3).abs() < 1e-9 && (r.ratio() - 0.65).abs() < 1e-9);
    assert_eq!(l.orientation, SplitOrientation::Vertical);
    assert_eq!(r.orientation, SplitOrientation::Horizontal);
    let url_of = |id| back.get_tab_by_id(id).unwrap().url.clone();
    assert_eq!(url_of(l.panes[0]), "https://a.example");
    assert_eq!(url_of(r.panes[1]), "https://d.example");
    // New groups never reuse a restored id.
    let e = back.create_tab("https://e.example", 200);
    let f = back.create_tab("https://f.example", 201);
    assert!(back.open_split(e, f).unwrap() > g2);
}

#[test]
fn legacy_single_split_session_migrates_into_a_group() {
    let json = r#"{
        "tabs": [
            { "url": "https://one.example", "title": "One", "workspace": 1 },
            { "url": "https://two.example", "title": "Two", "workspace": 1 }
        ],
        "active": 0,
        "workspaces": [{
            "id": 1, "name": "Main", "color": "blue", "active": 0,
            "split": { "panes": [0, 1], "orientation": "horizontal", "ratio": 0.35 }
        }],
        "active_workspace": 1
    }"#;
    let saved: shoji_core::userdata::SessionWindow = serde_json::from_str(json).unwrap();
    assert!(saved.workspaces[0].splits.is_empty());
    let mut tm = TabManager::new();
    tm.restore_session_window(&saved, 5);
    assert_eq!(tm.splits().len(), 1);
    let s = &tm.splits()[0];
    assert_eq!(s.name, format!("Split {}", s.id));
    assert!(s.id >= 1);
    assert_eq!(s.color, "blue");
    assert!((s.ratio() - 0.35).abs() < 1e-9);
    assert_eq!(
        s.orientation,
        shoji_core::tabs::SplitOrientation::Horizontal
    );
    assert!(tm.visible_split().is_some());

    // Out-of-range or duplicate pane indices are dropped rather than trusted.
    let bad = r#"{
        "tabs": [{ "url": "https://one.example", "title": "One", "workspace": 1 }],
        "active": 0,
        "workspaces": [{
            "id": 1, "name": "Main", "color": "blue", "active": 0,
            "splits": [{ "panes": [0, 0] }, { "panes": [0, 7] }]
        }],
        "active_workspace": 1
    }"#;
    let saved: shoji_core::userdata::SessionWindow = serde_json::from_str(bad).unwrap();
    let mut tm = TabManager::new();
    tm.restore_session_window(&saved, 5);
    assert!(tm.splits().is_empty());
}

#[test]
fn legacy_workspace_split_field_is_migrated_on_load() {
    let (tm, [a, b, ..]) = four_tabs();
    let mut v = serde_json::to_value(&tm).unwrap();
    v["workspaces"][0]["split"] = serde_json::json!({ "panes": [a, b], "ratio": 0.4 });
    let mut loaded: TabManager = serde_json::from_value(v).unwrap();
    loaded.create_tab("https://e.example", 9);
    assert_eq!(loaded.splits().len(), 1);
    assert_eq!(loaded.splits()[0].panes, [a, b]);
    assert!(loaded.splits()[0].id >= 1);
    assert!(loaded.workspaces()[0].split.is_none());
}

#[test]
fn non_web_split_panes_are_kept_in_the_session() {
    let mut tm = TabManager::new();
    let a = tm.create_tab("https://a.example", 1);
    let n1 = tm.create_tab("shoji://newtab", 2);
    tm.create_tab("shoji://newtab", 3);
    tm.open_split(a, n1).unwrap();
    let saved = tm.to_session_window().unwrap();
    // The un-split blank tab is dropped; the split one survives.
    assert_eq!(saved.tabs.len(), 2);
    assert_eq!(saved.workspaces[0].splits[0].panes, [0, 1]);
}

#[test]
fn windows_sharing_an_id_counter_never_reuse_a_tab_id() {
    use shoji_core::tabs::TabManager;
    use std::sync::{atomic::AtomicU64, Arc};
    let ids = Arc::new(AtomicU64::new(1));
    let mut a = TabManager::with_shared_ids(ids.clone());
    let mut b = TabManager::with_shared_ids(ids);
    let mut seen = std::collections::HashSet::new();
    for _ in 0..5 {
        assert!(seen.insert(a.create_tab("https://a.test", 0)));
        assert!(seen.insert(b.create_tab("https://b.test", 0)));
    }
}

#[test]
fn test_new_defaults_apply_to_old_files_but_not_to_private_setup() {
    let dir = std::env::temp_dir().join(format!("shoji_defaults_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");

    // Fresh: suggestions on, downloads go straight to the folder.
    let fresh = Settings::default();
    assert!(fresh.appearance.search_suggestions);
    assert!(!fresh.downloads.ask_where_to_save);

    // An old file that carries the previous defaults is brought along.
    std::fs::write(
        &path,
        r#"{"appearance":{"search_suggestions":false},"downloads":{"ask_where_to_save":true}}"#,
    )
    .unwrap();
    let s = Settings::load_from_path(&path);
    assert!(s.appearance.search_suggestions);
    assert!(!s.downloads.ask_where_to_save);

    // Private mode chosen in setup keeps suggestions off.
    std::fs::write(
        &path,
        r#"{"privacy":{"ephemeral_default":true,"mode_chosen":true},"appearance":{"search_suggestions":false}}"#,
    )
    .unwrap();
    assert!(
        !Settings::load_from_path(&path)
            .appearance
            .search_suggestions
    );

    // After that, the user's own choices stick.
    let mut s = Settings::default();
    s.appearance.search_suggestions = false;
    s.downloads.ask_where_to_save = true;
    s.save_to_path(&path).unwrap();
    let back = Settings::load_from_path(&path);
    assert!(!back.appearance.search_suggestions);
    assert!(back.downloads.ask_where_to_save);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_third_party_cookies_and_fingerprinting_default_on_once() {
    let dir = std::env::temp_dir().join(format!("shoji_rev3_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    let fresh = Settings::default();
    assert!(fresh.privacy.block_third_party_cookies);
    assert_eq!(fresh.privacy.fingerprinting, "standard");

    // A 0.1.5 file carries the old "off"; it is turned on once.
    std::fs::write(
        &path,
        r#"{"defaults_rev":2,"privacy":{"block_third_party_cookies":false}}"#,
    )
    .unwrap();
    let mut s = Settings::load_from_path(&path);
    assert!(s.privacy.block_third_party_cookies);
    assert_eq!(s.privacy.fingerprinting, "standard");

    // Turned off after that, it stays off.
    s.privacy.block_third_party_cookies = false;
    s.privacy.fingerprinting = "nonsense".into();
    s.save_to_path(&path).unwrap();
    let back = Settings::load_from_path(&path);
    assert!(!back.privacy.block_third_party_cookies);
    assert_eq!(back.privacy.fingerprinting, "standard");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_container_rules_cover_subdomains_and_skip_unknown_containers() {
    let mut s = Settings::default();
    s.containers.set_rule("www.github.com", Some("work"));
    assert_eq!(s.containers.for_host("github.com"), Some("work"));
    assert_eq!(s.containers.for_host("gist.github.com"), Some("work"));
    assert_eq!(s.containers.for_host("notgithub.com"), None);
    s.containers.set_rule("bank.example", Some("gone"));
    assert_eq!(s.containers.for_host("bank.example"), None);
    s.containers.set_rule("github.com", None);
    assert_eq!(s.containers.for_host("github.com"), None);
    // Ids become engine profile names: odd or duplicate ones are dropped on load.
    s.containers.list.push(shoji_core::settings::Container {
        id: "../evil".into(),
        name: "x".into(),
        color: "red".into(),
    });
    s.containers.list.push(s.containers.list[0].clone());
    let path = std::env::temp_dir().join(format!("shoji_ctr_{}.json", std::process::id()));
    s.save_to_path(&path).unwrap();
    let back = Settings::load_from_path(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(back.containers.list.len(), 4);
}

#[test]
fn test_fingerprinting_follows_a_paused_shield_only() {
    let mut s = Settings::default();
    s.privacy.fingerprinting = "strict".into();
    s.shield.paused_sites = vec!["bank.example".into()];
    assert_eq!(s.fingerprint_mode_for("www.bank.example"), "off");
    assert_eq!(s.fingerprint_mode_for("news.example"), "strict");
    s.shield.mode = "off".into();
    assert_eq!(s.fingerprint_mode_for("news.example"), "strict");
}

#[test]
fn test_dark_mode_per_site_always_never_and_follow() {
    let mut s = Settings::default();
    // Dark mode off: only a site set to "always" is dark.
    s.appearance.dark_websites = Some(false);
    assert!(!s.dark_on("example.com"));
    assert!(!s.dark_needed());
    s.appearance.dark_websites_include = vec!["google.com".into()];
    assert!(s.dark_needed());
    assert!(s.dark_on("https://www.google.com/search?q=x"));
    assert!(s.dark_on("mail.google.com"));
    assert!(!s.dark_on("example.com"));
    // Dark mode on: every site is dark except one set to "never".
    s.appearance.dark_websites = Some(true);
    s.appearance.dark_websites_exclude = vec!["brave.com".into()];
    assert!(s.dark_on("example.com"));
    assert!(!s.dark_on("search.brave.com"));
    // "Never" wins if a host ends up in both lists.
    s.appearance.dark_websites_include.push("brave.com".into());
    assert!(!s.dark_on("brave.com"));
}

#[test]
fn sidebar_mode_is_clamped_and_bookmark_messages_stay_off_the_web() {
    let mut s = Settings::default();
    assert_eq!(s.appearance.sidebar, "off");
    assert!(!s.appearance.bookmarks_bar);
    assert!(
        s.apply_patch(&serde_json::json!({"appearance": {"sidebar": "on", "bookmarks_bar": true}}))
    );
    assert_eq!(s.appearance.sidebar, "on");
    assert!(s.appearance.bookmarks_bar);
    assert!(s.apply_patch(&serde_json::json!({"appearance": {"sidebar": "sideways"}})));
    assert_eq!(s.appearance.sidebar, "off");
    // "autohide" (an older value) reads as a shown button.
    assert!(s.apply_patch(&serde_json::json!({"appearance": {"sidebar": "autohide"}})));
    assert_eq!(s.appearance.sidebar, "on");

    // The bookmark requests parse, and a web page may not send any of them.
    use shoji_core::ipc::UiToHostMessage as M;
    let msgs = [
        r#"{"action":"BookmarkMove","payload":{"url":"https://a.com","folder":""}}"#,
        r#"{"action":"BookmarkRemove","payload":{"urls":["https://a.com"]}}"#,
        r#"{"action":"BookmarkMenu","payload":{"from":"bar","x":1,"y":0}}"#,
        r#"{"action":"BookmarkOverflow","payload":{"skip":3,"x":10}}"#,
        r#"{"action":"RunAction","payload":{"value":"exit"}}"#,
        r#"{"action":"SetSidebarMode","payload":{"mode":"on"}}"#,
        r#"{"action":"HoverBookmarks","payload":{"left":10,"right":40,"bar":true}}"#,
        r#"{"action":"PinHover"}"#,
    ];
    for m in msgs {
        let parsed: M = serde_json::from_str(m).expect(m);
        assert!(!parsed.allowed_from_web_content(), "{m}");
    }
}

/// The clean-up really waits out a file that is held open, then removes it and itself.
#[cfg(windows)]
#[test]
fn test_purge_outlasts_a_locked_file() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = std::env::temp_dir().join(format!("shoji_purge_{}", std::process::id()));
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let held = dir.join("sub").join("held.bin");
    std::fs::write(&held, b"x").unwrap();
    // No sharing at all: nothing can delete it while this handle lives.
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&held)
        .unwrap();
    assert!(shoji_core::env::start_purge(
        std::slice::from_ref(&dir),
        &[]
    ));
    std::thread::sleep(std::time::Duration::from_millis(2500));
    assert!(held.exists(), "a locked file can't go yet");
    drop(lock);
    let gone = (0..20).any(|_| {
        std::thread::sleep(std::time::Duration::from_millis(500));
        !dir.exists()
    });
    assert!(gone, "the folder was not removed once it was free");
    let scripts_left = std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .flatten()
        .any(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(&format!("shoji-cleanup-{}-", std::process::id()))
        });
    assert!(!scripts_left, "the clean-up script should delete itself");
}

#[test]
fn shield_categories_are_per_site() {
    let mut s = Settings::default();
    assert!(s.shield_on("news.example", "trackers"));
    s.set_site_category("www.news.example", "fingerprinting", false);
    assert!(!s.shield_on("news.example", "fingerprinting"));
    assert!(!s.shield_on("live.news.example", "fingerprinting"));
    assert_eq!(s.fingerprint_mode_for("news.example"), "off");
    // Fingerprinting off leaves blocking alone, and the other way round.
    assert_eq!(s.shield_mode_for("news.example"), "standard");
    s.set_site_category("news.example", "trackers", false);
    assert_eq!(s.shield_mode_for("news.example"), "off");
    s.set_site_category("news.example", "fingerprinting", true);
    assert_eq!(s.fingerprint_mode_for("news.example"), "standard");
    assert!(s.shield_on("other.example", "trackers"));
    s.set_site_category("news.example", "trackers", true);
    assert!(s.shield.site_off.is_empty());
    s.set_site_category("news.example", "made-up", false);
    assert!(s.shield.site_off.is_empty());
    // Unknown categories from a hand-edited file are dropped.
    assert!(s.apply_patch(
        &serde_json::json!({"shield": {"site_off": {"WWW.A.example": ["popups", "x"]}}})
    ));
    assert_eq!(
        s.shield.site_off.get("a.example"),
        Some(&vec!["popups".to_string()])
    );
    assert!(!s.shield_on("a.example", "popups"));
    s.shield.reject_cookie_notices = false;
    assert!(!s.shield_on("b.example", "cookie_notices"));
}

#[test]
fn forget_on_close_and_keep_list() {
    let mut s = Settings::default();
    s.set_forget_on_close("example.com", true);
    assert!(s.forgets_on_close("https://www.example.com/a"));
    assert!(s.forgets_on_close("mail.example.com"));
    assert!(!s.forgets_on_close("notexample.com"));
    s.set_forget_on_close("mail.example.com", false);
    assert!(!s.forgets_on_close("example.com"));
    s.privacy.persistent_sites = vec!["*.github.com".into()];
    assert!(s.keeps_site("gist.github.com"));
    assert!(s.keeps_site("github.com"));
    assert!(!s.keeps_site("example.com"));
    assert!(!s.privacy.clear_on_exit);
    assert!(s.permissions.quiet_notifications);
}
