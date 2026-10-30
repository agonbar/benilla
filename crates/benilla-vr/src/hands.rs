//! The Touch controllers in OpenXR: one action set on the core `oculus/touch_controller` profile
//! that holds every input the client reads, and the systems that attach it, sync it, read it into
//! a [`Touch`] and hand that to the pad ([`crate::touch`]).
//!
//! One set, not a pad set and a pointer set: OpenXR priority only decides who wins a path bound in
//! two active sets, and both would bind the same sticks and triggers. What each input means is
//! `benilla-input`'s mode machine's to say, and it says it the same way for a Deck.
//!
//! `WOW_VR_INPUT_TRACE=1` prints one line per pad token that changed: the Touch input behind it,
//! the command it resolved to (or the cursor act that took it), the mode and the panel hit.

use bevy::input::gamepad::RawGamepadEvent;
use bevy::prelude::*;
use bevy_mod_openxr::action_binding::{OxrSendActionBindings, OxrSuggestActionBinding};
use bevy_mod_openxr::action_set_attaching::OxrAttachActionSet;
use bevy_mod_openxr::action_set_syncing::{OxrActionSetSyncSet, OxrSyncActionSet};
use bevy_mod_openxr::openxr_session_running;
use bevy_mod_openxr::resources::OxrInstance;
use bevy_mod_openxr::session::OxrSession;
use bevy_mod_xr::session::{session_available, XrSessionCreated};
use bevy_mod_xr::spaces::{XrSpace, XrSpaceSyncSet};

use benilla_input::{Mode, PadMode, PadRouted, PadSystems, RoutedTrace};

use crate::laser::Aimed;
use crate::touch::{self, Changes, Touch};

const PROFILE: &str = "/interaction_profiles/oculus/touch_controller";

/// The set and its actions. Paired inputs are one action with a subaction path per hand.
#[derive(Resource)]
pub(crate) struct Actions {
    set: openxr::ActionSet,
    trigger: openxr::Action<f32>,
    squeeze: openxr::Action<f32>,
    stick: openxr::Action<openxr::Vector2f>,
    stick_click: openxr::Action<bool>,
    x: openxr::Action<bool>,
    y: openxr::Action<bool>,
    a: openxr::Action<bool>,
    b: openxr::Action<bool>,
    menu: openxr::Action<bool>,
    aim: openxr::Action<openxr::Posef>,
    left: openxr::Path,
    right: openxr::Path,
}

impl Actions {
    fn new(instance: &OxrInstance) -> openxr::Result<Self> {
        let set = instance.create_action_set("benilla", "Benilla", 0)?;
        let (left, right) = (
            instance.string_to_path("/user/hand/left")?,
            instance.string_to_path("/user/hand/right")?,
        );
        let both = [left, right];
        Ok(Actions {
            trigger: set.create_action("trigger", "Trigger", &both)?,
            squeeze: set.create_action("squeeze", "Grip", &both)?,
            stick: set.create_action("thumbstick", "Thumbstick", &both)?,
            stick_click: set.create_action("thumbstick_click", "Thumbstick click", &both)?,
            x: set.create_action("x_click", "X", &[])?,
            y: set.create_action("y_click", "Y", &[])?,
            a: set.create_action("a_click", "A", &[])?,
            b: set.create_action("b_click", "B", &[])?,
            menu: set.create_action("menu_click", "Menu", &[])?,
            aim: set.create_action("aim_pose", "Aim pose", &both)?,
            set,
            left,
            right,
        })
    }

    /// The right controller's aim pose as a space.
    pub(crate) fn aim_space(&self, session: &OxrSession) -> Option<XrSpace> {
        match session.create_action_space(&self.aim, self.right, Isometry3d::IDENTITY) {
            Ok(space) => Some(space),
            Err(e) => {
                warn!("vr: the aim space: {e}");
                None
            }
        }
    }

