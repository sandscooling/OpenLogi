//! Fixed binding inspector for the Buttons workspace.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use gpui::{
    Context, Entity, FocusHandle, InteractiveElement, IntoElement, ParentElement, Role,
    StatefulInteractiveElement as _, Styled, div, prelude::FluentBuilder as _, px, rgb, svg,
};
use gpui_base::Button as BaseButton;
use gpui_component::{
    Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::InputState,
    scroll::ScrollableElement as _,
    v_flex,
};
use openlogi_core::binding::{Action, ButtonId, ChordId, GestureDirection, default_binding};
use openlogi_core::config::ChordWindow;

use super::force_sensing::{PressForce, force_sensing_row};
use super::hotspots::{Hotspot, MouseControlId};
use super::thumbwheel::ThumbwheelPreset;
use super::view::MouseModelView;
use crate::features::binding_editor::{
    ActionOffer, GESTURE_BUTTON_ICON, PickFn, action_icon_path, action_rows_with,
    application_editor, command_editor, editor_section, gesture_direction_icon,
};
use crate::features::profiles::app_profile_summary;
use crate::state::{AppState, ChordEntry, ChordStanding};
use crate::ui::action::localized_action_label;
use crate::ui::components::{MenuRow, Toggle, control_button, control_input, toggle_row};
use crate::ui::theme::{self, ACCENT_BLUE, Palette, Typography as _};

pub(super) const INSPECTOR_W: f32 = 328.;

#[derive(Clone, Copy)]
pub(super) struct BindingInspectorData<'a> {
    pub selected: Option<MouseControlId>,
    pub gesture_direction: Option<GestureDirection>,
    pub bindings: &'a BTreeMap<ButtonId, Action>,
    pub gesture_maps: &'a BTreeMap<ButtonId, BTreeMap<GestureDirection, Action>>,
    pub dpi_gestures: bool,
    /// The device reports raw wheel on Back/Forward, so holding one can
    /// scroll sideways (`Capabilities::virtual_thumbwheel`).
    pub virtual_thumbwheel: bool,
    /// Buttons that hold the wheel to scroll sideways, from the global profile.
    pub wheel_hold: &'a BTreeSet<ButtonId>,
    pub editing_app: Option<&'a str>,
    pub overridden: Option<&'a BTreeMap<ButtonId, Action>>,
    /// The Chords section's facts, absent where chords cannot be edited.
    pub chords: Option<ChordData<'a>>,
    /// How many chord entries the open per-app profile stores.
    pub chord_overrides: usize,
    /// The haptic panel's press-force row, present only while
    /// [`ForceSensingFacts::current`](super::force_sensing::ForceSensingFacts::current)
    /// says it shows.
    pub press_force: Option<PressForce<'a>>,
    pub extra_actions: &'a [Action],
    /// Catalog actions the picker leaves out.
    pub hidden_actions: &'a [Action],
}

/// What the Chords section reads.
#[derive(Clone, Copy)]
pub(super) struct ChordData<'a> {
    /// The open profile's chords, whichever button is selected.
    pub entries: &'a [ChordEntry],
    /// The device's controls that can be in a chord.
    pub controls: &'a BTreeSet<ButtonId>,
    pub window: ChordWindow,
}

/// What the inspector's one picker is choosing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum InspectorPicker {
    #[default]
    Closed,
    /// An action for the selected button or gesture direction.
    Action,
    /// The second button of a new chord.
    ChordPartner,
    /// The action of a chord, which is saved only once one is picked.
    ChordAction(ChordId),
}

#[derive(Clone, Copy)]
pub(super) struct ActionPickerContext<'a> {
    pub target: InspectorPicker,
    pub search: &'a Entity<InputState>,
    pub shortcut_input: &'a Entity<InputState>,
    pub application_input: &'a Entity<InputState>,
    pub command_input: &'a Entity<InputState>,
    pub shortcut_mode: ShortcutMode,
    /// Whether Record is waiting for the next key press.
    pub shortcut_recording: bool,
    /// The Record button's area, which holds focus while recording.
    pub shortcut_record_focus: &'a FocusHandle,
    pub invalid: InvalidDrafts,
    pub view: &'a Entity<MouseModelView>,
}

impl ActionPickerContext<'_> {
    /// Whether the picker is choosing the selected control's own action.
    fn open(&self) -> bool {
        self.target == InspectorPicker::Action
    }
}

/// Which custom-action editors rejected their last Add, so each shows an
/// inline error until its text changes or the picker reopens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct InvalidDrafts {
    pub shortcut: Option<ShortcutError>,
    pub application: bool,
    pub command: bool,
}

/// Why the custom shortcut editor shows an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShortcutError {
    /// The typed text, or the recorded key, is not a chord a shortcut can
    /// send.
    Unrecognized,
    /// Record saw Shift with a digit or symbol key, which it cannot record
    /// (see [`Recording::ShiftedSymbol`]). Typing the chord still works.
    ///
    /// [`Recording::ShiftedSymbol`]: crate::features::binding_editor::Recording::ShiftedSymbol
    ShiftedSymbol,
}

/// Whether the custom shortcut editor records a tapped or a held chord.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShortcutMode {
    Tap,
    Hold,
}

impl ShortcutMode {
    fn action(self, combo: openlogi_core::binding::KeyCombo) -> Action {
        match self {
            Self::Tap => Action::CustomShortcut(combo),
            Self::Hold => Action::HoldShortcut(combo),
        }
    }
}

