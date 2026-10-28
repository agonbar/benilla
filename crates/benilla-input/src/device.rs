//! The pad as one state: gilrs' raw events (`RawGamepadEvent`) from whichever pad is active, shaped
//! into [`PadState`], the one thing the layers above read.
//!
//! Steam Input can show the game its virtual pad beside the physical one, and gilrs reports both.
//! The active pad is the one whose input counted last: another pad is ignored until the active one
//! has been silent for [`SILENCE`] seconds, or disconnects, and then takes over on its next
//! input. `device` in `pad.toml` pins one vendor:product instead.

use std::collections::{BTreeSet, HashMap};

use bevy::input::gamepad::{GamepadAxis, GamepadButton, GamepadConnection, RawGamepadEvent};
use bevy::prelude::*;

use crate::config::{Device, PadConfig};
use crate::tokens;

/// Seconds the active pad must be silent before another pad's input takes over.
pub const SILENCE: f32 = 2.0;
/// A direction of a stick presses from this share of the stick's travel along it, and holds down
/// to [`DIR_HOLD`]; the gap is the angular hysteresis between neighbouring sectors.
const DIR_PRESS: f32 = 0.38;
const DIR_HOLD: f32 = 0.30;
/// A stick direction or button releases at this share of its press threshold.
const RELEASE: f32 = 0.7;
/// Raw movement under this on a pad that is not active is noise, not input.
const NOISE: f32 = 0.1;

/// What gilrs says a connected pad is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PadInfo {
    pub name: String,
    pub vendor: Option<u16>,
    pub product: Option<u16>,
}

/// The active pad, shaped. Sticks are radially dead-zoned and rescaled (so the edge of the dead
/// zone is 0 and full travel is 1, y up), triggers are 0 to 1, and `down` holds the token of every
/// input that counts as pressed, stick directions and triggers included.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct PadState {
    pub left: Vec2,
    pub right: Vec2,
    pub left_trigger: f32,
    pub right_trigger: f32,
    down: BTreeSet<&'static str>,
}

impl PadState {
    pub fn is_down(&self, token: &str) -> bool {
        self.down.contains(token)
    }

    pub fn down(&self) -> &BTreeSet<&'static str> {
        &self.down
    }
}