    /// Every input as the runtime last synced it. An input the runtime has no controller for is
    /// at rest.
    fn read(&self, session: &OxrSession) -> Touch {
        let float = |a: &openxr::Action<f32>, hand| {
            a.state(session, hand)
                .ok()
                .filter(|s| s.is_active)
                .map_or(0.0, |s| s.current_state)
        };
        let button = |a: &openxr::Action<bool>, hand| {
            a.state(session, hand)
                .ok()
                .is_some_and(|s| s.is_active && s.current_state)
        };
        let stick = |hand| {
            self.stick
                .state(session, hand)
                .ok()
                .filter(|s| s.is_active)
                .map_or(Vec2::ZERO, |s| {
                    Vec2::new(s.current_state.x, s.current_state.y)
                })
        };
        let none = openxr::Path::NULL;
        Touch {
            left_stick: stick(self.left),
            right_stick: stick(self.right),
            left_trigger: float(&self.trigger, self.left),
            right_trigger: float(&self.trigger, self.right),
            left_grip: float(&self.squeeze, self.left),
            right_grip: float(&self.squeeze, self.right),
            left_stick_click: button(&self.stick_click, self.left),
            right_stick_click: button(&self.stick_click, self.right),
            x: button(&self.x, none),
            y: button(&self.y, none),
            a: button(&self.a, none),
            b: button(&self.b, none),
            menu: button(&self.menu, none),
        }
    }
}

/// The set exists and can be read.
pub(crate) fn ready(actions: Option<Res<Actions>>) -> bool {
    actions.is_some()
}

pub struct HandsPlugin;

impl Plugin for HandsPlugin {
    fn build(&self, app: &mut App) {
        let trace = std::env::var_os("WOW_VR_INPUT_TRACE").is_some_and(|v| v != "0");
        if trace {
            app.insert_resource(RoutedTrace(true))
                .init_resource::<PadMode>()
                .add_message::<PadRouted>()
                .add_systems(Update, trace_input);
        }
        app.add_systems(Startup, create_actions.run_if(session_available))
            .add_systems(XrSessionCreated, attach_set.run_if(ready))
            .add_systems(OxrSendActionBindings, suggest_bindings.run_if(ready))
            .add_systems(
                PreUpdate,
                (
                    sync_actions
                        .before(OxrActionSetSyncSet)
                        .run_if(openxr_session_running)
                        .run_if(ready),
                    read_touch
                        .after(OxrActionSetSyncSet)
                        .after(XrSpaceSyncSet)
                        .before(PadSystems::Read)
                        .run_if(ready),
                ),
            );
    }
}

fn create_actions(instance: Res<OxrInstance>, mut commands: Commands) {
    match Actions::new(&instance) {
        Ok(actions) => commands.insert_resource(actions),
        Err(e) => error!("vr: creating the action set: {e}"),
    }
}

fn attach_set(actions: Res<Actions>, mut attach: MessageWriter<OxrAttachActionSet>) {
    attach.write(OxrAttachActionSet(actions.set.clone()));
}

fn sync_actions(actions: Res<Actions>, mut sync: MessageWriter<OxrSyncActionSet>) {
    sync.write(OxrSyncActionSet(actions.set.clone()));
}

