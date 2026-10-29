//! The on-screen keyboard's output: what a key of the shipped addon's keyboard frame becomes as
//! input. A typed character is a [`KeyboardInput`] with `text` on a key no layout names, which
//! `benilla-app`'s keyboard feed (`ui_script/input.rs`, `feed_keys`) hands to the focused EditBox as
//! a typed character and to no binding. Backspace and Enter are the real keys, since the EditBox
//! edits and sends on them.

use bevy::ecs::entity::Entity;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::input::ButtonState;

/// The key a typed character rides on: one no layout names (`chord::key_token` and `walk_name` in
/// `benilla-app` answer `None`), so no binding and no key walk takes it and only the text counts.
const TEXT_KEY: KeyCode = KeyCode::Lang1;

/// One thing the keyboard frame asked to be typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Typed {
    /// A character, or a few, as the frame's key shows it.
    Text(String),
    /// A key the EditBox acts on.
    Key(KeyCode),
}

/// The key the addon names: `BACKSPACE` or `ENTER`.
pub fn named_key(name: &str) -> Option<KeyCode> {
    match name {
        "BACKSPACE" => Some(KeyCode::Backspace),
        "ENTER" => Some(KeyCode::Enter),
        _ => None,
    }
}

impl Typed {
    /// The key press and the release a frame writes for it, on `window`.
    pub fn events(&self, window: Entity) -> [KeyboardInput; 2] {
        let (key_code, logical_key, text) = match self {
            Typed::Text(t) => (
                TEXT_KEY,
                Key::Character(t.as_str().into()),
                Some(t.as_str().into()),
            ),
            Typed::Key(KeyCode::Backspace) => (KeyCode::Backspace, Key::Backspace, None),
            Typed::Key(code) => (*code, Key::Enter, None),
        };
        let event = |state, text| KeyboardInput {
            key_code,
            logical_key: logical_key.clone(),
            state,
            text,
            repeat: false,
            window,
        };
        [
            event(ButtonState::Pressed, text),
            event(ButtonState::Released, None),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_is_text_on_a_key_no_binding_names() {
        let [down, up] = Typed::Text("q".into()).events(Entity::PLACEHOLDER);
        assert_eq!(down.key_code, TEXT_KEY);
        assert_eq!(down.state, ButtonState::Pressed);
        assert_eq!(down.text.as_deref(), Some("q"));
        assert_eq!(up.state, ButtonState::Released);
        assert_eq!(up.text, None);
        assert_eq!(up.key_code, down.key_code);
    }

    #[test]
    fn backspace_and_enter_are_their_own_keys_with_no_text() {
        for (name, code) in [("BACKSPACE", KeyCode::Backspace), ("ENTER", KeyCode::Enter)] {
            assert_eq!(named_key(name), Some(code));
            let [down, up] = Typed::Key(code).events(Entity::PLACEHOLDER);
            assert_eq!((down.key_code, down.text), (code, None));
            assert_eq!(up.state, ButtonState::Released);
        }
        assert_eq!(named_key("ESCAPE"), None);
    }
}
