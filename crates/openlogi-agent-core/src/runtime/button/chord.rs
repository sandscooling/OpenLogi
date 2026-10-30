//! Two-button chords: telling two buttons pressed together from two presses.
//!
//! A producer resolves a press's [`ChordMember`] at `Down`, as it resolves the
//! press's binding, and the arm travels on the [`ActivePress`] beside its
//! behavior. `None` means the press has no window open: it is not a member, or
//! its window already closed. Without an arm a press behaves exactly as if
//! chords did not exist.
//!
//! An open window ends one of three ways, each moving the press to its next
//! state in one place:
//!
//! - A partner's `Down` inside the window completes the chord. Both presses
//!   become [`PressBehavior::ChordConsumed`] and fire nothing further, and a
//!   synthetic [`PressControl::Chord`] press owns the action until the first
//!   member ends, so cancellation and shutdown balance it like any other press.
//! - The deadline, the press's own release, or any other press commits it.
//!   Only an `Immediate` solo owes anything then, because only that was held
//!   back. A long press, wheel hold or gesture decides at release anyway, so
//!   its window adds no lag.
//! - Cancellation ends it with the press, and a held-back solo never fires.
//!
//! The new press commits every open window, so at most one is open at a time
//! and a third button can never join a chord.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use openlogi_core::binding::{Action, ButtonId, ChordId};
use openlogi_core::bindings::chords_for;
use openlogi_core::config::Config;
use tracing::debug;

use super::{
    ActivePress, ButtonSource, ButtonState, PressBehavior, PressControl, PressKey, PressToken,
};

/// The chord a member makes with one partner.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ChordPartner {
    id: ChordId,
    action: Action,
}

/// One member's chords, by partner.
type Partners = BTreeMap<ButtonId, ChordPartner>;

/// One device's effective chords, published beside its bindings so a press
/// resolves both from the same snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChordTable {
    /// Config namespace of the chords. Two presses chord only under one.
    scope: Arc<str>,
    window: Duration,
    members: BTreeMap<ButtonId, Arc<Partners>>,
}

impl ChordTable {
    /// The chords `config_key` runs for `app`. Empty unless the device has
    /// some on this platform, and then every press skips this module entirely.
    #[must_use]
    pub fn for_device(config: &Config, config_key: Option<&str>, app: Option<&str>) -> Self {
        Self::new(
            config_key.unwrap_or_default(),
            config.app_settings.chords.window_ms.duration(),
            chords_for(config, config_key, app),
        )
    }

    fn new(
        scope: &str,
        window: Duration,
        chords: impl IntoIterator<Item = (ChordId, Action)>,
    ) -> Self {
        let mut members: BTreeMap<ButtonId, Partners> = BTreeMap::new();
        for (id, action) in chords {
            let [first, second] = id.members();
            let partner = ChordPartner { id, action };
            members
                .entry(first)
                .or_default()
                .insert(second, partner.clone());
            members.entry(second).or_default().insert(first, partner);
        }
        if members.is_empty() {
            return Self::default();
        }
        Self {
            scope: Arc::from(scope),
            window,
            members: members
                .into_iter()
                .map(|(button, partners)| (button, Arc::new(partners)))
                .collect(),
        }
    }

    /// What a press of `button` must know to open a window, or `None` for a
    /// button in no chord here.
    pub(crate) fn member(&self, button: ButtonId) -> Option<ChordMember> {
        let partners = self.members.get(&button)?;
        Some(ChordMember {
            scope: Arc::clone(&self.scope),
            window: self.window,
            partners: Arc::clone(partners),
        })
    }
}

/// One button's place in a [`ChordTable`], cheap enough to clone out of a
/// hook callback's read lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChordMember {
    scope: Arc<str>,
    window: Duration,
    partners: Arc<Partners>,
}

impl ChordMember {
    /// Open this member's window at the producer's press stamp.
    pub(super) fn arm(self, pressed_at: Instant) -> ChordArm {
        ChordArm {
            member: self,
            pressed_at,
        }
    }
}

/// A press whose chord window is open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ChordArm {
    member: ChordMember,
    pressed_at: Instant,
}

impl ChordArm {
    /// The last instant a partner's press still counts.
    pub(super) fn deadline(&self) -> Instant {
        self.pressed_at + self.member.window
    }

    /// How far apart two presses were, whichever came first.
    fn gap_to(&self, other: &Self) -> Duration {
        self.pressed_at.max(other.pressed_at) - self.pressed_at.min(other.pressed_at)
    }

    /// The chord an `arriving` press of `button` completes with this window,
    /// held by a press of `own`.
    ///
    /// Judged by the producer's stamps, not by arrival: the worker may reach
    /// a partner after the window's timer, and hook and HID++ edges queue
    /// separately. Both sides must list the chord, because each resolved its
    /// own snapshot and a profile can change between two presses.
    fn completed_by(
        &self,
        own: ButtonId,
        arriving: &Self,
        button: ButtonId,
    ) -> Option<&ChordPartner> {
        if arriving.member.scope != self.member.scope
            || arriving.pressed_at > self.deadline()
            || !arriving.member.partners.contains_key(&own)
        {
            return None;
        }
        self.member.partners.get(&button)
    }
}

