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

/// Saves patch the user's file rather than replace it, and a key is matched by
/// its text. A chord spelled by hand must come out as one key, or the next load
/// would refuse the file for naming it twice. An action also changes shape on
/// the way: a table for a shortcut, a string for the rest.
#[test]
fn edits_patch_a_hand_spelled_chord_into_one_key_that_still_loads() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    fs::write(
        &path,
        format!(
            "schema_version = {SCHEMA_VERSION}\n\n\
             [devices.mouse.chords]\n\
             \"Forward+Back\" = \"Copy\"\n\
             \"Back+MiddleClick\" = {{ CustomShortcut = \"Super+Tab\" }}\n\n\
             [devices.mouse.per_app_chords.app]\n\
             \"Forward+Back\" = \"None\"\n"
        ),
    )
    .expect("write config");
    let id = chord("Back+Forward");
    let tab = chord("Back+MiddleClick");
    let shortcut = Action::CustomShortcut("Super+Tab".parse().expect("shortcut"));

    let (mut config, mut file) = ConfigFile::load_from_path(&path).expect("load hand spelling");
    config.set_chord("mouse", id, Some(shortcut.clone()));
    config.set_chord("mouse", tab, Some(Action::Paste));
    config.set_per_app_chord("mouse", "app", id, Some(Action::Paste));
    file.save(&config).expect("save");

    let saved = fs::read_to_string(&path).expect("read saved config");
    assert!(!saved.contains("Forward+Back"), "saved: {saved}");
    assert!(!saved.contains("Back+MiddleClick"), "saved: {saved}");
    let (mut reloaded, mut file) = ConfigFile::load_from_path(&path).expect("reload");
    assert_eq!(
        reloaded.devices["mouse"].chords,
        config.devices["mouse"].chords
    );
    assert_eq!(
        reloaded.devices["mouse"].per_app_chords,
        config.devices["mouse"].per_app_chords
    );

    reloaded.set_chord("mouse", id, Some(Action::Copy));
    reloaded.set_per_app_chord("mouse", "app", id, None);
    reloaded.set_chord("mouse", tab, None);
    reloaded.set_chord("mouse", id, None);
    file.save(&reloaded).expect("save the removals");
    let saved = fs::read_to_string(&path).expect("read saved config");
    assert!(!saved.contains("chords"), "nothing left to write: {saved}");
    Config::load_from_path(&path).expect("load the emptied file");
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
fn a_floating_chord_window_snaps_to_a_step_inside_the_range() {
    for (raw, expected) in [
        (52.0, 50),
        (53.0, 55),
        (19.0, 20),
        (151.0, 150),
        (f32::NAN, 20),
        (f32::NEG_INFINITY, 20),
        (f32::INFINITY, 150),
    ] {
        assert_eq!(
            ChordWindow::from_rounded(raw),
            ChordWindow::from(expected),
            "{raw}"
        );
    }
    assert_eq!(
        ChordWindow::from_rounded(f32::from(ChordWindow::DEFAULT)),
        ChordWindow::DEFAULT
    );
}

#[test]
fn setting_chords_creates_the_entries_and_clearing_prunes_them() {
    let id = chord("Back+Forward");
    let mut config = Config::default();
    config.set_chord("mouse", id, Some(Action::Copy));
    config.set_per_app_chord("mouse", "app", id, Some(Action::Paste));
    assert_eq!(config.devices["mouse"].chords[&id], Action::Copy);
    assert_eq!(
        config.devices["mouse"].per_app_chords["app"][&id],
        Action::Paste
    );

    config.set_per_app_chord("mouse", "app", id, None);
    assert!(
        config.devices["mouse"].per_app_chords.is_empty(),
        "the empty application map is pruned"
    );
    config.set_chord("mouse", id, None);
    assert!(config.devices["mouse"].chords.is_empty());
}

#[test]
fn removing_a_chord_drops_only_the_app_entries_that_turned_it_off() {
    let other = chord("DpiToggle+GestureButton");
    let mut config = parse(CHORDS).expect("parse");
    config.set_per_app_chord("mouse", "org.mozilla.firefox", other, Some(Action::Copy));
    config.set_per_app_chord(
        "mouse",
        "com.example.Off",
        chord("Back+Forward"),
        Some(Action::None),
    );

    config.set_chord("mouse", chord("Back+Forward"), None);

    let overlays = &config.devices["mouse"].per_app_chords;
    assert!(
        !overlays.contains_key("com.example.Off"),
        "an application that only turned it off has nothing left"
    );
    assert_eq!(
        overlays["org.mozilla.firefox"],
        BTreeMap::from([(other, Action::Copy)]),
        "another chord of that application stays"
    );
    assert_eq!(
        overlays["com.example.Editor"],
        BTreeMap::from([(chord("Back+Forward"), Action::Paste)]),
        "an application's own action is still a chord there"
    );
}

/// A profile that holds only chords is a profile wherever chords run, and
/// elsewhere is left alone instead of showing as an empty one.
#[test]
fn a_chord_only_profile_is_a_profile_where_chords_run() {
    let app = "com.example.Editor";
    let mut config = parse(CHORDS).expect("parse");
    config.set_per_app_binding(
        "mouse",
        "com.example.Both",
        ButtonId::Back,
        Some(Action::Copy),
    );
    config.set_per_app_chord(
        "mouse",
        "com.example.Both",
        chord("Back+Forward"),
        Some(Action::Paste),
    );

    assert_eq!(config.has_app_override("mouse", app), chords_supported());
    assert_eq!(
        config.per_app_chord_overrides("mouse", app).is_some(),
        chords_supported()
    );
    let expected: &[&str] = if chords_supported() {
        &["com.example.Both", app, "org.mozilla.firefox"]
    } else {
        &["com.example.Both"]
    };
    assert_eq!(config.app_profiles("mouse").collect::<Vec<_>>(), expected);

    assert_eq!(config.remove_app_profile("mouse", app), chords_supported());
    assert_eq!(
        config.devices["mouse"].per_app_chords.contains_key(app),
        !chords_supported()
    );
    assert!(config.remove_app_profile("mouse", "com.example.Both"));
    assert_eq!(
        config.devices["mouse"]
            .per_app_chords
            .contains_key("com.example.Both"),
        !chords_supported()
    );
    assert!(!config.remove_app_profile("mouse", "com.example.Absent"));
    assert!(!config.remove_app_profile("absent", app));
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
