//! Regression tests for OS-hook state and dispatch policy.

use super::*;
use openlogi_core::binding::{GESTURE_SWIPE_THRESHOLD, LongPressBinding};
use openlogi_core::config::KeyModifiers;

/// A wake epoch for tests that never see a system resume.
static NO_WAKE: AtomicU64 = AtomicU64::new(0);

fn token(id: u64, button: ButtonId) -> PressToken {
    PressToken::hook_for_test(id, button)
}

#[test]
fn senderless_buttons_follow_the_platform_source_policy() {
    assert_eq!(button_source_may_remap(None), !cfg!(target_os = "macos"));
}

#[test]
fn attributed_sources_still_follow_the_device_policy() {
    let trackpad = EventDevice {
        product_name: Some("Apple Internal Keyboard / Trackpad".into()),
        ..EventDevice::default()
    };
    let logitech_mouse = EventDevice {
        product_name: Some("Logitech MX Master 3".into()),
        ..EventDevice::default()
    };

    assert!(!button_source_may_remap(Some(&trackpad)));
    assert!(button_source_may_remap(Some(&logitech_mouse)));
}

fn test_dispatcher() -> (
    ActionDispatcher,
    super::super::button::ButtonRuntimeOwner,
    mpsc::Receiver<super::super::button::ButtonRuntimeEvent>,
) {
    let (events, received) = mpsc::channel();
    let owner = super::super::button::ButtonRuntimeOwner::spawn(move |event| {
        events
            .send(event)
            .expect("test receiver should stay connected");
    })
    .expect("button worker should start");
    let (action_ring, _ring_events) = tokio::sync::mpsc::unbounded_channel();
    let dispatcher = ActionDispatcher {
        executor: super::super::ActionExecutor {
            dpi_cycle: Arc::new(RwLock::new(crate::DpiCycles::default())),
            pointer_speeds: Arc::default(),
            access: crate::hardware::DeviceAccess {
                channel: Arc::new(RwLock::new(None)),
                registry: openlogi_hid::ChannelRegistry::default(),
                receiver_access: crate::receiver_access::ReceiverAccess::default(),
                device_io: openlogi_hid::device_io_channel().1,
                transport_turns: crate::hardware::TransportTurns::default(),
            },
            action_ring,
        },
        buttons: owner.input(),
    };
    (dispatcher, owner, received)
}

// The mid-swipe gate itself is unit-tested on `SwipeAccumulator` in
// `openlogi-core`; these cover only what `HoldState` adds on top — tagging a
// commit with the exact press and held button, and matching the release.

#[test]
fn accumulate_tags_a_committed_swipe_with_the_held_press() {
    let mut hold = HoldState::default();
    let press = token(1, ButtonId::Back);
    hold.begin(ButtonId::Back, press.clone());
    hold.swipe.backdate_hold_for_test();

    assert_eq!(
        hold.accumulate(GESTURE_SWIPE_THRESHOLD + 10, 0),
        Some((press.clone(), ButtonId::Back, GestureDirection::Right))
    );
    assert_eq!(
        hold.accumulate(50, 0),
        None,
        "continuing the same swipe does not repeat"
    );
    assert_eq!(hold.end(ButtonId::Back), Some((press, false)));
}

#[test]
fn a_same_button_repress_restarts_the_stale_hold() {
    let mut hold = HoldState::default();
    let old = token(1, ButtonId::Back);
    assert!(matches!(
        hold.prepare_begin(ButtonId::Back),
        HoldAdmission::Begin
    ));
    hold.begin(ButtonId::Back, old);

    let replacement = token(2, ButtonId::Back);
    assert!(
        matches!(hold.prepare_begin(ButtonId::Back), HoldAdmission::Begin),
        "a same-button re-press is proof of a lost release"
    );
    hold.begin(ButtonId::Back, replacement.clone());
    hold.swipe.backdate_hold_for_test();
    assert_eq!(
        hold.accumulate(GESTURE_SWIPE_THRESHOLD + 10, 0),
        Some((replacement, ButtonId::Back, GestureDirection::Right))
    );
}

#[test]
fn an_aged_hold_yields_to_a_new_buttons_press() {
    let mut hold = HoldState::default();
    hold.begin(ButtonId::Back, token(1, ButtonId::Back));
    hold.backdate_for_test();

    let replacement = token(2, ButtonId::Forward);
    let HoldAdmission::Replace(stale) = hold.prepare_begin(ButtonId::Forward) else {
        panic!("an aged hold must yield to a new press");
    };
    assert_eq!(stale, token(1, ButtonId::Back));
    hold.begin(ButtonId::Forward, replacement.clone());
    hold.swipe.backdate_hold_for_test();
    assert_eq!(
        hold.accumulate(GESTURE_SWIPE_THRESHOLD + 10, 0),
        Some((replacement, ButtonId::Forward, GestureDirection::Right))
    );
}

#[test]
fn begin_is_first_wins_while_a_hold_is_active() {
    let mut hold = HoldState::default();
    let first = token(1, ButtonId::Back);
    hold.begin(ButtonId::Back, first.clone());
    hold.swipe.backdate_hold_for_test();
    assert!(
        matches!(hold.prepare_begin(ButtonId::Forward), HoldAdmission::Refuse),
        "a second press must not hijack the active hold"
    );

    assert_eq!(
        hold.accumulate(GESTURE_SWIPE_THRESHOLD + 10, 0),
        Some((first.clone(), ButtonId::Back, GestureDirection::Right))
    );
    assert_eq!(hold.end(ButtonId::Forward), None);
    assert_eq!(hold.end(ButtonId::Back), Some((first, false)));
}

