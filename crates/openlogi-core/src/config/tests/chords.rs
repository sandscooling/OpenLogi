//! Two-button chords in the config: their tables, overlays and setting.

use std::collections::BTreeMap;
#[cfg(target_os = "linux")]
use std::collections::BTreeSet;
use std::time::Duration;

use super::*;
use crate::binding::ChordId;
use crate::bindings::{chord_members_for, chords_for};

fn parse(body: &str) -> Result<Config, toml::de::Error> {
    toml::from_str(&format!("schema_version = {SCHEMA_VERSION}\n{body}"))
}

fn chord(name: &str) -> ChordId {
    name.parse().expect("valid chord")
}

const CHORDS: &str = r#"
[devices.mouse.chords]
"Back+Forward" = "Copy"
"GestureButton+HapticPanel" = { CustomShortcut = "Super+Tab" }

[devices.mouse.per_app_chords."org.mozilla.firefox"]
"Back+Forward" = "None"

[devices.mouse.per_app_chords."com.example.Editor"]
"Back+Forward" = "Paste"
"#;

#[test]
fn a_v11_file_loads_without_chords_and_a_save_stamps_the_current_version() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        "schema_version = 11\n[devices.mouse.bindings]\nBack = \"Copy\"\n",
    )
    .expect("write v11 config");

    let (mut config, mut file) = ConfigFile::load_from_path(&path).expect("load v11 config");
    assert_eq!(config.schema_version, SCHEMA_VERSION);
    assert_eq!(config.effective_chords("mouse", None), BTreeMap::new());
    assert_eq!(config.app_settings.chords, ChordSettings::default());

    config.set_selected_device(Some("mouse".into()));
    file.save(&config).expect("save");
    let saved = fs::read_to_string(&path).expect("read saved config");
    assert!(
        saved.starts_with(&format!("schema_version = {SCHEMA_VERSION}\n")),
        "saved: {saved}"
    );
    assert!(!saved.contains("chords"), "nothing to write: {saved}");
}

#[test]
fn chord_tables_round_trip_through_the_file() {
    let config = parse(CHORDS).expect("parse");
    let reloaded = write_and_read(&config);
    let device = &reloaded.devices["mouse"];
    assert_eq!(device.chords, config.devices["mouse"].chords);
    assert_eq!(
        device.per_app_chords,
        config.devices["mouse"].per_app_chords
    );
    assert_eq!(
        device.chords.get(&chord("Forward+Back")),
        Some(&Action::Copy),
        "a chord is one pair in either order"
    );
}

#[test]
fn a_key_naming_an_ineligible_button_fails_to_parse() {
    for key in [
        "LeftClick+Back",
        "Back+RightClick",
        "Thumbwheel+Back",
        "ThumbwheelScrollUp+Back",
        "Back+Back",
        "Back",
        "Back+Forward+MiddleClick",
    ] {
        let body = format!("[devices.mouse.chords]\n\"{key}\" = \"Copy\"\n");
        assert!(parse(&body).is_err(), "{key} must be rejected");
        let overlay = format!("[devices.mouse.per_app_chords.app]\n\"{key}\" = \"Copy\"\n");
        assert!(parse(&overlay).is_err(), "{key} must be rejected per app");
    }
}

#[test]
fn one_chord_under_both_spellings_is_an_error_not_a_merge() {
    let error = parse(
        "[devices.mouse.chords]\n\"Back+Forward\" = \"Copy\"\n\"Forward+Back\" = \"Paste\"\n",
    )
    .expect_err("one chord named twice");
    assert!(error.to_string().contains("name the same chord"), "{error}");
}

#[test]
fn an_overlay_replaces_or_removes_a_chord_for_its_application() {
    let config = parse(CHORDS).expect("parse");
    let back_forward = chord("Back+Forward");

    let global = config.effective_chords("mouse", None);
    assert_eq!(global.get(&back_forward), Some(&Action::Copy));
    assert_eq!(global.len(), 2);

    let edited = config.effective_chords("mouse", Some("com.example.Editor"));
    assert_eq!(edited.get(&back_forward), Some(&Action::Paste));
    assert_eq!(edited.len(), 2, "other chords fall through");

    let off = config.effective_chords("mouse", Some("org.mozilla.firefox"));
    assert_eq!(off.get(&back_forward), None, "`None` removes the chord");
    assert_eq!(off.len(), 1);
}

#[test]
fn a_chord_bound_to_nothing_is_no_chord() {
    let config = parse("[devices.mouse.chords]\n\"Back+Forward\" = \"None\"\n").expect("parse");
    assert_eq!(config.effective_chords("mouse", None), BTreeMap::new());
}

