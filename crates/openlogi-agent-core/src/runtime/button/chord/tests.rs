//! Regression tests for chord windows, driven through the real transitions.

use std::sync::atomic::AtomicU64;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use openlogi_core::binding::{
    Action, Binding, ButtonId, ChordId, LONG_PRESS_THRESHOLD, LongPressBinding, WheelHoldBinding,
};

use super::super::worker::{emit_canceled, emit_selected_due_presses, process_input};
use super::super::{
    ActionDispatchTarget, ActivePress, ButtonCommand, ButtonInput, ButtonRuntimeEvent,
    ButtonRuntimeOwner, ButtonSource, ButtonState, CancelReason, EVENT_QUEUE_CAPACITY, EndReason,
    HidppSessionId, PressBehavior, PressControl, PressId, PressKey, PressToken, settle_due_presses,
};
use super::*;

const WINDOW: Duration = Duration::from_millis(50);
const CHORD_ACTION: Action = Action::Copy;
const SOLO: Action = Action::Paste;
const PARTNER_SOLO: Action = Action::Undo;
const OTHER_SOLO: Action = Action::Cut;
const SWIPE: Action = Action::Redo;

fn id(a: ButtonId, b: ButtonId) -> ChordId {
    ChordId::new(a, b).expect("eligible pair")
}

fn table(window: Duration) -> ChordTable {
    ChordTable::new(
        "mouse",
        window,
        [(id(ButtonId::Back, ButtonId::Forward), CHORD_ACTION)],
    )
}

fn at(start: Instant, ms: u64) -> Instant {
    start + Duration::from_millis(ms)
}

/// A press of `button` from the hook, armed from `chords` as a producer would.
fn press(
    number: u64,
    button: ButtonId,
    binding: Option<&Binding>,
    chords: &ChordTable,
    pressed_at: Instant,
) -> ActivePress {
    press_from(
        ButtonSource::current_hook(),
        number,
        button,
        binding,
        chords,
        pressed_at,
    )
}

fn press_from(
    source: ButtonSource,
    number: u64,
    button: ButtonId,
    binding: Option<&Binding>,
    chords: &ChordTable,
    pressed_at: Instant,
) -> ActivePress {
    ActivePress {
        token: PressToken {
            id: PressId(number),
            key: PressKey::new(source, button),
            generation: 0,
        },
        behavior: PressBehavior::new(binding, pressed_at),
        chord: chords
            .member(button)
            .map(|member| Box::new(member.arm(pressed_at))),
        target: ActionDispatchTarget::Keyboard,
    }
}

fn apply(state: &mut ButtonState, input: ButtonInput) -> Vec<ButtonRuntimeEvent> {
    let mut events = Vec::new();
    process_input(state, input, &mut |event| events.push(event));
    events
}

fn up(key: &PressKey, released_at: Instant) -> ButtonInput {
    ButtonInput::Up {
        key: key.clone(),
        released_at,
    }
}

/// The actions the event handler would run for `events`, in order.
fn fired(events: &[ButtonRuntimeEvent]) -> Vec<Action> {
    events
        .iter()
        .filter_map(|event| match event {
            ButtonRuntimeEvent::Started(press) => press.start_action().cloned(),
            ButtonRuntimeEvent::Triggered { action, .. } => Some(action.clone()),
            ButtonRuntimeEvent::Ended { .. } => None,
        })
        .collect()
}

