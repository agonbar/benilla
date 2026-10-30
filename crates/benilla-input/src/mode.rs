//! The two modes the pad plays in. Gameplay mode is a held mouselook ([`crate::look`]): the right
//! stick turns the camera and the buttons press bound actions. Cursor mode has no look session; a
//! visible cursor ([`crate::cursor`]) and the buttons become pointer buttons.
//!
//! Cursor mode comes on when a window opens (a panel, a bag, the game menu, a popup), when a
//! targeting cursor is armed (a ground spell), or by hand with `PADBACK`. It goes off when a
//! window-driven mode has nothing left to show, when `PADBACK` is pressed again, or when the left
//! stick walks (with the right stick as the cursor, which is the default).
//!
//! Entering is by edge, not by level: a window opening switches in, but one left open after the
//! player walked away does not pull the pad back every poll.

use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Gameplay,
    Cursor,
}

/// What the machine reads each frame.
#[derive(Clone, Copy, Debug)]
pub struct Signals<'a> {
    /// The windows open now, by frame name.
    pub panels: &'a BTreeSet<String>,
    /// A targeting cursor is armed (`SpellIsTargeting`).
    pub targeting: bool,
    /// `PADBACK` went down this frame.
    pub back: bool,
    /// The left stick is out of its dead zone, and is the stick that walks.
    pub walk: bool,
    /// A laser points at the UI panel.
    pub pointing: bool,
}

#[derive(Debug, Default)]
pub struct Machine {
    mode: Mode,
    /// Cursor mode was asked for with `PADBACK`: it stays until `PADBACK` or a walk.
    manual: bool,
    panels: BTreeSet<String>,
    targeting: bool,
    pointing: bool,
}