pub(super) fn binding_inspector(
    data: BindingInspectorData<'_>,
    picker: ActionPickerContext<'_>,
    cx: &Context<MouseModelView>,
) -> gpui::Div {
    let pal = theme::palette(cx);
    let body = match data.selected {
        None => empty_inspector(
            data.editing_app,
            data.overridden.map_or(0, BTreeMap::len),
            data.chord_overrides,
            pal,
        ),
        Some(MouseControlId::ThumbwheelRotation) => thumbwheel_inspector(
            data.bindings,
            data.editing_app,
            data.overridden,
            picker,
            pal,
        ),
        Some(MouseControlId::Button(button)) => button_inspector(button, &data, picker, pal, cx),
    };

    v_flex()
        .debug_selector(|| "button-inspector".into())
        .w(px(INSPECTOR_W))
        .h_full()
        .min_h_0()
        .flex_shrink_0()
        .border_l_1()
        .border_color(pal.border)
        .bg(pal.panel)
        .child(
            div()
                .id("button-inspector-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .p_4()
                .child(body),
        )
}

fn empty_inspector(
    app: Option<&str>,
    override_count: usize,
    chord_count: usize,
    pal: Palette,
) -> gpui::Div {
    let summary = app.map_or_else(
        || tr!("profiles.select_device_button_description"),
        |app| app_profile_summary(app, override_count, chord_count),
    );
    v_flex()
        .gap_3()
        .child(inspector_heading(
            tr!("actions.button_inspector"),
            None,
            pal,
        ))
        .child(div().text_body().text_color(pal.text_muted).child(summary))
}

fn button_inspector(
    button: ButtonId,
    data: &BindingInspectorData<'_>,
    picker: ActionPickerContext<'_>,
    pal: Palette,
    cx: &Context<MouseModelView>,
) -> gpui::Div {
    let gesture_map = data.gesture_maps.get(&button);
    let overridden = data
        .overridden
        .is_some_and(|overrides| overrides.contains_key(&button));
    if data.editing_app.is_none()
        && let Some(gesture_map) = gesture_map
    {
        return gesture_inspector(button, gesture_map, data, picker, pal, cx);
    }
    if let Some(app) = data.editing_app
        && !overridden
        && gesture_map.is_some()
    {
        return inherited_gesture_inspector(button, app, data, picker, pal, cx);
    }

    let action = data
        .bindings
        .get(&button)
        .cloned()
        .unwrap_or_else(|| default_binding(button));
    let observer = picker.view.clone();
    let on_pick: PickFn = Rc::new(move |action, _window, cx| {
        AppState::apply(cx, |state| state.commit_binding(button, action));
        observer.update(cx, |view, cx| {
            view.close_action_picker();
            cx.notify();
        });
    });

    v_flex()
        .gap_3()
        .child(inspector_heading(
            tr!(button.translation_key()),
            Some(button_status(button, &action, data, overridden)),
            pal,
        ))
        .child(current_action_card(&action, picker, pal))
        .children(wheel_hold_section(
            button,
            data,
            overridden,
            picker.view,
            pal,
        ))
        .when(overridden, |panel| {
            panel.child(use_default_button(button, picker.view))
        })
        .when(can_enable_gestures(button, data.editing_app), |panel| {
            let observer = picker.view.clone();
            let unavailable = button == ButtonId::DpiToggle && !data.dpi_gestures;
            panel
                .child(
                    control_button("inspector-use-gestures")
                        .w_full()
                        .icon(Icon::empty().path(GESTURE_BUTTON_ICON))
                        .label(tr!("actions.use_gestures"))
                        .disabled(unavailable)
                        .on_click(move |_, _, cx| {
                            AppState::apply(cx, |state| state.commit_gesture_mode(button, true));
                            observer.update(cx, |view, cx| {
                                view.set_gesture_selected_dir(Some(GestureDirection::Click));
                                cx.notify();
                            });
                        }),
                )
                .when(unavailable, |panel| {
                    panel.child(
                        div()
                            .text_body()
                            .text_color(pal.text_muted)
                            .child(tr!("actions.dpi_gestures_unavailable")),
                    )
                })
        })
        .children(
            data.press_force
                .map(|press_force| force_sensing_row(press_force, pal)),
        )
        .children(chords_section(button, data, picker, pal, cx))
        .when(picker.open(), |panel| {
            panel.child(action_library(
                "inspector-action",
                Some(&action),
                picker,
                ActionOffer {
                    extra: data.extra_actions,
                    hidden: data.hidden_actions,
                },
                &on_pick,
                pal,
                cx,
            ))
        })
}

/// Drops the open app profile's override of `button`.
fn use_default_button(button: ButtonId, view: &Entity<MouseModelView>) -> impl IntoElement {
    let observer = view.clone();
    control_button("inspector-use-default")
        .w_full()
        .icon(IconName::Undo)
        .label(tr!("profiles.use_the_default_profile"))
        .on_click(move |_, _, cx| {
            AppState::apply(cx, |state| state.clear_app_binding(button));
            observer.update(cx, |view, cx| {
                view.close_action_picker();
                cx.notify();
            });
        })
}

/// The heading's status line. A button that holds the wheel is customized
/// even while its press keeps the default action.
fn button_status(
    button: ButtonId,
    action: &Action,
    data: &BindingInspectorData<'_>,
    overridden: bool,
) -> gpui::SharedString {
    let default = *action == default_binding(button) && !data.wheel_hold.contains(&button);
    match (data.editing_app, overridden, default) {
        (Some(app), true, _) => tr!("actions.overridden_in_app", app => app.to_string()),
        (Some(_), false, _) => tr!("profiles.inherited_from_default"),
        (None, _, true) => tr!("pointer.device_default"),
        (None, _, false) => tr!("profiles.customized"),
    }
}

/// Back/Forward's wheel-hold control: the switch in the global profile, or,
/// in an app profile that has not overridden the button, a note that an
/// action picked there replaces it.
fn wheel_hold_section(
    button: ButtonId,
    data: &BindingInspectorData<'_>,
    overridden: bool,
    view: &Entity<MouseModelView>,
    pal: Palette,
) -> Option<gpui::AnyElement> {
    let enabled = data.wheel_hold.contains(&button);
    match data.editing_app {
        None if button.supports_wheel_hold() && (data.virtual_thumbwheel || enabled) => {
            Some(wheel_hold_row(button, enabled, view, pal).into_any_element())
        }
        Some(app) if enabled && !overridden => Some(
            div()
                .debug_selector(|| "inspector-wheel-hold-app-note".into())
                .text_caption()
                .text_color(pal.text_muted)
                .child(tr!(
                    "actions.app_profile_action_replaces_wheel_hold",
                    app => app.to_string()
                ))
                .into_any_element(),
        ),
        _ => None,
    }
}

/// Back/Forward's wheel-hold switch. Shown only in the global profile: a
/// per-app entry holds one action, so the mode is device-wide. Turning it
/// off stays possible on a device that does not report raw wheel, so a
/// binding carried over from another mouse can be removed.
///
/// The gesture inspector offers it too. Turning it on there leaves gesture
/// mode, stashing the direction map, so the selected direction is cleared
/// as "Use a single action" does.
fn wheel_hold_row(
    button: ButtonId,
    enabled: bool,
    view: &Entity<MouseModelView>,
    pal: Palette,
) -> impl IntoElement {
    let observer = view.clone();
    toggle_row(
        "inspector-wheel-hold-row",
        pal,
        tr!("actions.hold_to_scroll_sideways"),
        tr!("actions.hold_to_scroll_sideways_description"),
        Toggle::new("inspector-wheel-hold-toggle")
            .selected(enabled)
            .on_change(move |enabled, _window, cx| {
                AppState::apply(cx, |state| state.commit_wheel_hold(button, *enabled));
                if *enabled {
                    observer.update(cx, |view, cx| {
                        view.set_gesture_selected_dir(None);
                        cx.notify();
                    });
                }
            }),
    )
}

fn inherited_gesture_inspector(
    button: ButtonId,
    app: &str,
    data: &BindingInspectorData<'_>,
    picker: ActionPickerContext<'_>,
    pal: Palette,
    cx: &Context<MouseModelView>,
) -> gpui::Div {
    let observer = picker.view.clone();
    let on_pick: PickFn = Rc::new(move |action, _window, cx| {
        AppState::apply(cx, |state| state.commit_binding(button, action));
        observer.update(cx, |view, cx| {
            view.close_action_picker();
            cx.notify();
        });
    });
    let edit_default = picker.view.clone();
    v_flex()
        .gap_3()
        .child(inspector_heading(
            tr!(button.translation_key()),
            Some(tr!("profiles.inherited_from_default")),
            pal,
        ))
        .child(gesture_summary_card(picker, pal))
        .child(div().text_caption().text_color(pal.text_muted).child(tr!(
            "actions.app_profile_action_replaces_gestures",
            app => app.to_string()
        )))
        .child(
            Button::new("inspector-edit-default-gestures")
                .small()
                .w_full()
                .label(tr!("actions.edit_default_gestures"))
                .on_click(move |_, _, cx| {
                    AppState::apply(cx, |state| state.set_editing_app(None));
                    edit_default.update(cx, |view, cx| {
                        view.set_gesture_selected_dir(Some(GestureDirection::Click));
                        cx.notify();
                    });
                }),
        )
        .children(chords_section(button, data, picker, pal, cx))
        .when(picker.open(), |panel| {
            panel.child(action_library(
                "inspector-gesture-override",
                None,
                picker,
                // The pick replaces the gestures with a button binding,
                // which gets a release, so held actions fit here.
                ActionOffer {
                    extra: data.extra_actions,
                    hidden: data.hidden_actions,
                },
                &on_pick,
                pal,
                cx,
            ))
        })
}

fn gesture_inspector(
    button: ButtonId,
    gesture_map: &BTreeMap<GestureDirection, Action>,
    data: &BindingInspectorData<'_>,
    picker: ActionPickerContext<'_>,
    pal: Palette,
    cx: &Context<MouseModelView>,
) -> gpui::Div {
    let direction = data.gesture_direction.unwrap_or(GestureDirection::Click);
    // Gesture mode never holds the wheel, so the switch here is off and only
    // turning it on is useful, which needs measured raw wheel.
    let offer_wheel_hold = button.supports_wheel_hold() && data.virtual_thumbwheel;
    let current = gesture_action(gesture_map, button, direction);
    let observer = picker.view.clone();
    let on_pick: PickFn = Rc::new(move |action, _window, cx| {
        AppState::apply(cx, |state| {
            state.commit_gesture_binding(button, direction, action)
        });
        observer.update(cx, |view, cx| {
            view.close_action_picker();
            cx.notify();
        });
    });
    let turn_off = picker.view.clone();

    v_flex()
        .gap_3()
        .child(inspector_heading(
            tr!(button.translation_key()),
            Some(tr!("actions.five_directions")),
            pal,
        ))
        .child(gesture_directions(
            direction,
            gesture_map,
            button,
            picker.view,
            pal,
        ))
        .child(current_action_card(&current, picker, pal))
        .when(offer_wheel_hold, |panel| {
            panel.child(wheel_hold_row(button, false, picker.view, pal))
        })
        .child(
            control_button("inspector-single-action")
                .w_full()
                .label(tr!("actions.use_a_single_action"))
                .on_click(move |_, _, cx| {
                    AppState::apply(cx, |state| state.commit_gesture_mode(button, false));
                    turn_off.update(cx, |view, cx| {
                        view.set_gesture_selected_dir(None);
                        cx.notify();
                    });
                }),
        )
        .children(
            data.press_force
                .map(|press_force| force_sensing_row(press_force, pal)),
        )
        .children(chords_section(button, data, picker, pal, cx))
        .when(picker.open(), |panel| {
            panel.child(action_library(
                "inspector-gesture-action",
                Some(&current),
                picker,
                ActionOffer {
                    extra: &[],
                    hidden: data.hidden_actions,
                },
                &on_pick,
                pal,
                cx,
            ))
        })
}

fn gesture_directions(
    active: GestureDirection,
    gesture_map: &BTreeMap<GestureDirection, Action>,
    button: ButtonId,
    view: &Entity<MouseModelView>,
    pal: Palette,
) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(editor_section(tr!("actions.direction"), pal))
        .children(
            GestureDirection::ALL
                .into_iter()
                .enumerate()
                .map(|(index, direction)| {
                    let selected = direction == active;
                    let action = gesture_action(gesture_map, button, direction);
                    let view = view.clone();
                    let aria_label = format!(
                        "{}: {}",
                        tr!(direction.translation_key()),
                        localized_action_label(&action)
                    );
                    MenuRow::new(("inspector-direction", index))
                        .selected(selected)
                        .role(Role::Button)
                        .aria_label(aria_label)
                        .child(
                            h_flex()
                                .min_w_0()
                                .gap_2()
                                // `.size_4()` is not decoration: a bare `Icon`
                                // falls through to the current font size, which
                                // would leave these a step under the 16px leading
                                // column the action rows below use.
                                .child(gesture_direction_icon(direction).size_4())
                                .child(
                                    v_flex()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .text_body()
                                                .child(tr!(direction.translation_key())),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_caption()
                                                .text_color(pal.text_muted)
                                                .child(localized_action_label(&action)),
                                        ),
                                ),
                        )
                        .when(selected, |row| {
                            row.child(
                                Icon::new(IconName::Check)
                                    .size_3()
                                    .text_color(rgb(ACCENT_BLUE)),
                            )
                        })
                        .on_click(move |_, _, cx| {
                            view.update(cx, |view, cx| {
                                view.set_gesture_selected_dir(Some(direction));
                                cx.notify();
                            });
                        })
                }),
        )
}

