//! The right stick as the camera: a held mouselook session that feeds `MouseMotion`. The session
//! opens with `MouselookStart` when the stick leaves its dead zone and closes with `MouselookStop`
//! once the stick has rested for [`REST`] seconds, so a stick brushing the zone does not open and
//! close it every frame, and the OS cursor is free whenever the stick is. Deviation from a
//! session held for the whole of gameplay: until the cursor mode of a later phase exists, a locked
//! cursor would take the mouse away from the player.

use bevy::prelude::*;

use crate::config::PadConfig;

/// Seconds the stick rests before the session closes.
pub const REST: f32 = 0.4;

/// What the session does this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    Start,
    Stop,
}

/// Whether we hold a mouselook session open, and for how long the stick has rested.
#[derive(Default)]
pub struct Session {
    pub open: bool,
    rested: f32,
}

impl Session {
    /// One frame: the call to make, if any. `deflected` is whether the stick is out of its dead
    /// zone, `dt` the frame's seconds.
    pub fn step(&mut self, deflected: bool, dt: f32) -> Option<Call> {
        if deflected {
            self.rested = 0.0;
            return (!self.open).then_some(Call::Start);
        }
        if !self.open {
            return None;
        }
        self.rested += dt;
        (self.rested >= REST).then_some(Call::Stop)
    }
}

/// The mouse motion, in pixels, of a shaped stick over `dt` seconds. The response curve is the
/// travel to the power 1.5, so small deflections turn finely and full travel is `look_speed`.
/// Stick up looks up, which is the mouse moving up, negative y.
pub fn motion(right: Vec2, cfg: &PadConfig, dt: f32) -> Vec2 {
    let m = right.length();
    if m == 0.0 {
        return Vec2::ZERO;
    }
    let speed = m.powf(1.5) * cfg.look_speed * dt;
    let dir = right / m;
    let y = if cfg.look_invert_y { dir.y } else { -dir.y };
    Vec2::new(dir.x, y) * speed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_opens_once_and_closes_after_the_stick_rests() {
        let mut s = Session::default();
        assert_eq!(s.step(true, 0.016), Some(Call::Start));
        s.open = true;
        assert_eq!(s.step(true, 0.016), None, "already open");
        assert_eq!(s.step(false, 0.2), None);
        assert_eq!(
            s.step(true, 0.016),
            None,
            "a brush of the zone restarts the rest"
        );
        assert_eq!(s.step(false, 0.3), None);
        assert_eq!(s.step(false, 0.2), Some(Call::Stop));
    }

    #[test]
    fn a_closed_session_stays_quiet() {
        let mut s = Session::default();
        for _ in 0..100 {
            assert_eq!(s.step(false, 0.1), None);
        }
    }

    #[test]
    fn motion_scales_with_time_and_looks_up_on_stick_up() {
        let cfg = PadConfig::default();
        let full = motion(Vec2::new(1.0, 0.0), &cfg, 0.5);
        assert!((full.x - 450.0).abs() < 1e-3 && full.y == 0.0);
        let up = motion(Vec2::new(0.0, 1.0), &cfg, 1.0);
        assert!(up.y < 0.0);
        let (cfg, _) = PadConfig::parse("look_invert_y = true");
        assert!(motion(Vec2::new(0.0, 1.0), &cfg, 1.0).y > 0.0);
        assert_eq!(motion(Vec2::ZERO, &cfg, 1.0), Vec2::ZERO);
    }

    #[test]
    fn half_travel_is_under_half_speed() {
        let cfg = PadConfig::default();
        let half = motion(Vec2::new(0.5, 0.0), &cfg, 1.0).x;
        // Linear would be 450 pixels a second, half of `look_speed`.
        assert!(half < 450.0 && half > 0.0, "{half}");
    }
}