/// `state` with exactly `down` pressed, for the tests of the layers above.
#[cfg(test)]
pub(crate) fn state_with(mut state: PadState, down: &[&'static str]) -> PadState {
    state.down = down.iter().copied().collect();
    state
}

/// The pads gilrs shows, which one is active, and its [`PadState`].
#[derive(Resource)]
pub struct PadDevice {
    cfg: PadConfig,
    pads: HashMap<Entity, PadInfo>,
    active: Option<Entity>,
    /// When the active pad last sent input that counts.
    last_input: f32,
    buttons: HashMap<GamepadButton, f32>,
    axes: HashMap<GamepadAxis, f32>,
    state: PadState,
    connects: Vec<PadInfo>,
}

impl PadDevice {
    pub fn new(cfg: PadConfig) -> Self {
        PadDevice {
            cfg,
            pads: HashMap::new(),
            active: None,
            last_input: 0.0,
            buttons: HashMap::new(),
            axes: HashMap::new(),
            state: PadState::default(),
            connects: Vec::new(),
        }
    }

    pub fn state(&self) -> &PadState {
        &self.state
    }

    pub fn active(&self) -> Option<Entity> {
        self.active
    }

    /// The pads connected since the last call, for the trace.
    pub fn take_connects(&mut self) -> Vec<PadInfo> {
        std::mem::take(&mut self.connects)
    }

    /// One raw event, `now` in seconds.
    pub fn handle(&mut self, ev: &RawGamepadEvent, now: f32) {
        match ev {
            RawGamepadEvent::Connection(c) => match &c.connection {
                GamepadConnection::Connected {
                    name,
                    vendor_id,
                    product_id,
                } => {
                    let info = PadInfo {
                        name: name.clone(),
                        vendor: *vendor_id,
                        product: *product_id,
                    };
                    self.connects.push(info.clone());
                    self.pads.insert(c.gamepad, info);
                }
                GamepadConnection::Disconnected => {
                    self.pads.remove(&c.gamepad);
                    if self.active == Some(c.gamepad) {
                        self.release_all();
                    }
                }
            },
            RawGamepadEvent::Button(b) => {
                let prev = self.buttons.get(&b.button).copied().unwrap_or(0.0);
                let counts = b.value.abs() > NOISE || prev.abs() > NOISE;
                if self.accept(b.gamepad, counts, now) {
                    self.buttons.insert(b.button, b.value);
                    self.recompute();
                }
            }
            RawGamepadEvent::Axis(a) => {
                let floor = self.cfg.move_dead_zone.min(self.cfg.look_dead_zone);
                let prev = self.axes.get(&a.axis).copied().unwrap_or(0.0);
                let counts = a.value.abs() > floor || prev.abs() > NOISE;
                if self.accept(a.gamepad, counts, now) {
                    self.axes.insert(a.axis, a.value);
                    self.recompute();
                }
            }
        }
    }

    /// Whether an event from `pad` is read, and who is active after it. `counts` is whether the
    /// event is input and not the noise of a resting stick.
    fn accept(&mut self, pad: Entity, counts: bool, now: f32) -> bool {
        if let Device::Pinned { vendor, product } = self.cfg.device {
            let hit = self
                .pads
                .get(&pad)
                .is_some_and(|i| i.vendor == Some(vendor) && i.product == Some(product));
            if !hit {
                return false;
            }
        }
        match self.active {
            Some(a) if a == pad => {
                if counts {
                    self.last_input = now;
                }
                true
            }
            Some(_) if !counts || now - self.last_input < SILENCE => false,
            _ if !counts => false,
            _ => {
                self.release_all();
                self.active = Some(pad);
                self.last_input = now;
                true
            }
        }
    }

    /// Forget the active pad's raw input, which releases everything it held.
    fn release_all(&mut self) {
        self.active = None;
        self.buttons.clear();
        self.axes.clear();
        self.state = PadState::default();
    }

    fn recompute(&mut self) {
        let ax = |a| self.axes.get(&a).copied().unwrap_or(0.0);
        let left_raw = Vec2::new(ax(GamepadAxis::LeftStickX), ax(GamepadAxis::LeftStickY));
        let right_raw = Vec2::new(ax(GamepadAxis::RightStickX), ax(GamepadAxis::RightStickY));
        let was = &self.state.down;
        let mut down = BTreeSet::new();

        let press = self.cfg.trigger_press;
        for (&button, &value) in &self.buttons {
            let Some(token) = tokens::button(button) else {
                continue;
            };
            let threshold = if was.contains(token) {
                press * RELEASE
            } else {
                press
            };
            if value >= threshold {
                down.insert(token);
            }
        }
        for (raw, dz, names) in [
            (left_raw, self.cfg.move_dead_zone, tokens::LEFT_STICK),
            (right_raw, self.cfg.look_dead_zone, tokens::RIGHT_STICK),
        ] {
            let dirs = stick_directions(raw, dz, |i| was.contains(names[i]));
            for (on, name) in dirs.into_iter().zip(names) {
                if on {
                    down.insert(name);
                }
            }
        }
        let trigger = |b| self.buttons.get(&b).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        self.state = PadState {
            left: shape(left_raw, self.cfg.move_dead_zone),
            right: shape(right_raw, self.cfg.look_dead_zone),
            left_trigger: trigger(GamepadButton::LeftTrigger2),
            right_trigger: trigger(GamepadButton::RightTrigger2),
            down,
        };
    }
}

/// A stick with its radial dead zone removed: 0 inside, rescaled so the zone's edge is 0 and full
/// travel is 1, direction kept.
pub fn shape(v: Vec2, dead_zone: f32) -> Vec2 {
    let m = v.length();
    if m <= dead_zone {
        return Vec2::ZERO;
    }
    v / m * ((m - dead_zone) / (1.0 - dead_zone)).min(1.0)
}

/// The four direction bits (up, right, down, left) of a stick, 8-way: two neighbours together make
/// a diagonal. Radial hysteresis (press at `dead_zone`, hold to 70 percent of it) and angular
/// hysteresis ([`DIR_PRESS`], [`DIR_HOLD`]), so a stick resting on a sector edge does not chatter
/// and a straight push does not leak into the strafe.
pub fn stick_directions(raw: Vec2, dead_zone: f32, was: impl Fn(usize) -> bool) -> [bool; 4] {
    let dead_zone = dead_zone.max(0.01);
    let m = raw.length();
    let comps = [raw.y, raw.x, -raw.y, -raw.x];
    std::array::from_fn(|i| {
        if was(i) {
            m > dead_zone * RELEASE && comps[i] / m > DIR_HOLD
        } else {
            m > dead_zone && comps[i] / m > DIR_PRESS
        }
    })
}

#[cfg(test)]
mod tests {
    use bevy::input::gamepad::{
        GamepadConnectionEvent, RawGamepadAxisChangedEvent, RawGamepadButtonChangedEvent,
    };

    use super::*;

    fn pad(n: u32) -> Entity {
        Entity::from_raw_u32(n).expect("a valid index")
    }

    fn connect(d: &mut PadDevice, e: Entity, vendor: u16, product: u16) {
        d.handle(
            &RawGamepadEvent::Connection(GamepadConnectionEvent::new(
                e,
                GamepadConnection::Connected {
                    name: "pad".into(),
                    vendor_id: Some(vendor),
                    product_id: Some(product),
                },
            )),
            0.0,
        );
    }

    fn button(d: &mut PadDevice, e: Entity, b: GamepadButton, v: f32, t: f32) {
        d.handle(
            &RawGamepadEvent::Button(RawGamepadButtonChangedEvent::new(e, b, v)),
            t,
        );
    }

    fn axis(d: &mut PadDevice, e: Entity, a: GamepadAxis, v: f32, t: f32) {
        d.handle(
            &RawGamepadEvent::Axis(RawGamepadAxisChangedEvent::new(e, a, v)),
            t,
        );
    }

    fn device() -> PadDevice {
        PadDevice::new(PadConfig::default())
    }

    #[test]
    fn a_dead_zone_swallows_a_resting_stick_and_rescales_past_it() {
        let mut d = device();
        let e = pad(1);
        connect(&mut d, e, 1, 1);
        axis(&mut d, e, GamepadAxis::LeftStickX, 0.1, 1.0);
        assert_eq!(d.state().left, Vec2::ZERO);
        assert_eq!(d.active(), None, "drift inside the zone claims nothing");
        axis(&mut d, e, GamepadAxis::LeftStickX, 1.0, 1.1);
        assert_eq!(d.state().left, Vec2::new(1.0, 0.0));
        axis(&mut d, e, GamepadAxis::LeftStickX, 0.625, 1.2);
        // (0.625 - 0.25) / 0.75
        assert!(
            (d.state().left.x - 0.5).abs() < 1e-5,
            "{:?}",
            d.state().left
        );
    }

    #[test]
    fn stick_directions_have_hysteresis_and_make_diagonals() {
        let mut d = device();
        let e = pad(1);
        connect(&mut d, e, 1, 1);
        axis(&mut d, e, GamepadAxis::LeftStickY, 0.3, 1.0);
        assert!(d.state().is_down("PADLSTICKUP"));
        // 0.2 is under the press threshold but over the hold threshold (0.175).
        axis(&mut d, e, GamepadAxis::LeftStickY, 0.2, 1.1);
        assert!(d.state().is_down("PADLSTICKUP"), "held through the gap");
        axis(&mut d, e, GamepadAxis::LeftStickY, 0.15, 1.2);
        assert!(!d.state().is_down("PADLSTICKUP"));
        // Up and right together, both sectors.
        axis(&mut d, e, GamepadAxis::LeftStickY, 0.7, 1.3);
        axis(&mut d, e, GamepadAxis::LeftStickX, 0.7, 1.3);
        assert!(d.state().is_down("PADLSTICKUP") && d.state().is_down("PADLSTICKRIGHT"));
        assert!(!d.state().is_down("PADLSTICKDOWN") && !d.state().is_down("PADLSTICKLEFT"));
    }

    #[test]
    fn a_straight_push_with_a_little_sideways_does_not_strafe() {
        let mut d = device();
        let e = pad(1);
        connect(&mut d, e, 1, 1);
        axis(&mut d, e, GamepadAxis::LeftStickY, 0.9, 1.0);
        axis(&mut d, e, GamepadAxis::LeftStickX, 0.3, 1.0);
        assert!(d.state().is_down("PADLSTICKUP"));
        assert!(!d.state().is_down("PADLSTICKRIGHT"));
    }

    #[test]
    fn a_trigger_presses_at_the_threshold_and_releases_lower() {
        let mut d = device();
        let e = pad(1);
        connect(&mut d, e, 1, 1);
        button(&mut d, e, GamepadButton::LeftTrigger2, 0.4, 1.0);
        assert!(!d.state().is_down("PADLTRIGGER"));
        assert_eq!(d.state().left_trigger, 0.4);
        button(&mut d, e, GamepadButton::LeftTrigger2, 0.6, 1.1);
        assert!(d.state().is_down("PADLTRIGGER"));
        button(&mut d, e, GamepadButton::LeftTrigger2, 0.4, 1.2);
        assert!(d.state().is_down("PADLTRIGGER"), "held above 0.35");
        button(&mut d, e, GamepadButton::LeftTrigger2, 0.3, 1.3);
        assert!(!d.state().is_down("PADLTRIGGER"));
    }

    #[test]
    fn buttons_map_to_tokens_and_release() {
        let mut d = device();
        let e = pad(1);
        connect(&mut d, e, 1, 1);
        button(&mut d, e, GamepadButton::South, 1.0, 1.0);
        button(&mut d, e, GamepadButton::LeftThumb, 1.0, 1.0);
        assert!(d.state().is_down("PAD1") && d.state().is_down("PADLSTICK"));
        button(&mut d, e, GamepadButton::South, 0.0, 1.1);
        assert!(!d.state().is_down("PAD1") && d.state().is_down("PADLSTICK"));
    }

    #[test]
    fn the_second_pad_is_ignored_while_the_first_is_active() {
        let mut d = device();
        let (a, b) = (pad(1), pad(2));
        connect(&mut d, a, 0x28de, 0x1205);
        connect(&mut d, b, 0x28de, 0x11ff);
        button(&mut d, a, GamepadButton::South, 1.0, 1.0);
        assert_eq!(d.active(), Some(a));
        // The mirror pad sends the same input a moment later, and its release after.
        button(&mut d, b, GamepadButton::South, 1.0, 1.001);
        button(&mut d, b, GamepadButton::East, 1.0, 1.5);
        assert!(!d.state().is_down("PAD2"));
        button(&mut d, b, GamepadButton::South, 0.0, 1.9);
        assert!(
            d.state().is_down("PAD1"),
            "the other pad's release is not ours"
        );
        assert_eq!(d.active(), Some(a));
    }

    #[test]
    fn another_pad_takes_over_after_silence_and_drops_what_the_first_held() {
        let mut d = device();
        let (a, b) = (pad(1), pad(2));
        connect(&mut d, a, 1, 1);
        connect(&mut d, b, 2, 2);
        button(&mut d, a, GamepadButton::South, 1.0, 1.0);
        button(&mut d, b, GamepadButton::East, 1.0, 2.9);
        assert_eq!(d.active(), Some(a), "1.9 s of silence is not enough");
        button(&mut d, b, GamepadButton::East, 1.0, 3.1);
        assert_eq!(d.active(), Some(b));
        assert!(d.state().is_down("PAD2") && !d.state().is_down("PAD1"));
    }

    #[test]
    fn a_resting_stick_on_the_other_pad_never_takes_over() {
        let mut d = device();
        let (a, b) = (pad(1), pad(2));
        connect(&mut d, a, 1, 1);
        connect(&mut d, b, 2, 2);
        button(&mut d, a, GamepadButton::South, 1.0, 1.0);
        axis(&mut d, b, GamepadAxis::LeftStickX, 0.05, 9.0);
        assert_eq!(d.active(), Some(a));
    }

    #[test]
    fn disconnecting_the_active_pad_releases_everything() {
        let mut d = device();
        let (a, b) = (pad(1), pad(2));
        connect(&mut d, a, 1, 1);
        connect(&mut d, b, 2, 2);
        button(&mut d, a, GamepadButton::South, 1.0, 1.0);
        axis(&mut d, a, GamepadAxis::LeftStickY, 1.0, 1.0);
        d.handle(
            &RawGamepadEvent::Connection(GamepadConnectionEvent::new(
                a,
                GamepadConnection::Disconnected,
            )),
            1.1,
        );
        assert_eq!(d.active(), None);
        assert_eq!(*d.state(), PadState::default());
        // The survivor is active on its next input at once, with no silence to wait out.
        button(&mut d, b, GamepadButton::North, 1.0, 1.2);
        assert_eq!(d.active(), Some(b));
    }

    #[test]
    fn a_pinned_device_reads_only_its_vendor_and_product() {
        let (cfg, _) = PadConfig::parse("device = \"28de:1205\"");
        let mut d = PadDevice::new(cfg);
        let (a, b) = (pad(1), pad(2));
        connect(&mut d, a, 0x28de, 0x11ff);
        connect(&mut d, b, 0x28de, 0x1205);
        button(&mut d, a, GamepadButton::South, 1.0, 1.0);
        assert_eq!(d.active(), None);
        button(&mut d, b, GamepadButton::East, 1.0, 1.1);
        assert_eq!(d.active(), Some(b));
        assert!(d.state().is_down("PAD2"));
    }

    #[test]
    fn connects_are_reported_once_with_their_ids() {
        let mut d = device();
        connect(&mut d, pad(1), 0x28de, 0x1205);
        let c = d.take_connects();
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].vendor, c[0].product), (Some(0x28de), Some(0x1205)));
        assert!(d.take_connects().is_empty());
    }
}