/// The controls a chord can name on this device: the ones it draws, plus a
/// haptic panel its measured controls report, which a device image with no
/// marker for it cannot show.
pub(super) fn chord_controls(hotspots: &[Hotspot], haptic_panel: bool) -> BTreeSet<ButtonId> {
    hotspots
        .iter()
        .filter_map(|hotspot| hotspot.id.button())
        .chain(haptic_panel.then_some(ButtonId::HapticPanel))
        .filter(|button| button.supports_chord())
        .collect()
}

/// The selected button's chords in the open profile, and a way to add one. Each
/// member reads the same entries, so a chord edited from one is the chord the
/// other shows.
fn chords_section(
    button: ButtonId,
    data: &BindingInspectorData<'_>,
    picker: ActionPickerContext<'_>,
    pal: Palette,
    cx: &Context<MouseModelView>,
) -> Option<gpui::Div> {
    let chords = data.chords.filter(|_| button.supports_chord())?;
    let rows: Vec<(ButtonId, &ChordEntry)> = chords
        .entries
        .iter()
        .filter_map(|entry| Some((entry.id.partner_of(button)?, entry)))
        .collect();
    let partners: Vec<ButtonId> = chords
        .controls
        .iter()
        .copied()
        .filter(|control| *control != button && !rows.iter().any(|(partner, _)| partner == control))
        .collect();
    if rows.is_empty() && partners.is_empty() {
        return None;
    }
    let choosing_partner = picker.target == InspectorPicker::ChordPartner;
    let editing = match picker.target {
        InspectorPicker::ChordAction(chord) => Some(chord),
        _ => None,
    };
    // A chord that cannot run is not diverted, so it costs its buttons nothing.
    let mut members: BTreeSet<ButtonId> = rows
        .iter()
        .filter(|(_, entry)| entry.deliverable)
        .flat_map(|(_, entry)| entry.id.members())
        .chain(editing.into_iter().flat_map(ChordId::members))
        .collect();
    if choosing_partner {
        members.insert(button);
    }
    let app = data.editing_app.unwrap_or_default();

    Some(
        v_flex()
            .debug_selector(|| "inspector-chords".into())
            .gap_1()
            .child(editor_section(tr!("actions.chords"), pal))
            .children(
                rows.iter()
                    .map(|(partner, entry)| chord_row(*partner, entry, app, picker, pal)),
            )
            .when(!partners.is_empty(), |section| {
                let view = picker.view.clone();
                section.child(
                    control_button("inspector-add-chord")
                        .debug_selector(|| "inspector-add-chord".into())
                        .w_full()
                        .icon(IconName::Plus)
                        .label(tr!("actions.add_chord"))
                        .selected(choosing_partner)
                        .on_click(move |_, _, cx| {
                            view.update(cx, |view, cx| {
                                view.toggle_chord_partner();
                                cx.notify();
                            });
                        }),
                )
            })
            .when(choosing_partner, |section| {
                section.child(chord_partner_list(button, &partners, picker.view, pal))
            })
            .children(chord_notes(&members, chords.window, pal))
            .children(editing.and_then(|chord| {
                let partner = chord.partner_of(button)?;
                let current = rows
                    .iter()
                    .find(|(_, entry)| {
                        entry.id == chord && entry.standing != ChordStanding::TurnedOff
                    })
                    .map(|(_, entry)| &entry.action);
                Some(chord_action_library(
                    chord,
                    partner,
                    current,
                    data.hidden_actions,
                    picker,
                    pal,
                    cx,
                ))
            })),
    )
}

