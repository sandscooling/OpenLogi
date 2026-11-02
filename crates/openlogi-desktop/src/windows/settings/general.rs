//! General settings page.

use super::{
    App, AppState, ChordWindow, ClickLockHold, Disableable, Entity, FluentBuilder, IconName,
    InteractiveElement, ParentElement, SettingField, SettingGroup, SettingItem, SettingPage,
    SharedString, Sizable, Slider, SliderState, Styled, ThumbwheelSensitivity,
    VerticalScrollSensitivity, div, theme,
};
use crate::ui::commit_slider::SliderUnit;
use crate::ui::slider_row::SliderRow;
use crate::ui::theme::Typography as _;
use gpui_base::Button as BaseButton;
use gpui_component::radio::{Radio, RadioGroup};
use gpui_component::switch::Switch;
use openlogi_core::config::{MouseProfileTarget, chords_supported, click_lock_supported};

use crate::features::pointer::momentum::MomentumPanel;
use crate::platform::registration::ServiceStatus;

/// The page's two sensitivity sliders, named so a call site cannot swap two
/// same-typed `Entity<SliderState>`s without the compiler noticing.
pub(super) struct SensitivitySliders {
    pub(super) vertical_scroll: Entity<SliderState>,
    pub(super) thumbwheel: Entity<SliderState>,
}

/// What the click-lock rows show, resolved by the view that owns the slider.
pub(super) struct ClickLockRows {
    pub(super) enabled: bool,
    /// Whether the agent's input hook could be running it. Without one the
    /// rows say so instead of offering a switch that does nothing.
    pub(super) can_run: bool,
    /// The hold to show: the one under a dragged thumb, else the committed one.
    pub(super) hold: ClickLockHold,
    pub(super) hold_slider: Entity<SliderState>,
}

/// What the chord window row shows, resolved by the view that owns the slider.
pub(super) struct ChordWindowRow {
    /// The window to show: the one under a dragged thumb, else the committed one.
    pub(super) window: ChordWindow,
    pub(super) slider: Entity<SliderState>,
}

pub(super) fn general_page(
    sliders: SensitivitySliders,
    click_lock: ClickLockRows,
    chord_window: ChordWindowRow,
    momentum: Entity<MomentumPanel>,
    registration_status: ServiceStatus,
) -> SettingPage {
    let SensitivitySliders {
        vertical_scroll,
        thumbwheel,
    } = sliders;
    let group = SettingGroup::new().item(mouse_profile_target_item());
    let group = click_lock_items(group, click_lock);
    let group = if chords_supported() {
        group.item(chord_window_item(chord_window))
    } else {
        group
    };
    let group = group
        .item(smooth_scrolling_item())
        .item(sensitivity_item(
            tr!("pointer.vertical_scroll_sensitivity"),
            tr!("pointer.vertical_scroll_sensitivity_description"),
            vertical_scroll,
            VerticalScrollSensitivity::MAX,
            VerticalScrollSensitivity::DEFAULT,
        ))
        .item(sensitivity_item(
            tr!("pointer.thumb_wheel_sensitivity"),
            tr!("pointer.thumbwheel_sensitivity_description"),
            thumbwheel,
            ThumbwheelSensitivity::MAX,
            ThumbwheelSensitivity::DEFAULT,
        ));
    // Only macOS animates a desktop switch, so only macOS shows the switch.
    // `cfg!` keeps the item and its setter compiled, and used, everywhere.
    let group = if cfg!(target_os = "macos") {
        group.item(animate_desktop_switch_item())
    } else {
        group
    };
    let group = group.item(launch_at_login_item());

    // Switched off under System Settings › Login Items: nothing can start
    // the service until the user flips it back on there — surface it instead
    // of letting the switch above claim a state macOS is overriding.
    let group = if registration_status == ServiceStatus::RequiresApproval {
        group.item(login_item_approval_notice())
    } else {
        group
    };

    // One `show_in_menu_bar` setting drives the macOS status item and the
    // Windows notification-area icon (live on macOS, at the next agent launch
    // on Windows); Linux has no tray, so no switch.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let group = group.item(
        SettingItem::new(
            if cfg!(target_os = "macos") {
                tr!("app.show_in_menu_bar")
            } else {
                tr!("app.show_in_the_notification_area")
            },
            SettingField::switch(
                |cx| AppState::try_read(cx).is_some_and(|s| s.app_settings().show_in_menu_bar),
                |enabled, cx| {
                    AppState::apply(cx, |state| state.commit_show_in_menu_bar(enabled));
                },
            ),
        )
        .description(if cfg!(target_os = "macos") {
            tr!("app.menu_bar_visibility_description")
        } else {
            tr!("app.notification_area_visibility_description")
        }),
    );

    let group = group
        .item(low_battery_notifications_item())
        .item(lock_key_notifications_item());

    // Momentum scrolling's app-wide default. A mouse can set its own on its
    // Pointer tab; the card counts those.
    let momentum_group = SettingGroup::new()
        .title(tr!("pointer.momentum_title"))
        .description(tr!("pointer.momentum_general_description"))
        .item(SettingItem::render(move |_, _, _| momentum.clone()));
    SettingPage::new(tr!("app.general"))
        .icon(IconName::Settings)
        .resettable(false)
        .group(group)
        .group(momentum_group)
}