#[test]
fn end_matches_the_held_button_and_returns_its_token() {
    let mut hold = HoldState::default();
    let press = token(1, ButtonId::Back);
    hold.begin(ButtonId::Back, press.clone());
    assert_eq!(hold.end(ButtonId::Forward), None);
    assert_eq!(hold.end(ButtonId::Back), Some((press, true)));
}

#[test]
fn resolve_gesture_click_prefers_explicit_then_falls_back_to_default() {
    let gestures = BTreeMap::from([(
        ButtonId::Back,
        BTreeMap::from([(GestureDirection::Click, Action::Copy)]),
    )]);
    assert_eq!(
        resolve_gesture_click(&gestures, ButtonId::Back),
        Action::Copy
    );

    let off = BTreeMap::from([(
        ButtonId::Back,
        BTreeMap::from([(GestureDirection::Click, Action::None)]),
    )]);
    assert_eq!(resolve_gesture_click(&off, ButtonId::Back), Action::None);
}

#[test]
fn fail_open_press_pairs_release() {
    let mut fail_open = HashSet::new();
    assert_eq!(
        remapped_press_disposition(ButtonId::Back, true, &mut fail_open),
        EventDisposition::Suppress
    );
    assert_eq!(
        remapped_release_disposition(ButtonId::Back, &mut fail_open),
        EventDisposition::Suppress
    );
    assert_eq!(
        remapped_press_disposition(ButtonId::Forward, false, &mut fail_open),
        EventDisposition::PassThrough
    );
    assert_eq!(
        remapped_release_disposition(ButtonId::Forward, &mut fail_open),
        EventDisposition::PassThrough
    );
    assert_eq!(
        remapped_release_disposition(ButtonId::Forward, &mut fail_open),
        EventDisposition::PassThrough
    );
}

#[test]
fn rejected_key_edges_fail_open() {
    assert_eq!(queued_event_disposition(true), EventDisposition::Suppress);
    assert_eq!(
        queued_event_disposition(false),
        EventDisposition::PassThrough
    );
}

#[test]
fn queued_key_action_retains_its_press_time_target() {
    let (dispatcher, mut owner, _events) = test_dispatcher();
    let keycode = 0x7a;
    let modifiers = KeyModifiers::default();
    let bindings = Arc::new(RwLock::new(BTreeMap::from([(
        KeyTrigger { keycode, modifiers },
        Action::BrowserBack,
    )])));
    let (actions, queued) = mpsc::sync_channel(1);
    let target = ActionDispatchTarget::SafariProcess(417);

    assert_eq!(
        handle_key(
            KeyEvent {
                keycode,
                pressed: true,
                modifiers: KeyModifiers::default(),
            },
            &bindings,
            &actions,
            &dispatcher,
            || target,
        ),
        EventDisposition::Suppress
    );
    assert_eq!(
        queued.recv().expect("action should be queued"),
        (Action::BrowserBack, target)
    );
    assert!(owner.shutdown());
}

/// A held action needs the key's release, which the one-shot action queue
/// never delivers.
#[test]
fn dpi_shift_on_a_key_runs_as_a_held_press() {
    use super::super::button::ButtonRuntimeEvent;

    let (dispatcher, mut owner, events) = test_dispatcher();
    let keycode = 0x7a;
    let modifiers = KeyModifiers::default();
    let bindings = Arc::new(RwLock::new(BTreeMap::from([(
        KeyTrigger { keycode, modifiers },
        Action::DpiShift,
    )])));
    let (actions, queued) = mpsc::sync_channel(1);
    let key = |pressed| {
        handle_key(
            KeyEvent {
                keycode,
                pressed,
                modifiers,
            },
            &bindings,
            &actions,
            &dispatcher,
            ActionDispatchTarget::capture,
        )
    };

    assert_eq!(key(true), EventDisposition::Suppress);
    queued.try_recv().unwrap_err();
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(1)),
        Ok(ButtonRuntimeEvent::Started(_))
    ));
    assert_eq!(key(false), EventDisposition::Suppress);
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(1)),
        Ok(ButtonRuntimeEvent::Ended { .. })
    ));
    assert!(owner.shutdown());
}