/// One chord: the row opens its action, and the trailing button leaves it, in
/// the way the open profile can. An application cannot remove a Default chord,
/// only turn it off there, so an inherited chord offers that instead.
fn chord_row(
    partner: ButtonId,
    entry: &ChordEntry,
    app: &str,
    picker: ActionPickerContext<'_>,
    pal: Palette,
) -> impl IntoElement {
    let chord = entry.id;
    let name = tr!("actions.chord_with", button => tr!(partner.translation_key()));
    let action = match entry.standing {
        ChordStanding::TurnedOff => tr!("actions.chord_off_in_app", app => app.to_string()),
        _ => localized_action_label(&entry.action),
    };
    let status = match entry.standing {
        ChordStanding::Default | ChordStanding::TurnedOff => None,
        ChordStanding::Inherited => Some(tr!("profiles.inherited_from_default")),
        ChordStanding::Overridden => Some(tr!("actions.overridden_in_app", app => app.to_string())),
        ChordStanding::AppOnly => Some(tr!("actions.chord_only_in_app", app => app.to_string())),
    };
    let accessible_label = format!("{name}: {action}");
    let view = picker.view.clone();
    h_flex()
        .w_full()
        .items_center()
        .gap_1()
        .child(
            div().flex_1().min_w_0().child(
                MenuRow::new(format!("inspector-chord-row-{partner:?}"))
                    .debug_selector(|| format!("inspector-chord-row-{partner:?}"))
                    .selected(picker.target == InspectorPicker::ChordAction(chord))
                    .role(Role::Button)
                    .aria_label(accessible_label)
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(div().truncate().text_body().child(name))
                            .child(
                                div()
                                    .truncate()
                                    .text_caption()
                                    .text_color(pal.text_muted)
                                    .child(action),
                            )
                            .children(status.map(|status| {
                                div()
                                    .truncate()
                                    .text_caption()
                                    .text_color(pal.text_muted)
                                    .child(status)
                            }))
                            .when(!entry.deliverable, |row| {
                                row.child(
                                    div()
                                        .debug_selector(|| {
                                            format!("inspector-chord-unavailable-{partner:?}")
                                        })
                                        .text_caption()
                                        .text_color(pal.text_muted)
                                        .child(tr!("actions.chord_unavailable")),
                                )
                            }),
                    )
                    .on_click(move |_, window, cx| {
                        view.update(cx, |view, cx| {
                            view.toggle_chord_action(chord, window, cx);
                        });
                    }),
            ),
        )
        .child(chord_row_action(partner, entry, app, picker.view))
}

