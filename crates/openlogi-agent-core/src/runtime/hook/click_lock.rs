//! Click lock: hold the left button for the hold time and release it, and the
//! button stays down so the mouse drags; the next left click lets go.
//!
//! Every decision is pass or suppress on one edge. Nothing is synthesised: a
//! suppressed release leaves the virtual device's button down, and when that
//! device closes the kernel releases it, on a crash as on a clean exit.
//!
//! The object is sans-I/O: edges are named methods with an explicit `now`, and
//! the caller supplies the wake epoch and the hold. It is deliberately not a
//! [`Binding`](openlogi_core::binding::Binding), so it stays out of the button
//! runtime and never meets a binding on the left button.

use std::time::{Duration, Instant};

use openlogi_hook::EventDisposition;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum State {
    #[default]
    Idle,
    /// The press went through. The hold was snapshotted when it did, so a
    /// config change during the press cannot move the goalposts. Stamped with
    /// the wake epoch too: the kernel releases a held button at suspend, the
    /// virtual one with it, so a press from an older epoch has nothing to lock.
    Armed {
        pressed_at: Instant,
        hold: Duration,
        wake_epoch: u64,
    },
    /// The release was held back, so the virtual button is down. Stamped with
    /// the wake epoch it locked in: the kernel drops held keys at suspend, so
    /// a lock from an older epoch no longer holds anything.
    Locked { wake_epoch: u64 },
    /// The unlocking click's press was held back; its release ends the drag.
    Unlocking,
}

/// One mouse's click-lock state.
#[derive(Debug, Default)]
pub(super) struct ClickLock {
    state: State,
}

impl ClickLock {
    /// A left press. `hold` is read only when the press could start a lock:
    /// never while locked, so turning click lock off (or a contended config
    /// lock) cannot strand a held button, whose next click must still release
    /// it. `None` leaves the press untracked.
    pub(super) fn press(
        &mut self,
        now: Instant,
        wake_epoch: u64,
        hold: impl FnOnce() -> Option<Duration>,
    ) -> EventDisposition {
        match self.state {
            // A button cannot be pressed while down, so its release was lost.
            // The hold is timed from this press, or the next quick click would
            // lock on the age of the lost one.
            State::Armed { hold, .. } => {
                self.state = State::Armed {
                    pressed_at: now,
                    hold,
                    wake_epoch,
                };
                EventDisposition::PassThrough
            }
            State::Unlocking => EventDisposition::Suppress,
            State::Locked { wake_epoch: locked } if locked == wake_epoch => {
                self.state = State::Unlocking;
                EventDisposition::Suppress
            }
            State::Idle | State::Locked { .. } => {
                self.state = hold().map_or(State::Idle, |hold| State::Armed {
                    pressed_at: now,
                    hold,
                    wake_epoch,
                });
                EventDisposition::PassThrough
            }
        }
    }

    /// A left release. Whatever it decides, the press it pairs with is over.
    pub(super) fn release(&mut self, now: Instant, wake_epoch: u64) -> EventDisposition {
        match std::mem::take(&mut self.state) {
            State::Armed {
                pressed_at,
                hold,
                wake_epoch: armed,
            } if armed == wake_epoch && now.saturating_duration_since(pressed_at) >= hold => {
                self.state = State::Locked { wake_epoch };
                EventDisposition::Suppress
            }
            _ => EventDisposition::PassThrough,
        }
    }
}

#[cfg(test)]
mod tests {
    use openlogi_hook::EventDisposition::{self, PassThrough, Suppress};

    use super::*;

    const HOLD: Duration = Duration::from_secs(1);
    const EPOCH: u64 = 7;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    fn lock_in(state: State) -> ClickLock {
        ClickLock { state }
    }

    /// Every state against every edge, hold outcome and enabled setting. The
    /// `read` column pins the rule that only a press that could start a lock
    /// looks at the config: a stale lock counts as Idle, a live one does not.
    #[test]
    fn every_state_and_edge() {
        #[derive(Clone, Copy, Debug)]
        enum Edge {
            Press { enabled: bool },
            Release { held_ms: u64 },
        }
        use Edge::{Press, Release};

        let start = Instant::now();
        let armed = State::Armed {
            pressed_at: start,
            hold: HOLD,
            wake_epoch: EPOCH,
        };
        let old_armed = State::Armed {
            pressed_at: start,
            hold: HOLD,
            wake_epoch: EPOCH - 1,
        };
        let live = State::Locked { wake_epoch: EPOCH };
        let stale = State::Locked {
            wake_epoch: EPOCH - 1,
        };
        let rearmed = State::Armed {
            pressed_at: at(start, 5),
            hold: HOLD,
            wake_epoch: EPOCH,
        };
        let (idle, unlocking) = (State::Idle, State::Unlocking);
        let (on, off) = (Press { enabled: true }, Press { enabled: false });
        let released = |held_ms| Release { held_ms };

        // (state, edge, disposition, next state, config read)
        let table = [
            (idle, on, PassThrough, rearmed, true),
            (idle, off, PassThrough, idle, true),
            (idle, released(5000), PassThrough, idle, false),
            (armed, on, PassThrough, rearmed, false),
            (armed, off, PassThrough, rearmed, false),
            (armed, released(999), PassThrough, idle, false),
            (armed, released(1000), Suppress, live, false),
            (armed, released(1001), Suppress, live, false),
            (old_armed, on, PassThrough, rearmed, false),
            (old_armed, released(5000), PassThrough, idle, false),
            (live, on, Suppress, unlocking, false),
            (live, off, Suppress, unlocking, false),
            (live, released(5000), PassThrough, idle, false),
            (stale, on, PassThrough, rearmed, true),
            (stale, off, PassThrough, idle, true),
            (stale, released(5000), PassThrough, idle, false),
            (unlocking, on, Suppress, unlocking, false),
            (unlocking, off, Suppress, unlocking, false),
            (unlocking, released(5000), PassThrough, idle, false),
        ];

        for (state, edge, disposition, next, reads) in table {
            let mut lock = lock_in(state);
            let mut read = false;
            let got = match edge {
                Press { enabled } => lock.press(at(start, 5), EPOCH, || {
                    read = true;
                    enabled.then_some(HOLD)
                }),
                Release { held_ms } => lock.release(at(start, held_ms), EPOCH),
            };
            assert_eq!(got, disposition, "{state:?} / {edge:?}");
            assert_eq!(lock.state, next, "{state:?} / {edge:?}");
            assert_eq!(read, reads, "{state:?} / {edge:?} config read");
        }
    }