fn chord_press(state: &ButtonState) -> Option<&ActivePress> {
    state
        .active
        .values()
        .find(|press| matches!(press.control(), PressControl::Chord(_)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Open,
    Closed,
    Consumed,
    Gone,
}

fn phase(state: &ButtonState, key: &PressKey) -> Phase {
    match state.active.get(key) {
        None => Phase::Gone,
        Some(press) if press.chord.is_some() => Phase::Open,
        Some(press) if press.behavior == PressBehavior::ChordConsumed => Phase::Consumed,
        Some(_) => Phase::Closed,
    }
}

/// A press shape a member can be in when an edge arrives.
#[derive(Clone, Copy, Debug)]
enum Start {
    /// Open, holding a plain solo back.
    Held,
    /// Open, bound to nothing, so nothing is held back.
    Silent,
    Long,
    Wheel,
    Gesture,
    /// A plain solo whose window closed.
    Committed,
    /// A chord already took it.
    Spent,
}

/// An edge a member can meet.
///
/// `Swipe` applies only to presses that can receive one: the hook and the
/// HID++ dispatcher send while-pressed actions for gesture presses alone, so
/// an `Immediate` press never meets one with its solo still held back.
#[derive(Clone, Copy, Debug)]
enum Edge {
    /// The partner, stamped inside the window.
    Partner,
    /// The partner, stamped after the window, not yet timed out.
    Late,
    /// A button that shares no chord with this one.
    Stranger,
    /// This button again, proving its release was lost.
    Repeat,
    Release,
    /// The window's timer at its deadline.
    Expire,
    /// The timer a millisecond too soon.
    Early,
    /// A gesture's action arriving while the press is down.
    Swipe,
}

/// (start, edge, fired, this press's next phase, chord still live)
fn truth_table() -> Vec<(Start, Edge, Vec<Action>, Phase, bool)> {
    use Edge::{Early, Expire, Late, Partner, Release, Repeat, Stranger, Swipe};
    use Phase::{Closed, Consumed, Gone, Open};
    use Start::{Committed, Gesture, Held, Long, Silent, Spent, Wheel};

    let chord = || vec![CHORD_ACTION];
    let solo = || vec![SOLO];
    let none = Vec::new;
    let other = || vec![OTHER_SOLO];
    vec![
        (Held, Partner, chord(), Consumed, true),
        (Held, Late, solo(), Closed, false),
        (Held, Stranger, vec![SOLO, OTHER_SOLO], Closed, false),
        (Held, Repeat, solo(), Open, false),
        (Held, Release, solo(), Gone, false),
        (Held, Expire, solo(), Closed, false),
        (Held, Early, none(), Open, false),
        (Silent, Partner, chord(), Consumed, true),
        (Silent, Late, none(), Closed, false),
        (Silent, Stranger, other(), Closed, false),
        (Silent, Repeat, none(), Open, false),
        (Silent, Release, none(), Gone, false),
        (Silent, Expire, none(), Closed, false),
        (Silent, Early, none(), Open, false),
        (Long, Partner, chord(), Consumed, true),
        (Long, Late, none(), Closed, false),
        (Long, Stranger, other(), Closed, false),
        (Long, Repeat, none(), Open, false),
        (Long, Release, solo(), Gone, false),
        (Long, Expire, none(), Closed, false),
        (Long, Early, none(), Open, false),
        (Wheel, Partner, chord(), Consumed, true),
        (Wheel, Late, none(), Closed, false),
        (Wheel, Stranger, other(), Closed, false),
        (Wheel, Repeat, none(), Open, false),
        (Wheel, Release, solo(), Gone, false),
        (Wheel, Expire, none(), Closed, false),
        (Wheel, Early, none(), Open, false),
        (Gesture, Partner, chord(), Consumed, true),
        (Gesture, Late, none(), Closed, false),
        (Gesture, Stranger, other(), Closed, false),
        (Gesture, Repeat, none(), Open, false),
        (Gesture, Release, none(), Gone, false),
        (Gesture, Expire, none(), Closed, false),
        (Gesture, Early, none(), Open, false),
        (Gesture, Swipe, vec![SWIPE], Closed, false),
        (Committed, Partner, none(), Closed, false),
        (Committed, Late, none(), Closed, false),
        (Committed, Stranger, other(), Closed, false),
        (Committed, Repeat, none(), Open, false),
        (Committed, Release, none(), Gone, false),
        (Committed, Expire, none(), Closed, false),
        (Committed, Early, none(), Closed, false),
        (Spent, Partner, none(), Consumed, false),
        (Spent, Late, none(), Consumed, false),
        (Spent, Stranger, other(), Consumed, true),
        (Spent, Repeat, none(), Open, false),
        (Spent, Release, none(), Gone, false),
        (Spent, Expire, none(), Consumed, true),
        (Spent, Early, none(), Consumed, true),
        (Spent, Swipe, none(), Consumed, true),
    ]
}

/// One press of Back being walked to a row's start and through its edge.
struct Row {
    start: Instant,
    chords: ChordTable,
    state: ButtonState,
    pressed: ActivePress,
    binding: Option<Binding>,
}

impl Row {
    fn begin(from: Start) -> Self {
        let binding = match from {
            Start::Held | Start::Committed | Start::Spent => Some(Binding::Single(SOLO)),
            Start::Silent => Some(Binding::Single(Action::None)),
            Start::Long => Some(Binding::LongPress(LongPressBinding::new(
                SOLO,
                Action::VolumeUp,
            ))),
            Start::Wheel => Some(Binding::WheelHold(WheelHoldBinding::horizontal_scroll(
                SOLO,
            ))),
            Start::Gesture => None,
        };
        let start = Instant::now();
        let chords = table(WINDOW);
        let pressed = press(1, ButtonId::Back, binding.as_ref(), &chords, start);
        let mut state = ButtonState::default();
        apply(&mut state, ButtonInput::Down(pressed.clone()));
        let mut row = Self {
            start,
            chords,
            state,
            pressed,
            binding,
        };
        match from {
            Start::Committed => {
                let events = row.timer(50);
                assert_eq!(fired(&events), [SOLO], "{from:?}: setup");
            }
            Start::Spent => {
                row.partner(2, 10);
            }
            Start::Held | Start::Silent | Start::Long | Start::Wheel | Start::Gesture => {}
        }
        row
    }

    fn partner(&mut self, number: u64, stamped_ms: u64) -> Vec<ButtonRuntimeEvent> {
        let binding = Binding::Single(PARTNER_SOLO);
        let stamped = at(self.start, stamped_ms);
        let partner = press(
            number,
            ButtonId::Forward,
            Some(&binding),
            &self.chords,
            stamped,
        );
        apply(&mut self.state, ButtonInput::Down(partner))
    }

    fn timer(&mut self, ms: u64) -> Vec<ButtonRuntimeEvent> {
        let now = at(self.start, ms);
        let due = self.state.due_presses(now);
        let mut events = Vec::new();
        emit_selected_due_presses(&mut self.state, &due, now, &mut |event| {
            events.push(event);
        });
        events
    }

    fn meet(&mut self, edge: Edge) -> Vec<ButtonRuntimeEvent> {
        let (start, key) = (self.start, self.pressed.token.key.clone());
        match edge {
            Edge::Partner => self.partner(3, 30),
            Edge::Late => self.partner(3, 51),
            Edge::Stranger => {
                let binding = Binding::Single(OTHER_SOLO);
                let other = press(
                    3,
                    ButtonId::MiddleClick,
                    Some(&binding),
                    &self.chords,
                    at(start, 30),
                );
                apply(&mut self.state, ButtonInput::Down(other))
            }
            Edge::Repeat => {
                let again = press(
                    3,
                    ButtonId::Back,
                    self.binding.as_ref(),
                    &self.chords,
                    at(start, 30),
                );
                apply(&mut self.state, ButtonInput::Down(again))
            }
            Edge::Release => apply(&mut self.state, up(&key, at(start, 30))),
            Edge::Expire => self.timer(50),
            Edge::Early => self.timer(49),
            Edge::Swipe => {
                let input = ButtonInput::TriggerWhilePressed {
                    token: self.pressed.token.clone(),
                    action: SWIPE,
                };
                apply(&mut self.state, input)
            }
        }
    }
}

/// Every press shape a member can be in, against every edge it can meet.
#[test]
fn every_state_and_edge() {
    for (from, edge, expected, next, chord_live) in truth_table() {
        let row_name = format!("{from:?} / {edge:?}");
        let mut row = Row::begin(from);
        let events = row.meet(edge);
        let key = &row.pressed.token.key;
        assert_eq!(fired(&events), expected, "{row_name}: fired");
        assert_eq!(phase(&row.state, key), next, "{row_name}: phase");
        assert_eq!(
            chord_press(&row.state).is_some(),
            chord_live,
            "{row_name}: chord"
        );
    }
}

#[test]
fn a_chord_counts_by_the_producers_stamp_inside_the_window_inclusive() {
    let start = Instant::now();
    let chords = table(WINDOW);
    let arm = |button| chords.member(button).expect("member").arm(start);
    let back = arm(ButtonId::Back);
    let partner_at = |stamp| chords.member(ButtonId::Forward).expect("member").arm(stamp);

    let on_the_edge = partner_at(at(start, 50));
    assert_eq!(
        back.completed_by(ButtonId::Back, &on_the_edge, ButtonId::Forward)
            .map(|partner| &partner.action),
        Some(&CHORD_ACTION)
    );
    let late = partner_at(at(start, 51));
    assert_eq!(
        back.completed_by(ButtonId::Back, &late, ButtonId::Forward),
        None
    );
    let early = partner_at(start.checked_sub(Duration::from_millis(5)).expect("past"));
    assert!(
        back.completed_by(ButtonId::Back, &early, ButtonId::Forward)
            .is_some(),
        "a partner stamped first but queued second still completes"
    );
}

#[test]
fn the_log_judges_every_held_partner_with_its_gap_while_it_is_known() {
    let start = Instant::now();
    let chords = ChordTable::new(
        "mouse",
        WINDOW,
        [
            (id(ButtonId::Back, ButtonId::Forward), CHORD_ACTION),
            (id(ButtonId::Forward, ButtonId::GestureButton), CHORD_ACTION),
            (
                id(ButtonId::MiddleClick, ButtonId::GestureButton),
                CHORD_ACTION,
            ),
        ],
    );
    let binding = Binding::Single(SOLO);
    let mut state = ButtonState::default();
    apply(
        &mut state,
        ButtonInput::Down(press(1, ButtonId::Back, Some(&binding), &chords, start)),
    );
    let judged = |state: &ButtonState, button, stamped| {
        let arriving = press(2, button, None, &chords, stamped);
        let arm = arriving.chord.as_deref().expect("a member arms");
        state.judgements(button, arm).collect::<Vec<_>>()
    };

    let hit = ChordJudgement {
        held: ButtonId::Back,
        gap: Some(Duration::from_millis(30)),
        matched: true,
    };
    assert_eq!(judged(&state, ButtonId::Forward, at(start, 30)), [hit]);
    let miss = ChordJudgement {
        gap: Some(Duration::from_millis(60)),
        matched: false,
        ..hit
    };
    assert_eq!(
        judged(&state, ButtonId::Forward, at(start, 60)),
        [miss],
        "a near miss the worker reaches before the timer keeps its number"
    );
    assert_eq!(
        judged(&state, ButtonId::GestureButton, at(start, 10)),
        [],
        "a member of other chords is no attempt at this one"
    );

    let due = state.due_presses(at(start, 50));
    emit_selected_due_presses(&mut state, &due, at(start, 50), &mut |_| {});
    let closed = ChordJudgement {
        gap: None,
        matched: false,
        ..hit
    };
    assert_eq!(
        judged(&state, ButtonId::Forward, at(start, 60)),
        [closed],
        "after the timer the miss still shows, without a number"
    );

    let mut spent = ButtonState::default();
    completed(&mut spent, &CHORD_ACTION, start);
    assert_eq!(
        judged(&spent, ButtonId::Forward, at(start, 100)),
        [],
        "a press a chord took is not a partner any more"
    );
}

#[test]
fn a_chord_needs_one_scope_and_both_sides_to_list_it() {
    let start = Instant::now();
    let chords = table(WINDOW);
    let back = chords.member(ButtonId::Back).expect("member").arm(start);

    let elsewhere = ChordTable::new(
        "other-mouse",
        WINDOW,
        [(id(ButtonId::Back, ButtonId::Forward), CHORD_ACTION)],
    );
    let other_device = elsewhere
        .member(ButtonId::Forward)
        .expect("member")
        .arm(start);
    assert_eq!(
        back.completed_by(ButtonId::Back, &other_device, ButtonId::Forward),
        None,
        "two devices never chord together"
    );

    let other_profile = ChordTable::new(
        "mouse",
        WINDOW,
        [(id(ButtonId::Forward, ButtonId::MiddleClick), CHORD_ACTION)],
    );
    let stale_view = other_profile
        .member(ButtonId::Forward)
        .expect("member")
        .arm(start);
    assert_eq!(
        back.completed_by(ButtonId::Back, &stale_view, ButtonId::Forward),
        None,
        "a partner resolved against a profile without this chord does not complete it"
    );
}

#[test]
fn a_device_without_chords_never_arms_a_press_or_holds_one_back() {
    let start = Instant::now();
    let empty = ChordTable::for_device(&openlogi_core::config::Config::default(), Some("m"), None);
    assert_eq!(empty, ChordTable::default());
    let binding = Binding::Single(SOLO);
    for button in ButtonId::ALL {
        assert_eq!(empty.member(button), None, "{button}");
    }

    let mut state = ButtonState::default();
    let pressed = press(1, ButtonId::Back, Some(&binding), &empty, start);
    assert_eq!(pressed.start_action(), Some(&SOLO));
    assert_eq!(pressed.deadline(), None);
    let events = apply(&mut state, ButtonInput::Down(pressed));
    assert_eq!(fired(&events), [SOLO], "the solo runs at the press");
    assert_eq!(
        state.next_deadline(),
        None,
        "the worker has nothing to wake for"
    );
}

#[test]
fn a_non_member_in_a_table_with_chords_is_not_held_back() {
    let chords = table(WINDOW);
    assert_eq!(chords.member(ButtonId::MiddleClick), None);
    let binding = Binding::Single(SOLO);
    let pressed = press(
        1,
        ButtonId::MiddleClick,
        Some(&binding),
        &chords,
        Instant::now(),
    );
    assert_eq!(pressed.start_action(), Some(&SOLO));
}

#[test]
fn a_button_can_belong_to_several_chords_and_the_first_pair_wins() {
    let chords = ChordTable::new(
        "mouse",
        WINDOW,
        [
            (id(ButtonId::Back, ButtonId::Forward), Action::Copy),
            (id(ButtonId::Back, ButtonId::MiddleClick), Action::Cut),
        ],
    );
    let start = Instant::now();
    let mut state = ButtonState::default();
    apply(
        &mut state,
        ButtonInput::Down(press(1, ButtonId::Back, None, &chords, start)),
    );
    let events = apply(
        &mut state,
        ButtonInput::Down(press(2, ButtonId::MiddleClick, None, &chords, at(start, 5))),
    );
    assert_eq!(fired(&events), [Action::Cut]);

    // Back is consumed, so the other chord it belongs to cannot start.
    let events = apply(
        &mut state,
        ButtonInput::Down(press(3, ButtonId::Forward, None, &chords, at(start, 10))),
    );
    assert_eq!(fired(&events), []);
    assert_eq!(
        state
            .active
            .values()
            .filter(|press| matches!(press.control(), PressControl::Chord(_)))
            .count(),
        1,
        "no three-button chord"
    );
}

#[test]
fn a_stranger_runs_after_the_solo_it_interrupted() {
    let start = Instant::now();
    let chords = table(WINDOW);
    let (solo, other) = (Binding::Single(SOLO), Binding::Single(OTHER_SOLO));
    let mut state = ButtonState::default();
    apply(
        &mut state,
        ButtonInput::Down(press(1, ButtonId::Back, Some(&solo), &chords, start)),
    );
    let events = apply(
        &mut state,
        ButtonInput::Down(press(
            2,
            ButtonId::MiddleClick,
            Some(&other),
            &chords,
            at(start, 5),
        )),
    );
    assert!(matches!(
        &events[0],
        ButtonRuntimeEvent::Triggered { action, press }
            if *action == SOLO && press.control() == &PressControl::Button(ButtonId::Back)
    ));
    assert!(matches!(&events[1], ButtonRuntimeEvent::Started(press)
        if press.control() == &PressControl::Button(ButtonId::MiddleClick)));
    assert_eq!(fired(&events), [SOLO, OTHER_SOLO]);
}

#[test]
fn a_pulse_commits_an_open_window_too() {
    let start = Instant::now();
    let chords = table(WINDOW);
    let (solo, other) = (Binding::Single(SOLO), Binding::Single(OTHER_SOLO));
    let mut state = ButtonState::default();
    apply(
        &mut state,
        ButtonInput::Down(press(1, ButtonId::Back, Some(&solo), &chords, start)),
    );
    let pulse = press(2, ButtonId::Thumbwheel, Some(&other), &chords, at(start, 5));
    let events = apply(&mut state, ButtonInput::Pulse(pulse));
    assert_eq!(fired(&events), [SOLO, OTHER_SOLO]);
}

/// Complete Back+Forward with `action`, returning both member keys.
fn completed(state: &mut ButtonState, action: &Action, start: Instant) -> (PressKey, PressKey) {
    let chords = ChordTable::new(
        "mouse",
        WINDOW,
        [(id(ButtonId::Back, ButtonId::Forward), action.clone())],
    );
    let back = press(1, ButtonId::Back, None, &chords, start);
    let forward = press(2, ButtonId::Forward, None, &chords, at(start, 5));
    let keys = (back.token.key.clone(), forward.token.key.clone());
    apply(state, ButtonInput::Down(back));
    apply(state, ButtonInput::Down(forward));
    keys
}

#[test]
fn a_held_chord_ends_at_the_first_release_in_either_order() {
    let start = Instant::now();
    for first_released in [ButtonId::Back, ButtonId::Forward] {
        let mut state = ButtonState::default();
        let (back, forward) = completed(&mut state, &CHORD_ACTION, start);
        let (first, second) = if first_released == ButtonId::Back {
            (back, forward)
        } else {
            (forward, back)
        };
        let chord = chord_press(&state).expect("live chord").clone();

        let events = apply(&mut state, up(&first, at(start, 100)));
        let [
            ButtonRuntimeEvent::Ended { press: member, .. },
            ButtonRuntimeEvent::Ended {
                press: ended,
                reason: EndReason::Released,
            },
        ] = events.as_slice()
        else {
            panic!("the member and then its chord end");
        };
        assert_eq!(member.token.key, first);
        assert_eq!(
            ended.token(),
            chord.token(),
            "the held output is released by the token that started it"
        );

        let events = apply(&mut state, up(&second, at(start, 200)));
        assert_eq!(events.len(), 1, "the other member only ends itself");
        assert!(fired(&events).is_empty());
        assert!(state.active.is_empty());
    }
}

#[test]
fn retapping_a_member_under_a_held_chord_does_not_repeat_it() {
    let start = Instant::now();
    let chords = table(WINDOW);
    let mut state = ButtonState::default();
    let (_, forward) = completed(&mut state, &CHORD_ACTION, start);
    apply(&mut state, up(&forward, at(start, 100)));
    let binding = Binding::Single(PARTNER_SOLO);
    let retap = press(
        3,
        ButtonId::Forward,
        Some(&binding),
        &chords,
        at(start, 120),
    );
    let events = apply(&mut state, ButtonInput::Down(retap));
    assert_eq!(fired(&events), [], "held back as a fresh solo");
    assert_eq!(chord_press(&state), None);

    let mut settled = Vec::new();
    let now = at(start, 170);
    let due = state.due_presses(now);
    emit_selected_due_presses(&mut state, &due, now, &mut |event| settled.push(event));
    assert_eq!(fired(&settled), [PARTNER_SOLO]);
}

#[test]
fn a_consumed_press_keeps_no_timer_and_owes_nothing() {
    let start = Instant::now();
    let chords = table(WINDOW);
    let long = Binding::LongPress(LongPressBinding::new(SOLO, Action::VolumeUp));
    let mut state = ButtonState::default();
    let back = press(1, ButtonId::Back, Some(&long), &chords, start);
    let key = back.token.key.clone();
    apply(&mut state, ButtonInput::Down(back));
    apply(
        &mut state,
        ButtonInput::Down(press(2, ButtonId::Forward, None, &chords, at(start, 5))),
    );

    assert_eq!(state.next_deadline(), None, "neither window nor threshold");
    let events = apply(&mut state, up(&key, start + LONG_PRESS_THRESHOLD * 2));
    assert_eq!(fired(&events), []);
}

#[test]
fn a_wheel_hold_that_scrolled_cannot_chord() {
    let start = Instant::now();
    let chords = table(WINDOW);
    let wheel = Binding::WheelHold(WheelHoldBinding::horizontal_scroll(SOLO));
    let mut state = ButtonState::default();
    let back = press(1, ButtonId::Back, Some(&wheel), &chords, start);
    let token = back.token.clone();
    apply(&mut state, ButtonInput::Down(back));
    apply(&mut state, ButtonInput::WheelWhilePressed { token });

    let events = apply(
        &mut state,
        ButtonInput::Down(press(2, ButtonId::Forward, None, &chords, at(start, 5))),
    );
    assert_eq!(fired(&events), []);
    assert_eq!(chord_press(&state), None);
}

/// One queue and one state, driven through [`settle_due_presses`] as the
/// worker loop drives them.
fn settle(
    state: &mut ButtonState,
    queued: impl IntoIterator<Item = ButtonInput>,
) -> Vec<ButtonRuntimeEvent> {
    let (commands, receiver) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
    for input in queued {
        commands
            .send(ButtonCommand::Input {
                generation: 0,
                input,
            })
            .expect("test queue should accept the input");
    }
    let (_shutdown, shutdown) = mpsc::channel();
    let shared_generation = AtomicU64::new(0);
    let mut generation = 0;
    let mut events = Vec::new();
    assert!(!settle_due_presses(
        &receiver,
        &shutdown,
        &shared_generation,
        &mut generation,
        state,
        None,
        &mut |event| events.push(event),
    ));
    events
}

fn overdue_start() -> Instant {
    Instant::now()
        .checked_sub(Duration::from_secs(1))
        .expect("test process should have run beyond the window")
}

#[test]
fn a_queued_partner_beats_an_overdue_window() {
    let start = overdue_start();
    let chords = table(WINDOW);
    let binding = Binding::Single(SOLO);
    let mut state = ButtonState::default();
    apply(
        &mut state,
        ButtonInput::Down(press(1, ButtonId::Back, Some(&binding), &chords, start)),
    );

    let partner = press(2, ButtonId::Forward, Some(&binding), &chords, at(start, 20));
    let events = settle(&mut state, [ButtonInput::Down(partner)]);
    assert_eq!(
        fired(&events),
        [CHORD_ACTION],
        "the worker fell behind, but the buttons were pressed together"
    );
}

#[test]
fn a_late_stamped_partner_does_not_chord_however_it_is_queued() {
    let start = overdue_start();
    let chords = table(WINDOW);
    let binding = Binding::Single(SOLO);
    let mut state = ButtonState::default();
    apply(
        &mut state,
        ButtonInput::Down(press(1, ButtonId::Back, Some(&binding), &chords, start)),
    );

    let partner = press(2, ButtonId::Forward, None, &chords, at(start, 200));
    let events = settle(&mut state, [ButtonInput::Down(partner)]);
    assert_eq!(fired(&events), [SOLO]);
    assert_eq!(chord_press(&state), None);
}

#[test]
fn a_queued_release_fires_an_overdue_solo_exactly_once() {
    let start = overdue_start();
    let chords = table(WINDOW);
    let binding = Binding::Single(SOLO);
    let mut state = ButtonState::default();
    let back = press(1, ButtonId::Back, Some(&binding), &chords, start);
    let key = back.token.key.clone();
    apply(&mut state, ButtonInput::Down(back));

    let events = settle(&mut state, [up(&key, at(start, 200))]);
    assert_eq!(fired(&events), [SOLO]);
    assert!(state.active.is_empty());
}

#[test]
fn canceling_a_member_ends_its_chord_for_every_reason() {
    let start = Instant::now();

    let mut state = ButtonState::default();
    completed(&mut state, &CHORD_ACTION, start);
    assert!(chord_press(&state).is_some());
    assert_eq!(state.cancel_hooks().len(), 3, "both members and the chord");
    assert!(state.active.is_empty());

    let mut state = ButtonState::default();
    completed(&mut state, &CHORD_ACTION, start);
    let mut events = Vec::new();
    emit_canceled(state.cancel_all(), CancelReason::Shutdown, &mut |event| {
        events.push(event);
    });
    assert_eq!(events.len(), 3, "both members and the chord");
    assert!(state.active.is_empty());

    let mut state = ButtonState::default();
    let (back, _) = completed(&mut state, &CHORD_ACTION, start);
    let member = state.active.get(&back).expect("member").token.clone();
    let canceled = state.cancel_press(&member).expect("live press");
    assert_eq!(canceled.token, member);
    let orphans = state.orphaned_chords();
    assert_eq!(orphans.len(), 1, "the chord cannot outlive a member");
    assert_eq!(
        state.active.len(),
        1,
        "the other member still owns its release"
    );
}

#[test]
fn a_repeated_down_of_a_member_ends_its_chord() {
    let start = Instant::now();
    let chords = table(WINDOW);
    let mut state = ButtonState::default();
    completed(&mut state, &CHORD_ACTION, start);
    let again = press(3, ButtonId::Forward, None, &chords, at(start, 100));
    let events = apply(&mut state, ButtonInput::Down(again));
    let ended = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                ButtonRuntimeEvent::Ended {
                    reason: EndReason::Canceled(CancelReason::RepeatedDown),
                    ..
                }
            )
        })
        .count();
    assert_eq!(ended, 2, "the lost press and the chord it held");
    assert_eq!(chord_press(&state), None);
}