/// The row's trailing button: remove the chord from this profile, turn off an
/// inherited one, or go back to the Default profile's.
fn chord_row_action(
    partner: ButtonId,
    entry: &ChordEntry,
    app: &str,
    view: &Entity<MouseModelView>,
) -> impl IntoElement {
    let chord = entry.id;
    let (name, icon, label) = match entry.standing {
        ChordStanding::Default | ChordStanding::AppOnly => (
            "remove",
            Icon::new(IconName::Close),
            tr!("actions.remove_chord"),
        ),
        ChordStanding::Inherited => (
            "turn-off",
            Icon::empty().path("action-icons/ban.svg"),
            tr!("actions.turn_off_chord_in_app", app => app.to_string()),
        ),
        ChordStanding::Overridden | ChordStanding::TurnedOff => (
            "use-default",
            Icon::new(IconName::Undo),
            tr!("profiles.use_the_default_profile"),
        ),
    };
    let turn_off = entry.standing == ChordStanding::Inherited;
    let view = view.clone();
    control_button(format!("inspector-chord-{name}-{partner:?}"))
        .debug_selector(move || format!("inspector-chord-{name}-{partner:?}"))
        .ghost()
        .flex_none()
        .icon(icon)
        .tooltip(label)
        .on_click(move |_, _, cx| {
            AppState::apply(cx, |state| {
                if turn_off {
                    state.commit_chord(chord, Action::None)
                } else {
                    state.clear_chord(chord)
                }
            });
            view.update(cx, |view, cx| {
                view.close_action_picker();
                cx.notify();
            });
        })
}

/// The buttons a new chord can pair with. Picking one saves nothing until an
/// action is chosen.
fn chord_partner_list(
    button: ButtonId,
    partners: &[ButtonId],
    view: &Entity<MouseModelView>,
    pal: Palette,
) -> impl IntoElement {
    v_flex()
        .gap_0p5()
        .child(editor_section(tr!("actions.chord_partner_heading"), pal))
        .children(partners.iter().copied().map(|partner| {
            let view = view.clone();
            MenuRow::new(format!("inspector-chord-partner-{partner:?}"))
                .debug_selector(|| format!("inspector-chord-partner-{partner:?}"))
                .role(Role::MenuItem)
                .aria_label(tr!(partner.translation_key()))
                .child(div().child(tr!(partner.translation_key())))
                .on_click(move |_, window, cx| {
                    let Ok(chord) = ChordId::new(button, partner) else {
                        return;
                    };
                    view.update(cx, |view, cx| {
                        view.toggle_chord_action(chord, window, cx);
                    });
                })
        }))
}

/// What being in a chord costs the buttons in `members`, in the words of what
/// the user will notice. Nothing while the button is in no chord.
fn chord_notes(
    members: &BTreeSet<ButtonId>,
    window: ChordWindow,
    pal: Palette,
) -> Option<gpui::Div> {
    if members.is_empty() {
        return None;
    }
    let mut notes = vec![(
        "inspector-chord-note-delay",
        tr!("actions.chord_delay_note", ms => u16::from(window).to_string()),
    )];
    if members.contains(&ButtonId::Back) || members.contains(&ButtonId::Forward) {
        notes.push(("inspector-chord-note-tap", tr!("actions.chord_tap_note")));
    }
    if members.contains(&ButtonId::MiddleClick) {
        notes.push((
            "inspector-chord-note-middle",
            tr!("actions.chord_middle_note"),
        ));
    }
    Some(
        v_flex()
            .gap_1()
            .pt_1()
            .px_2()
            .text_caption()
            .text_color(pal.text_muted)
            .children(
                notes.into_iter().map(|(selector, note)| {
                    div().debug_selector(move || selector.into()).child(note)
                }),
            ),
    )
}

