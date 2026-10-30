//! The Touch controllers as a pad. One OpenXR action set holds every Touch input; each frame it is
//! read into a [`Touch`], mapped by [`inputs`] onto the buttons and axes of a gamepad, and written
//! as `RawGamepadEvent`s from a pad of its own. `benilla-input`'s device layer then shapes them
//! (dead zones, thresholds, direction edges) exactly as it shapes a gilrs pad's, so everything
//! above `PadState` (bindings, modes, the cursor) is the code the Steam Deck runs.
//!
//! Touch has no d-pad, and the right menu button belongs to the system, so `PADDUP` to `PADDRIGHT`
//! and `PADFORWARD` stay unreached. The profile is the core `oculus/touch_controller`.

use bevy::input::gamepad::{
    GamepadAxis, GamepadButton, GamepadConnection, GamepadConnectionEvent,
    RawGamepadAxisChangedEvent, RawGamepadButtonChangedEvent, RawGamepadEvent,
};
use bevy::prelude::*;

/// One frame of the Touch controllers as the runtime reports them. A controller that is off or
/// asleep reads as all zeros.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Touch {
    /// Thumbsticks, y up.
    pub left_stick: Vec2,
    pub right_stick: Vec2,
    /// Index triggers and grips (squeeze), 0 to 1.
    pub left_trigger: f32,
    pub right_trigger: f32,
    pub left_grip: f32,
    pub right_grip: f32,
    pub left_stick_click: bool,
    pub right_stick_click: bool,
    pub x: bool,
    pub y: bool,
    pub a: bool,
    pub b: bool,
    /// The left menu button; the right one is the system's.
    pub menu: bool,
}

/// A gamepad input a Touch input drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Button(GamepadButton),
    Axis(GamepadAxis),
}

/// How many gamepad inputs a [`Touch`] drives.
pub const INPUTS: usize = 15;

/// Every gamepad input a reading drives, in a fixed order, with its value. The map: `a`, `b`, `x`,
/// `y` are the south, east, west and north face buttons (`PAD1` to `PAD4`), the grips the bumpers
/// (`PADLSHOULDER`, `PADRSHOULDER`), the triggers the triggers, the stick clicks `PADLSTICK` and
/// `PADRSTICK`, the left menu `PADBACK`, and the thumbsticks the two sticks.
pub fn inputs(t: &Touch) -> [(Input, f32); INPUTS] {
    let on = |b: bool| if b { 1.0 } else { 0.0 };
    use GamepadAxis as A;
    use GamepadButton as B;
    use Input::{Axis, Button};
    [
        (Axis(A::LeftStickX), t.left_stick.x),
        (Axis(A::LeftStickY), t.left_stick.y),
        (Axis(A::RightStickX), t.right_stick.x),
        (Axis(A::RightStickY), t.right_stick.y),
        (Button(B::South), on(t.a)),
        (Button(B::East), on(t.b)),
        (Button(B::West), on(t.x)),
        (Button(B::North), on(t.y)),
        (Button(B::LeftTrigger), t.left_grip),
        (Button(B::RightTrigger), t.right_grip),
        (Button(B::LeftTrigger2), t.left_trigger),
        (Button(B::RightTrigger2), t.right_trigger),
        (Button(B::LeftThumb), on(t.left_stick_click)),
        (Button(B::RightThumb), on(t.right_stick_click)),
        (Button(B::Select), on(t.menu)),
    ]
}

/// What the Touch input behind a pad token is called, for the trace. `None` for a token Touch
/// cannot make.
pub fn source(token: &str) -> Option<&'static str> {
    Some(match token {
        "PAD1" => "A",
        "PAD2" => "B",
        "PAD3" => "X",
        "PAD4" => "Y",
        "PADLSHOULDER" => "left grip",
        "PADRSHOULDER" => "right grip",
        "PADLTRIGGER" => "left trigger",
        "PADRTRIGGER" => "right trigger",
        "PADLSTICK" => "left stick click",
        "PADRSTICK" => "right stick click",
        "PADBACK" => "left menu",
        "PADLSTICKUP" => "left stick up",
        "PADLSTICKRIGHT" => "left stick right",
        "PADLSTICKDOWN" => "left stick down",
        "PADLSTICKLEFT" => "left stick left",
        "PADRSTICKUP" => "right stick up",
        "PADRSTICKRIGHT" => "right stick right",
        "PADRSTICKDOWN" => "right stick down",
        "PADRSTICKLEFT" => "right stick left",
        _ => return None,
    })
}

/// The inputs that changed since the last reading. The first reading is compared with a controller
/// at rest, so a held button at session start is a press.
#[derive(Default)]
pub struct Changes {
    last: [f32; INPUTS],
}

impl Changes {
    pub fn step(&mut self, t: &Touch) -> Vec<(Input, f32)> {
        let now = inputs(t);
        let mut out = Vec::new();
        for (i, (input, value)) in now.into_iter().enumerate() {
            if value != self.last[i] {
                self.last[i] = value;
                out.push((input, value));
            }
        }
        out
    }

    /// The inputs that put everything back to rest, for a controller that went away.
    pub fn release(&mut self) -> Vec<(Input, f32)> {
        self.step(&Touch::default())
    }
}

/// Vendor id of Oculus VR, which Touch controllers report in the OpenXR runtime's own terms; the
/// pad only needs a name and an id for `device` in `pad.toml` and the connect trace.
const VENDOR: u16 = 0x2833;

