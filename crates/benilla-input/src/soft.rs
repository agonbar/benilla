//! The buttons that act on the soft target ([`benilla_app::SoftTargetInput`]) in gameplay mode. A
//! lock is a device-layer action, not a binding: 1.12 has no command for one, so it cannot be
//! rebound in the Key Bindings window, and the shoulders no longer press a bound command.
//!
//! | Input | Action |
//! |---|---|
//! | `PADLSHOULDER` | lock the next friend |
//! | `PADRSHOULDER` | lock the next enemy |
//! | `SHIFT-PADLSHOULDER` held, then the d-pad | `TargetUnit("party1")` to `party4`, up, right, down, left |
//!
//! A press with any other modifier falls through to the bindings. The interact is two bindable
//! commands the shipped addon declares, `INTERACTTARGET` and `INTERACTORJUMP`. A body only asks
//! ([`asks`]), and [`ask_action`] says what the ask does given what the soft target has in front.

use std::collections::BTreeSet;

use benilla_app::{SoftSide, SoftTargetInput};

use crate::layers::Modifiers;

/// What a pad press does to targeting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Soft(SoftTargetInput),
    /// `TargetUnit("party<n>")`, `n` from 1 to 4.
    Party(u8),
    /// Taken, and does nothing: the left bumper that opens the party layer.
    Layer,
}

impl Press {
    /// The Lua that carries a party press out, through the verb the reference has.
    pub fn lua(self) -> Option<String> {
        match self {
            Press::Party(n) => Some(format!("TargetUnit(\"party{n}\")")),
            _ => None,
        }
    }
}

/// The party member a d-pad token names, clockwise from the top.
fn party_slot(token: &str) -> Option<u8> {
    Some(match token {
        "PADDUP" => 1,
        "PADDRIGHT" => 2,
        "PADDDOWN" => 3,
        "PADDLEFT" => 4,
        _ => return None,
    })
}

/// What an interact command asks for, in the addon's words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// `INTERACTTARGET`: interact with the soft target.
    Interact,
    /// `INTERACTORJUMP`: interact when there is a service in front, else jump.
    InteractOrJump,
}

/// The Lua that takes the addon's pending asks, one word each, and clears them.
pub const TAKE_ASKS: &str = "local a = BenillaPadAsks BenillaPadAsks = nil return a";

/// The asks in the text [`TAKE_ASKS`] returned, in order. An unknown word is dropped.
pub fn asks(text: &str) -> Vec<Ask> {
    text.split_whitespace()
        .filter_map(|w| match w {
            "interact" => Some(Ask::Interact),
            "interactorjump" => Some(Ask::InteractOrJump),
            _ => None,
        })
        .collect()
}

/// What an ask does: the interact, or the jump. `ready` is whether the soft target has an NPC, a
/// corpse or an object in front ([`benilla_app::SoftTargetReady`]).
pub fn ask_action(ask: Ask, ready: bool) -> Option<SoftTargetInput> {
    match ask {
        Ask::Interact => Some(SoftTargetInput::Interact),
        Ask::InteractOrJump => ready.then_some(SoftTargetInput::Interact),
    }
}

/// What `token` going down does, with the layer it carries and everything held at that moment.
pub fn press(token: &str, mods: Modifiers, down: &BTreeSet<&'static str>) -> Option<Press> {
    let plain = mods == Modifiers::default();
    let shift = mods
        == Modifiers {
            shift: true,
            ..Modifiers::default()
        };
    match token {
        "PADLSHOULDER" if plain => Some(Press::Soft(SoftTargetInput::Cycle(SoftSide::Friend))),
        "PADLSHOULDER" if shift => Some(Press::Layer),
        "PADRSHOULDER" if plain => Some(Press::Soft(SoftTargetInput::Cycle(SoftSide::Enemy))),
        t if shift && down.contains("PADLSHOULDER") => party_slot(t).map(Press::Party),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers {
        alt: false,
        ctrl: false,
        shift: false,
    };
    const SHIFT: Modifiers = Modifiers {
        shift: true,
        ..NONE
    };
    const CTRL: Modifiers = Modifiers { ctrl: true, ..NONE };
    const ALT: Modifiers = Modifiers { alt: true, ..NONE };

    fn held(tokens: &[&'static str]) -> BTreeSet<&'static str> {
        tokens.iter().copied().collect()
    }

    #[test]
    fn the_bumpers_lock_a_side_and_the_trigger_layer_is_a_binding() {
        let down = held(&[]);
        assert_eq!(
            press("PADLSHOULDER", NONE, &down),
            Some(Press::Soft(SoftTargetInput::Cycle(SoftSide::Friend)))
        );
        assert_eq!(
            press("PADRSHOULDER", NONE, &down),
            Some(Press::Soft(SoftTargetInput::Cycle(SoftSide::Enemy)))
        );
        // The other layers leave the bumpers to the bindings, which is where the interact is.
        assert_eq!(press("PADRSHOULDER", SHIFT, &down), None);
        for m in [CTRL, ALT] {
            assert_eq!(press("PADRSHOULDER", m, &down), None);
            assert_eq!(press("PADLSHOULDER", m, &down), None);
        }
    }

    #[test]
    fn the_left_bumper_under_the_left_trigger_opens_the_party_layer_and_locks_nothing() {
        assert_eq!(
            press("PADLSHOULDER", SHIFT, &held(&["PADLTRIGGER"])),
            Some(Press::Layer)
        );
    }

    #[test]
    fn the_d_pad_in_the_party_layer_names_a_member_and_outside_it_is_a_bar_slot() {
        let layer = held(&["PADLTRIGGER", "PADLSHOULDER"]);
        let slots = [
            ("PADDUP", 1),
            ("PADDRIGHT", 2),
            ("PADDDOWN", 3),
            ("PADDLEFT", 4),
        ];
        for (token, n) in slots {
            assert_eq!(
                press(token, SHIFT, &layer),
                Some(Press::Party(n)),
                "{token}"
            );
            // Without the bumper, or without the trigger, it is the bar's.
            assert_eq!(press(token, SHIFT, &held(&["PADLTRIGGER"])), None);
            assert_eq!(press(token, NONE, &layer), None);
        }
        // The face buttons are never party slots.
        assert_eq!(press("PAD1", SHIFT, &layer), None);
    }

    #[test]
    fn a_party_press_runs_the_reference_verb() {
        assert_eq!(
            Press::Party(3).lua().as_deref(),
            Some("TargetUnit(\"party3\")")
        );
        assert_eq!(Press::Layer.lua(), None);
    }

    #[test]
    fn the_addons_asks_parse_in_order_and_drop_what_they_do_not_know() {
        assert_eq!(asks(""), []);
        assert_eq!(
            asks("interact interactorjump nonsense interact "),
            [Ask::Interact, Ask::InteractOrJump, Ask::Interact]
        );
    }

    #[test]
    fn the_contextual_ask_interacts_with_a_service_and_otherwise_jumps() {
        assert_eq!(
            ask_action(Ask::InteractOrJump, true),
            Some(SoftTargetInput::Interact)
        );
        assert_eq!(ask_action(Ask::InteractOrJump, false), None, "a jump");
        // The plain interact asks the soft target either way: it may attack an enemy in front.
        assert_eq!(
            ask_action(Ask::Interact, false),
            Some(SoftTargetInput::Interact)
        );
    }
}