fn chord_action_library(
    chord: ChordId,
    partner: ButtonId,
    current: Option<&Action>,
    hidden: &[Action],
    picker: ActionPickerContext<'_>,
    pal: Palette,
    cx: &Context<MouseModelView>,
) -> impl IntoElement + use<> {
    let observer = picker.view.clone();
    let on_pick: PickFn = Rc::new(move |action, _window, cx| {
        AppState::apply(cx, |state| state.commit_chord(chord, action));
        observer.update(cx, |view, cx| {
            view.close_action_picker();
            cx.notify();
        });
    });
    v_flex()
        .gap_1()
        .child(editor_section(
            tr!("actions.chord_with", button => tr!(partner.translation_key())),
            pal,
        ))
        .child(action_library(
            "inspector-chord-action",
            current,
            picker,
            ActionOffer { extra: &[], hidden },
            &on_pick,
            pal,
            cx,
        ))
}

/// Whether the default-profile inspector may promote `button` into gesture
/// mode. Per-app bindings are single-action overrides, so they cannot carry a
/// direction map.
fn can_enable_gestures(button: ButtonId, editing_app: Option<&str>) -> bool {
    editing_app.is_none() && button.supports_gesture_mode()
}

fn thumbwheel_inspector(
    bindings: &BTreeMap<ButtonId, Action>,
    editing_app: Option<&str>,
    overridden: Option<&BTreeMap<ButtonId, Action>>,
    picker: ActionPickerContext<'_>,
    pal: Palette,
) -> gpui::Div {
    let backward = bindings
        .get(&ButtonId::ThumbwheelScrollDown)
        .cloned()
        .unwrap_or_else(|| default_binding(ButtonId::ThumbwheelScrollDown));
    let forward = bindings
        .get(&ButtonId::ThumbwheelScrollUp)
        .cloned()
        .unwrap_or_else(|| default_binding(ButtonId::ThumbwheelScrollUp));
    let current = ThumbwheelPreset::recognize(&backward, &forward);
    let is_overridden = overridden.is_some_and(|overrides| {
        overrides.contains_key(&ButtonId::ThumbwheelScrollDown)
            || overrides.contains_key(&ButtonId::ThumbwheelScrollUp)
    });
    let status = match (editing_app, is_overridden) {
        (Some(app), true) => tr!("actions.overridden_in_app", app => app.to_string()),
        (Some(_), false) => tr!("profiles.inherited_from_default"),
        (None, _) => tr!("profiles.default_profile"),
    };
    let current_label = current.map_or_else(|| tr!("common.custom"), |p| tr!(p.translation_key()));
    let current_icon = current.map_or("action-icons/chevrons-right.svg", ThumbwheelPreset::icon);
    let observer = picker.view.clone();

    v_flex()
        .gap_3()
        .child(inspector_heading(
            tr!("pointer.thumb_wheel"),
            Some(status),
            pal,
        ))
        .child(selection_card(
            "inspector-current-thumbwheel-preset",
            tr!("common.preset"),
            current_icon,
            current_label,
            picker,
            pal,
        ))
        .when(picker.open(), |panel| {
            panel.child(
                v_flex()
                    .gap_1()
                    .child(editor_section(tr!("common.preset"), pal))
                    .children(ThumbwheelPreset::ALL.into_iter().enumerate().map(
                        |(index, preset)| {
                            let selected = current == Some(preset);
                            let observer = observer.clone();
                            MenuRow::new(("inspector-thumbwheel", index))
                                .selected(selected)
                                .role(Role::Button)
                                .aria_label(tr!(preset.translation_key()))
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            svg()
                                                .path(preset.icon())
                                                .size_4()
                                                .text_color(pal.text_muted),
                                        )
                                        .child(div().child(tr!(preset.translation_key()))),
                                )
                                .when(selected, |row| {
                                    row.child(
                                        Icon::new(IconName::Check)
                                            .size_3()
                                            .text_color(rgb(ACCENT_BLUE)),
                                    )
                                })
                                .on_click(move |_, _, cx| {
                                    AppState::apply(cx, |state| {
                                        state.commit_thumbwheel_preset(preset)
                                    });
                                    observer.update(cx, |view, cx| {
                                        view.close_action_picker();
                                        cx.notify();
                                    });
                                })
                        },
                    )),
            )
        })
        .when(is_overridden, |panel| {
            let observer = picker.view.clone();
            panel.child(
                Button::new("inspector-thumbwheel-use-default")
                    .small()
                    .w_full()
                    .icon(IconName::Undo)
                    .label(tr!("profiles.use_the_default_profile"))
                    .on_click(move |_, _, cx| {
                        AppState::apply(cx, AppState::clear_app_thumbwheel);
                        observer.update(cx, |view, cx| {
                            view.close_action_picker();
                            cx.notify();
                        });
                    }),
            )
        })
}

fn inspector_heading(
    title: gpui::SharedString,
    status: Option<gpui::SharedString>,
    pal: Palette,
) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_heading().child(title))
        .children(status.map(|status| {
            div()
                .text_caption()
                .text_color(pal.text_muted)
                .child(status)
        }))
}

fn current_action_card(
    action: &Action,
    picker: ActionPickerContext<'_>,
    pal: Palette,
) -> impl IntoElement {
    selection_card(
        "inspector-current-action",
        tr!("actions.current_action"),
        action_icon_path(action),
        localized_action_label(action),
        picker,
        pal,
    )
}

