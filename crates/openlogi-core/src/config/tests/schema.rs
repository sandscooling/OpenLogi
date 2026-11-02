//! Schema versions and the strictness of the current one.

use std::collections::BTreeSet;

use super::*;

#[test]
fn rejects_next_schema_version_but_accepts_v1() {
    // A future version is rejected loudly; the current and older versions
    // load (older ones migrate through the shim).
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    let future = SCHEMA_VERSION + 1;
    fs::write(&path, format!("schema_version = {future}\n")).expect("write");
    let error = Config::load_from_path(&path).expect_err("next schema should fail");
    assert_matches!(
        error,
        ConfigError::UnsupportedSchemaVersion { found, .. } if found == future
    );

    fs::write(&path, "schema_version = 1\n").expect("write");
    assert!(
        Config::load_from_path(&path).is_ok(),
        "v1 should still load"
    );
}

#[test]
fn future_version_is_rejected_before_incompatible_fields_are_parsed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        "schema_version = 99\n[app_settings]\nthumbwheel_sensitivity = \"future\"\n",
    )
    .expect("write");
    assert_matches!(
        Config::load_from_path(&path).expect_err("future schema must fail by version"),
        ConfigError::UnsupportedSchemaVersion { found: 99, .. }
    );
}

#[test]
fn v9_without_disabled_keys_loads_and_a_save_stamps_the_current_schema() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        "schema_version = 9\n[devices.keyboard]\nfn_lock = true\n",
    )
    .expect("write v9 config");

    let (mut config, mut file) = ConfigFile::load_from_path(&path).expect("load v9 config");
    assert_eq!(config.schema_version, SCHEMA_VERSION);
    assert_eq!(config.disabled_keys("keyboard"), None);

    let disabled_keys = BTreeSet::from([DisableKey::CapsLock, DisableKey::WindowsCommand]);
    config.set_disabled_keys("keyboard", disabled_keys.clone());
    file.save(&config).expect("save v10 config");

    let saved = fs::read_to_string(&path).expect("read saved config");
    assert!(
        saved.starts_with(&format!("schema_version = {SCHEMA_VERSION}\n")),
        "saved header: {saved}"
    );
    let reloaded = Config::load_from_path(&path).expect("reload v10 config");
    assert_eq!(reloaded.disabled_keys("keyboard"), Some(&disabled_keys));
}

#[test]
fn v10_without_click_lock_loads_and_a_save_stamps_the_current_schema() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        "schema_version = 10\n[app_settings]\nsmooth_scroll = true\n",
    )
    .expect("write v10 config");

    let (mut config, mut file) = ConfigFile::load_from_path(&path).expect("load v10 config");
    assert_eq!(config.schema_version, SCHEMA_VERSION);
    assert_eq!(config.app_settings.click_lock, ClickLock::default());
    assert_eq!(config.flow, FlowConfig::default());

    config.app_settings.click_lock.enabled = true;
    file.save(&config).expect("save migrated config");

    let saved = fs::read_to_string(&path).expect("read saved config");
    assert!(
        saved.starts_with(&format!("schema_version = {SCHEMA_VERSION}\n")),
        "saved: {saved}"
    );
    assert!(
        saved.contains("[app_settings.click_lock]"),
        "saved: {saved}"
    );
    let reloaded = Config::load_from_path(&path).expect("reload migrated config");
    assert!(reloaded.app_settings.click_lock.enabled);
    assert!(reloaded.app_settings.smooth_scroll);
}

/// The live config this fork's v12 builds wrote: click lock and chords stay as
/// they were, and the Flow step, numbered v13 here, adds only its defaults.
#[test]
fn v12_with_click_lock_and_chords_keeps_them_and_gets_the_flow_defaults() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        "schema_version = 12\n\
         [app_settings.click_lock]\n\
         enabled = true\n\
         hold_ms = 1500\n\n\
         [app_settings.chords]\n\
         window_ms = 80\n\n\
         [devices.mouse.chords]\n\
         \"Back+Forward\" = \"Copy\"\n\n\
         [devices.mouse.per_app_chords.app]\n\
         \"Back+Forward\" = \"Paste\"\n",
    )
    .expect("write v12 config");
    let chord: crate::binding::ChordId = "Back+Forward".parse().expect("chord");

    let (config, mut file) = ConfigFile::load_from_path(&path).expect("load v12 config");
    let check = |config: &Config| {
        assert_eq!(config.schema_version, SCHEMA_VERSION);
        assert!(config.app_settings.click_lock.enabled);
        assert_eq!(u16::from(config.app_settings.click_lock.hold_ms), 1500);
        assert_eq!(u16::from(config.app_settings.chords.window_ms), 80);
        let device = &config.devices["mouse"];
        assert_eq!(device.chords.get(&chord), Some(&Action::Copy));
        assert_eq!(
            device.per_app_chords["app"].get(&chord),
            Some(&Action::Paste)
        );
        assert_eq!(config.flow, FlowConfig::default());
    };
    check(&config);

    file.save(&config).expect("save v13 config");
    let saved = fs::read_to_string(&path).expect("read saved config");
    assert!(
        saved.starts_with(&format!("schema_version = {SCHEMA_VERSION}\n")),
        "saved: {saved}"
    );
    check(&Config::load_from_path(&path).expect("reload v13 config"));
}

