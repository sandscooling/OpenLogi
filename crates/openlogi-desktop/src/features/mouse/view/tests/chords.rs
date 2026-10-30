//! The Chords section, driven through real clicks on the mouse view. Linux
//! only: nowhere else shows it, which `tests.rs` proves.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{Render, TestAppContext, Window, WindowOptions, div};
use openlogi_core::config::UiScale;

use super::*;
use crate::features::profiles::{AppCatalogPicker, ProfileIconCache, button_profile_scope_bar};
use crate::state::tests::{KNOWN_MOUSE_KEY, direct_inventory};
use crate::state::{ChordStanding, ConfigPersistence};

const SAFARI: &str = "com.apple.Safari";

fn chord(name: &str) -> ChordId {
    name.parse().expect("valid chord")
}

/// The selected button's Chords section, on the known mouse over `config`.
/// Tall enough for the whole action library to be clickable. Callers hold
/// `LOCALE_LOCK` in English, for the reason [`wheel_hold_view`] gives.
fn chord_view(
    cx: &mut TestAppContext,
    config: Config,
    persistence: ConfigPersistence,
    button: ButtonId,
) -> (Entity<MouseModelView>, &mut VisualTestContext) {
    cx.update(gpui_component::init);
    cx.update(|cx| {
        let resolver = AssetResolver::new();
        let (commands, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let state = cx.new(|_| {
            AppState::new(Sources {
                inventories: &[direct_inventory([0xa3, 0x93, 0xca, 0xe0])],
                persistence,
                ..Sources::in_memory(config, &resolver, commands)
            })
        });
        AppState::set_global(state, cx);
    });
    let (view, cx) = cx.add_window_view(MouseModelView::new);
    cx.simulate_resize(size(px(1200.), px(3000.)));
    view.update(cx, |view, _| view.select(MouseControlId::Button(button)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (view, cx)
}

fn memory_view(
    cx: &mut TestAppContext,
    config: Config,
    button: ButtonId,
) -> (Entity<MouseModelView>, &mut VisualTestContext) {
    chord_view(cx, config, ConfigPersistence::MemoryOnly, button)
}

fn close(view: Entity<MouseModelView>, cx: &mut VisualTestContext) {
    drop(view);
    cx.update(|window, _| window.remove_window());
    cx.run_until_parked();
}

fn select(view: &Entity<MouseModelView>, cx: &mut VisualTestContext, button: ButtonId) {
    view.update(cx, |view, _| view.select(MouseControlId::Button(button)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn open_app(cx: &mut VisualTestContext, app: Option<&str>) {
    let app = app.map(str::to_string);
    cx.update(|window, cx| {
        AppState::apply(cx, |state| state.set_editing_app(app));
        window.draw(cx).clear(cx);
    });
}

/// The open profile's chords as `(id, action, standing)`.
fn chords(cx: &mut VisualTestContext) -> Vec<(ChordId, Action, ChordStanding)> {
    cx.update(|_, cx| {
        AppState::try_read(cx)
            .expect("state is installed")
            .chord_entries()
            .expect("this device and platform edit chords")
            .into_iter()
            .map(|entry| (entry.id, entry.action, entry.standing))
            .collect()
    })
}

fn with_chord(id: ChordId, action: Action) -> Config {
    let mut config = Config::ephemeral();
    config.set_chord(KNOWN_MOUSE_KEY, id, Some(action));
    config
}

#[gpui::test]
fn adding_a_chord_saves_it_once_an_action_is_picked(cx: &mut TestAppContext) {
    let _locale = LOCALE_LOCK.lock().unwrap();
    rust_i18n::set_locale("en");
    let (view, cx) = memory_view(cx, Config::ephemeral(), ButtonId::Back);

    click_picker_control(cx, "inspector-add-chord");
    assert!(cx.debug_bounds("inspector-chord-partner-Forward").is_some());
    assert!(
        cx.debug_bounds("inspector-chord-partner-Back").is_none(),
        "a button cannot pair with itself"
    );
    assert!(
        cx.debug_bounds("inspector-chord-partner-HapticPanel")
            .is_none(),
        "the partner list holds only this device's controls"
    );

    click_picker_control(cx, "inspector-chord-partner-Forward");
    assert!(chords(cx).is_empty(), "a partner alone saves nothing");
    assert_eq!(
        view.read_with(cx, |view, _| view.picker),
        InspectorPicker::ChordAction(chord("Back+Forward"))
    );

    click_picker_control(cx, "inspector-chord-action-Copy");
    assert_eq!(
        chords(cx),
        [(chord("Back+Forward"), Action::Copy, ChordStanding::Default)]
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.picker),
        InspectorPicker::Closed
    );
    assert!(cx.debug_bounds("inspector-chord-row-Forward").is_some());
    close(view, cx);
}

#[gpui::test]
fn a_chord_can_run_a_custom_shortcut(cx: &mut TestAppContext) {
    let _locale = LOCALE_LOCK.lock().unwrap();
    rust_i18n::set_locale("en");
    let (view, cx) = memory_view(cx, Config::ephemeral(), ButtonId::DpiToggle);

    click_picker_control(cx, "inspector-add-chord");
    click_picker_control(cx, "inspector-chord-partner-Back");
    click_picker_control(cx, "inspector-chord-action-custom-shortcut-input");
    cx.simulate_input("Cmd+K");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    click_picker_control(cx, "inspector-chord-action-custom-shortcut-add");

    assert_eq!(
        chords(cx),
        [(
            chord("Back+DpiToggle"),
            Action::CustomShortcut("Cmd+K".parse().unwrap()),
            ChordStanding::Default
        )]
    );
    close(view, cx);
}

/// A chord is one entry: whichever member changed it, both read the result, and
/// a button already paired is not offered again from either side.
#[gpui::test]
fn either_member_edits_the_same_chord(cx: &mut TestAppContext) {
    let _locale = LOCALE_LOCK.lock().unwrap();
    rust_i18n::set_locale("en");
    let id = chord("Back+Forward");
    let (view, cx) = memory_view(cx, with_chord(id, Action::Copy), ButtonId::Forward);

    click_picker_control(cx, "inspector-add-chord");
    assert!(
        cx.debug_bounds("inspector-chord-partner-Back").is_none(),
        "the pair already exists"
    );
    assert!(
        cx.debug_bounds("inspector-chord-partner-MiddleClick")
            .is_some()
    );
    click_picker_control(cx, "inspector-add-chord");

    click_picker_control(cx, "inspector-chord-row-Back");
    assert!(
        cx.debug_bounds("inspector-chord-action-Copy-check")
            .is_some(),
        "the picker opens on the chord's action"
    );
    click_picker_control(cx, "inspector-chord-action-Paste");
    assert_eq!(chords(cx), [(id, Action::Paste, ChordStanding::Default)]);

    select(&view, cx, ButtonId::Back);
    click_picker_control(cx, "inspector-chord-row-Forward");
    assert!(
        cx.debug_bounds("inspector-chord-action-Paste-check")
            .is_some(),
        "the other member shows the same action"
    );
    click_picker_control(cx, "inspector-chord-action-Undo");
    assert_eq!(chords(cx), [(id, Action::Undo, ChordStanding::Default)]);

    select(&view, cx, ButtonId::MiddleClick);
    assert!(
        cx.debug_bounds("inspector-chord-row-Forward").is_none(),
        "a button outside the chord does not list it"
    );
    close(view, cx);
}

#[gpui::test]
fn a_chord_can_be_removed_from_the_lower_member(cx: &mut TestAppContext) {
    let _locale = LOCALE_LOCK.lock().unwrap();
    rust_i18n::set_locale("en");
    let id = chord("Back+Forward");
    let (view, cx) = memory_view(cx, with_chord(id, Action::Copy), ButtonId::Back);

    click_picker_control(cx, "inspector-chord-row-Forward");
    click_picker_control(cx, "inspector-chord-remove-Forward");
    assert!(chords(cx).is_empty());
    assert!(cx.debug_bounds("inspector-chord-row-Forward").is_none());
    assert_eq!(
        view.read_with(cx, |view, _| view.picker),
        InspectorPicker::Closed,
        "removing the chord being edited closes its picker"
    );
    close(view, cx);
}

#[gpui::test]
fn a_chord_can_be_removed_from_the_higher_member(cx: &mut TestAppContext) {
    let _locale = LOCALE_LOCK.lock().unwrap();
    rust_i18n::set_locale("en");
    let id = chord("Back+Forward");
    let (view, cx) = memory_view(cx, with_chord(id, Action::Copy), ButtonId::Forward);
    click_picker_control(cx, "inspector-chord-remove-Back");
    assert!(chords(cx).is_empty());
    close(view, cx);
}

/// Adding in an app profile takes the same clicks as adding globally, and
/// stays in that profile.
#[gpui::test]
fn a_chord_added_in_an_app_profile_stays_in_it(cx: &mut TestAppContext) {
    let _locale = LOCALE_LOCK.lock().unwrap();
    rust_i18n::set_locale("en");
    let id = chord("Back+Forward");
    let (view, cx) = memory_view(cx, Config::ephemeral(), ButtonId::Back);
    open_app(cx, Some(SAFARI));

    click_picker_control(cx, "inspector-add-chord");
    click_picker_control(cx, "inspector-chord-partner-Forward");
    click_picker_control(cx, "inspector-chord-action-Copy");
    assert_eq!(chords(cx), [(id, Action::Copy, ChordStanding::AppOnly)]);

    open_app(cx, Some("com.google.Chrome"));
    assert!(
        chords(cx).is_empty(),
        "another application does not have it"
    );
    open_app(cx, None);
    assert!(chords(cx).is_empty(), "nor does the Default profile");

    open_app(cx, Some(SAFARI));
    click_picker_control(cx, "inspector-chord-remove-Forward");
    assert!(chords(cx).is_empty());
    close(view, cx);
}

/// The same states buttons have: a Default chord is inherited until the app
/// overrides it, and using the default brings it back.
#[gpui::test]
fn an_app_profile_overrides_and_restores_a_default_chord(cx: &mut TestAppContext) {
    let _locale = LOCALE_LOCK.lock().unwrap();
    rust_i18n::set_locale("en");
    let id = chord("Back+Forward");
    let (view, cx) = memory_view(cx, with_chord(id, Action::Copy), ButtonId::Back);
    open_app(cx, Some(SAFARI));
    assert_eq!(chords(cx), [(id, Action::Copy, ChordStanding::Inherited)]);

    click_picker_control(cx, "inspector-chord-row-Forward");
    click_picker_control(cx, "inspector-chord-action-Paste");
    assert_eq!(chords(cx), [(id, Action::Paste, ChordStanding::Overridden)]);

    open_app(cx, None);
    assert_eq!(
        chords(cx),
        [(id, Action::Copy, ChordStanding::Default)],
        "the override changes only that application"
    );

    open_app(cx, Some(SAFARI));
    click_picker_control(cx, "inspector-chord-use-default-Forward");
    assert_eq!(chords(cx), [(id, Action::Copy, ChordStanding::Inherited)]);
    close(view, cx);
}

/// `None` removes a Default chord for one application, both through the row's
/// switch-off and by picking Do Nothing, and undoing it inherits again.
#[gpui::test]
fn a_none_override_turns_a_default_chord_off_in_one_app(cx: &mut TestAppContext) {
    let _locale = LOCALE_LOCK.lock().unwrap();
    rust_i18n::set_locale("en");
    let id = chord("Back+Forward");
    let (view, cx) = memory_view(cx, with_chord(id, Action::Copy), ButtonId::Forward);
    open_app(cx, Some(SAFARI));
    assert!(
        cx.debug_bounds("inspector-chord-remove-Back").is_none(),
        "an application cannot remove a Default chord, only turn it off"
    );

    click_picker_control(cx, "inspector-chord-turn-off-Back");
    assert_eq!(chords(cx), [(id, Action::Copy, ChordStanding::TurnedOff)]);

    click_picker_control(cx, "inspector-chord-use-default-Back");
    assert_eq!(chords(cx), [(id, Action::Copy, ChordStanding::Inherited)]);

    click_picker_control(cx, "inspector-chord-row-Back");
    click_picker_control(cx, "inspector-chord-action-None");
    assert_eq!(chords(cx), [(id, Action::Copy, ChordStanding::TurnedOff)]);
    close(view, cx);
}

/// Gesture buttons have their own inspectors, in both profiles, and two of the
/// four pairs this was built for are DPI and Gesture.
#[gpui::test]
fn a_gesture_button_edits_chords_like_any_other(cx: &mut TestAppContext) {
    let _locale = LOCALE_LOCK.lock().unwrap();
    rust_i18n::set_locale("en");
    let id = chord("DpiToggle+GestureButton");
    let (view, cx) = memory_view(cx, Config::ephemeral(), ButtonId::GestureButton);
    assert!(in_gesture_mode(cx, ButtonId::GestureButton));

    click_picker_control(cx, "inspector-add-chord");
    click_picker_control(cx, "inspector-chord-partner-DpiToggle");
    click_picker_control(cx, "inspector-chord-action-Copy");
    assert_eq!(chords(cx), [(id, Action::Copy, ChordStanding::Default)]);

    open_app(cx, Some(SAFARI));
    assert!(
        cx.debug_bounds("inspector-current-gesture-summary")
            .is_some(),
        "an app profile that inherits the gestures has its own inspector"
    );
    click_picker_control(cx, "inspector-chord-row-DpiToggle");
    click_picker_control(cx, "inspector-chord-action-Paste");
    assert_eq!(chords(cx), [(id, Action::Paste, ChordStanding::Overridden)]);
    close(view, cx);
}

#[gpui::test]
fn left_and_right_offer_no_chords(cx: &mut TestAppContext) {
    let _locale = LOCALE_LOCK.lock().unwrap();
    rust_i18n::set_locale("en");
    let config = with_chord(chord("Back+Forward"), Action::Copy);
    let (view, cx) = memory_view(cx, config, ButtonId::LeftClick);
    assert!(cx.debug_bounds("inspector-current-action").is_some());
    assert!(cx.debug_bounds("inspector-chords").is_none());
    select(&view, cx, ButtonId::RightClick);
    assert!(cx.debug_bounds("inspector-current-action").is_some());
    assert!(cx.debug_bounds("inspector-chords").is_none());
    select(&view, cx, ButtonId::Back);
    assert!(
        cx.debug_bounds("inspector-chords").is_some(),
        "the fixture would show them on a button that can chord"
    );
    close(view, cx);
}

#[gpui::test]
fn a_device_with_no_saved_identity_offers_no_chords(cx: &mut TestAppContext) {
    let _locale = LOCALE_LOCK.lock().unwrap();
    rust_i18n::set_locale("en");
    cx.update(gpui_component::init);
    install_app_state(cx);
    let (view, cx) = cx.add_window_view(MouseModelView::new);
    cx.simulate_resize(size(px(1200.), px(1000.)));
    view.update(cx, |view, _| {
        view.select(MouseControlId::Button(ButtonId::Back));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("inspector-current-action").is_some());
    assert!(cx.debug_bounds("inspector-chords").is_none());
    close(view, cx);
}

/// What the user accepts is on screen before they accept it, and names only the
/// buttons it applies to.
#[gpui::test]
fn the_notes_name_what_the_members_give_up(cx: &mut TestAppContext) {
    let _locale = LOCALE_LOCK.lock().unwrap();
    rust_i18n::set_locale("en");
    let shown = |cx: &mut VisualTestContext| {
        (
            cx.debug_bounds("inspector-chord-note-delay").is_some(),
            cx.debug_bounds("inspector-chord-note-tap").is_some(),
            cx.debug_bounds("inspector-chord-note-middle").is_some(),
        )
    };
    let (view, cx) = memory_view(cx, Config::ephemeral(), ButtonId::GestureButton);
    assert_eq!(
        shown(cx),
        (false, false, false),
        "no chord, nothing to accept"
    );

    click_picker_control(cx, "inspector-add-chord");
    assert_eq!(shown(cx), (true, false, false));
    click_picker_control(cx, "inspector-chord-partner-Back");
    assert_eq!(shown(cx), (true, true, false), "the partner counts too");
    click_picker_control(cx, "inspector-add-chord");
    click_picker_control(cx, "inspector-chord-partner-MiddleClick");
    assert_eq!(shown(cx), (true, false, true));
    click_picker_control(cx, "inspector-chord-action-Copy");
    assert_eq!(
        shown(cx),
        (true, false, true),
        "a saved chord keeps its note"
    );
    close(view, cx);
}

#[gpui::test]
fn a_chord_that_cannot_run_says_so(cx: &mut TestAppContext) {
    let _locale = LOCALE_LOCK.lock().unwrap();
    rust_i18n::set_locale("en");
    let mut config = with_chord(chord("Back+Forward"), Action::Copy);
    config.set_chord(
        KNOWN_MOUSE_KEY,
        chord("DpiToggle+GestureButton"),
        Some(Action::Paste),
    );
    config.app_settings.capture_mouse_events = false;
    let (view, cx) = memory_view(cx, config, ButtonId::Back);
    assert!(
        cx.debug_bounds("inspector-chord-unavailable-Forward")
            .is_some(),
        "Back and Forward report no press without the hook"
    );
    assert!(
        cx.debug_bounds("inspector-chord-note-delay").is_none(),
        "a chord that cannot run costs its buttons nothing"
    );
    assert!(cx.debug_bounds("inspector-chord-note-tap").is_none());
    select(&view, cx, ButtonId::DpiToggle);
    assert!(
        cx.debug_bounds("inspector-chord-unavailable-GestureButton")
            .is_none(),
        "HID++ members still run"
    );
    assert!(cx.debug_bounds("inspector-chord-note-delay").is_some());
    close(view, cx);
}

/// The delay note quotes the window, and Settings changes it from another
/// window, where no click repaints this one.
#[gpui::test]
fn a_new_chord_window_repaints_the_note_that_quotes_it(cx: &mut TestAppContext) {
    let _locale = LOCALE_LOCK.lock().unwrap();
    rust_i18n::set_locale("en");
    let (view, cx) = memory_view(
        cx,
        with_chord(chord("Back+Forward"), Action::Copy),
        ButtonId::Back,
    );
    let repaints = Rc::new(Cell::new(0));
    let counted = Rc::clone(&repaints);
    let _counting =
        cx.update(|_, cx| cx.observe(&view, move |_, _| counted.set(counted.get() + 1)));

    cx.update(|_, cx| {
        AppState::apply(cx, |state| state.commit_chord_window(ChordWindow::MAX));
    });
    cx.run_until_parked();

    assert_ne!(repaints.get(), 0, "the note still shows the old window");
    close(view, cx);
}

/// The most text this section can hold, at the largest scale in the locale with
/// the longest copy: every row keeps its trailing button inside the inspector.
#[gpui::test]
fn every_chord_row_fits_at_the_largest_scale(cx: &mut TestAppContext) {
    let _locale = LOCALE_LOCK.lock().unwrap();
    rust_i18n::set_locale("fr");
    let mut config = with_chord(chord("Back+Forward"), Action::Copy);
    for (partner, action) in [
        ("GestureButton", Action::MissionControl),
        ("HapticPanel", Action::Copy),
        ("MiddleClick", Action::Paste),
    ] {
        let id = chord(&format!("Back+{partner}"));
        config.set_chord(KNOWN_MOUSE_KEY, id, Some(action));
    }
    let long_name = "com.example.An-Application-With-A-Very-Long-Name-Indeed".to_string();
    config.set_per_app_chord(
        KNOWN_MOUSE_KEY,
        &long_name,
        chord("Back+Forward"),
        Some(Action::None),
    );
    config.set_per_app_chord(
        KNOWN_MOUSE_KEY,
        &long_name,
        chord("Back+MiddleClick"),
        Some(Action::Undo),
    );
    config.set_per_app_chord(
        KNOWN_MOUSE_KEY,
        &long_name,
        chord("Back+WheelTiltLeft"),
        Some(Action::Redo),
    );
    config.app_settings.capture_mouse_events = false;
    let (view, cx) = memory_view(cx, config, ButtonId::Back);
    cx.update(|window, _| window.set_rem_size(crate::ui::theme::rem_size(UiScale::ExtraLarge)));
    open_app(cx, Some(&long_name));

    let inspector = cx.debug_bounds("button-inspector").expect("inspector");
    for (selector, trailing) in [
        (
            "inspector-chord-row-Forward",
            "inspector-chord-use-default-Forward",
        ),
        (
            "inspector-chord-row-GestureButton",
            "inspector-chord-turn-off-GestureButton",
        ),
        (
            "inspector-chord-row-HapticPanel",
            "inspector-chord-turn-off-HapticPanel",
        ),
        (
            "inspector-chord-row-MiddleClick",
            "inspector-chord-use-default-MiddleClick",
        ),
        (
            "inspector-chord-row-WheelTiltLeft",
            "inspector-chord-remove-WheelTiltLeft",
        ),
    ] {
        let row = cx.debug_bounds(selector).expect(selector);
        let button = cx.debug_bounds(trailing).expect(trailing);
        assert!(
            row.right() <= button.left(),
            "{selector} {row:?} {button:?}"
        );
        assert!(
            button.right() <= inspector.right(),
            "{trailing} {button:?} {inspector:?}"
        );
    }
    for note in ["inspector-chord-unavailable-Forward", "inspector-add-chord"] {
        let bounds = cx.debug_bounds(note).expect(note);
        assert!(bounds.right() <= inspector.right(), "{note} {bounds:?}");
    }
    rust_i18n::set_locale("en");
    close(view, cx);
}

/// A refused write leaves the stored chords, in either direction, reports the
/// problem, and shows what is stored.
#[gpui::test]
fn a_failed_save_keeps_the_stored_chords(cx: &mut TestAppContext) {
    let _locale = LOCALE_LOCK.lock().unwrap();
    rust_i18n::set_locale("en");
    let id = chord("Back+Forward");
    let (view, cx) = chord_view(
        cx,
        with_chord(id, Action::Copy),
        ConfigPersistence::ReadOnly("read-only test".into()),
        ButtonId::Back,
    );

    click_picker_control(cx, "inspector-chord-remove-Forward");
    assert_eq!(chords(cx), [(id, Action::Copy, ChordStanding::Default)]);
    assert!(cx.debug_bounds("inspector-chord-row-Forward").is_some());

    click_picker_control(cx, "inspector-add-chord");
    click_picker_control(cx, "inspector-chord-partner-MiddleClick");
    click_picker_control(cx, "inspector-chord-action-Copy");
    assert_eq!(chords(cx), [(id, Action::Copy, ChordStanding::Default)]);
    assert!(cx.debug_bounds("inspector-chord-row-MiddleClick").is_none());

    open_app(cx, Some(SAFARI));
    click_picker_control(cx, "inspector-chord-turn-off-Forward");
    assert_eq!(chords(cx), [(id, Action::Copy, ChordStanding::Inherited)]);

    cx.update(|_, cx| {
        assert_eq!(
            AppState::try_read(cx).unwrap().config_issue(),
            Some("read-only test")
        );
    });
    close(view, cx);
}

/// One picker at a time: the button's own action and a chord's cannot both be
/// open, and neither survives moving to another button.
#[gpui::test]
fn the_button_picker_and_the_chord_pickers_replace_each_other(cx: &mut TestAppContext) {
    let _locale = LOCALE_LOCK.lock().unwrap();
    rust_i18n::set_locale("en");
    let (view, cx) = memory_view(cx, Config::ephemeral(), ButtonId::Back);

    click_picker_control(cx, "inspector-add-chord");
    click_picker_control(cx, "inspector-current-action");
    assert_eq!(
        view.read_with(cx, |view, _| view.picker),
        InspectorPicker::Action
    );
    assert!(cx.debug_bounds("inspector-chord-partner-Forward").is_none());
    assert!(cx.debug_bounds("inspector-action-Copy").is_some());

    click_picker_control(cx, "inspector-add-chord");
    assert!(cx.debug_bounds("inspector-action-Copy").is_none());
    click_picker_control(cx, "inspector-chord-partner-Forward");
    select(&view, cx, ButtonId::Forward);
    assert_eq!(
        view.read_with(cx, |view, _| view.picker),
        InspectorPicker::Closed
    );
    close(view, cx);
}

/// The Buttons workspace's real profile bar.
struct ProfileBar {
    icons: ProfileIconCache,
    catalog: Entity<AppCatalogPicker>,
}

impl Render for ProfileBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().children(button_profile_scope_bar(&self.icons, &self.catalog, cx))
    }
}

/// A profile holding nothing but chords is a profile: it has a tab, its summary
/// and the active-profile badge know it, and removing the app's profiles takes
/// it with them.
#[gpui::test]
fn a_chord_only_app_profile_is_listed_badged_and_removed(cx: &mut TestAppContext) {
    let _locale = LOCALE_LOCK.lock().unwrap();
    rust_i18n::set_locale("en");
    let id = chord("Back+Forward");
    let mut config = Config::ephemeral();
    config.set_per_app_chord(KNOWN_MOUSE_KEY, SAFARI, id, Some(Action::Copy));
    let (view, cx) = memory_view(cx, config, ButtonId::Back);
    let summary = |state: &AppState| {
        crate::features::profiles::app_profile_summary(
            SAFARI,
            state.editing_app_overrides().map_or(0, BTreeMap::len),
            state.editing_app_chord_count(),
        )
    };

    let bar_window = cx.update(|_, cx| {
        cx.open_window(WindowOptions::default(), |window, cx| {
            let icons = ProfileIconCache::default();
            let catalog = {
                let icons = icons.clone();
                cx.new(|cx| AppCatalogPicker::new(icons, window, cx))
            };
            cx.new(|_| ProfileBar { icons, catalog })
        })
        .expect("the bar opens")
    });
    let mut bar = VisualTestContext::from_window(bar_window.into(), cx);
    bar.update(|window, cx| window.draw(cx).clear(cx));
    let tab = "button-profile:app:com.apple.Safari";
    let bounds = bar
        .debug_bounds(tab)
        .expect("a chord-only profile has a tab");
    bar.simulate_click(bounds.center(), Modifiers::default());

    cx.update(|window, cx| {
        let state = AppState::try_read(cx).unwrap();
        assert_eq!(state.editing_app(), Some(SAFARI), "the tab opens it");
        assert_eq!(state.app_profiles().collect::<Vec<_>>(), [(SAFARI, 1)]);
        assert_eq!(
            summary(state),
            tr!("profiles.app_profile_chord_only", app => SAFARI)
        );
        window.draw(cx).clear(cx);
    });
    assert_eq!(chords(cx), [(id, Action::Copy, ChordStanding::AppOnly)]);

    cx.update(|_, cx| {
        let safari = openlogi_core::app::ForegroundApp {
            id: SAFARI.to_string(),
            display_name: "Safari".to_string(),
        };
        AppState::apply(cx, |state| {
            state.set_foreground(openlogi_ipc::ForegroundApps {
                current: Some(safari.clone()),
                recent: vec![safari],
            })
        });
        assert_eq!(
            AppState::try_read(cx).unwrap().active_profile_name(),
            Some("Safari")
        );

        AppState::apply(cx, |state| {
            state.remove_all_app_profiles(&DeviceKey::from(KNOWN_MOUSE_KEY), SAFARI)
        });
        let state = AppState::try_read(cx).unwrap();
        assert_eq!(state.editing_app(), None);
        assert!(state.app_profiles().next().is_none());
        assert_eq!(state.active_profile_name(), None);
    });
    bar.update(|window, cx| window.draw(cx).clear(cx));
    assert!(bar.debug_bounds(tab).is_none(), "and the tab goes with it");
    bar.update(|window, _| window.remove_window());
    close(view, cx);
}