fn gesture_summary_card(picker: ActionPickerContext<'_>, pal: Palette) -> impl IntoElement {
    selection_card(
        "inspector-current-gesture-summary",
        tr!("actions.current_action"),
        GESTURE_BUTTON_ICON,
        tr!("actions.five_directions"),
        picker,
        pal,
    )
}

fn selection_card(
    id: &'static str,
    caption: gpui::SharedString,
    icon: &'static str,
    value: gpui::SharedString,
    picker: ActionPickerContext<'_>,
    pal: Palette,
) -> impl IntoElement {
    let toggle = picker.view.clone();
    let search = picker.search.clone();
    let opening = !picker.open();
    let accessible_label = value.clone();
    BaseButton::new(id)
        .debug_selector(move || id.to_string())
        .accessibility_label(accessible_label)
        .aria_expanded(picker.open())
        .flex()
        .flex_col()
        .items_stretch()
        .gap_2()
        .rounded(pal.control_radius)
        .border_1()
        .border_color(pal.border)
        .bg(pal.control)
        .p_3()
        .cursor_pointer()
        .hover(move |card| card.bg(pal.control_hover))
        .focus_visible(move |card| card.bg(pal.control_hover).border_color(rgb(ACCENT_BLUE)))
        .child(
            div()
                .text_caption()
                .text_color(pal.text_muted)
                .child(caption),
        )
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    h_flex()
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .child(
                            svg()
                                .path(icon)
                                .size_4()
                                .flex_none()
                                .text_color(pal.text_muted),
                        )
                        .child(div().min_w_0().truncate().text_body().child(value)),
                )
                .child(
                    svg()
                        .path(if picker.open() {
                            "action-icons/chevrons-up.svg"
                        } else {
                            "action-icons/chevrons-down.svg"
                        })
                        .size_3()
                        .flex_none()
                        .text_color(pal.text_muted),
                ),
        )
        .on_click(move |_, window, cx| {
            if opening {
                search.update(cx, |search, cx| search.set_value("", window, cx));
            }
            toggle.update(cx, |view, cx| {
                if opening {
                    view.clear_custom_action_drafts(window, cx);
                }
                view.toggle_action_picker();
                cx.notify();
            });
        })
}

fn action_library(
    id_prefix: &'static str,
    current: Option<&Action>,
    picker: ActionPickerContext<'_>,
    offer: ActionOffer<'_>,
    on_pick: &PickFn,
    pal: Palette,
    cx: &Context<MouseModelView>,
) -> impl IntoElement {
    let query = picker.search.read(cx).value();
    let rows = action_rows_with(id_prefix, current, &query, offer, on_pick, pal);
    v_flex()
        .gap_2()
        .pt_1()
        .child(custom_shortcut_editor(id_prefix, &picker, on_pick, pal))
        .child(application_editor(
            id_prefix,
            picker.application_input,
            picker.invalid.application,
            Rc::new({
                let view = picker.view.clone();
                move |cx| {
                    view.update(cx, |view, cx| {
                        view.invalid_drafts.application = true;
                        cx.notify();
                    });
                }
            }),
            on_pick,
            pal,
        ))
        .child(command_editor(
            id_prefix,
            picker.command_input,
            picker.invalid.command,
            Rc::new({
                let view = picker.view.clone();
                move |cx| {
                    view.update(cx, |view, cx| {
                        view.invalid_drafts.command = true;
                        cx.notify();
                    });
                }
            }),
            on_pick,
            pal,
        ))
        .child(editor_section(tr!("actions.actions"), pal))
        .child(control_input(picker.search).cleanable(true))
        .child(
            v_flex()
                .gap_0p5()
                .when(rows.is_empty(), |list| {
                    list.child(
                        div()
                            .py_3()
                            .text_body()
                            .text_color(pal.text_muted)
                            .child(tr!("actions.no_actions_found")),
                    )
                })
                .children(rows),
        )
}