#[test]
fn mouse_press_uses_the_target_published_with_its_binding_not_frontmost_safari() {
    use super::super::button::ButtonRuntimeEvent;
    use openlogi_hook::PointerTarget;

    let (dispatcher, mut owner, events) = test_dispatcher();
    let hooks = Arc::new(RwLock::new(HookMaps {
        bindings: BTreeMap::from([(ButtonId::Back, Action::PreviousDesktop.into())]),
        pointer_target: Some(PointerTarget::Desktop),
        ..HookMaps::default()
    }));
    let mouse = EventDevice {
        vendor_id: Some(0x046d),
        product_name: Some("Logitech MX Master 3".into()),
        ..EventDevice::default()
    };
    assert_eq!(
        handle_button(
            ButtonId::Back,
            true,
            Some(&mouse),
            &hooks,
            &NO_WAKE,
            &dispatcher,
            || { panic!("pointer-scoped binding must not capture foreground Safari") }
        ),
        EventDisposition::Suppress
    );
    let ButtonRuntimeEvent::Started(press) =
        events.recv_timeout(Duration::from_secs(1)).expect("down")
    else {
        panic!("started");
    };
    assert_eq!(
        press.target(),
        ActionDispatchTarget::Pointer {
            target: PointerTarget::Desktop,
            fallback_safari_pid: None,
        }
    );
    assert_eq!(press.start_action(), Some(&Action::PreviousDesktop));
    // Pointer movement can restore the native binding before release. The
    // suppressed down must still have a suppressed up and end its lifecycle.
    hooks
        .write()
        .expect("maps")
        .bindings
        .insert(ButtonId::Back, Action::MouseBack.into());
    assert_eq!(
        handle_button(
            ButtonId::Back,
            false,
            Some(&mouse),
            &hooks,
            &NO_WAKE,
            &dispatcher,
            || { panic!("release does not retarget") }
        ),
        EventDisposition::Suppress
    );
    let ButtonRuntimeEvent::Ended { press: ended, .. } =
        events.recv_timeout(Duration::from_secs(1)).expect("up")
    else {
        panic!("ended");
    };
    assert_eq!(ended.token(), press.token());

    // The inverse crossing must not swallow the release of a native press.
    assert_eq!(
        handle_button(
            ButtonId::Back,
            true,
            Some(&mouse),
            &hooks,
            &NO_WAKE,
            &dispatcher,
            || None
        ),
        EventDisposition::PassThrough
    );
    hooks
        .write()
        .expect("maps")
        .bindings
        .insert(ButtonId::Back, Action::PreviousDesktop.into());
    assert_eq!(
        handle_button(
            ButtonId::Back,
            false,
            Some(&mouse),
            &hooks,
            &NO_WAKE,
            &dispatcher,
            || { panic!("release does not retarget") }
        ),
        EventDisposition::PassThrough
    );
    assert!(owner.shutdown());
}

#[test]
fn queued_unidentified_pointer_action_retains_its_press_time_target() {
    use super::super::button::ButtonRuntimeEvent;
    use openlogi_hook::PointerTarget;
    use std::cell::Cell;

    let mouse = EventDevice {
        vendor_id: Some(0x046d),
        product_name: Some("Logitech MX Master 3".into()),
        ..EventDevice::default()
    };
    let mut routes = Vec::new();
    for (pressed_safari, later_safari, current_pointer) in [
        (Some(417), None, PointerTarget::Unavailable),
        (None, Some(518), PointerTarget::Unavailable),
        (Some(417), Some(518), PointerTarget::Unavailable),
        (Some(417), None, PointerTarget::Desktop),
    ] {
        let (dispatcher, mut owner, events) = test_dispatcher();
        let hooks = Arc::new(RwLock::new(HookMaps {
            bindings: BTreeMap::from([(ButtonId::Back, Action::BrowserBack.into())]),
            pointer_target: Some(PointerTarget::Unavailable),
            ..HookMaps::default()
        }));
        let focused_safari = Cell::new(pressed_safari);
        assert_eq!(
            handle_button(
                ButtonId::Back,
                true,
                Some(&mouse),
                &hooks,
                &NO_WAKE,
                &dispatcher,
                || focused_safari.get(),
            ),
            EventDisposition::Suppress
        );

        focused_safari.set(later_safari);
        let ButtonRuntimeEvent::Started(press) =
            events.recv_timeout(Duration::from_secs(1)).expect("down")
        else {
            panic!("started");
        };
        let action = press.start_action().expect("browser binding");
        let resolved = press.target().resolve_with(
            action,
            || current_pointer,
            |_| panic!("an unidentified target has no window to check"),
            || focused_safari.get(),
        );
        let mut navigation = (None, false);
        if let Some(target) = resolved {
            assert!(super::super::dispatch_browser_navigation(
                action,
                target,
                |pid, forward| {
                    assert!(!forward);
                    navigation.0 = Some(pid);
                    true
                },
                || navigation.1 = true,
            ));
        }
        routes.push(navigation);
        assert_eq!(
            handle_button(
                ButtonId::Back,
                false,
                Some(&mouse),
                &hooks,
                &NO_WAKE,
                &dispatcher,
                || panic!("release does not retarget"),
            ),
            EventDisposition::Suppress
        );
        assert!(owner.shutdown());
    }
    assert_eq!(
        routes,
        [
            (Some(417), false),
            (None, true),
            (Some(417), false),
            (None, false)
        ]
    );
}