/// How one held partner compared with an arriving press, for tuning the window
/// from the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ChordJudgement {
    held: ButtonId,
    /// `None` once the held press's window has closed: its stamp went with it,
    /// and all that is known is that the arrival missed.
    gap: Option<Duration>,
    matched: bool,
}

impl ActivePress {
    /// Close this press's window. Returns the solo action it held back, if
    /// it held one back.
    pub(super) fn commit_chord(&mut self) -> Option<Action> {
        self.chord.take()?;
        self.behavior
            .start_action()
            .filter(|action| **action != Action::None)
            .cloned()
    }

    /// A chord took this press. Nothing it owed fires now, and only its
    /// release is left.
    fn consume_for_chord(&mut self) {
        self.chord = None;
        self.behavior = PressBehavior::ChordConsumed;
    }

    fn button(&self) -> Option<ButtonId> {
        match self.token.key.control {
            PressControl::Button(button) => Some(button),
            PressControl::Key(_) | PressControl::Chord(_) => None,
        }
    }
}

impl ButtonState {
    /// How each partner still held, and not already in a chord, compares with
    /// an arriving press of `button`.
    fn judgements<'a>(
        &'a self,
        button: ButtonId,
        arm: &'a ChordArm,
    ) -> impl Iterator<Item = ChordJudgement> + 'a {
        self.active.values().filter_map(move |held| {
            let other = held
                .button()
                .filter(|other| arm.member.partners.contains_key(other))?;
            if held.behavior == PressBehavior::ChordConsumed {
                return None;
            }
            let window = held.chord.as_deref();
            Some(ChordJudgement {
                held: other,
                gap: window.map(|window| window.gap_to(arm)),
                matched: window
                    .is_some_and(|window| window.completed_by(other, arm, button).is_some()),
            })
        })
    }

    /// Complete the chord `press` makes with an open window, if it makes one:
    /// both presses are consumed, and the returned press owns the action.
    pub(super) fn complete_chord(&mut self, press: &mut ActivePress) -> Option<ActivePress> {
        let button = press.button()?;
        let arm = press.chord.as_ref()?;
        for judged in self.judgements(button, arm) {
            debug!(
                held = %judged.held.config_name(),
                pressed = %button.config_name(),
                gap_ms = judged.gap.map(|gap| gap.as_millis()),
                window_ms = arm.member.window.as_millis(),
                matched = judged.matched,
                "chord partner judged"
            );
        }
        let (open, partner) = self.active.values_mut().find_map(|open| {
            let window = open.chord.as_ref()?;
            let partner = window.completed_by(open.button()?, arm, button)?.clone();
            Some((open, partner))
        })?;
        open.consume_for_chord();
        press.consume_for_chord();
        // The action follows a HID++ member when there is one, since only
        // that source knows which device it came from.
        let source = match &press.token.key.source {
            ButtonSource::OsHook(_) => open.token.key.source.clone(),
            hidpp @ ButtonSource::Hidpp(_) => hidpp.clone(),
        };
        Some(ActivePress {
            // The completing press completes at most one chord, so its id
            // stays unique under the chord's own key.
            token: PressToken {
                id: press.token.id,
                key: PressKey {
                    source,
                    control: PressControl::Chord(partner.id),
                },
                generation: press.token.generation,
            },
            behavior: PressBehavior::Chord {
                action: partner.action,
                members: Box::new([open.token.clone(), press.token.clone()]),
            },
            chord: None,
            target: press.target,
        })
    }

    /// Commit every open window, for a press that completed none of them.
    pub(super) fn commit_open_chords(&mut self) -> Vec<(ActivePress, Action)> {
        self.active
            .values_mut()
            .filter_map(|press| press.commit_chord().map(|action| (press.clone(), action)))
            .collect()
    }

    /// Remove and return every chord press with a member that has ended, so
    /// each path that ends a member ends the chord the same way.
    pub(super) fn orphaned_chords(&mut self) -> Vec<ActivePress> {
        let orphans: Vec<PressKey> = self
            .active
            .values()
            .filter(|press| {
                matches!(
                    &press.behavior,
                    PressBehavior::Chord { members, .. }
                        if !members.iter().all(|member| self.active(member).is_some())
                )
            })
            .map(|press| press.token.key.clone())
            .collect();
        orphans
            .iter()
            .filter_map(|key| self.active.remove(key))
            .collect()
    }

    /// The press an action arriving while `token` is down runs under, or
    /// `None` when it no longer should. This is the one place a consumed
    /// press drops such an action, which covers hook clicks and swipes and
    /// HID++ gestures alike. A swipe already committed cannot join a chord.
    pub(super) fn trigger_target(&mut self, token: &PressToken) -> Option<ActivePress> {
        let press = self
            .active
            .get_mut(&token.key)
            .filter(|press| press.token.id == token.id)?;
        if press.behavior == PressBehavior::ChordConsumed {
            return None;
        }
        press.chord = None;
        Some(press.clone())
    }
}

#[cfg(test)]
pub(super) mod tests;