    /// Timed edge traces run through `Lock.handle` in `~/.local/bin/clicklock`,
    /// with `HOLD_SECONDS = 1.0`. Each edge is (ms since start, is a press,
    /// whether the script forwarded it).
    #[test]
    fn the_script_s_traces_replay_identically() {
        let press = true;
        let release = false;
        let traces: [&[(u64, bool, bool)]; 7] = [
            // A plain click.
            &[(0, press, true), (80, release, true)],
            // Exactly the hold time locks; the script compares with `>=`.
            &[(0, press, true), (1000, release, false)],
            // A lock and the click that ends it.
            &[
                (0, press, true),
                (1500, release, false),
                (9000, press, false),
                (9080, release, true),
            ],
            // A slow drag locks too.
            &[(0, press, true), (4000, release, false)],
            // Just short of the hold, then a click: nothing ever locks.
            &[
                (0, press, true),
                (999, release, true),
                (1200, press, true),
                (1280, release, true),
            ],
            // Lock, unlock, then lock again from a clean slate.
            &[
                (0, press, true),
                (1000, release, false),
                (2000, press, false),
                (2100, release, true),
                (3000, press, true),
                (4000, release, false),
                (5000, press, false),
                (5050, release, true),
            ],
            // A lost release, then a quick click: the clock restarts, so the
            // click stays a click.
            &[(0, press, true), (5000, press, true), (5080, release, true)],
        ];

        for trace in traces {
            let start = Instant::now();
            let mut lock = ClickLock::default();
            for &(ms, is_press, forwarded) in trace {
                let got: EventDisposition = if is_press {
                    lock.press(at(start, ms), EPOCH, || Some(HOLD))
                } else {
                    lock.release(at(start, ms), EPOCH)
                };
                let want = if forwarded { PassThrough } else { Suppress };
                assert_eq!(got, want, "{trace:?} at {ms} ms");
            }
        }
    }

    /// A press after a lost release restarts the clock but is no new read: the
    /// hold stays the one snapshotted, whatever the config says now.
    #[test]
    fn a_second_press_restarts_the_clock_and_keeps_the_hold() {
        let start = Instant::now();
        let short = Duration::from_millis(300);
        let rearm = |ms| {
            let mut lock = ClickLock::default();
            lock.press(start, EPOCH, || Some(short));
            let again = lock.press(at(start, ms), EPOCH, || {
                panic!("a held press must not read the config")
            });
            assert_eq!(again, PassThrough);
            lock
        };

        assert_eq!(rearm(5000).release(at(start, 5299), EPOCH), PassThrough);
        assert_eq!(rearm(5000).release(at(start, 5300), EPOCH), Suppress);
    }

    /// The hold is the one read at the press, not whatever the config says by
    /// the release.
    #[test]
    fn the_hold_is_snapshotted_at_the_press() {
        let start = Instant::now();
        let mut lock = ClickLock::default();
        assert_eq!(
            lock.press(start, EPOCH, || Some(Duration::from_millis(300))),
            PassThrough
        );
        assert_eq!(lock.release(at(start, 300), EPOCH), Suppress);
    }

    /// A release racing a clock that stepped backwards must not panic or lock.
    #[test]
    fn a_release_before_its_press_is_a_click() {
        let start = Instant::now();
        let mut lock = ClickLock::default();
        lock.press(at(start, 100), EPOCH, || Some(HOLD));
        assert_eq!(lock.release(start, EPOCH), PassThrough);
    }

    /// The kernel dropped the button at suspend, so the first click after wake
    /// is an ordinary one rather than a click swallowed by a dead lock, and it
    /// can start the next lock.
    #[test]
    fn a_lock_from_before_a_wake_does_not_swallow_the_first_click() {
        let start = Instant::now();
        let mut lock = ClickLock::default();
        lock.press(start, EPOCH, || Some(HOLD));
        assert_eq!(lock.release(at(start, 1000), EPOCH), Suppress);

        let woken = EPOCH + 1;
        assert_eq!(
            lock.press(at(start, 60_000), woken, || Some(HOLD)),
            PassThrough
        );
        assert_eq!(lock.release(at(start, 61_000), woken), Suppress);
        assert_eq!(
            lock.state,
            State::Locked { wake_epoch: woken },
            "the new lock is stamped with the epoch it locked in"
        );
        assert_eq!(lock.press(at(start, 62_000), woken, || None), Suppress);
    }
}