#[test]
fn safari_target_never_relaxes_device_isolation() {
    let (dispatcher, mut owner, events) = test_dispatcher();
    let hooks = Arc::new(RwLock::new(HookMaps {
        bindings: BTreeMap::from([
            (ButtonId::Back, Action::BrowserBack.into()),
            (ButtonId::Forward, Action::BrowserForward.into()),
        ]),
        ..HookMaps::default()
    }));
    let sources = [
        Some(EventDevice {
            vendor_id: Some(0x045e),
            product_name: Some("Microsoft Mouse".into()),
            ..EventDevice::default()
        }),
        Some(EventDevice {
            product_name: Some("Magic Trackpad".into()),
            ..EventDevice::default()
        }),
        None,
    ];
    for source in &sources {
        // Linux/Windows filter attachment upstream and permit unknown senders.
        if source.is_none() && !cfg!(target_os = "macos") {
            continue;
        }
        for id in [ButtonId::Back, ButtonId::Forward] {
            for pressed in [true, false] {
                assert_eq!(
                    handle_button(
                        id,
                        pressed,
                        source.as_ref(),
                        &hooks,
                        &NO_WAKE,
                        &dispatcher,
                        || { Some(417) }
                    ),
                    EventDisposition::PassThrough
                );
            }
        }
    }
    assert!(owner.shutdown());
    assert!(matches!(
        events.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
}

#[test]
fn scroll_interception_uses_the_button_source_safety_policy_and_skips_trackpads() {
    let logitech = EventDevice {
        vendor_id: Some(openlogi_hook::LOGITECH_VENDOR_ID),
        product_name: Some("Logitech MX Master".to_string()),
        ..EventDevice::default()
    };
    let trackpad = EventDevice {
        product_name: Some("Magic Trackpad".to_string()),
        ..EventDevice::default()
    };

    assert!(scroll_source_may_intercept(false, Some(&logitech)));
    assert!(!scroll_source_may_intercept(true, Some(&logitech)));
    assert!(!scroll_source_may_intercept(false, Some(&trackpad)));
    assert_eq!(
        scroll_source_may_intercept(false, None),
        !cfg!(target_os = "macos"),
        "only macOS requires callback-time device attribution"
    );
}

#[test]
fn rebound_horizontal_wheel_maps_to_thumbwheel_directions() {
    let maps = HookMaps {
        bindings: BTreeMap::from([
            (ButtonId::ThumbwheelScrollUp, Action::NextTab.into()),
            (ButtonId::ThumbwheelScrollDown, Action::PrevTab.into()),
        ]),
        ..HookMaps::default()
    };
    assert_eq!(
        rebound_thumbwheel_action(&maps, 1.0),
        Some((ButtonId::ThumbwheelScrollDown, Action::PrevTab))
    );
    assert_eq!(
        rebound_thumbwheel_action(&maps, -1.0),
        Some((ButtonId::ThumbwheelScrollUp, Action::NextTab))
    );
    assert_eq!(rebound_thumbwheel_action(&maps, 0.0), None);
}

#[test]
fn rebound_horizontal_wheel_uses_the_selected_devices_polarity() {
    let maps = HookMaps {
        bindings: BTreeMap::from([
            (ButtonId::ThumbwheelScrollUp, Action::NextTab.into()),
            (ButtonId::ThumbwheelScrollDown, Action::PrevTab.into()),
        ]),
        selected_device: Some("mx3".to_owned()),
        thumbwheel_positive_is_forward: BTreeMap::from([("mx3".to_owned(), true)]),
        ..HookMaps::default()
    };
    assert_eq!(
        rebound_thumbwheel_action(&maps, 1.0),
        Some((ButtonId::ThumbwheelScrollUp, Action::NextTab))
    );
    assert_eq!(
        rebound_thumbwheel_action(&maps, -1.0),
        Some((ButtonId::ThumbwheelScrollDown, Action::PrevTab))
    );
}

#[test]
fn rebound_horizontal_wheel_does_not_guess_a_selected_devices_polarity() {
    let maps = HookMaps {
        bindings: BTreeMap::from([
            (ButtonId::ThumbwheelScrollUp, Action::NextTab.into()),
            (ButtonId::ThumbwheelScrollDown, Action::PrevTab.into()),
        ]),
        selected_device: Some("not-learned-yet".to_owned()),
        ..HookMaps::default()
    };
    assert_eq!(rebound_thumbwheel_action(&maps, 1.0), None);
    assert_eq!(rebound_thumbwheel_action(&maps, -1.0), None);
}

#[test]
fn native_thumbwheel_scroll_stays_os_native() {
    let maps = HookMaps {
        bindings: BTreeMap::from([
            (
                ButtonId::ThumbwheelScrollUp,
                default_binding(ButtonId::ThumbwheelScrollUp).into(),
            ),
            (
                ButtonId::ThumbwheelScrollDown,
                default_binding(ButtonId::ThumbwheelScrollDown).into(),
            ),
        ]),
        ..HookMaps::default()
    };
    assert_eq!(rebound_thumbwheel_action(&maps, 1.0), None);
    assert_eq!(rebound_thumbwheel_action(&maps, -1.0), None);
}

#[test]
fn long_press_never_passes_through_as_a_native_click() {
    let binding = Binding::LongPress(LongPressBinding::new(
        default_binding(ButtonId::Back),
        Action::MissionControl,
    ));
    assert!(!binding_is_native_click(ButtonId::Back, &binding));
}

#[test]
fn resolve_gesture_click_falls_back_when_click_is_absent() {
    let no_click = BTreeMap::from([(
        ButtonId::Back,
        BTreeMap::from([(GestureDirection::Up, Action::Copy)]),
    )]);
    assert_eq!(
        resolve_gesture_click(&no_click, ButtonId::Back),
        default_binding(ButtonId::Back)
    );

    let empty = BTreeMap::new();
    assert_eq!(
        resolve_gesture_click(&empty, ButtonId::Forward),
        default_binding(ButtonId::Forward)
    );
}

#[test]
fn held_gshift_replaces_a_gesture_button_with_its_shifted_binding() {
    let mut maps = HookMaps {
        selected_device: Some("g502".into()),
        ..HookMaps::default()
    };
    maps.gestures.insert(ButtonId::Back, BTreeMap::new());
    maps.shift_bindings
        .insert(ButtonId::Back, Binding::Single(Action::Copy));

    let (binding, is_gesture) = maps.press_binding(ButtonId::Back);
    assert!(is_gesture && binding.is_none());

    maps.shift_held.insert("g502".into());
    let (binding, is_gesture) = maps.press_binding(ButtonId::Back);
    assert!(!is_gesture);
    assert_eq!(binding, Some(Binding::Single(Action::Copy)));

    // A button with no shifted binding keeps its gestures under G-Shift.
    maps.gestures.insert(ButtonId::Forward, BTreeMap::new());
    assert!(maps.press_binding(ButtonId::Forward).1);
}

#[cfg(target_os = "linux")]
#[test]
fn a_press_in_the_held_gshift_layer_is_no_chord_member() {
    let mut maps = HookMaps {
        selected_device: Some("mouse".into()),
        chords: chord_table(),
        ..HookMaps::default()
    };
    maps.shift_bindings
        .insert(ButtonId::Back, Binding::Single(Action::Copy));
    assert!(maps.chord_member(ButtonId::Back).is_some());

    maps.shift_held.insert("mouse".into());
    assert_eq!(maps.chord_member(ButtonId::Back), None);
    assert!(
        maps.chord_member(ButtonId::Forward).is_some(),
        "a button the layer leaves out keeps its chord"
    );
}

/// A Signature M650 L connected over Bluetooth, as evdev reports it (#2158).
fn m650_bluetooth() -> EventDevice {
    EventDevice {
        vendor_id: Some(0x046d),
        product_id: Some(0xb02a),
        product_name: Some("Signature M650 L Mouse".into()),
    }
}

fn m650_route() -> openlogi_hid::DeviceRoute {
    openlogi_hid::DeviceRoute::Direct {
        vendor_id: 0x046d,
        product_id: 0xb02a,
    }
}

fn m650_session(epoch: u64) -> crate::runtime::HidppSessionId {
    crate::runtime::HidppSessionId::with_epoch("m650", epoch)
}

#[test]
fn a_diverted_button_is_owned_only_on_its_own_direct_device() {
    let mut diverted = DivertedButtons::default();
    diverted.record(&m650_session(7), &m650_route(), ButtonId::Back);
    assert!(diverted.owns(&m650_bluetooth(), ButtonId::Back));
    assert!(
        !diverted.owns(&m650_bluetooth(), ButtonId::Forward),
        "only what the session diverted"
    );
    let other_mouse = EventDevice {
        product_id: Some(0xb034),
        ..m650_bluetooth()
    };
    assert!(
        !diverted.owns(&other_mouse, ButtonId::Back),
        "another mouse's Back is a real press"
    );
    assert!(
        !diverted.owns(&EventDevice::default(), ButtonId::Back),
        "an unattributed source is never an echo"
    );
}

#[test]
fn receiver_routes_and_hidpp_only_buttons_are_not_recorded() {
    let mut diverted = DivertedButtons::default();
    let bolt = openlogi_hid::DeviceRoute::Bolt {
        receiver_uid: "C0FFEE".into(),
        slot: 1,
    };
    diverted.record(&m650_session(7), &bolt, ButtonId::Back);
    diverted.record(&m650_session(7), &m650_route(), ButtonId::DpiToggle);
    assert!(
        !diverted.owns(&m650_bluetooth(), ButtonId::Back),
        "a receiver route names no OS input identity"
    );
    assert!(!diverted.owns(&m650_bluetooth(), ButtonId::DpiToggle));
}

#[test]
fn releasing_a_stale_epoch_keeps_its_successors_diversion() {
    let mut diverted = DivertedButtons::default();
    diverted.record(&m650_session(7), &m650_route(), ButtonId::Back);
    diverted.record(&m650_session(8), &m650_route(), ButtonId::Back);
    diverted.release(&m650_session(7));
    assert!(diverted.owns(&m650_bluetooth(), ButtonId::Back));
    diverted.release(&m650_session(8));
    assert!(!diverted.owns(&m650_bluetooth(), ButtonId::Back));
}

/// #2158: Linux 7.3 re-reports a Back the M650's capture session diverted as
/// an evdev press. The session dispatches it, so the hook must drop both
/// edges instead of running the binding a second time.
#[test]
fn an_echo_of_a_hidpp_diverted_press_is_dropped_on_both_edges() {
    use super::super::button::ButtonRuntimeEvent;
    let (dispatcher, mut owner, events) = test_dispatcher();
    let mut maps = HookMaps {
        bindings: BTreeMap::from([(ButtonId::Back, Action::NextTab.into())]),
        ..HookMaps::default()
    };
    maps.diverted_buttons
        .record(&m650_session(7), &m650_route(), ButtonId::Back);
    let hooks = Arc::new(RwLock::new(maps));
    let m650 = m650_bluetooth();
    for pressed in [true, false] {
        assert_eq!(
            handle_button(
                ButtonId::Back,
                pressed,
                Some(&m650),
                &hooks,
                &NO_WAKE,
                &dispatcher,
                || None
            ),
            EventDisposition::Suppress,
            "pressed {pressed}"
        );
    }
    assert!(
        events.recv_timeout(Duration::from_millis(200)).is_err(),
        "the capture session dispatches this press, not the hook"
    );

    let other_mouse = EventDevice {
        product_id: Some(0xb034),
        ..m650_bluetooth()
    };
    assert_eq!(
        handle_button(
            ButtonId::Back,
            true,
            Some(&other_mouse),
            &hooks,
            &NO_WAKE,
            &dispatcher,
            || None
        ),
        EventDisposition::Suppress
    );
    let ButtonRuntimeEvent::Started(press) =
        events.recv_timeout(Duration::from_secs(1)).expect("down")
    else {
        panic!("started");
    };
    assert_eq!(press.start_action(), Some(&Action::NextTab));
    assert_eq!(
        handle_button(
            ButtonId::Back,
            false,
            Some(&other_mouse),
            &hooks,
            &NO_WAKE,
            &dispatcher,
            || None
        ),
        EventDisposition::Suppress
    );
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(1)).expect("up"),
        ButtonRuntimeEvent::Ended { .. }
    ));
    assert!(owner.shutdown());
}

