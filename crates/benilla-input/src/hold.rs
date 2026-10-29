//! `PADBACK` is two buttons in one: a tap switches the mode, a long press shows or hides the
//! on-screen keyboard. A tap is known only at its release, so the mode switch waits for it.

/// Seconds `PADBACK` is held before it counts as the long press.
pub const LONG: f32 = 0.6;

/// What the button did this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Back {
    Nothing,
    /// Released before [`LONG`].
    Tap,
    /// Held to [`LONG`]; fires while still held, and the release after it does nothing.
    Long,
}

#[derive(Debug, Default)]
pub struct BackHold {
    held: Option<f32>,
    long: bool,
}

impl BackHold {
    /// One frame: whether the button is `down` and the seconds `dt` since the last.
    pub fn step(&mut self, down: bool, dt: f32) -> Back {
        match (down, self.held) {
            (true, None) => {
                self.held = Some(0.0);
                self.long = false;
                Back::Nothing
            }
            (true, Some(t)) => {
                let t = t + dt;
                self.held = Some(t);
                if t >= LONG && !self.long {
                    self.long = true;
                    Back::Long
                } else {
                    Back::Nothing
                }
            }
            (false, Some(_)) => {
                self.held = None;
                if self.long {
                    Back::Nothing
                } else {
                    Back::Tap
                }
            }
            (false, None) => Back::Nothing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(frames: &[(bool, f32)]) -> Vec<Back> {
        let mut hold = BackHold::default();
        frames.iter().map(|(d, dt)| hold.step(*d, *dt)).collect()
    }

    #[test]
    fn a_tap_fires_on_release_and_only_then() {
        let out = run(&[(true, 0.016), (true, 0.016), (false, 0.016), (false, 0.016)]);
        assert_eq!(
            out,
            [Back::Nothing, Back::Nothing, Back::Tap, Back::Nothing]
        );
    }

    #[test]
    fn a_long_press_fires_once_while_held_and_its_release_is_not_a_tap() {
        let mut frames = vec![(true, 0.0)];
        frames.extend([(true, 0.25); 4]);
        frames.push((false, 0.016));
        let out = run(&frames);
        assert_eq!(out.iter().filter(|b| **b == Back::Long).count(), 1);
        assert_eq!(out.iter().filter(|b| **b == Back::Tap).count(), 0);
        assert_eq!(
            out[3],
            Back::Long,
            "0.75 s into the hold, the first frame past 0.6"
        );
    }

    #[test]
    fn a_second_press_starts_over() {
        let out = run(&[
            (true, 0.0),
            (true, 0.7),
            (false, 0.0),
            (true, 0.0),
            (false, 0.0),
        ]);
        assert_eq!(
            out,
            [
                Back::Nothing,
                Back::Long,
                Back::Nothing,
                Back::Nothing,
                Back::Tap
            ]
        );
    }
}