/// A single-field "Custom Shortcut" editor, matching the Action Ring editor's
/// `shortcut_editor` (`features/action_ring/editor.rs`) so the same custom
/// action is reachable from the plain per-button picker, not just the ring.
/// Record fills the field from the next key press; Add commits the field.
fn custom_shortcut_editor(
    id_prefix: &'static str,
    picker: &ActionPickerContext<'_>,
    on_pick: &PickFn,
    pal: Palette,
) -> impl IntoElement {
    let input = picker.shortcut_input;
    let mode = picker.shortcut_mode;
    let recording = picker.shortcut_recording;
    let view = picker.view;
    let submit_input = input.clone();
    let on_pick = on_pick.clone();
    let mode_button = |option: ShortcutMode| {
        let (id, label) = match option {
            ShortcutMode::Tap => ("tap", tr!("actions.shortcut_tap")),
            ShortcutMode::Hold => ("hold", tr!("actions.shortcut_hold")),
        };
        let view = view.clone();
        control_button(format!("{id_prefix}-shortcut-{id}"))
            .debug_selector(move || format!("{id_prefix}-shortcut-{id}"))
            .selected(mode == option)
            .label(label)
            .on_click(move |_, _, cx| {
                view.update(cx, |view, cx| {
                    view.shortcut_mode = option;
                    cx.notify();
                });
            })
    };
    let tap_button = mode_button(ShortcutMode::Tap);
    let hold_button = mode_button(ShortcutMode::Hold);
    let record_view = view.clone();
    let view = view.clone();
    v_flex()
        .gap_1()
        .child(editor_section(tr!("action_ring.custom_shortcut"), pal))
        .child(h_flex().gap_2().child(tap_button).child(hold_button))
        .child(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .debug_selector(move || format!("{id_prefix}-custom-shortcut-input"))
                        .child(control_input(input).cleanable(true)),
                )
                .child(
                    div().track_focus(picker.shortcut_record_focus).child(
                        Button::new(format!("{id_prefix}-custom-shortcut-record"))
                            .debug_selector(move || format!("{id_prefix}-custom-shortcut-record"))
                            .compact()
                            .selected(recording)
                            .label(if recording {
                                tr!("common.cancel")
                            } else {
                                tr!("actions.record_shortcut")
                            })
                            .on_click(move |_, window, cx| {
                                record_view.update(cx, |view, cx| {
                                    view.toggle_shortcut_recording(window, cx);
                                });
                            }),
                    ),
                )
                .child(
                    Button::new(format!("{id_prefix}-custom-shortcut-add"))
                        .debug_selector(move || format!("{id_prefix}-custom-shortcut-add"))
                        .compact()
                        .label(tr!("common.add"))
                        .on_click(move |_, window, cx| {
                            let shortcut = submit_input.read(cx).value().to_string();
                            match shortcut.parse::<openlogi_core::binding::KeyCombo>() {
                                Ok(combo) => (on_pick)(mode.action(combo), window, cx),
                                Err(_) => view.update(cx, |view, cx| {
                                    view.invalid_drafts.shortcut =
                                        Some(ShortcutError::Unrecognized);
                                    cx.notify();
                                }),
                            }
                        }),
                ),
        )
        .when(recording, |editor| {
            editor.child(
                div()
                    .debug_selector(move || format!("{id_prefix}-custom-shortcut-recording"))
                    .text_caption()
                    .text_color(pal.text_muted)
                    .child(tr!("actions.press_the_shortcut_to_record")),
            )
        })
        .when_some(picker.invalid.shortcut, |editor, error| {
            editor.child(
                div()
                    .debug_selector(move || format!("{id_prefix}-custom-shortcut-error"))
                    .text_caption()
                    .text_color(rgb(0x00ef_4444))
                    .child(match error {
                        ShortcutError::Unrecognized => {
                            tr!("action_ring.custom_action_invalid_input")
                        }
                        ShortcutError::ShiftedSymbol => tr!("actions.shifted_symbol_unrecordable"),
                    }),
            )
        })
}

fn gesture_action(
    gesture_map: &BTreeMap<GestureDirection, Action>,
    button: ButtonId,
    direction: GestureDirection,
) -> Action {
    gesture_map.get(&direction).cloned().unwrap_or_else(|| {
        if direction == GestureDirection::Click {
            default_binding(button)
        } else {
            Action::None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_offers_gestures_for_every_supported_button() {
        let supported: Vec<_> = ButtonId::ALL
            .into_iter()
            .filter(|button| can_enable_gestures(*button, None))
            .collect();

        assert_eq!(
            supported,
            vec![
                ButtonId::MiddleClick,
                ButtonId::Back,
                ButtonId::Forward,
                ButtonId::DpiToggle,
                ButtonId::GestureButton,
                ButtonId::HapticPanel,
            ]
        );
    }

    #[test]
    fn per_app_profile_does_not_offer_forward_gesture_mode() {
        assert!(!can_enable_gestures(
            ButtonId::Forward,
            Some("com.apple.Safari")
        ));
    }

    fn global_profile<'a>(
        bindings: &'a BTreeMap<ButtonId, Action>,
        gesture_maps: &'a BTreeMap<ButtonId, BTreeMap<GestureDirection, Action>>,
        wheel_hold: &'a BTreeSet<ButtonId>,
    ) -> BindingInspectorData<'a> {
        BindingInspectorData {
            selected: Some(MouseControlId::Button(ButtonId::Back)),
            gesture_direction: None,
            bindings,
            gesture_maps,
            dpi_gestures: false,
            extra_actions: &[],
            hidden_actions: &[],
            virtual_thumbwheel: true,
            wheel_hold,
            editing_app: None,
            overridden: None,
            chords: None,
            chord_overrides: 0,
            press_force: None,
        }
    }

    #[test]
    fn a_chord_can_name_only_controls_the_device_has_and_that_can_chord() {
        let hotspot = |id: ButtonId| Hotspot {
            id: id.into(),
            x: 0.,
            y: 0.,
            w: 1.,
            h: 1.,
        };
        let drawn = [
            hotspot(ButtonId::LeftClick),
            hotspot(ButtonId::RightClick),
            hotspot(ButtonId::Back),
            hotspot(ButtonId::Thumbwheel),
            Hotspot {
                id: MouseControlId::ThumbwheelRotation,
                ..hotspot(ButtonId::Back)
            },
        ];
        assert_eq!(
            chord_controls(&drawn, false),
            BTreeSet::from([ButtonId::Back])
        );
        assert_eq!(
            chord_controls(&drawn, true),
            BTreeSet::from([ButtonId::Back, ButtonId::HapticPanel])
        );
    }

    #[test]
    fn a_button_holding_the_wheel_reads_customized_even_with_its_default_click() {
        let _locale = crate::services::i18n::LOCALE_LOCK.lock().unwrap();
        let (bindings, gesture_maps) = (BTreeMap::new(), BTreeMap::new());
        let action = default_binding(ButtonId::Back);
        let plain = BTreeSet::new();
        let data = global_profile(&bindings, &gesture_maps, &plain);
        assert_eq!(
            button_status(ButtonId::Back, &action, &data, false),
            tr!("pointer.device_default")
        );
        let held = BTreeSet::from([ButtonId::Back]);
        let data = global_profile(&bindings, &gesture_maps, &held);
        assert_eq!(
            button_status(ButtonId::Back, &action, &data, false),
            tr!("profiles.customized")
        );
    }
}