#[cfg(target_os = "linux")]
#[test]
fn chords_for_is_the_effective_map_and_empty_without_a_device() {
    let config = parse(CHORDS).expect("parse");
    assert_eq!(
        chords_for(&config, Some("mouse"), Some("org.mozilla.firefox")),
        config.effective_chords("mouse", Some("org.mozilla.firefox"))
    );
    assert_eq!(chords_for(&config, None, None), BTreeMap::new());
    assert_eq!(chords_for(&config, Some("other"), None), BTreeMap::new());
}

#[cfg(target_os = "linux")]
#[test]
fn members_are_the_union_across_profiles_and_ignore_a_chord_no_profile_runs() {
    let config = parse(
        r#"
[devices.mouse.chords]
"Back+Forward" = "Copy"
"GestureButton+HapticPanel" = "None"

[devices.mouse.per_app_chords.app]
"Back+Forward" = "None"
"DpiToggle+MiddleClick" = "Paste"
"#,
    )
    .expect("parse");
    assert_eq!(
        chord_members_for(&config, "mouse"),
        BTreeSet::from([
            ButtonId::Back,
            ButtonId::Forward,
            ButtonId::DpiToggle,
            ButtonId::MiddleClick,
        ])
    );
    assert_eq!(chord_members_for(&config, "other"), BTreeSet::new());
}

#[cfg(target_os = "linux")]
#[test]
fn with_the_hook_opted_out_a_chord_naming_an_os_hook_button_cannot_run() {
    let mut config = parse(
        r#"
[devices.mouse.chords]
"Back+GestureButton" = "Copy"
"GestureButton+HapticPanel" = "Paste"
"#,
    )
    .expect("parse");
    config.app_settings.capture_mouse_events = false;
    assert_eq!(
        chords_for(&config, Some("mouse"), None),
        BTreeMap::from([(chord("GestureButton+HapticPanel"), Action::Paste)])
    );
    assert_eq!(
        chord_members_for(&config, "mouse"),
        BTreeSet::from([ButtonId::GestureButton, ButtonId::HapticPanel]),
        "Back is left native, so nothing is diverted for its chord"
    );
}

#[test]
fn only_linux_runs_chords() {
    let config = parse(CHORDS).expect("parse");
    assert_eq!(
        chords_for(&config, Some("mouse"), None).is_empty(),
        !chords_supported()
    );
    assert_eq!(
        chord_members_for(&config, "mouse").is_empty(),
        !chords_supported()
    );
}

#[test]
fn the_chord_window_defaults_and_clamps() {
    let window = |ms: u16| ChordWindow::from(ms).duration();
    assert_eq!(
        ChordSettings::default().window_ms.duration(),
        Duration::from_millis(50)
    );
    assert_eq!(window(5), Duration::from_millis(20));
    assert_eq!(window(20), Duration::from_millis(20));
    assert_eq!(window(150), Duration::from_millis(150));
    assert_eq!(window(900), Duration::from_millis(150));

    let loaded = parse("[app_settings.chords]\nwindow_ms = 900\n").expect("parse");
    assert_eq!(loaded.app_settings.chords.window_ms, ChordWindow::from(150));
    let loaded = parse("[app_settings.chords]\n").expect("parse");
    assert_eq!(loaded.app_settings.chords, ChordSettings::default());
}

#[test]
fn a_window_too_far_out_to_be_a_u16_still_clamps_on_load() {
    for (written, clamped) in [("-1", 20), ("0", 20), ("65536", 150), ("99999999", 150)] {
        let body = format!("[app_settings.chords]\nwindow_ms = {written}\n");
        let loaded = parse(&body).expect("a window is never a load error");
        assert_eq!(
            loaded.app_settings.chords.window_ms,
            ChordWindow::from(clamped),
            "{written}"
        );
    }
}

#[test]
fn an_untouched_window_stays_out_of_the_file() {
    let saved = toml::to_string(&Config::default()).expect("serialize");
    assert!(!saved.contains("chords"), "saved: {saved}");

    let mut config = Config::default();
    config.app_settings.chords.window_ms = ChordWindow::from(80);
    let saved = toml::to_string(&config).expect("serialize");
    assert!(saved.contains("window_ms = 80"), "saved: {saved}");
}

#[test]
fn a_chord_counts_as_a_setting_and_survives_key_adoption() {
    let config = parse(CHORDS).expect("parse");
    assert!(config.devices["mouse"].holds_settings());
}

#[test]
fn a_per_app_chord_profile_naming_no_alias_fails_to_load() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        format!(
            "schema_version = {SCHEMA_VERSION}\n\
             [devices.mouse.per_app_chords.\"app:nope\"]\n\"Back+Forward\" = \"Copy\"\n"
        ),
    )
    .expect("write");
    assert_matches!(
        Config::load_from_path(&path).expect_err("an undefined alias must fail"),
        ConfigError::UndefinedAppAlias { .. }
    );
}
