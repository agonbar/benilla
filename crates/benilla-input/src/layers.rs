//! The default pad bindings and the modifier layers. Holding the left trigger adds `SHIFT-` to
//! what the buttons press, the right trigger `CTRL-`, and both `ALT-` alone, so a miss in a layer
//! falls back (the store drops the leftmost modifier) to the plain button and never to a
//! neighbouring layer. The table is data: the player rebinds any of it in the Key Bindings window.

use crate::config::{PadConfig, Source};
use crate::device::PadState;

/// The modifiers a press carries, as the chord's prefixes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
}

/// What the emulated modifiers say right now. With `emulate_alt = "both"`, the shift and ctrl
/// sources held together are `ALT-` and neither of the other two.
pub fn modifiers(cfg: &PadConfig, state: &PadState) -> Modifiers {
    let held = |s: Source| matches!(s, Source::Token(t) if state.is_down(t));
    let (shift, ctrl) = (held(cfg.shift), held(cfg.ctrl));
    if cfg.alt == Source::Both && shift && ctrl {
        return Modifiers {
            alt: true,
            ctrl: false,
            shift: false,
        };
    }
    Modifiers {
        alt: held(cfg.alt),
        ctrl,
        shift,
    }
}

/// The eight buttons a layer puts actions on, in slot order: the face buttons, then the d-pad
/// down, right, left, up.
pub const SLOTS: [&str; 8] = [
    "PAD1",
    "PAD2",
    "PAD3",
    "PAD4",
    "PADDDOWN",
    "PADDRIGHT",
    "PADDLEFT",
    "PADDUP",
];

/// The command on slot `i` (0 to 7) of each layer: plain, `SHIFT-`, `CTRL-`, `ALT-`. 32 in all.
fn layer_command(layer: usize, i: usize) -> String {
    let n = i + 1;
    match layer {
        0 => format!("ACTIONBUTTON{n}"),
        1 if n <= 4 => format!("ACTIONBUTTON{}", n + 8),
        1 => format!("MULTIACTIONBAR1BUTTON{}", n - 4),
        2 => format!("MULTIACTIONBAR1BUTTON{}", n + 4),
        _ => format!("MULTIACTIONBAR2BUTTON{n}"),
    }
}

const LAYER_PREFIX: [&str; 4] = ["", "SHIFT-", "CTRL-", "ALT-"];