/// A release follows its press: an echo whose session ends mid-press still
/// has its release dropped, rather than a stray native Back release reaching
/// the desktop.
#[test]
fn an_echoed_press_keeps_its_release_after_the_session_ends() {
    let (dispatcher, mut owner, events) = test_dispatcher();
    let mut maps = HookMaps {
        bindings: BTreeMap::from([(ButtonId::Back, Action::NextTab.into())]),
        ..HookMaps::default()
    };
    maps.diverted_buttons
        .record(&m650_session(7), &m650_route(), ButtonId::Back);
    let hooks = Arc::new(RwLock::new(maps));
    let m650 = m650_bluetooth();
    assert_eq!(
        handle_button(
            ButtonId::Back,
            true,
            Some(&m650),
            &hooks,
            &NO_WAKE,
            &dispatcher,
            || None
        ),
        EventDisposition::Suppress
    );
    hooks
        .write()
        .expect("maps")
        .diverted_buttons
        .release(&m650_session(7));
    assert_eq!(
        handle_button(
            ButtonId::Back,
            false,
            Some(&m650),
            &hooks,
            &NO_WAKE,
            &dispatcher,
            || None
        ),
        EventDisposition::Suppress
    );
    assert!(
        events.recv_timeout(Duration::from_millis(200)).is_err(),
        "neither edge reached the button runtime"
    );
    assert!(owner.shutdown());
}

