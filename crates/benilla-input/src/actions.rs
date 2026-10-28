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

/// The act of `token` going down in cursor mode, if the cursor takes that button.
pub fn press(token: &str) -> Option<Act> {
    let click = |button, shift| Act::Button {
        button,
        shift,
        down: true,
    };
    Some(match token {
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
pub fn consumed(token: &str, left_stick_is_cursor: bool) -> bool {
    press(token).is_some()
        || token == "PADBACK"
        || (left_stick_is_cursor && crate::tokens::LEFT_STICK.contains(&token))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_face_buttons_click_and_the_d_pad_snaps() {
        assert_eq!(
            press("PAD1"),
            Some(Act::Button {
                button: MouseButton::Right,
                shift: false,
                down: true
            })
        );
        assert!(matches!(
            press("PAD4"),
            Some(Act::Button {
                button: MouseButton::Left,
                shift: true,
                down: true
            })
        ));
        assert_eq!(press("PADDLEFT"), Some(Act::Snap(SnapDir::Left)));
        assert_eq!(press("PAD2"), Some(Act::Close));
        assert_eq!(press("PADLSTICK"), None, "the stick click still jumps");
    }

    #[test]
    fn only_a_button_has_a_release() {
        let down = press("PAD3").unwrap();
        assert!(matches!(
            release(down),
            Some(Act::Button { down: false, .. })
        ));
        assert_eq!(release(Act::Close), None);
        assert_eq!(release(Act::Wheel(1.0)), None);
    }

    #[test]
    fn the_left_stick_is_the_cursors_only_when_the_setting_says_so() {
        assert!(!consumed("PADLSTICKUP", false));
        assert!(consumed("PADLSTICKUP", true));
        assert!(consumed("PAD1", false));
        assert!(consumed("PADBACK", false));
        assert!(
            !consumed("PADFORWARD", false),
            "the menu button still opens the game menu"
        );
    }
}