fn mouse_profile_target_item() -> SettingItem {
    SettingItem::new(
        tr!("pointer.mouse_button_profiles"),
        SettingField::render(|_, _, cx| {
            let target = AppState::try_read(cx).map_or(MouseProfileTarget::Pointer, |s| {
                s.app_settings().mouse_profile_target
            });
            RadioGroup::vertical("mouse-profile-target")
                .child(
                    Radio::new("mouse-profile-pointer")
                        .label(tr!("pointer.mouse_profile_pointer"))
                        .debug_selector(|| "mouse-profile-pointer".into()),
                )
                .child(
                    Radio::new("mouse-profile-focused")
                        .label(tr!("pointer.mouse_profile_focused"))
                        .debug_selector(|| "mouse-profile-focused".into()),
                )
                .selected_index(Some(match target {
                    MouseProfileTarget::Pointer => 0,
                    MouseProfileTarget::Focused => 1,
                }))
                .on_change(|index, _, cx| {
                    let target = match index {
                        0 => MouseProfileTarget::Pointer,
                        1 => MouseProfileTarget::Focused,
                        _ => unreachable!("mouse profile target has exactly two choices"),
                    };
                    AppState::apply(cx, |state| state.commit_mouse_profile_target(target));
                })
        }),
    )
    .layout(gpui::Axis::Vertical)
    .description(tr!("pointer.mouse_profile_target_description"))
}

/// The click-lock rows, on the platforms that can act on the setting. The hold
/// time only means something while click lock is on, so it shows only then.
fn click_lock_items(group: SettingGroup, rows: ClickLockRows) -> SettingGroup {
    if !click_lock_supported() {
        return group;
    }
    let ClickLockRows {
        enabled,
        can_run,
        hold,
        hold_slider,
    } = rows;
    let group = group.item(click_lock_item(enabled, can_run));
    if enabled {
        group.item(click_lock_hold_item(hold_slider, hold, can_run))
    } else {
        group
    }
}

/// The click-lock switch. `SettingField::switch` has no handle on its control,
/// so this renders the same `Switch` under a selector of its own.
fn click_lock_item(enabled: bool, can_run: bool) -> SettingItem {
    SettingItem::new(
        tr!("pointer.click_lock"),
        SettingField::render(move |options, _, _| {
            div().debug_selector(|| "click-lock-switch".into()).child(
                Switch::new("click-lock")
                    .checked(enabled)
                    .disabled(options.is_disabled())
                    .with_size(options.size())
                    .on_click(|enabled: &bool, _, cx: &mut App| {
                        AppState::apply(cx, |state| state.commit_click_lock(*enabled));
                    }),
            )
        }),
    )
    .disabled(!can_run)
    .description(if can_run {
        tr!("pointer.click_lock_description")
    } else {
        tr!("pointer.click_lock_unavailable")
    })
}

