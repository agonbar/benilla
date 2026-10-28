//! The binding tokens a pad's inputs go by. The names are retail's `PAD*` set (the warcraft.wiki.gg
//! page "Game Pad buttons"); 1.12.1 has none. The stick clicks are `PADLSTICK` and `PADRSTICK`,
//! and `PAD5` and `PAD6` are further numbered buttons, which gilrs' `C` and `Z` fill.

use bevy::input::gamepad::GamepadButton;

/// The four directions a stick makes as digital inputs, in the order up, right, down, left.
pub const LEFT_STICK: [&str; 4] = [
    "PADLSTICKUP",
    "PADLSTICKRIGHT",
    "PADLSTICKDOWN",
    "PADLSTICKLEFT",
];
pub const RIGHT_STICK: [&str; 4] = [
    "PADRSTICKUP",
    "PADRSTICKRIGHT",
    "PADRSTICKDOWN",
    "PADRSTICKLEFT",
];

/// Every token a pad can press, registered with the binding store so a stored binding parses.
pub const ALL: [&str; 32] = [
    "PAD1",
    "PAD2",
    "PAD3",
    "PAD4",
    "PAD5",
    "PAD6",
    "PADDUP",
    "PADDRIGHT",
    "PADDDOWN",
    "PADDLEFT",
    "PADLSHOULDER",
    "PADLTRIGGER",
    "PADRSHOULDER",
    "PADRTRIGGER",
    "PADLSTICK",
    "PADRSTICK",
    "PADLSTICKUP",
    "PADLSTICKRIGHT",
    "PADLSTICKDOWN",
    "PADLSTICKLEFT",
    "PADRSTICKUP",
    "PADRSTICKRIGHT",
    "PADRSTICKDOWN",
    "PADRSTICKLEFT",
    "PADFORWARD",
    "PADBACK",
    "PADSYSTEM",
    "PADPADDLE1",
    "PADPADDLE2",
    "PADPADDLE3",
    "PADPADDLE4",
    "PADSOCIAL",
];

/// The registered token spelled `name`, if there is one.
pub fn find(name: &str) -> Option<&'static str> {
    ALL.iter().copied().find(|t| *t == name)
}

/// The token of a gilrs button. South, east, west and north are `PAD1` to `PAD4`. `None` for a
/// button with no token.
pub fn button(b: GamepadButton) -> Option<&'static str> {
    use GamepadButton::*;
    Some(match b {
        South => "PAD1",
        East => "PAD2",
        West => "PAD3",
        North => "PAD4",
        C => "PAD5",
        Z => "PAD6",
        DPadUp => "PADDUP",
        DPadRight => "PADDRIGHT",
        DPadDown => "PADDDOWN",
        DPadLeft => "PADDLEFT",
        LeftTrigger => "PADLSHOULDER",
        RightTrigger => "PADRSHOULDER",
        LeftTrigger2 => "PADLTRIGGER",
        RightTrigger2 => "PADRTRIGGER",
        LeftThumb => "PADLSTICK",
        RightThumb => "PADRSTICK",
        Start => "PADFORWARD",
        Select => "PADBACK",
        Mode => "PADSYSTEM",
        Other(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mapped_button_has_a_registered_token() {
        for b in GamepadButton::all() {
            if let Some(t) = button(b) {
                assert!(find(t).is_some(), "{t}");
            }
        }
    }

    #[test]
    fn tokens_are_unique_and_parse_as_chord_bases() {
        for (i, t) in ALL.iter().enumerate() {
            assert!(!ALL[..i].contains(t), "{t} twice");
            assert!(!t.contains(['-', ' ']), "{t}");
            assert_eq!(*t, t.to_ascii_uppercase());
        }
    }
}