fn suggest_bindings(actions: Res<Actions>, mut out: MessageWriter<OxrSuggestActionBinding>) {
    let mut bind = |action: openxr::sys::Action, paths: &[&str]| {
        out.write(OxrSuggestActionBinding {
            action,
            interaction_profile: PROFILE.into(),
            bindings: paths.iter().map(|p| p.to_string().into()).collect(),
        });
    };
    let hands = |input: &str| {
        [
            format!("/user/hand/left/input/{input}"),
            format!("/user/hand/right/input/{input}"),
        ]
    };
    let mut both = |action: openxr::sys::Action, input: &str| {
        let paths = hands(input);
        bind(action, &[&paths[0], &paths[1]]);
    };
    both(actions.trigger.as_raw(), "trigger/value");
    both(actions.squeeze.as_raw(), "squeeze/value");
    both(actions.stick.as_raw(), "thumbstick");
    both(actions.stick_click.as_raw(), "thumbstick/click");
    both(actions.aim.as_raw(), "aim/pose");
    bind(actions.x.as_raw(), &["/user/hand/left/input/x/click"]);
    bind(actions.y.as_raw(), &["/user/hand/left/input/y/click"]);
    bind(actions.menu.as_raw(), &["/user/hand/left/input/menu/click"]);
    bind(actions.a.as_raw(), &["/user/hand/right/input/a/click"]);
    bind(actions.b.as_raw(), &["/user/hand/right/input/b/click"]);
}

/// The synced actions into the pad, as the events of a pad of their own: it connects when the
/// session runs and disconnects when it stops, and only what changed is sent.
fn read_touch(
    actions: Res<Actions>,
    session: Option<Res<OxrSession>>,
    state: Option<Res<bevy_mod_xr::session::XrState>>,
    mut commands: Commands,
    mut raw: MessageWriter<RawGamepadEvent>,
    mut pad: Local<Option<Entity>>,
    mut changes: Local<Changes>,
) {
    let running = matches!(
        state.as_deref(),
        Some(bevy_mod_xr::session::XrState::Running)
    );
    match (running, session, *pad) {
        (true, Some(session), None) => {
            let entity = commands.spawn(Name::new("touch controllers")).id();
            *pad = Some(entity);
            raw.write(touch::connection(entity, true));
            // The sync of this frame is already in; a button held at start is a press.
            for (input, value) in changes.step(&actions.read(&session)) {
                raw.write(touch::raw(entity, input, value));
            }
        }
        (true, Some(session), Some(entity)) => {
            for (input, value) in changes.step(&actions.read(&session)) {
                raw.write(touch::raw(entity, input, value));
            }
        }
        (false, _, Some(entity)) => {
            for (input, value) in changes.release() {
                raw.write(touch::raw(entity, input, value));
            }
            raw.write(touch::connection(entity, false));
            commands.entity(entity).despawn();
            *pad = None;
        }
        _ => {}
    }
}

/// One `WOW_VR_INPUT_TRACE` line.
pub fn trace_line(
    source: &str,
    pressed: bool,
    token: &str,
    what: &str,
    mode: Mode,
    hit: Option<Vec2>,
) -> String {
    format!(
        "vr input: {source} {} -> {token} -> {what} | mode {} | panel {}",
        if pressed { "down" } else { "up" },
        if mode == Mode::Cursor {
            "cursor"
        } else {
            "gameplay"
        },
        hit.map_or("none".to_owned(), |c| format!("({:.0}, {:.0})", c.x, c.y)),
    )
}

fn trace_input(mut routed: MessageReader<PadRouted>, mode: Res<PadMode>, aimed: Res<Aimed>) {
    for r in routed.read() {
        let source = touch::source(r.token).unwrap_or("-");
        info!(
            "{}",
            trace_line(
                source,
                r.pressed,
                r.token,
                &r.what,
                mode.0,
                aimed.hit.map(|h| h.cursor)
            )
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trace_line_names_the_input_the_token_the_command_the_mode_and_the_hit() {
        assert_eq!(
            trace_line(
                "right trigger",
                true,
                "PADRTRIGGER",
                "cursor:Left",
                Mode::Cursor,
                Some(Vec2::new(640.4, 360.0))
            ),
            "vr input: right trigger down -> PADRTRIGGER -> cursor:Left | mode cursor | panel (640, 360)"
        );
        assert_eq!(
            trace_line("A", false, "PAD1", "binding", Mode::Gameplay, None),
            "vr input: A up -> PAD1 -> binding | mode gameplay | panel none"
        );
    }
}