/// The click-lock hold-time slider. Seconds, because a hold reads shorter that
/// way than as "3000 ms".
fn click_lock_hold_item(
    slider: Entity<SliderState>,
    hold: ClickLockHold,
    can_run: bool,
) -> SettingItem {
    let label = |hold: ClickLockHold| {
        let seconds = format!("{:.1}", hold.duration().as_secs_f32());
        tr!("pointer.click_lock_hold_value", seconds => seconds)
    };
    let (title, description) = (
        tr!("pointer.click_lock_hold"),
        tr!("pointer.click_lock_hold_description"),
    );
    SettingItem::render({
        let (title, description) = (title.clone(), description.clone());
        move |_, _, _| {
            let bar = div()
                .w_full()
                .debug_selector(|| "click-lock-hold".into())
                .child(Slider::new(&slider).horizontal().disabled(!can_run));
            SliderRow::new(title.clone(), bar)
                .value(label(hold), [label(ClickLockHold::MAX)])
                .when(hold == ClickLockHold::DEFAULT, |row| {
                    row.marker(tr!("common.default"))
                })
                .description(description.clone())
                .disabled(!can_run)
        }
    })
    .keywords([title, description])
}

/// The chord window slider. Chords run over HID++ as well as the input hook,
/// so unlike click lock this stays live without a hook.
fn chord_window_item(row: ChordWindowRow) -> SettingItem {
    let ChordWindowRow { window, slider } = row;
    let label = |window: ChordWindow| {
        let ms = u16::from(window).to_string();
        tr!("pointer.chord_window_value", ms => ms)
    };
    let (title, description) = (
        tr!("pointer.chord_window"),
        tr!("pointer.chord_window_description"),
    );
    SettingItem::render({
        let (title, description) = (title.clone(), description.clone());
        move |_, _, _| {
            let bar = div()
                .w_full()
                .debug_selector(|| "chord-window".into())
                .child(Slider::new(&slider).horizontal());
            SliderRow::new(title.clone(), bar)
                .value(label(window), [label(ChordWindow::MAX)])
                .when(window == ChordWindow::DEFAULT, |row| {
                    row.marker(tr!("common.default"))
                })
                .description(description.clone())
        }
    })
    .keywords([title, description])
}

/// The smooth-scrolling switch.
fn smooth_scrolling_item() -> SettingItem {
    SettingItem::new(
        tr!("pointer.smooth_scrolling"),
        SettingField::switch(
            |cx| AppState::try_read(cx).is_some_and(|s| s.app_settings().smooth_scroll),
            |enabled, cx| {
                AppState::apply(cx, |state| state.commit_smooth_scroll(enabled));
            },
        ),
    )
    .description(tr!("pointer.smooth_scrolling_description"))
}

/// The switch for the Previous/Next Desktop slide.
fn animate_desktop_switch_item() -> SettingItem {
    SettingItem::new(
        tr!("app.animate_desktop_switch"),
        SettingField::switch(
            |cx| AppState::try_read(cx).is_some_and(|s| s.app_settings().animate_desktop_switch),
            |enabled, cx| {
                AppState::apply(cx, |state| state.commit_animate_desktop_switch(enabled));
            },
        ),
    )
    .description(tr!("app.animate_desktop_switch_description"))
}

/// The switch for the overlay's low-battery notices.
fn low_battery_notifications_item() -> SettingItem {
    SettingItem::new(
        tr!("app.low_battery_notifications"),
        SettingField::switch(
            |cx| AppState::try_read(cx).is_some_and(|s| s.app_settings().low_battery_notifications),
            |enabled, cx| {
                AppState::apply(cx, |state| state.commit_low_battery_notifications(enabled));
            },
        ),
    )
    .description(tr!("app.low_battery_notifications_description"))
}