/// The default `(key, command)` pairs, added to the player's defaults.
pub fn default_bindings() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (layer, prefix) in LAYER_PREFIX.iter().enumerate() {
        for (i, slot) in SLOTS.iter().enumerate() {
            // `PAD1`, the A button, is the contextual interact or jump, so bar slot 1 of the plain
            // layer moves to the left stick's click.
            let key = if (layer, i) == (0, 0) {
                "PADLSTICK".to_owned()
            } else {
                format!("{prefix}{slot}")
            };
            out.push((key, layer_command(layer, i)));
        }
    }
    // Movement from the left stick's four directions; two together are a diagonal.
    for (key, command) in [
        ("PADLSTICKUP", "MOVEFORWARD"),
        ("PADLSTICKDOWN", "MOVEBACKWARD"),
        ("PADLSTICKLEFT", "STRAFELEFT"),
        ("PADLSTICKRIGHT", "STRAFERIGHT"),
        // A interacts with the NPC, corpse or object in front and jumps when there is none. `JUMP`
        // and `INTERACTTARGET` stay separate commands for a player who wants two buttons.
        ("PAD1", "INTERACTORJUMP"),
        // The right stick's click, `PADRSTICK` in retail's names.
        ("PADRSTICK", "TOGGLEAUTORUN"),
        // The bumpers lock the soft target's friend and enemy: `crate::soft`, not a binding. The
        // left trigger and the right bumper interact.
        ("SHIFT-PADRSHOULDER", "INTERACTTARGET"),
        ("PADFORWARD", "TOGGLEGAMEMENU"),
        // The radial ring, held. The back buttons are what a Steam Input layout maps there; the
        // right stick's click under the left trigger is the one every pad has.
        ("PADPADDLE1", "RADIALRING"),
        ("SHIFT-PADRSTICK", "RADIALRING"),
    ] {
        out.push((key.to_owned(), command.to_owned()));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn state(down: &[&'static str]) -> PadState {
        crate::device::state_with(PadState::default(), down)
    }

    #[test]
    fn the_table_has_32_distinct_action_slots_and_distinct_keys() {
        let table = default_bindings();
        let keys: HashSet<_> = table.iter().map(|(k, _)| k).collect();
        assert_eq!(keys.len(), table.len(), "a key bound twice");
        let slots: Vec<_> = table
            .iter()
            .filter(|(_, c)| c.contains("ACTIONBUTTON") || c.contains("ACTIONBAR"))
            .collect();
        assert_eq!(slots.len(), 32);
        let commands: HashSet<_> = slots.iter().map(|(_, c)| c).collect();
        assert_eq!(commands.len(), 32, "a command on two slots");
    }

    #[test]
    fn layers_hold_the_planned_commands() {
        let table: std::collections::HashMap<_, _> = default_bindings().into_iter().collect();
        assert_eq!(table["PAD1"], "INTERACTORJUMP");
        assert_eq!(table["PADLSTICK"], "ACTIONBUTTON1");
        assert_eq!(table["PAD2"], "ACTIONBUTTON2");
        assert_eq!(table["PADDUP"], "ACTIONBUTTON8");
        assert_eq!(table["SHIFT-PAD1"], "ACTIONBUTTON9");
        assert_eq!(table["SHIFT-PAD4"], "ACTIONBUTTON12");
        assert_eq!(table["SHIFT-PADDDOWN"], "MULTIACTIONBAR1BUTTON1");
        assert_eq!(table["SHIFT-PADDUP"], "MULTIACTIONBAR1BUTTON4");
        assert_eq!(table["CTRL-PAD1"], "MULTIACTIONBAR1BUTTON5");
        assert_eq!(table["CTRL-PADDUP"], "MULTIACTIONBAR1BUTTON12");
        assert_eq!(table["ALT-PAD1"], "MULTIACTIONBAR2BUTTON1");
        assert_eq!(table["ALT-PADDUP"], "MULTIACTIONBAR2BUTTON8");
        assert!(
            !table.values().any(|c| c == "JUMP"),
            "JUMP is a command to bind, not a default"
        );
        assert_eq!(table["SHIFT-PADRSHOULDER"], "INTERACTTARGET");
        assert_eq!(table["PADRSTICK"], "TOGGLEAUTORUN");
        assert_eq!(table["PADFORWARD"], "TOGGLEGAMEMENU");
        assert!(
            !table.contains_key("PADBACK"),
            "PADBACK is the mode switch, not a binding"
        );
        for key in ["PADLSHOULDER", "PADRSHOULDER"] {
            assert!(!table.contains_key(key), "{key} is the soft target's");
        }
    }

    #[test]
    fn every_default_key_is_a_registered_token_or_a_layer_of_one() {
        for (key, _) in default_bindings() {
            let base = key.rsplit('-').next().unwrap();
            assert!(crate::tokens::find(base).is_some(), "{key}");
        }
    }

    #[test]
    fn the_triggers_are_the_layers() {
        let cfg = PadConfig::default();
        let m = |down: &[&'static str]| modifiers(&cfg, &state(down));
        assert_eq!(m(&[]), Modifiers::default());
        assert_eq!(
            m(&["PADLTRIGGER"]),
            Modifiers {
                shift: true,
                ..Default::default()
            }
        );
        assert_eq!(
            m(&["PADRTRIGGER"]),
            Modifiers {
                ctrl: true,
                ..Default::default()
            }
        );
        assert_eq!(
            m(&["PADLTRIGGER", "PADRTRIGGER"]),
            Modifiers {
                alt: true,
                ..Default::default()
            }
        );
    }

    #[test]
    fn emulation_sources_follow_the_config() {
        let (cfg, _) = PadConfig::parse(
            "emulate_shift = \"PADLSHOULDER\"\nemulate_ctrl = \"\"\nemulate_alt = \"PAD1\"",
        );
        let m = |down: &[&'static str]| modifiers(&cfg, &state(down));
        assert!(m(&["PADLSHOULDER"]).shift);
        assert!(!m(&["PADRTRIGGER"]).ctrl, "off is off");
        assert!(m(&["PAD1"]).alt);
        // With alt on its own token, shift keeps working beside it.
        let both = m(&["PADLSHOULDER", "PAD1"]);
        assert!(both.shift && both.alt);
    }

    /// Every command the defaults bind is one the stock `Bindings.xml` declares; a misspelled one
    /// would bind to nothing and say so nowhere.
    #[test]
    fn every_default_command_is_declared_by_the_stock_bindings_file() {
        let data = benilla_formats::wow_data_or_skip!();
        let chain = benilla_formats::Chain::open(&data).expect("the install's archives open");
        let bytes = chain
            .read(r"Interface\FrameXML\Bindings.xml")
            .expect("the chain has Bindings.xml");
        // The stock file's commands, and the two the shipped addon declares.
        let declared: HashSet<String> =
            benilla_ui::bindings_xml::parse(&benilla_ui::source::decode(&bytes))
                .expect("parses")
                .into_iter()
                .chain(benilla_ui::bindings_xml::parse(crate::addon::BINDINGS).expect("parses"))
                .map(|b| b.name)
                .collect();
        let unknown: Vec<_> = default_bindings()
            .into_iter()
            .map(|(_, command)| command)
            .filter(|c| !declared.contains(c))
            .collect();
        assert!(unknown.is_empty(), "undeclared commands: {unknown:?}");
    }
}
