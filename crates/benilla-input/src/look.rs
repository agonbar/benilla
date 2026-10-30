//! The right stick as the camera: gameplay mode ([`crate::mode`]) holds a mouselook session for as
//! long as it lasts, opened with `MouselookStart` and closed with `MouselookStop`, and the stick
//! feeds `MouseMotion` into it. The OS cursor stays locked and hidden for the whole session, so it
//! never flickers between frames.
//!
//! `MouselookStop` also ends a right-button look that is still held, as it does in 1.12.1, so the
//! stop waits for the mouse's right button to come up: a session closing must not cut off a drag
//! the player's trackpad is making.

use bevy::prelude::*;

use crate::config::PadConfig;
use crate::LookFeed;

/// What the session does this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    Start,
    Stop,
}

/// Whether we hold a mouselook session open.
#[derive(Default)]
pub struct Session {
    pub open: bool,
}

impl Session {
    /// One frame: the call to make, if any. `want` is whether gameplay mode wants a session,
    /// `covered` whether the loading cover is up (a mouse cannot open a look there, so the pad
    /// does not either, and a session already open closes), `right_held` whether the mouse's right
    /// button is down.
    pub fn step(&mut self, want: bool, covered: bool, right_held: bool) -> Option<Call> {
        let want = want && !covered;
        match (want, self.open) {
            (true, false) => Some(Call::Start),
            (false, true) if !right_held => Some(Call::Stop),
            _ => None,
        }
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

/// The mouse motion for this frame: the stick's, or the external source's ([`LookFeed`]), whose
/// motion is spent once so a turn that missed an open session is dropped, not kept for later.
pub fn feed(feed: &mut LookFeed, right: Vec2, cfg: &PadConfig, dt: f32) -> Vec2 {
    if feed.external {
        std::mem::take(&mut feed.motion)
    } else {
        motion(right, cfg, dt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_opens_once_and_closes_when_gameplay_mode_ends() {
        let mut s = Session::default();
        assert_eq!(s.step(true, false, false), Some(Call::Start));
        s.open = true;
        for _ in 0..100 {
            assert_eq!(s.step(true, false, false), None, "held, however long");
        }
        assert_eq!(s.step(false, false, false), Some(Call::Stop));
        s.open = false;
        assert_eq!(s.step(false, false, false), None);
    }

    #[test]
    fn a_held_right_button_keeps_the_stop_back_until_it_comes_up() {
        let mut s = Session { open: true };
        assert_eq!(
            s.step(false, false, true),
            None,
            "the stop would end the mouse's drag"
        );
        assert_eq!(s.step(false, false, true), None);
        assert_eq!(s.step(false, false, false), Some(Call::Stop));
    }

    #[test]
    fn the_cover_blocks_a_start_and_closes_an_open_session() {
        let mut s = Session::default();
        assert_eq!(s.step(true, true, false), None, "no start under the cover");
        assert_eq!(s.step(true, false, false), Some(Call::Start));
        s.open = true;
        assert_eq!(s.step(true, true, false), Some(Call::Stop));
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

    #[test]
    fn an_external_source_replaces_the_stick_and_is_spent_once() {
        let cfg = PadConfig::default();
        let mut f = LookFeed::default();
        let stick = Vec2::new(1.0, 0.0);
        assert_eq!(feed(&mut f, stick, &cfg, 1.0), motion(stick, &cfg, 1.0));
        f.external = true;
        f.motion = Vec2::new(174.0, 0.0);
        assert_eq!(feed(&mut f, stick, &cfg, 1.0), Vec2::new(174.0, 0.0));
        assert_eq!(feed(&mut f, stick, &cfg, 1.0), Vec2::ZERO, "spent");
    }
}
