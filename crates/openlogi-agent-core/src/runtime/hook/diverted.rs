//! The buttons HID++ capture sessions have taken over, so the OS hook can tell
//! an echo of a diverted press from a press only the hook sees (#2158), and a
//! press from the session's device from one from another mouse.
//!
//! Diverting a `0x1b04` control moves its presses out of the native HID report
//! and into HID++ notifications, so the OS normally never sees them. Linux
//! 7.3's `hid-logitech-hidpp` reads those notifications as well, and for a
//! Bluetooth Signature M650 it reports a diverted Back/Forward as evdev
//! buttons. The hook then receives a press the capture session has already
//! dispatched.

use std::collections::{BTreeSet, HashMap};

use openlogi_core::binding::ButtonId;
use openlogi_hid::DeviceRoute;
use openlogi_hook::EventDevice;

use crate::runtime::HidppSessionId;

/// The vendor and product ids a directly connected device reports both on its
/// HID++ route and as an OS input source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DirectDevice {
    vendor_id: u16,
    product_id: u16,
}

impl DirectDevice {
    /// The OS input identity of `route`. Only a direct route names one: a
    /// receiver route names the receiver and a slot, not the ids the OS
    /// reports for the paired device.
    fn of(route: &DeviceRoute) -> Option<Self> {
        match *route {
            DeviceRoute::Direct {
                vendor_id,
                product_id,
            } => Some(Self {
                vendor_id,
                product_id,
            }),
            _ => None,
        }
    }

    /// Whether `source` is this device's OS input source. Two connected units
    /// of one model report the same ids, so they cannot be told apart here.
    fn is_source_of(self, source: &EventDevice) -> bool {
        source.vendor_id == Some(u32::from(self.vendor_id))
            && source.product_id == Some(u32::from(self.product_id))
    }
}

/// The OS-hook buttons one capture session diverted on one device.
#[derive(Debug)]
struct Diversion {
    /// Config namespace of the device, which every route has.
    device_key: String,
    /// OS input identity of the device, which only a direct route names.
    direct: Option<DirectDevice>,
    buttons: BTreeSet<ButtonId>,
}

/// The OS-hook buttons each live capture session has diverted, keyed by its
/// capture epoch. A session reports what arming wrote
/// ([`CapturedInput::ButtonDiverted`](openlogi_hid::CapturedInput::ButtonDiverted)),
/// and its entry is released when its task ends.
#[derive(Debug, Default)]
pub(crate) struct DivertedButtons(HashMap<u64, Diversion>);

impl DivertedButtons {
    /// Record that `session` diverted `button` on `route`. Only an OS-hook
    /// button can reach the hook, so any other diversion is not recorded.
    pub(crate) fn record(
        &mut self,
        session: &HidppSessionId,
        route: &DeviceRoute,
        button: ButtonId,
    ) {
        if !button.is_os_hook_button() {
            return;
        }
        self.0
            .entry(session.epoch())
            .or_insert_with(|| Diversion {
                device_key: session.device_key().to_owned(),
                direct: DirectDevice::of(route),
                buttons: BTreeSet::new(),
            })
            .buttons
            .insert(button);
    }

    /// Forget `session`'s diversions once its task has ended. Any capture
    /// epoch, current or stale, releases only its own entry.
    pub(crate) fn release(&mut self, session: &HidppSessionId) {
        self.0.remove(&session.epoch());
    }

    /// Whether a live capture session has diverted `button` on `source`, so
    /// that an OS edge for it is an echo of a press the session reports.
    pub(crate) fn owns(&self, source: &EventDevice, button: ButtonId) -> bool {
        self.0.values().any(|diversion| {
            diversion.buttons.contains(&button)
                && diversion
                    .direct
                    .is_some_and(|device| device.is_source_of(source))
        })
    }

    /// Whether the device `device_key` names reports `button` over HID++, so
    /// that a press the hook sees that is not an echo came from another mouse.
    pub(crate) fn reports_over_hidpp(&self, device_key: &str, button: ButtonId) -> bool {
        self.0.values().any(|diversion| {
            diversion.device_key == device_key && diversion.buttons.contains(&button)
        })
    }
}