/// What a test mouse's left button does through [`handle_button`], with a
/// fresh click-lock state. The tests own their hold: zero always locks, and
/// [`FOREVER`] never does.
struct LeftButton {
    hooks: SharedHookMaps,
    wake_epoch: AtomicU64,
    dispatcher: ActionDispatcher,
    owner: super::super::button::ButtonRuntimeOwner,
    events: mpsc::Receiver<super::super::button::ButtonRuntimeEvent>,
}

const FOREVER: Duration = Duration::from_secs(3600);

impl LeftButton {
    fn with_hold(hold: Option<Duration>) -> Self {
        CLICK_LOCK.with_borrow_mut(|lock| *lock = ClickLock::default());
        let (dispatcher, owner, events) = test_dispatcher();
        Self {
            hooks: Arc::new(RwLock::new(HookMaps {
                click_lock: hold,
                ..HookMaps::default()
            })),
            wake_epoch: AtomicU64::new(0),
            dispatcher,
            owner,
            events,
        }
    }

    fn edge(&self, id: ButtonId, pressed: bool) -> EventDisposition {
        let mouse = EventDevice {
            vendor_id: Some(0x046d),
            product_name: Some("Logitech MX Master 3".into()),
            ..EventDevice::default()
        };
        handle_button(
            id,
            pressed,
            Some(&mouse),
            &self.hooks,
            &self.wake_epoch,
            &self.dispatcher,
            || panic!("click lock never targets an application"),
        )
    }

    fn press(&self) -> EventDisposition {
        self.edge(ButtonId::LeftClick, true)
    }

    fn release(&self) -> EventDisposition {
        self.edge(ButtonId::LeftClick, false)
    }

    /// Hold left past a zero hold and release: the button is now locked.
    fn lock(&self) {
        assert_eq!(self.press(), EventDisposition::PassThrough);
        assert_eq!(self.release(), EventDisposition::Suppress);
    }

    fn set_hold(&self, hold: Option<Duration>) {
        self.hooks.write().expect("maps").click_lock = hold;
    }

    /// Stop the worker and prove the left button gave it nothing to do: its
    /// channel closes without a single event.
    fn assert_nothing_queued(mut self) {
        assert!(self.owner.shutdown());
        assert_eq!(
            self.events.try_recv().err(),
            Some(mpsc::TryRecvError::Disconnected),
            "the button runtime must never see the left button"
        );
    }
}

#[cfg(target_os = "linux")]
fn chord_table() -> ChordTable {
    let mut config = openlogi_core::config::Config::default();
    config
        .devices
        .entry("mouse".into())
        .or_default()
        .chords
        .insert("Back+Forward".parse().expect("valid chord"), Action::Copy);
    ChordTable::for_device(&config, Some("mouse"), None)
}

#[cfg(target_os = "linux")]
#[test]
fn left_still_reaches_click_lock_with_chords_configured() {
    let left = LeftButton::with_hold(Some(Duration::ZERO));
    left.hooks.write().expect("maps").chords = chord_table();
    left.lock();
    assert_eq!(left.press(), EventDisposition::Suppress, "unlocking press");
    assert_eq!(left.release(), EventDisposition::PassThrough);
    left.assert_nothing_queued();
}

#[cfg(target_os = "linux")]
#[test]
fn a_chord_member_the_hook_sees_is_suppressed_though_its_action_is_native() {
    use super::super::button::ButtonRuntimeEvent;

    SUPPRESSED_PRESSES.with_borrow_mut(HashSet::clear);
    let (dispatcher, mut owner, events) = test_dispatcher();
    let hooks = Arc::new(RwLock::new(HookMaps {
        chords: chord_table(),
        bindings: BTreeMap::from([
            (ButtonId::Back, Action::MouseBack.into()),
            (ButtonId::MiddleClick, Action::MiddleClick.into()),
        ]),
        ..HookMaps::default()
    }));
    let mouse = EventDevice {
        vendor_id: Some(0x046d),
        product_name: Some("Logitech MX Master 3".into()),
        ..EventDevice::default()
    };
    let edge = |id, pressed| {
        handle_button(
            id,
            pressed,
            Some(&mouse),
            &hooks,
            &NO_WAKE,
            &dispatcher,
            || None,
        )
    };

    assert_eq!(
        edge(ButtonId::Back, true),
        EventDisposition::Suppress,
        "the chord must be able to claim this press"
    );
    let Ok(ButtonRuntimeEvent::Started(press)) = events.recv_timeout(Duration::from_secs(1)) else {
        panic!("the member's press reaches the runtime");
    };
    assert_eq!(press.start_action(), None, "its solo is held back");
    assert_eq!(edge(ButtonId::Back, false), EventDisposition::Suppress);

    assert_eq!(
        edge(ButtonId::MiddleClick, true),
        EventDisposition::PassThrough,
        "a button in no chord keeps its native click"
    );
    assert_eq!(
        edge(ButtonId::MiddleClick, false),
        EventDisposition::PassThrough
    );
    assert!(owner.shutdown());
}

