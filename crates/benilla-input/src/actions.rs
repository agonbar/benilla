//! What a pad button is in cursor mode: a pointer button, a snap, a wheel notch or a close. In
//! gameplay mode the same buttons press bound actions ([`crate::layers`]); this table is only for
//! the moment the mode says the cursor is the pad's.

use benilla_ui::script::snap::SnapDir;
use bevy::input::mouse::MouseButton;

/// A pad input as the cursor reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    /// A pointer button; `shift` holds the Shift key for the length of the click.
    Button {
        button: MouseButton,
        shift: bool,
        down: bool,
    },
    Wheel(f32),
    /// Close the topmost window, or cancel a targeting cursor.
    Close,
    Snap(SnapDir),
}

/// Wheel notches a second at full stick travel while the laser is on the panel.
pub const SCROLL_RATE: f32 = 12.0;

/// The act of `token` going down in cursor mode, if the cursor takes that button. With the laser
/// on the panel the right trigger is also a left click, held for a drag.
pub fn press(token: &str, pointing: bool) -> Option<Act> {
    let click = |button, shift| Act::Button {
        button,
        shift,
        down: true,
    };
    Some(match token {
        "PADRTRIGGER" if pointing => click(MouseButton::Left, false),
        // A right click uses, sells, deposits and attaches; a left click picks up and puts down;
        // a shift-left click splits a stack or links, so two clicks replace a drag.
        "PAD1" => click(MouseButton::Right, false),
        "PAD3" => click(MouseButton::Left, false),
        "PAD4" => click(MouseButton::Left, true),
        "PAD2" => Act::Close,
        "PADDUP" => Act::Snap(SnapDir::Up),
        "PADDDOWN" => Act::Snap(SnapDir::Down),
        "PADDLEFT" => Act::Snap(SnapDir::Left),
        "PADDRIGHT" => Act::Snap(SnapDir::Right),
        // The bumpers scroll: up on the left, down on the right.
        "PADLSHOULDER" => Act::Wheel(1.0),
        "PADRSHOULDER" => Act::Wheel(-1.0),
        _ => return None,
    })
}

/// The act that undoes a [`press`], for the button's release.
pub fn release(act: Act) -> Option<Act> {
    match act {
        Act::Button { button, shift, .. } => Some(Act::Button {
            button,
            shift,
            down: false,
        }),
        _ => None,
    }
}

/// Whether the cursor takes `token` in cursor mode, so it does not press its bound action. The
/// left stick's directions are the cursor's when the left stick is the cursor stick.
pub fn consumed(token: &str, left_stick_is_cursor: bool, pointing: bool) -> bool {
    press(token, pointing).is_some()
        || token == "PADBACK"
        || (left_stick_is_cursor && crate::tokens::LEFT_STICK.contains(&token))
}

/// The right stick's y travel over `dt` seconds added to the `held` fraction of a notch, as whole
/// wheel notches (up is positive) and what is left over. A slow push still scrolls, a notch at a
/// time, and a stick at rest keeps nothing.
pub fn scroll(held: f32, stick_y: f32, dt: f32) -> (f32, f32) {
    if stick_y == 0.0 {
        return (0.0, 0.0);
    }
    let total = held + stick_y * SCROLL_RATE * dt;
    let notches = total.trunc();
    (notches, total - notches)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_face_buttons_click_and_the_d_pad_snaps() {
        assert_eq!(
            press("PAD1", false),
            Some(Act::Button {
                button: MouseButton::Right,
                shift: false,
                down: true
            })
        );
        assert!(matches!(
            press("PAD4", false),
            Some(Act::Button {
                button: MouseButton::Left,
                shift: true,
                down: true
            })
        ));
        assert_eq!(press("PADDLEFT", false), Some(Act::Snap(SnapDir::Left)));
        assert_eq!(press("PAD2", false), Some(Act::Close));
        assert_eq!(
            press("PADLSTICK", false),
            None,
            "the stick click still jumps"
        );
    }

    #[test]
    fn only_a_button_has_a_release() {
        let down = press("PAD3", false).unwrap();
        assert!(matches!(
            release(down),
            Some(Act::Button { down: false, .. })
        ));
        assert_eq!(release(Act::Close), None);
        assert_eq!(release(Act::Wheel(1.0)), None);
    }

    #[test]
    fn the_left_stick_is_the_cursors_only_when_the_setting_says_so() {
        assert!(!consumed("PADLSTICKUP", false, false));
        assert!(consumed("PADLSTICKUP", true, false));
        assert!(consumed("PAD1", false, false));
        assert!(consumed("PADBACK", false, false));
        assert!(
            !consumed("PADFORWARD", false, false),
            "the menu button still opens the game menu"
        );
    }

    #[test]
    fn the_right_trigger_clicks_only_while_the_laser_is_on_the_panel() {
        assert_eq!(press("PADRTRIGGER", false), None);
        assert!(!consumed("PADRTRIGGER", false, false));
        assert_eq!(
            press("PADRTRIGGER", true),
            Some(Act::Button {
                button: MouseButton::Left,
                shift: false,
                down: true
            })
        );
        assert!(consumed("PADRTRIGGER", false, true));
        assert_eq!(press("PADLTRIGGER", true), None);
    }

    #[test]
    fn the_stick_scrolls_in_whole_notches_and_keeps_the_rest() {
        // Full travel is 12 notches a second: 0.05 s is 0.6 of one.
        let (n, held) = scroll(0.0, 1.0, 0.05);
        assert_eq!(n, 0.0);
        assert!((held - 0.6).abs() < 1e-5, "{held}");
        let (n, held) = scroll(held, 1.0, 0.05);
        assert_eq!(n, 1.0);
        assert!((held - 0.2).abs() < 1e-5, "{held}");
        // Down is negative and the remainder keeps its sign.
        let (n, held) = scroll(0.0, -1.0, 0.2);
        assert_eq!(n, -2.0);
        assert!((held + 0.4).abs() < 1e-5, "{held}");
        // A stick at rest drops what it had built up.
        assert_eq!(scroll(0.9, 0.0, 0.05), (0.0, 0.0));
    }
}