/// Upstream numbers its Flow step v11, this fork's click lock. Its file carries
/// `[flow]` and nothing of click lock, and every field parses at any supported
/// version, so it loads with Flow intact and click lock off.
#[test]
fn a_v11_file_from_an_upstream_build_keeps_its_flow() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        "schema_version = 11\n\
         [flow]\n\
         enabled = true\n\
         require_modifier = true\n",
    )
    .expect("write upstream v11 config");

    let config = Config::load_from_path(&path).expect("load upstream v11 config");
    assert_eq!(config.schema_version, SCHEMA_VERSION);
    assert!(config.flow.enabled);
    assert!(config.flow.require_modifier);
    assert_eq!(config.app_settings.click_lock, ClickLock::default());
}

#[test]
fn schema_version_zero_is_rejected() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("config.toml");
    fs::write(&path, "schema_version = 0\n").expect("write config");

    assert!(matches!(
        Config::load_from_path(&path),
        Err(ConfigError::UnsupportedSchemaVersion { found: 0, .. })
    ));
}

#[test]
fn current_schema_rejects_unknown_and_obsolete_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        "schema_version = 6\n[app_settings]\nthumbwheel_sensitivty = 14\n",
    )
    .expect("write typo");
    assert_matches!(
        Config::load_from_path(&path).expect_err("typo must fail"),
        ConfigError::Parse { .. }
    );

    fs::write(
        &path,
        r#"schema_version = 6
[devices.mouse.identity]
display_name = "Mouse"
kind = "mouse"
capabilities = { buttons = true, pointer = true, lighting = false, scroll_inversion = false, typo = true }
"#,
    )
    .expect("write nested typo");
    assert_matches!(
        Config::load_from_path(&path).expect_err("nested typo must fail"),
        ConfigError::Parse { .. }
    );

    fs::write(
        &path,
        "schema_version = 11\n[app_settings.click_lock]\nhold = 1000\n",
    )
    .expect("write click lock typo");
    assert_matches!(
        Config::load_from_path(&path).expect_err("click lock typo must fail"),
        ConfigError::Parse { .. }
    );

    fs::write(
        &path,
        "schema_version = 6\n[devices.mouse]\ngesture_owner = \"Off\"\n",
    )
    .expect("write obsolete field");
    assert_matches!(
        Config::load_from_path(&path).expect_err("current-schema legacy field must fail"),
        ConfigError::ObsoleteField { .. }
    );
}

#[test]
fn persisted_numeric_contracts_reject_unsafe_values() {
    for body in [
        "schema_version = 6\n[app_settings]\nthumbwheel_sensitivity = 0\n",
        "schema_version = 6\n[app_settings]\nthumbwheel_sensitivity = 101\n",
        "schema_version = 6\n[app_settings]\nthumbwheel_sensitivity = -2147483648\n",
        "schema_version = 6\n[app_settings]\nvertical_scroll_sensitivity = 0\n",
        "schema_version = 6\n[app_settings]\nvertical_scroll_sensitivity = 101\n",
        "schema_version = 6\n[app_settings]\nvertical_scroll_sensitivity = -2147483648\n",
        "schema_version = 11\n[app_settings.click_lock]\nhold_ms = 299\n",
        "schema_version = 11\n[app_settings.click_lock]\nhold_ms = 3001\n",
        "schema_version = 11\n[app_settings.click_lock]\nhold_ms = -1\n",
        "schema_version = 11\n[app_settings.click_lock]\nhold_ms = 65536\n",
        "schema_version = 6\n[devices.mouse]\nthumbwheel_sensitivity = -1\n",
        "schema_version = 6\n[devices.mouse]\ndpi = 65536\n",
        "schema_version = 6\n[devices.mouse]\ndpi_presets = [800, 70000]\n",
    ] {
        assert!(toml::from_str::<Config>(body).is_err(), "accepted: {body}");
    }
}