/// The hook applies the selected device's maps to every mouse it grabs, and a
/// diverted button stops reaching it from that device. A press it still sees
/// came from another mouse, which has no chords of its own to wait for.
#[cfg(target_os = "linux")]
#[test]
fn another_mouse_is_not_held_back_by_the_selected_devices_chords() {
    SUPPRESSED_PRESSES.with_borrow_mut(HashSet::clear);
    let (dispatcher, mut owner, events) = test_dispatcher();
    let mut maps = HookMaps {
        chords: chord_table(),
        selected_device: Some("mouse".into()),
        bindings: BTreeMap::from([(ButtonId::Back, Action::MouseBack.into())]),
        ..HookMaps::default()
    };
    let bolt = openlogi_hid::DeviceRoute::Bolt {
        receiver_uid: "C0FFEE".into(),
        slot: 1,
    };
    maps.diverted_buttons.record(
        &crate::runtime::HidppSessionId::with_epoch("mouse", 7),
        &bolt,
        ButtonId::Back,
    );
    let hooks = Arc::new(RwLock::new(maps));
    let other_mouse = EventDevice {
        vendor_id: Some(0x046d),
        product_name: Some("Logitech M185".into()),
        ..EventDevice::default()
    };
    for pressed in [true, false] {
        assert_eq!(
            handle_button(
                ButtonId::Back,
                pressed,
                Some(&other_mouse),
                &hooks,
                &NO_WAKE,
                &dispatcher,
                || None,
            ),
            EventDisposition::PassThrough,
            "its native Back keeps working, pressed = {pressed}"
        );
    }
    assert!(
        events.recv_timeout(Duration::from_millis(50)).is_err(),
        "the press never reaches the runtime"
    );
    assert!(owner.shutdown());
}

#[test]
fn a_selected_device_hands_a_button_to_hidpp_on_any_route() {
    let bolt = openlogi_hid::DeviceRoute::Bolt {
        receiver_uid: "C0FFEE".into(),
        slot: 1,
    };
    let session = crate::runtime::HidppSessionId::with_epoch("mouse", 7);
    let mut diverted = DivertedButtons::default();
    diverted.record(&session, &bolt, ButtonId::Back);
    assert!(diverted.reports_over_hidpp("mouse", ButtonId::Back));
    assert!(!diverted.reports_over_hidpp("mouse", ButtonId::Forward));
    assert!(!diverted.reports_over_hidpp("other", ButtonId::Back));
    diverted.release(&session);
    assert!(!diverted.reports_over_hidpp("mouse", ButtonId::Back));
}

#[test]
fn left_passes_and_queues_nothing_with_click_lock_off() {
    let left = LeftButton::with_hold(None);
    for _ in 0..3 {
        assert_eq!(left.press(), EventDisposition::PassThrough);
        assert_eq!(left.release(), EventDisposition::PassThrough);
    }
    left.assert_nothing_queued();
}

#[test]
fn a_full_lock_cycle_holds_one_release_and_one_press_back() {
    let left = LeftButton::with_hold(Some(Duration::ZERO));
    left.lock();
    assert_eq!(left.press(), EventDisposition::Suppress, "unlocking press");
    assert_eq!(
        left.release(),
        EventDisposition::PassThrough,
        "ends the drag"
    );
    left.lock();
    assert_eq!(left.press(), EventDisposition::Suppress);
    assert_eq!(left.release(), EventDisposition::PassThrough);
    left.assert_nothing_queued();
}

#[test]
fn a_click_shorter_than_the_hold_passes_both_edges() {
    let left = LeftButton::with_hold(Some(FOREVER));
    assert_eq!(left.press(), EventDisposition::PassThrough);
    assert_eq!(left.release(), EventDisposition::PassThrough);
    left.assert_nothing_queued();
}

/// A left binding is a config-schema fact the hook has never honoured, and
/// click lock must not start: it is not a binding and meets none.
#[test]
fn a_binding_on_left_does_not_change_click_lock() {
    let left = LeftButton::with_hold(Some(Duration::ZERO));
    left.hooks
        .write()
        .expect("maps")
        .bindings
        .insert(ButtonId::LeftClick, Action::Copy.into());
    left.lock();
    assert_eq!(left.press(), EventDisposition::Suppress);
    assert_eq!(left.release(), EventDisposition::PassThrough);
    left.assert_nothing_queued();
}