/// The switch for the overlay's lock-key indicator.
fn lock_key_notifications_item() -> SettingItem {
    SettingItem::new(
        tr!("app.lock_key_notifications"),
        SettingField::switch(
            |cx| AppState::try_read(cx).is_some_and(|s| s.app_settings().lock_key_notifications),
            |enabled, cx| {
                AppState::apply(cx, |state| state.commit_lock_key_notifications(enabled));
            },
        ),
    )
    // macOS gives Num Lock and Scroll Lock no meaning, so the indicator shows
    // only Caps Lock there (`LockKey::is_indicated_here`).
    .description(if cfg!(target_os = "macos") {
        tr!("app.lock_key_notifications_description_macos")
    } else {
        tr!("app.lock_key_notifications_description")
    })
}

/// A sensitivity slider laid out as the device pages lay theirs out — label
/// and value on one line, the slider across the row — with "Default" beside
/// the value while it rests there. A custom item has no title of its own, so
/// its title and description are its search keywords.
fn sensitivity_item<T: SliderUnit + std::fmt::Display>(
    title: SharedString,
    description: SharedString,
    slider: Entity<SliderState>,
    max: T,
    default: T,
) -> SettingItem {
    SettingItem::render({
        let (title, description) = (title.clone(), description.clone());
        move |_, _, cx| {
            let value = T::from_slider(slider.read(cx).value().start());
            SliderRow::new(title.clone(), Slider::new(&slider).horizontal())
                .value(value.to_string(), [max.to_string()])
                .when(value == default, |row| row.marker(tr!("common.default")))
                .description(description.clone())
        }
    })
    .keywords([title, description])
}

/// The launch-at-login switch — a persisted config value the agent reads
/// (the sunk switch); the setter never unregisters.
fn launch_at_login_item() -> SettingItem {
    SettingItem::new(
        tr!("app.launch_at_login"),
        SettingField::switch(
            |cx| AppState::try_read(cx).is_some_and(|s| s.app_settings().launch_at_login),
            |enabled, cx| {
                AppState::apply(cx, |state| state.commit_launch_at_login(enabled));
            },
        ),
    )
    .description(if cfg!(target_os = "macos") {
        tr!("app.launch_at_login_macos_description")
    } else {
        tr!("app.launch_at_login_description")
    })
}

/// The `RequiresApproval` notice: with the direct-launch fallback gone, the
/// switched-off login item stops the agent entirely, whatever the preference.
fn login_item_approval_notice() -> SettingItem {
    SettingItem::new(
        tr!("app.login_item_disabled_in_system_settings"),
        SettingField::render(|_, _, cx| open_login_items_button(cx)),
    )
    .description(tr!("app.login_item_disabled_description"))
}