#[test]
fn a_pointer_change_cancels_a_held_back_solo_like_a_long_press() {
    use openlogi_hook::PointerTarget;

    let start = Instant::now();
    let chords = table(WINDOW);
    let binding = Binding::Single(SOLO);
    let mut state = ButtonState::default();
    let mut back = press(1, ButtonId::Back, Some(&binding), &chords, start);
    back.target = ActionDispatchTarget::Pointer {
        target: PointerTarget::Desktop,
        focus: crate::runtime::FocusBinding::Distinct,
        fallback_safari_pid: None,
    };
    apply(&mut state, ButtonInput::Down(back));

    let canceled = state.cancel_pointer_except(PointerTarget::Unavailable);
    assert_eq!(canceled.len(), 1);
    assert!(state.active.is_empty());
    assert_eq!(state.next_deadline(), None);
}

#[test]
fn a_pointer_change_ends_a_chord_but_not_the_members_release() {
    use openlogi_hook::PointerTarget;

    let start = Instant::now();
    let chords = table(WINDOW);
    let target = ActionDispatchTarget::Pointer {
        target: PointerTarget::Desktop,
        focus: crate::runtime::FocusBinding::Distinct,
        fallback_safari_pid: None,
    };
    let mut state = ButtonState::default();
    let mut back = press(1, ButtonId::Back, None, &chords, start);
    let mut forward = press(2, ButtonId::Forward, None, &chords, at(start, 5));
    back.target = target;
    forward.target = target;
    apply(&mut state, ButtonInput::Down(back));
    apply(&mut state, ButtonInput::Down(forward));

    let canceled = state.cancel_pointer_except(PointerTarget::Unavailable);
    assert_eq!(canceled.len(), 1, "only the chord holds output");
    assert!(matches!(canceled[0].control(), PressControl::Chord(_)));
    assert_eq!(
        state.active.len(),
        2,
        "both members still own their releases"
    );
}

