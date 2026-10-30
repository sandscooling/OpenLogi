//! Two-button chords: the validated pair of buttons that keys a chord table.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;

use super::button::{ButtonId, ParseButtonIdError};

/// Two different chord-eligible buttons, pressed together.
///
/// A pair is unordered, so the members are kept sorted and `Forward+Back` is
/// the same chord as `Back+Forward`. The only way to build one checks
/// [`ButtonId::supports_chord`], so holding a `ChordId` proves both members may
/// chord.
///
/// # Serialization
///
/// Persisted as `"<button>+<button>"`, so a `ChordId` can key a TOML table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChordId {
    first: ButtonId,
    second: ButtonId,
}

impl ChordId {
    /// The chord of `a` and `b`, in either order.
    ///
    /// # Errors
    ///
    /// Fails when a member cannot chord, or when both name one button.
    pub fn new(a: ButtonId, b: ButtonId) -> Result<Self, ParseChordIdError> {
        for button in [a, b] {
            if !button.supports_chord() {
                return Err(ParseChordIdError::Ineligible(
                    button.config_name().into_owned(),
                ));
            }
        }
        if a == b {
            return Err(ParseChordIdError::SameButton(a.config_name().into_owned()));
        }
        let (first, second) = if a < b { (a, b) } else { (b, a) };
        Ok(Self { first, second })
    }

    /// Both members, in the chord's canonical order.
    #[must_use]
    pub const fn members(self) -> [ButtonId; 2] {
        [self.first, self.second]
    }

    /// The other member, when `button` is one of the two.
    #[must_use]
    pub fn partner_of(self, button: ButtonId) -> Option<ButtonId> {
        if button == self.first {
            Some(self.second)
        } else if button == self.second {
            Some(self.first)
        } else {
            None
        }
    }
}

impl fmt::Display for ChordId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}+{}",
            self.first.config_name(),
            self.second.config_name()
        )
    }
}

/// A persisted chord name that names no chord.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseChordIdError {
    /// The name is not two button names joined by `+`.
    #[error("chord `{0}` is not two buttons joined by `+`")]
    Shape(String),
    /// A member is not a button.
    #[error(transparent)]
    Button(#[from] ParseButtonIdError),
    /// A member is a button that cannot chord.
    #[error("`{0}` cannot be part of a chord")]
    Ineligible(String),
    /// Both members are one button.
    #[error("a chord needs two different buttons, not `{0}` twice")]
    SameButton(String),
}

impl FromStr for ChordId {
    type Err = ParseChordIdError;

    /// The inverse of the [`Display`](fmt::Display) form, in either order.
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        let (a, b) = name
            .split_once('+')
            .ok_or_else(|| ParseChordIdError::Shape(name.to_owned()))?;
        Self::new(a.parse()?, b.parse()?)
    }
}

impl Serialize for ChordId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ChordId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chord_is_one_pair_in_either_order() {
        let back_forward: ChordId = "Back+Forward".parse().expect("valid chord");
        let forward_back: ChordId = "Forward+Back".parse().expect("valid chord");
        assert_eq!(back_forward, forward_back);
        assert_eq!(forward_back.to_string(), "Back+Forward");
        assert_eq!(
            back_forward.members(),
            [ButtonId::Back, ButtonId::Forward],
            "members are canonical"
        );
    }

    #[test]
    fn a_member_finds_its_partner_and_a_stranger_finds_none() {
        let chord = ChordId::new(ButtonId::Forward, ButtonId::Back).expect("eligible pair");
        assert_eq!(chord.partner_of(ButtonId::Back), Some(ButtonId::Forward));
        assert_eq!(chord.partner_of(ButtonId::Forward), Some(ButtonId::Back));
        assert_eq!(chord.partner_of(ButtonId::MiddleClick), None);
    }

    #[test]
    fn every_eligible_pair_round_trips_through_its_name() {
        let eligible = ButtonId::ALL.map(|button| (button, button.supports_chord()));
        for (a, _) in eligible.iter().filter(|(_, eligible)| *eligible) {
            for (b, _) in eligible.iter().filter(|(_, eligible)| *eligible) {
                if a == b {
                    continue;
                }
                let chord = ChordId::new(*a, *b).expect("eligible pair");
                assert_eq!(chord.to_string().parse(), Ok(chord));
            }
        }
    }

    #[test]
    fn only_physical_press_and_release_controls_can_chord() {
        let ineligible = ButtonId::ALL.map(|button| (button, !button.supports_chord()));
        let ineligible: Vec<_> = ineligible
            .into_iter()
            .filter_map(|(button, ineligible)| ineligible.then_some(button))
            .collect();
        assert_eq!(
            ineligible,
            [
                ButtonId::LeftClick,
                ButtonId::RightClick,
                ButtonId::Thumbwheel,
                ButtonId::ThumbwheelScrollUp,
                ButtonId::ThumbwheelScrollDown,
            ]
        );
        assert!(!ButtonId::control(0x010a).supports_chord());
    }

    #[test]
    fn a_name_that_is_not_an_eligible_pair_is_rejected() {
        for (name, expected) in [
            ("Back", ParseChordIdError::Shape("Back".into())),
            (
                "LeftClick+Back",
                ParseChordIdError::Ineligible("LeftClick".into()),
            ),
            (
                "Back+RightClick",
                ParseChordIdError::Ineligible("RightClick".into()),
            ),
            (
                "Thumbwheel+Back",
                ParseChordIdError::Ineligible("Thumbwheel".into()),
            ),
            (
                "ThumbwheelScrollUp+Back",
                ParseChordIdError::Ineligible("ThumbwheelScrollUp".into()),
            ),
            ("Back+Back", ParseChordIdError::SameButton("Back".into())),
            (
                "Back+Forward+MiddleClick",
                ParseChordIdError::Button(ParseButtonIdError("Forward+MiddleClick".into())),
            ),
            (
                "Back+Nope",
                ParseChordIdError::Button(ParseButtonIdError("Nope".into())),
            ),
        ] {
            assert_eq!(name.parse::<ChordId>(), Err(expected), "{name}");
        }
    }
}
