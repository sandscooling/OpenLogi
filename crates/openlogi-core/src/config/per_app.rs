//! Per-application profiles: the button and chord overrides a device stores
//! for one application, and how the application in front is matched to them.
//!
//! An override replaces a whole button or chord with a single action. Editing
//! addresses a profile by its exact key; matching goes through [`overlay_for`], which
//! also falls back from a Windows executable path to that executable's
//! `exe:<filename>` key, and then to the `app:<alias>` key of the
//! [`Config::apps`] alias that lists the application.

use std::collections::{BTreeMap, BTreeSet};

use super::{Config, chords_supported};
use crate::app::{overlay_for, profile_key_for};
use crate::binding::{Action, Binding, ButtonId, ChordId};

impl Config {
    /// Resolve the effective binding map for `device_key`, overlaying the
    /// per-app entry for `bundle_id` (if any) on top of the global per-device
    /// `bindings`. A per-app override replaces the whole button with a
    /// [`Binding::Single`]; everything else falls through.
    ///
    /// Returns an empty map when the device has no recorded bindings yet.
    /// Callers (the GUI / hook) layer their own defaults on top.
    #[must_use]
    pub fn effective_bindings(
        &self,
        device_key: &str,
        bundle_id: Option<&str>,
    ) -> BTreeMap<ButtonId, Binding> {
        let Some(device) = self.devices.get(device_key) else {
            return BTreeMap::new();
        };
        let mut out = device.bindings.clone();
        if let Some(bid) = bundle_id
            && let Some(overlay) = overlay_for(&device.per_app_bindings, &self.apps, bid)
        {
            for (k, v) in overlay {
                out.insert(*k, Binding::Single(v.clone()));
            }
        }
        out
    }

    /// Resolve the chords `device_key` runs for `bundle_id`, overlaying that
    /// application's `per_app_chords` entry, if any, on the device's `chords`.
    /// [`Action::None`] means no chord, so an overlay holding it removes the
    /// chord for that application.
    #[must_use]
    pub fn effective_chords(
        &self,
        device_key: &str,
        bundle_id: Option<&str>,
    ) -> BTreeMap<ChordId, Action> {
        let Some(device) = self.devices.get(device_key) else {
            return BTreeMap::new();
        };
        let mut out = device.chords.clone();
        if let Some(bid) = bundle_id
            && let Some(overlay) = overlay_for(&device.per_app_chords, &self.apps, bid)
        {
            out.extend(overlay.iter().map(|(id, action)| (*id, action.clone())));
        }
        out.retain(|_, action| *action != Action::None);
        out
    }

    /// Records a per-app override. Creates the device + app entries as
    /// needed; passing an action of `None` removes the override and prunes
    /// the empty app map.
    pub fn set_per_app_binding(
        &mut self,
        device_key: &str,
        bundle_id: &str,
        button: ButtonId,
        action: Option<Action>,
    ) {
        let entry = self
            .devices
            .entry(device_key.to_string())
            .or_default()
            .per_app_bindings
            .entry(bundle_id.to_string())
            .or_default();
        match action {
            Some(a) => {
                entry.insert(button, a);
            }
            None => {
                entry.remove(&button);
            }
        }
        if let Some(d) = self.devices.get_mut(device_key) {
            d.per_app_bindings.retain(|_, m| !m.is_empty());
        }
    }

    /// Records a chord in `device_key`'s default profile, or with `None`
    /// removes it. Removing one also drops the per-app entries that only turned
    /// it off: they have nothing left to turn off, and would keep an application
    /// listed as a profile with nothing in it.
    pub fn set_chord(&mut self, device_key: &str, chord: ChordId, action: Option<Action>) {
        let device = self.devices.entry(device_key.to_string()).or_default();
        if let Some(action) = action {
            device.chords.insert(chord, action);
            return;
        }
        device.chords.remove(&chord);
        for overlay in device.per_app_chords.values_mut() {
            overlay.retain(|id, action| *id != chord || *action != Action::None);
        }
        device
            .per_app_chords
            .retain(|_, overlay| !overlay.is_empty());
    }

    /// Records a per-app chord entry. [`Action::None`] turns the chord off for
    /// that application; passing `None` removes the entry, so the application
    /// inherits the device's chord again, and prunes the empty app map.
    pub fn set_per_app_chord(
        &mut self,
        device_key: &str,
        bundle_id: &str,
        chord: ChordId,
        action: Option<Action>,
    ) {
        let device = self.devices.entry(device_key.to_string()).or_default();
        let overlay = device
            .per_app_chords
            .entry(bundle_id.to_string())
            .or_default();
        match action {
            Some(action) => {
                overlay.insert(chord, action);
            }
            None => {
                overlay.remove(&chord);
            }
        }
        device
            .per_app_chords
            .retain(|_, overlay| !overlay.is_empty());
    }