impl Machine {
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// One frame; the mode after it.
    pub fn step(&mut self, s: Signals) -> Mode {
        let opened = s.panels.difference(&self.panels).next().is_some();
        let armed = s.targeting && !self.targeting;
        let aimed = s.pointing && !self.pointing;
        let cause = !s.panels.is_empty() || s.targeting || s.pointing;
        // The left stick walks in cursor mode too, bound as it is; a laser on the panel keeps the
        // mode, because walking with a hand up is not a wish to look away.
        let walk = s.walk && !s.pointing;
        if s.back {
            self.mode = match self.mode {
                Mode::Gameplay => Mode::Cursor,
                Mode::Cursor => Mode::Gameplay,
            };
            self.manual = self.mode == Mode::Cursor;
        } else if opened || armed || aimed {
            self.mode = Mode::Cursor;
        } else if self.mode == Mode::Cursor && (walk || (!self.manual && !cause)) {
            self.mode = Mode::Gameplay;
            self.manual = false;
        }
        if s.panels != &self.panels {
            self.panels.clone_from(s.panels);
        }
        self.targeting = s.targeting;
        self.pointing = s.pointing;
        self.mode
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panels(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    fn step(m: &mut Machine, open: &[&str], targeting: bool, back: bool, walk: bool) -> Mode {
        m.step(Signals {
            panels: &panels(open),
            targeting,
            back,
            walk,
            pointing: false,
        })
    }

    #[test]
    fn opening_a_window_switches_in_and_closing_the_last_one_switches_out() {
        let mut m = Machine::default();
        assert_eq!(step(&mut m, &[], false, false, false), Mode::Gameplay);
        assert_eq!(
            step(&mut m, &["ContainerFrame1"], false, false, false),
            Mode::Cursor
        );
        assert_eq!(
            step(&mut m, &["ContainerFrame1"], false, false, false),
            Mode::Cursor
        );
        assert_eq!(
            step(
                &mut m,
                &["ContainerFrame1", "MerchantFrame"],
                false,
                false,
                false
            ),
            Mode::Cursor
        );
        assert_eq!(
            step(&mut m, &["MerchantFrame"], false, false, false),
            Mode::Cursor
        );
        assert_eq!(step(&mut m, &[], false, false, false), Mode::Gameplay);
    }

    #[test]
    fn walking_leaves_cursor_mode_and_the_open_window_does_not_pull_it_back() {
        let mut m = Machine::default();
        step(&mut m, &["ContainerFrame1"], false, false, false);
        assert_eq!(
            step(&mut m, &["ContainerFrame1"], false, false, true),
            Mode::Gameplay
        );
        for _ in 0..10 {
            assert_eq!(
                step(&mut m, &["ContainerFrame1"], false, false, false),
                Mode::Gameplay
            );
        }
        // Another window opening is a new cause.
        assert_eq!(
            step(
                &mut m,
                &["ContainerFrame1", "MerchantFrame"],
                false,
                false,
                false
            ),
            Mode::Cursor
        );
    }

    #[test]
    fn padback_switches_by_hand_and_a_hand_made_cursor_mode_outlasts_no_window() {
        let mut m = Machine::default();
        assert_eq!(step(&mut m, &[], false, true, false), Mode::Cursor);
        for _ in 0..5 {
            assert_eq!(
                step(&mut m, &[], false, false, false),
                Mode::Cursor,
                "nothing open, still on"
            );
        }
        assert_eq!(step(&mut m, &[], false, true, false), Mode::Gameplay);
        // By hand over an open window, then by hand off: the window stays open and the mode off.
        step(&mut m, &["CharacterFrame"], false, false, false);
        assert_eq!(
            step(&mut m, &["CharacterFrame"], false, true, false),
            Mode::Gameplay
        );
        assert_eq!(
            step(&mut m, &["CharacterFrame"], false, false, false),
            Mode::Gameplay
        );
    }

    #[test]
    fn a_hand_made_cursor_mode_ends_on_a_walk() {
        let mut m = Machine::default();
        step(&mut m, &[], false, true, false);
        assert_eq!(step(&mut m, &[], false, false, true), Mode::Gameplay);
    }

    #[test]
    fn an_armed_targeting_cursor_switches_in_and_its_end_switches_out() {
        let mut m = Machine::default();
        assert_eq!(step(&mut m, &[], true, false, false), Mode::Cursor);
        assert_eq!(step(&mut m, &[], true, false, false), Mode::Cursor);
        assert_eq!(step(&mut m, &[], false, false, false), Mode::Gameplay);
    }

    #[test]
    fn a_walk_with_the_left_stick_as_the_cursor_is_not_a_walk() {
        // The caller passes `walk = false` when the left stick is the cursor stick.
        let mut m = Machine::default();
        step(&mut m, &["ContainerFrame1"], false, false, false);
        assert_eq!(
            step(&mut m, &["ContainerFrame1"], false, false, false),
            Mode::Cursor
        );
    }

    fn aim(m: &mut Machine, open: &[&str], pointing: bool, walk: bool) -> Mode {
        m.step(Signals {
            panels: &panels(open),
            targeting: false,
            back: false,
            walk,
            pointing,
        })
    }

    #[test]
    fn a_laser_on_the_panel_is_cursor_mode_and_leaving_it_is_gameplay() {
        let mut m = Machine::default();
        assert_eq!(aim(&mut m, &[], false, false), Mode::Gameplay);
        assert_eq!(aim(&mut m, &[], true, false), Mode::Cursor);
        assert_eq!(aim(&mut m, &[], true, false), Mode::Cursor);
        assert_eq!(aim(&mut m, &[], false, false), Mode::Gameplay);
    }

    #[test]
    fn walking_with_the_laser_on_the_panel_stays_in_cursor_mode() {
        let mut m = Machine::default();
        aim(&mut m, &[], true, false);
        assert_eq!(aim(&mut m, &[], true, true), Mode::Cursor);
        // Off the panel, a walk leaves a mode a window holds, as it does without a laser.
        aim(&mut m, &["ContainerFrame1"], false, false);
        assert_eq!(
            aim(&mut m, &["ContainerFrame1"], false, true),
            Mode::Gameplay
        );
    }

    #[test]
    fn an_open_window_holds_cursor_mode_after_the_laser_leaves() {
        let mut m = Machine::default();
        aim(&mut m, &["ContainerFrame1"], true, false);
        assert_eq!(
            aim(&mut m, &["ContainerFrame1"], false, false),
            Mode::Cursor
        );
    }
}