/// The `RawGamepadEvent` for `connected` or not.
pub fn connection(pad: Entity, connected: bool) -> RawGamepadEvent {
    RawGamepadEvent::Connection(GamepadConnectionEvent::new(
        pad,
        if connected {
            GamepadConnection::Connected {
                name: "Touch controllers".into(),
                vendor_id: Some(VENDOR),
                product_id: None,
            }
        } else {
            GamepadConnection::Disconnected
        },
    ))
}

/// The `RawGamepadEvent` for one changed input of `pad`.
pub fn raw(pad: Entity, input: Input, value: f32) -> RawGamepadEvent {
    match input {
        Input::Button(b) => {
            RawGamepadEvent::Button(RawGamepadButtonChangedEvent::new(pad, b, value))
        }
        Input::Axis(a) => RawGamepadEvent::Axis(RawGamepadAxisChangedEvent::new(pad, a, value)),
    }
}

#[cfg(test)]
mod tests {
    use benilla_input::{PadConfig, PadDevice};

    use super::*;

    /// A device fed the events of `readings` in turn, one frame (0.1 s) apart.
    fn feed(readings: &[Touch]) -> PadDevice {
        let pad = Entity::from_raw_u32(7).unwrap();
        let mut device = PadDevice::new(PadConfig::default());
        device.handle(&connection(pad, true), 0.0);
        let mut changes = Changes::default();
        for (i, t) in readings.iter().enumerate() {
            for (input, value) in changes.step(t) {
                device.handle(&raw(pad, input, value), 0.1 * (i + 1) as f32);
            }
        }
        device
    }

    fn down(t: Touch) -> Vec<&'static str> {
        feed(&[t]).state().down().iter().copied().collect()
    }

    #[test]
    fn each_touch_button_is_the_pad_token_the_map_says() {
        let one = |f: fn(&mut Touch)| {
            let mut t = Touch::default();
            f(&mut t);
            down(t)
        };
        assert_eq!(one(|t| t.a = true), ["PAD1"]);
        assert_eq!(one(|t| t.b = true), ["PAD2"]);
        assert_eq!(one(|t| t.x = true), ["PAD3"]);
        assert_eq!(one(|t| t.y = true), ["PAD4"]);
        assert_eq!(one(|t| t.left_grip = 1.0), ["PADLSHOULDER"]);
        assert_eq!(one(|t| t.right_grip = 1.0), ["PADRSHOULDER"]);
        assert_eq!(one(|t| t.left_trigger = 1.0), ["PADLTRIGGER"]);
        assert_eq!(one(|t| t.right_trigger = 1.0), ["PADRTRIGGER"]);
        assert_eq!(one(|t| t.left_stick_click = true), ["PADLSTICK"]);
        assert_eq!(one(|t| t.right_stick_click = true), ["PADRSTICK"]);
        assert_eq!(one(|t| t.menu = true), ["PADBACK"]);
    }

    #[test]
    fn the_sticks_are_the_pads_sticks_with_y_up() {
        let pad = feed(&[Touch {
            left_stick: Vec2::new(0.0, 1.0),
            right_stick: Vec2::new(1.0, -1.0).normalize(),
            ..default()
        }]);
        let s = pad.state();
        assert_eq!(s.left, Vec2::new(0.0, 1.0));
        assert!((s.right - Vec2::new(1.0, -1.0).normalize()).length() < 1e-5);
        assert!(s.is_down("PADLSTICKUP"));
        assert!(s.is_down("PADRSTICKRIGHT") && s.is_down("PADRSTICKDOWN"));
    }

    #[test]
    fn a_trigger_pulled_halfway_presses_through_the_pads_threshold() {
        assert!(down(Touch {
            right_trigger: 0.3,
            ..default()
        })
        .is_empty());
        assert_eq!(
            down(Touch {
                right_trigger: 0.6,
                ..default()
            }),
            ["PADRTRIGGER"]
        );
    }

    #[test]
    fn touch_has_no_d_pad_and_no_forward_button() {
        let all = Touch {
            left_stick: Vec2::ONE.normalize(),
            right_stick: Vec2::ONE.normalize(),
            left_trigger: 1.0,
            right_trigger: 1.0,
            left_grip: 1.0,
            right_grip: 1.0,
            left_stick_click: true,
            right_stick_click: true,
            x: true,
            y: true,
            a: true,
            b: true,
            menu: true,
        };
        let d = down(all);
        for token in ["PADDUP", "PADDDOWN", "PADDLEFT", "PADDRIGHT", "PADFORWARD"] {
            assert!(!d.contains(&token), "{token}");
        }
        // Every token Touch makes has a name for the trace.
        for token in &d {
            assert!(source(token).is_some(), "{token}");
        }
    }

    #[test]
    fn only_what_changed_is_sent_and_a_release_is_sent_once() {
        let mut changes = Changes::default();
        let held = Touch {
            a: true,
            right_stick: Vec2::new(0.5, 0.0),
            ..default()
        };
        assert_eq!(changes.step(&held).len(), 2);
        assert!(changes.step(&held).is_empty(), "nothing moved");
        let released = changes.step(&Touch {
            right_stick: Vec2::new(0.5, 0.0),
            ..default()
        });
        assert_eq!(
            released,
            [(Input::Button(GamepadButton::South), 0.0)],
            "only the button let go"
        );
        // The controller went away: everything still held lets go.
        assert_eq!(changes.release().len(), 1);
        assert!(changes.release().is_empty());
    }

    #[test]
    fn the_pad_drops_everything_when_the_controllers_disconnect() {
        let pad = Entity::from_raw_u32(7).unwrap();
        let mut device = feed(&[Touch {
            a: true,
            ..default()
        }]);
        assert!(device.state().is_down("PAD1"));
        device.handle(&connection(pad, false), 1.0);
        assert!(device.state().down().is_empty());
        assert!(!device.connected());
    }
}