/// Deep link to System Settings › Login Items — the only place that can
/// re-enable a service switched off there.
fn open_login_items_button(cx: &App) -> BaseButton {
    let pal = theme::palette(cx);
    BaseButton::new("open-login-items")
        .accessibility_label(tr!("app.open_login_items"))
        .px_2()
        .py_1()
        .rounded(pal.control_radius)
        .border_1()
        .border_color(pal.border)
        .text_caption()
        .cursor_pointer()
        .bg(pal.control)
        .hover(move |s| s.bg(pal.control_hover))
        .focus_visible(move |s| s.bg(pal.control_hover))
        .child(tr!("app.open_login_items"))
        .on_click(|_, _, _| crate::platform::registration::open_login_items_settings())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{assets::AssetResolver, i18n::LOCALE_LOCK};
    use crate::state::{ConfigPersistence, Sources};
    use crate::windows::{WindowRegistry, settings};
    use gpui::{
        AppContext as _, Bounds, Modifiers, TestAppContext, VisualTestContext, point, px, size,
    };
    use openlogi_core::config::{Config, UiScale};

    /// Open Settings on a real window, over the given config and a read-only
    /// or writable save path.
    fn settings_window(
        cx: &mut TestAppContext,
        config: Config,
        read_only: bool,
    ) -> VisualTestContext {
        cx.update(|cx| {
            gpui_component::init(cx);
            theme::register_builtin_themes(cx);
        });
        let handle = cx.update(|cx| {
            let (commands, _) = tokio::sync::mpsc::unbounded_channel();
            let resolver = AssetResolver::new();
            let mut sources = Sources::in_memory(config, &resolver, commands);
            if read_only {
                sources.persistence = ConfigPersistence::ReadOnly("read-only test".into());
            }
            AppState::set_global(cx.new(|_| AppState::new(sources)), cx);
            settings::open(cx);
            cx.global::<WindowRegistry>().settings.unwrap()
        });
        let mut visual = VisualTestContext::from_window(handle.into(), cx);
        visual.simulate_resize(size(px(920.), px(640.)));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual
    }

    #[cfg(target_os = "linux")]
    fn click_at(visual: &mut VisualTestContext, selector: &'static str) {
        let bounds = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} renders"));
        visual.simulate_click(bounds.center(), Modifiers::default());
        visual.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[cfg(target_os = "linux")]
    fn click_lock(visual: &mut VisualTestContext) -> openlogi_core::config::ClickLock {
        visual.update(|_, cx| AppState::try_read(cx).unwrap().app_settings().click_lock)
    }

    /// The agent reports its status, with or without an input hook.
    #[cfg(target_os = "linux")]
    fn set_hook_installed(visual: &mut VisualTestContext, hook_installed: bool) {
        use crate::state::AgentLink;
        visual.update(|window, cx| {
            AppState::apply(cx, |state| {
                state.set_agent_link(AgentLink::Ready(openlogi_ipc::AgentStatus {
                    accessibility_granted: true,
                    hook_installed,
                    launch_at_login: false,
                    inventory: openlogi_ipc::InventoryHealth::Ready,
                    protocol_version: openlogi_ipc::PROTOCOL_VERSION,
                    agent_version: "click-lock-test".to_string(),
                    input_monitoring_granted: true,
                    hid_open_failures: false,
                    secure_input: openlogi_ipc::SecureInput::Off,
                    virtual_keyboard: openlogi_ipc::VirtualKeyboardStatus::Unsupported,
                }))
            });
            window.draw(cx).clear(cx);
        });
    }

    /// Greek carries the longest caption: it must wrap, not push the switch
    /// out of the window.
    #[cfg(target_os = "linux")]
    #[gpui::test]
    fn the_click_lock_switch_turns_the_setting_on_and_off(cx: &mut TestAppContext) {
        let _locale = LOCALE_LOCK.lock().unwrap();
        rust_i18n::set_locale("el");
        let mut visual = settings_window(cx, Config::ephemeral(), false);
        let viewport = Bounds::new(point(px(0.), px(0.)), size(px(920.), px(640.)));
        let switch = visual.debug_bounds("click-lock-switch").unwrap();
        assert!(viewport.contains(&switch.origin) && viewport.contains(&switch.bottom_right()));

        assert!(!click_lock(&mut visual).enabled);
        assert!(
            visual.debug_bounds("click-lock-hold").is_none(),
            "the hold means nothing while click lock is off"
        );

        click_at(&mut visual, "click-lock-switch");
        assert!(click_lock(&mut visual).enabled);
        assert!(visual.debug_bounds("click-lock-hold").is_some());

        click_at(&mut visual, "click-lock-switch");
        assert!(!click_lock(&mut visual).enabled);
        assert!(visual.debug_bounds("click-lock-hold").is_none());
        rust_i18n::set_locale("en");
    }

    /// The row keeps the save's verdict: a refused write leaves the setting
    /// off, and the switch showing what is actually stored.
    #[cfg(target_os = "linux")]
    #[gpui::test]
    fn a_failed_save_leaves_click_lock_off(cx: &mut TestAppContext) {
        let _locale = LOCALE_LOCK.lock().unwrap();
        rust_i18n::set_locale("en");
        let mut visual = settings_window(cx, Config::ephemeral(), true);
        click_at(&mut visual, "click-lock-switch");
        assert!(!click_lock(&mut visual).enabled);
    }

    /// A click on the bar commits one hold, on a step, inside the range. The
    /// page does not touch the switch, and the thumb follows the save.
    #[cfg(target_os = "linux")]
    #[gpui::test]
    fn the_hold_slider_commits_a_step_on_release(cx: &mut TestAppContext) {
        let _locale = LOCALE_LOCK.lock().unwrap();
        rust_i18n::set_locale("en");
        let mut config = Config::ephemeral();
        config.app_settings.click_lock.enabled = true;
        let mut visual = settings_window(cx, config, false);
        assert_eq!(click_lock(&mut visual).hold_ms, ClickLockHold::DEFAULT);

        click_at(&mut visual, "click-lock-hold");
        let lock = click_lock(&mut visual);
        assert!(lock.enabled);
        assert_ne!(lock.hold_ms, ClickLockHold::DEFAULT);
        assert!(lock.hold_ms > ClickLockHold::MIN && lock.hold_ms < ClickLockHold::MAX);
        assert_eq!(
            u16::from(lock.hold_ms) % ClickLockHold::STEP_MS,
            0,
            "{}",
            lock.hold_ms
        );
    }

    /// No hook, no click lock: with `capture_mouse_events` off the rows must
    /// not offer a switch that does nothing, and a live hook brings them back.
    #[cfg(target_os = "linux")]
    #[gpui::test]
    fn the_click_lock_rows_refuse_input_without_a_hook(cx: &mut TestAppContext) {
        let _locale = LOCALE_LOCK.lock().unwrap();
        rust_i18n::set_locale("en");
        let mut config = Config::ephemeral();
        config.app_settings.click_lock.enabled = true;
        let mut visual = settings_window(cx, config, false);

        set_hook_installed(&mut visual, false);
        click_at(&mut visual, "click-lock-switch");
        assert!(click_lock(&mut visual).enabled, "the switch is disabled");
        click_at(&mut visual, "click-lock-hold");
        assert_eq!(
            click_lock(&mut visual).hold_ms,
            ClickLockHold::DEFAULT,
            "the slider is disabled"
        );

        set_hook_installed(&mut visual, true);
        click_at(&mut visual, "click-lock-switch");
        assert!(!click_lock(&mut visual).enabled);
    }

    #[cfg(target_os = "linux")]
    fn chord_window(visual: &mut VisualTestContext) -> ChordWindow {
        visual.update(|_, cx| {
            AppState::try_read(cx)
                .unwrap()
                .app_settings()
                .chords
                .window_ms
        })
    }

    /// A click on the bar commits one window, on a step, inside the range, and
    /// the value column shows it in milliseconds. Greek stays inside the window.
    #[cfg(target_os = "linux")]
    #[gpui::test]
    fn the_chord_window_slider_commits_a_step_on_release(cx: &mut TestAppContext) {
        let _locale = LOCALE_LOCK.lock().unwrap();
        rust_i18n::set_locale("el");
        let mut visual = settings_window(cx, Config::ephemeral(), false);
        let viewport = Bounds::new(point(px(0.), px(0.)), size(px(920.), px(640.)));
        let row = visual.debug_bounds("chord-window").unwrap();
        assert!(viewport.contains(&row.origin) && viewport.contains(&row.bottom_right()));
        assert_eq!(chord_window(&mut visual), ChordWindow::DEFAULT);

        click_at(&mut visual, "chord-window");
        let window = chord_window(&mut visual);
        assert_ne!(window, ChordWindow::DEFAULT);
        assert!(window > ChordWindow::MIN && window < ChordWindow::MAX);
        assert_eq!(u16::from(window) % ChordWindow::STEP_MS, 0, "{window:?}");
        rust_i18n::set_locale("en");
    }

    /// A refused write leaves the stored window, and the thumb follows it back.
    #[cfg(target_os = "linux")]
    #[gpui::test]
    fn a_failed_save_keeps_the_chord_window(cx: &mut TestAppContext) {
        let _locale = LOCALE_LOCK.lock().unwrap();
        rust_i18n::set_locale("en");
        let mut visual = settings_window(cx, Config::ephemeral(), true);
        click_at(&mut visual, "chord-window");
        assert_eq!(chord_window(&mut visual), ChordWindow::DEFAULT);
    }

    /// Everywhere else the field only parses, so no row claims otherwise.
    #[cfg(not(target_os = "linux"))]
    #[gpui::test]
    fn no_other_platform_shows_the_chord_window(cx: &mut TestAppContext) {
        let _locale = LOCALE_LOCK.lock().unwrap();
        rust_i18n::set_locale("en");
        let mut visual = settings_window(cx, Config::ephemeral(), false);
        assert!(visual.debug_bounds("chord-window").is_none());
    }

    /// Everywhere else the field only parses, so no row claims otherwise.
    #[cfg(not(target_os = "linux"))]
    #[gpui::test]
    fn no_other_platform_shows_click_lock(cx: &mut TestAppContext) {
        let _locale = LOCALE_LOCK.lock().unwrap();
        rust_i18n::set_locale("en");
        let mut config = Config::ephemeral();
        config.app_settings.click_lock.enabled = true;
        let mut visual = settings_window(cx, config, false);
        assert!(visual.debug_bounds("click-lock-switch").is_none());
        assert!(visual.debug_bounds("click-lock-hold").is_none());
    }

    #[gpui::test]
    fn mouse_profile_target_choices_update_state_and_respect_failed_saves(cx: &mut TestAppContext) {
        let _locale = LOCALE_LOCK.lock().unwrap();
        cx.update(|cx| {
            gpui_component::init(cx);
            theme::register_builtin_themes(cx);
        });

        for (locale, scale, read_only) in [
            ("en", UiScale::Normal, false),
            ("zh-TW", UiScale::ExtraLarge, false),
            ("en", UiScale::Normal, true),
        ] {
            rust_i18n::set_locale(locale);
            let handle = cx.update(|cx| {
                let mut config = Config::ephemeral();
                config.app_settings.ui_scale = scale;
                let (commands, _) = tokio::sync::mpsc::unbounded_channel();
                let resolver = AssetResolver::new();
                let mut sources = Sources::in_memory(config, &resolver, commands);
                if read_only {
                    sources.persistence = ConfigPersistence::ReadOnly("read-only test".into());
                }
                AppState::set_global(cx.new(|_| AppState::new(sources)), cx);
                settings::open(cx);
                cx.global::<WindowRegistry>().settings.unwrap()
            });
            let mut visual = VisualTestContext::from_window(handle.into(), cx);
            visual.simulate_resize(size(px(920.), px(640.)));
            visual.update(|window, cx| window.draw(cx).clear(cx));
            let viewport = Bounds::new(point(px(0.), px(0.)), size(px(920.), px(640.)));
            let pointer = visual.debug_bounds("mouse-profile-pointer").unwrap();
            let focused = visual.debug_bounds("mouse-profile-focused").unwrap();
            assert!(focused.top() >= pointer.bottom());
            for bounds in [pointer, focused] {
                assert!(
                    viewport.contains(&bounds.origin) && viewport.contains(&bounds.bottom_right())
                );
            }

            visual.simulate_click(focused.center(), Modifiers::default());
            visual.update(|window, cx| {
                window.draw(cx).clear(cx);
                let state = AppState::try_read(cx).unwrap();
                assert_eq!(
                    state.app_settings().mouse_profile_target,
                    if read_only {
                        MouseProfileTarget::Pointer
                    } else {
                        MouseProfileTarget::Focused
                    }
                );
            });
            visual.simulate_click(pointer.center(), Modifiers::default());
            visual.update(|window, cx| {
                window.draw(cx).clear(cx);
                assert_eq!(
                    AppState::try_read(cx)
                        .unwrap()
                        .app_settings()
                        .mouse_profile_target,
                    MouseProfileTarget::Pointer
                );
                window.remove_window();
            });
        }
        rust_i18n::set_locale("en");
    }
}