#[test]
fn right_is_unaffected_before_during_and_after_a_lock() {
    let left = LeftButton::with_hold(Some(Duration::ZERO));
    let right = |pressed| left.edge(ButtonId::RightClick, pressed);
    assert_eq!(right(true), EventDisposition::PassThrough);
    assert_eq!(right(false), EventDisposition::PassThrough);
    left.lock();
    assert_eq!(right(true), EventDisposition::PassThrough);
    assert_eq!(right(false), EventDisposition::PassThrough);
    // Right neither unlocked nor disturbed the lock.
    assert_eq!(left.press(), EventDisposition::Suppress);
    assert_eq!(left.release(), EventDisposition::PassThrough);
    left.assert_nothing_queued();
}

/// A source the hook may not touch keeps its native left button, whatever the
/// setting says: the trackpad on macOS's global tap, in practice.
#[test]
fn a_source_that_may_not_be_remapped_never_locks() {
    let left = LeftButton::with_hold(Some(Duration::ZERO));
    let trackpad = EventDevice {
        product_name: Some("Apple Internal Keyboard / Trackpad".into()),
        ..EventDevice::default()
    };
    for pressed in [true, false, true, false] {
        assert_eq!(
            handle_button(
                ButtonId::LeftClick,
                pressed,
                Some(&trackpad),
                &left.hooks,
                &left.wake_epoch,
                &left.dispatcher,
                || None,
            ),
            EventDisposition::PassThrough
        );
    }
    left.assert_nothing_queued();
}

/// A held write lock is a contended `try_read`. Idle fails open, so the press
/// that could not read the hold does not lock on release; Locked needs no read
/// at all, so a rebuild in flight cannot strand a held button.
#[test]
fn a_contended_config_lock_fails_open_and_still_unlocks() {
    let left = LeftButton::with_hold(Some(Duration::ZERO));
    {
        let _rebuild = left.hooks.write().expect("maps");
        assert_eq!(left.press(), EventDisposition::PassThrough);
        assert_eq!(left.release(), EventDisposition::PassThrough);
    }

    left.lock();
    {
        let _rebuild = left.hooks.write().expect("maps");
        assert_eq!(left.press(), EventDisposition::Suppress);
        assert_eq!(left.release(), EventDisposition::PassThrough);
    }
    left.assert_nothing_queued();
}

#[test]
fn disabling_while_locked_still_lets_the_next_click_release() {
    let left = LeftButton::with_hold(Some(Duration::ZERO));
    left.lock();
    left.set_hold(None);
    assert_eq!(left.press(), EventDisposition::Suppress);
    assert_eq!(left.release(), EventDisposition::PassThrough);
    // And it is off from then on.
    assert_eq!(left.press(), EventDisposition::PassThrough);
    assert_eq!(left.release(), EventDisposition::PassThrough);
    left.assert_nothing_queued();
}

#[test]
fn a_hold_is_read_at_the_press_so_a_mid_press_change_waits_for_the_next() {
    let left = LeftButton::with_hold(Some(FOREVER));
    assert_eq!(left.press(), EventDisposition::PassThrough);
    left.set_hold(Some(Duration::ZERO));
    assert_eq!(left.release(), EventDisposition::PassThrough);
    left.lock();
}

/// The kernel dropped the held button at suspend, so nothing is held any more
/// and the first click after wake is an ordinary one, not a swallowed press.
#[test]
fn a_stale_wake_epoch_makes_the_first_click_after_wake_ordinary() {
    let left = LeftButton::with_hold(Some(FOREVER));
    left.set_hold(Some(Duration::ZERO));
    left.lock();
    left.set_hold(Some(FOREVER));
    left.wake_epoch.fetch_add(1, Ordering::Relaxed);
    assert_eq!(left.press(), EventDisposition::PassThrough);
    assert_eq!(left.release(), EventDisposition::PassThrough);
    left.assert_nothing_queued();
}

/// A resume only speaks for what was locked before it.
#[test]
fn a_lock_taken_after_a_wake_still_unlocks_normally() {
    let left = LeftButton::with_hold(Some(Duration::ZERO));
    left.wake_epoch.fetch_add(1, Ordering::Relaxed);
    left.lock();
    assert_eq!(left.press(), EventDisposition::Suppress);
    assert_eq!(left.release(), EventDisposition::PassThrough);
    left.assert_nothing_queued();
}

/// The kernel released the button at suspend with the press still down, and the
/// mouse's release is only read after the wake. Nothing is held any more, so
/// holding that release back would lock nothing and swallow the next click.
#[test]
fn a_press_held_across_a_wake_does_not_lock() {
    let left = LeftButton::with_hold(Some(Duration::ZERO));
    assert_eq!(left.press(), EventDisposition::PassThrough);
    left.wake_epoch.fetch_add(1, Ordering::Relaxed);
    assert_eq!(left.release(), EventDisposition::PassThrough);

    left.set_hold(Some(FOREVER));
    assert_eq!(left.press(), EventDisposition::PassThrough);
    assert_eq!(left.release(), EventDisposition::PassThrough);
    left.assert_nothing_queued();
}

/// The epoch moves on the suspend itself, so a lock is already stale in the gap
/// between the thaw and logind reporting the resume.
#[tokio::test]
async fn a_suspend_moves_the_wake_epoch_before_any_resume() {
    let (signal, gate) = openlogi_hid::device_io_channel();
    let epoch = Arc::new(AtomicU64::new(0));
    let counter = tokio::spawn(count_suspends(gate, epoch.clone()));

    assert!(signal.suspend());
    tokio::time::timeout(Duration::from_secs(5), async {
        while epoch.load(Ordering::Relaxed) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the suspend advances the epoch while the gate is still closed");

    assert!(signal.resume());
    drop(signal);
    counter.await.expect("the counter ends with its producer");
    assert_eq!(epoch.load(Ordering::Relaxed), 1, "only the suspend counts");
}