    /// The overrides `device_key` stores for the application key `app`,
    /// or `None` when it has no profile for it.
    ///
    /// Exact key, deliberately: this answers "what did the user author under
    /// this key", which is what an editor needs to show and to clear. The
    /// question [`Self::has_app_override`] answers ("will the app in front hit
    /// a profile") is the matcher's, and goes through the same `exe:` and
    /// alias fallbacks the matcher does. The two look interchangeable and are
    /// not.
    #[must_use]
    pub fn per_app_overrides(
        &self,
        device_key: &str,
        app: &str,
    ) -> Option<&BTreeMap<ButtonId, Action>> {
        self.devices
            .get(device_key)?
            .per_app_bindings
            .get(app)
            .filter(|overrides| !overrides.is_empty())
    }

    /// The chord entries `device_key` stores for the application key `app`, on
    /// a platform that runs chords, or `None` when it has none. Exact key, like
    /// [`Self::per_app_overrides`].
    #[must_use]
    pub fn per_app_chord_overrides(
        &self,
        device_key: &str,
        app: &str,
    ) -> Option<&BTreeMap<ChordId, Action>> {
        self.devices
            .get(device_key)
            .filter(|_| chords_supported())?
            .per_app_chords
            .get(app)
            .filter(|overrides| !overrides.is_empty())
    }

    /// Every application key `device_key` has a profile for, in key order: a
    /// profile of button overrides, of chord entries, or of both. Chord entries
    /// count only on a platform that runs them, so elsewhere they make no
    /// profile the user could not see the contents of.
    pub fn app_profiles(&self, device_key: &str) -> impl Iterator<Item = &str> {
        self.devices.get(device_key).into_iter().flat_map(|device| {
            let chords = device.per_app_chords.keys().filter(|_| chords_supported());
            device
                .per_app_bindings
                .keys()
                .chain(chords)
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
        })
    }

    /// Drop `device_key`'s whole profile for `app`, reporting whether there was
    /// one. Chord entries go with it on a platform that runs them, and stay
    /// untouched elsewhere, where they are not shown.
    pub fn remove_app_profile(&mut self, device_key: &str, app: &str) -> bool {
        let Some(device) = self.devices.get_mut(device_key) else {
            return false;
        };
        let buttons = device.per_app_bindings.remove(app).is_some();
        let chords = chords_supported() && device.per_app_chords.remove(app).is_some();
        buttons || chords
    }

    /// Whether `device_key` has a non-empty per-app overlay of bindings or, on
    /// a platform that runs them, chords for the foreground app `app`. Drives
    /// the active-profile badge: when the current app has its own overlay for
    /// this device, the global profile is (partly) overridden.
    #[must_use]
    pub fn has_app_override(&self, device_key: &str, app: &str) -> bool {
        self.has_app_chords(device_key, app)
            || self.devices.get(device_key).is_some_and(|d| {
                overlay_for(&d.per_app_bindings, &self.apps, app)
                    .is_some_and(|overlay| !overlay.is_empty())
            })
    }

    /// Whether `device_key` has a chord overlay for the foreground app `app`,
    /// on a platform that runs chords. That changes what a held button resolves
    /// to, as a binding overlay does.
    #[must_use]
    pub fn has_app_chords(&self, device_key: &str, app: &str) -> bool {
        chords_supported()
            && self.devices.get(device_key).is_some_and(|d| {
                overlay_for(&d.per_app_chords, &self.apps, app)
                    .is_some_and(|overlay| !overlay.is_empty())
            })
    }

    /// The key of the profile `device_key` runs the foreground application
    /// `app` under, matched as [`Self::effective_bindings`] matches it — the
    /// exact key, an `exe:<filename>` key, or an `app:<alias>` key — or `None`
    /// when the global bindings apply.
    #[must_use]
    pub fn app_profile_for(&self, device_key: &str, app: &str) -> Option<&str> {
        profile_key_for(
            &self.devices.get(device_key)?.per_app_bindings,
            &self.apps,
            app,
        )
    }

    /// The first per-app profile, by device and then key, whose `app:` key
    /// names an alias [`Self::apps`] does not define, as `(device, key)`.
    /// Such a profile could never apply, so loading rejects it.
    #[cfg(feature = "fs")]
    pub(super) fn undefined_app_alias(&self) -> Option<(&str, &str)> {
        self.devices.iter().find_map(|(device_key, device)| {
            device
                .per_app_bindings
                .keys()
                .chain(device.per_app_chords.keys())
                .chain(device.action_ring.per_app.keys())
                .find(|key| self.apps.is_undefined_reference(key))
                .map(|key| (device_key.as_str(), key.as_str()))
        })
    }
}