#[test]
fn a_chord_can_cross_the_hook_and_hidpp_and_follows_the_hidpp_device() {
    let start = Instant::now();
    let chords = table(WINDOW);
    let session = HidppSessionId::with_epoch("mouse", 7);
    for hook_first in [true, false] {
        let hook = press(1, ButtonId::Back, None, &chords, start);
        let hidpp = press_from(
            ButtonSource::Hidpp(session.clone()),
            2,
            ButtonId::Forward,
            None,
            &chords,
            at(start, 5),
        );
        let (first, second) = if hook_first {
            (hook, hidpp)
        } else {
            (hidpp, hook)
        };
        let mut state = ButtonState::default();
        apply(&mut state, ButtonInput::Down(first));
        let events = apply(&mut state, ButtonInput::Down(second));
        assert_eq!(fired(&events), [CHORD_ACTION]);
        assert_eq!(
            chord_press(&state).and_then(ActivePress::device_key),
            Some("mouse")
        );

        // Losing the hook alone still ends the chord the HID++ member started.
        let canceled = state.cancel_hooks();
        assert_eq!(canceled.len(), 2, "the hook member and the chord");
        assert_eq!(state.active.len(), 1, "the HID++ member stays");
    }
}

#[test]
fn the_worker_times_a_window_out_on_its_own() {
    let (sent, received) = mpsc::channel();
    let mut owner = ButtonRuntimeOwner::spawn(move |event| {
        sent.send(event)
            .expect("test receiver should stay connected");
    })
    .expect("button worker should start");
    let input = owner.input();
    let chords = table(Duration::from_millis(1));
    let binding = Binding::Single(SOLO);
    input
        .try_hook_down_with_target(
            ButtonId::Back,
            Some(&binding),
            chords.member(ButtonId::Back),
            ActionDispatchTarget::Keyboard,
        )
        .expect("down should be queued");

    let recv = || {
        received
            .recv_timeout(Duration::from_secs(1))
            .expect("button worker should emit an event")
    };
    let ButtonRuntimeEvent::Started(started) = recv() else {
        panic!("the press starts first");
    };
    assert_eq!(started.start_action(), None, "the solo is held back");
    assert!(matches!(
        recv(),
        ButtonRuntimeEvent::Triggered { action, .. } if action == SOLO
    ));
    assert!(owner.shutdown());
}

