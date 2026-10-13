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
    assert_eq!(settings.performance.sleep_after_secs, 300);
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
    // Under normal user runs or developer testing, this process is not elevated.
    let elevated = is_process_elevated();
    assert!(
        !elevated,
        "Process expected to run unelevated in standard environment"
    );
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
    use shoji_core::env::delayed_purge_script;
    use std::path::PathBuf;

    let dirs = [
        PathBuf::from(r"C:\Users\a\AppData\Roaming\shoji"),
        PathBuf::from(r"C:\Users\%USERNAME%\evil"),
        PathBuf::from(r#"C:\Users\a\x" & calc"#),
        PathBuf::from(r"C:\"),
    ];
    let files = [PathBuf::from(r"C:\Users\a\Desktop\Shoji.lnk")];
    let script = delayed_purge_script(&dirs, &files).unwrap();
    assert!(script.starts_with("ping -n 4 127.0.0.1 >nul"));
    assert!(script.contains(r#"rmdir /s /q "C:\Users\a\AppData\Roaming\shoji""#));
    assert!(script.contains(r#"del /f /q "C:\Users\a\Desktop\Shoji.lnk""#));
    assert!(!script.contains('%') && !script.contains("calc"));
    assert_eq!(script.matches("rmdir").count(), 1);

    assert!(delayed_purge_script(&[PathBuf::from(r"C:\")], &[]).is_none());
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
    assert_eq!(s.performance.sleep_after_secs, 300);

    // Settings files written before the option existed load with it on.
    let old: Settings =
        serde_json::from_str(r#"{ "performance": { "sleep_after_secs": 60 } }"#).unwrap();
    assert!(old.performance.run_in_background);
}
