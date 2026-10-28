//! The pad's virtual cursor, in logical window pixels, y down, like `Window::cursor_position`.
//!
//! The OS mouse and the pad's cursor can both be live, as on a Steam Deck with its trackpad (a
//! mouse) and its sticks. One owns the cursor at a time. A real mouse motion or click gives it to
//! the OS pointer, without leaving cursor mode; the pad's next cursor input takes it back, from
//! where the OS pointer is, so neither jump when it changes hands.

use bevy::prelude::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Owner {
    #[default]
    Os,
    Pad,
}

/// What moves the cursor this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    /// The window's size, logical px.
    pub size: Vec2,
    /// Where the OS pointer is, if it is in the window.
    pub os: Option<Vec2>,
    /// The cursor stick, shaped, y up.
    pub stick: Vec2,
    pub dt: f32,
    /// Full deflection, px a second.
    pub speed: f32,
    /// A d-pad snap landed here.
    pub snap: Option<Vec2>,
    /// The pad did something that is the cursor's (a click button), without moving it.
    pub pad_action: bool,
    /// A real mouse motion or click.
    pub real: bool,
}

#[derive(Debug, Default)]
pub struct VirtualCursor {
    pub owner: Owner,
    pub pos: Vec2,
}

impl VirtualCursor {
    /// One frame. Returns whether the pad's cursor moved, for the caller to move the OS pointer to.
    pub fn step(&mut self, i: &Input) -> bool {
        let pad_input = i.stick != Vec2::ZERO || i.snap.is_some() || i.pad_action;
        if i.real {
            self.owner = Owner::Os;
            if let Some(os) = i.os {
                self.pos = os;
            }
        } else if self.owner == Owner::Os && pad_input {
            self.owner = Owner::Pad;
            self.pos =
                i.os.or_else(|| (self.pos != Vec2::ZERO).then_some(self.pos))
                    .unwrap_or(i.size / 2.0);
        }
        if self.owner != Owner::Pad {
            return false;
        }
        let before = self.pos;
        match i.snap {
            Some(to) => self.pos = to,
            None => self.pos += delta(i.stick, i.speed, i.dt),
        }
        // Inside the window, the last pixel included.
        self.pos = self
            .pos
            .clamp(Vec2::ZERO, (i.size - Vec2::ONE).max(Vec2::ZERO));
        self.pos != before
    }
}

/// The cursor's move for a shaped stick over `dt` seconds. Quadratic in the travel, so a small
/// push places the cursor finely and full travel is `speed`. Stick up is cursor up, negative y.
pub fn delta(stick: Vec2, speed: f32, dt: f32) -> Vec2 {
    let m = stick.length();
    if m == 0.0 {
        return Vec2::ZERO;
    }
    let dir = stick / m;
    Vec2::new(dir.x, -dir.y) * (m * m * speed * dt)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Vec2 = Vec2::new(1280.0, 800.0);

    fn input() -> Input {
        Input {
            size: SIZE,
            dt: 0.1,
            speed: 1000.0,
            ..default()
        }
    }

    #[test]
    fn the_pad_takes_the_cursor_from_where_the_os_pointer_is() {
        let mut c = VirtualCursor::default();
        let at = Vec2::new(300.0, 200.0);
        assert!(!c.step(&Input {
            os: Some(at),
            ..input()
        }));
        assert_eq!(c.owner, Owner::Os, "idle input takes nothing");
        c.step(&Input {
            os: Some(at),
            stick: Vec2::new(1.0, 0.0),
            ..input()
        });
        assert_eq!(c.owner, Owner::Pad);
        assert_eq!(
            c.pos,
            at + Vec2::new(100.0, 0.0),
            "from the pointer, no jump"
        );
    }

    #[test]
    fn a_real_mouse_takes_it_back_where_the_pointer_is() {
        let mut c = VirtualCursor::default();
        c.step(&Input {
            os: Some(Vec2::new(10.0, 10.0)),
            stick: Vec2::new(0.0, -1.0),
            ..input()
        });
        assert_eq!(c.owner, Owner::Pad);
        assert!(!c.step(&Input {
            os: Some(Vec2::new(640.0, 400.0)),
            real: true,
            ..input()
        }));
        assert_eq!(c.owner, Owner::Os);
        assert_eq!(c.pos, Vec2::new(640.0, 400.0));
        // And a pad press with the mouse at rest takes it again from there.
        c.step(&Input {
            os: Some(Vec2::new(640.0, 400.0)),
            pad_action: true,
            ..input()
        });
        assert_eq!(c.owner, Owner::Pad);
        assert_eq!(c.pos, Vec2::new(640.0, 400.0));
    }

    #[test]
    fn with_no_os_pointer_the_cursor_starts_at_the_middle_then_where_it_was() {
        let mut c = VirtualCursor::default();
        c.step(&Input {
            pad_action: true,
            ..input()
        });
        assert_eq!(c.pos, SIZE / 2.0);
        c.owner = Owner::Os;
        c.step(&Input {
            pad_action: true,
            ..input()
        });
        assert_eq!(c.pos, SIZE / 2.0);
    }

    #[test]
    fn a_snap_lands_on_the_target_and_the_cursor_stays_inside_the_window() {
        let mut c = VirtualCursor::default();
        assert!(c.step(&Input {
            snap: Some(Vec2::new(5.0, 6.0)),
            ..input()
        }));
        assert_eq!(c.pos, Vec2::new(5.0, 6.0));
        c.step(&Input {
            stick: Vec2::new(-1.0, 1.0).normalize(),
            ..input()
        });
        assert_eq!(c.pos, Vec2::ZERO, "pinned at the top left");
        c.step(&Input {
            snap: Some(Vec2::new(9999.0, 9999.0)),
            ..input()
        });
        assert_eq!(c.pos, SIZE - Vec2::ONE);
    }

    #[test]
    fn the_stick_is_quadratic_and_up_is_up() {
        let up = delta(Vec2::new(0.0, 1.0), 1000.0, 0.5);
        assert!(up.x == 0.0 && (up.y + 500.0).abs() < 1e-3, "{up}");
        let half = delta(Vec2::new(0.5, 0.0), 1000.0, 1.0).x;
        assert!((half - 250.0).abs() < 1e-3, "{half}");
        assert_eq!(delta(Vec2::ZERO, 1000.0, 1.0), Vec2::ZERO);
    }
}