#[test]
fn shutdown_balances_a_chord_the_worker_started() {
    let (sent, received) = mpsc::channel();
    let mut owner = ButtonRuntimeOwner::spawn(move |event| {
        sent.send(event)
            .expect("test receiver should stay connected");
    })
    .expect("button worker should start");
    let input = owner.input();
    // Long enough that a slow machine cannot expire it between the two presses.
    let chords = table(Duration::from_secs(60));
    for button in [ButtonId::Back, ButtonId::Forward] {
        input
            .try_hook_down_with_target(
                button,
                None,
                chords.member(button),
                ActionDispatchTarget::Keyboard,
            )
            .expect("down should be queued");
    }
    // Shutdown does not wait for the queue, so see all three presses start.
    let mut events: Vec<_> = (0..3)
        .map(|_| {
            received
                .recv_timeout(Duration::from_secs(1))
                .expect("button worker should emit an event")
        })
        .collect();
    assert!(owner.shutdown());

    events.extend(received.try_iter());
    assert_eq!(fired(&events), [CHORD_ACTION]);
    let count =
        |wanted: fn(&ButtonRuntimeEvent) -> bool| events.iter().filter(|e| wanted(e)).count();
    let started = count(|event| matches!(event, ButtonRuntimeEvent::Started(_)));
    let ended = count(|event| matches!(event, ButtonRuntimeEvent::Ended { .. }));
    assert_eq!(started, 3, "both members and the chord");
    assert_eq!(ended, started, "every start has its end");
}
