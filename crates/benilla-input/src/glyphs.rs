//! What the bars call a pad input: the short labels the hotkey text shows in controller
//! presentation (`A`, `LT-A`). The addon reads them from `BenillaPadGlyphs` and `BenillaPadLayers`,
//! which [`lua_tables`] writes, so the labels live here once and follow the emulation settings.

use crate::config::{PadConfig, Source};

/// The label of a pad token; `None` for one the bars never show.
pub fn glyph(token: &str) -> Option<&'static str> {
    Some(match token {
        "PAD1" => "A",
        "PAD2" => "B",
        "PAD3" => "X",
        "PAD4" => "Y",
        "PAD5" => "C",
        "PAD6" => "Z",
        "PADDUP" => "DU",
        "PADDRIGHT" => "DR",
        "PADDDOWN" => "DD",
        "PADDLEFT" => "DL",
        "PADLSHOULDER" => "LB",
        "PADRSHOULDER" => "RB",
        "PADLTRIGGER" => "LT",
        "PADRTRIGGER" => "RT",
        "PADLSTICK" => "L3",
        "PADRSTICK" => "R3",
        "PADLSTICKUP" => "LSU",
        "PADLSTICKRIGHT" => "LSR",
        "PADLSTICKDOWN" => "LSD",
        "PADLSTICKLEFT" => "LSL",
        "PADRSTICKUP" => "RSU",
        "PADRSTICKRIGHT" => "RSR",
        "PADRSTICKDOWN" => "RSD",
        "PADRSTICKLEFT" => "RSL",
        "PADFORWARD" => "Menu",
        "PADBACK" => "View",
        "PADSYSTEM" => "Home",
        "PADPADDLE1" => "P1",
        "PADPADDLE2" => "P2",
        "PADPADDLE3" => "P3",
        "PADPADDLE4" => "P4",
        "PADSOCIAL" => "Share",
        _ => return None,
    })
}

/// What a modifier prefix (`SHIFT`, `CTRL`, `ALT`) reads as: the glyph of the input that holds it.
/// `both` is the shift and ctrl inputs together.
pub fn layer_label(cfg: &PadConfig, modifier: &str) -> Option<String> {
    let of = |s: Source| match s {
        Source::Token(t) => glyph(t).map(str::to_owned),
        Source::Off | Source::Both => None,
    };
    match modifier {
        "SHIFT" => of(cfg.shift),
        "CTRL" => of(cfg.ctrl),
        "ALT" => match cfg.alt {
            Source::Both => Some(format!(
                "{}+{}",
                of(cfg.shift).unwrap_or_default(),
                of(cfg.ctrl).unwrap_or_default()
            )),
            s => of(s),
        },
        _ => None,
    }
}

/// A Lua chunk that sets `BenillaPadGlyphs` (token to label) and `BenillaPadLayers` (modifier to
/// label).
pub fn lua_tables(cfg: &PadConfig) -> String {
    let mut out = String::from("BenillaPadGlyphs = {");
    for t in crate::tokens::ALL {
        if let Some(g) = glyph(t) {
            out.push_str(&format!("{t}=\"{g}\","));
        }
    }
    out.push_str("}\nBenillaPadLayers = {");
    for m in ["SHIFT", "CTRL", "ALT"] {
        if let Some(l) = layer_label(cfg, m) {
            out.push_str(&format!("{m}=\"{l}\","));
        }
    }
    out.push('}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bar_token_has_a_short_label() {
        for t in crate::layers::SLOTS {
            let g = glyph(t).expect(t);
            assert!(g.len() <= 2, "{t}: {g}");
        }
        assert_eq!(glyph("PAD1"), Some("A"));
        assert_eq!(glyph("PADDDOWN"), Some("DD"));
    }

    #[test]
    fn labels_are_unique_and_have_no_hyphen() {
        let all: Vec<_> = crate::tokens::ALL.iter().filter_map(|t| glyph(t)).collect();
        for (i, g) in all.iter().enumerate() {
            assert!(!all[..i].contains(g), "{g} twice");
            assert!(!g.contains('-'), "{g}");
        }
    }

    #[test]
    fn the_layers_read_as_the_triggers_that_hold_them() {
        let cfg = PadConfig::default();
        assert_eq!(layer_label(&cfg, "SHIFT").as_deref(), Some("LT"));
        assert_eq!(layer_label(&cfg, "CTRL").as_deref(), Some("RT"));
        assert_eq!(layer_label(&cfg, "ALT").as_deref(), Some("LT+RT"));
        let (cfg, _) = PadConfig::parse("emulate_shift = \"PADLSHOULDER\"\nemulate_alt = \"PAD1\"");
        assert_eq!(layer_label(&cfg, "SHIFT").as_deref(), Some("LB"));
        assert_eq!(layer_label(&cfg, "ALT").as_deref(), Some("A"));
        assert_eq!(layer_label(&cfg, "META"), None);
    }

    #[test]
    fn the_chunk_lists_both_tables() {
        let chunk = lua_tables(&PadConfig::default());
        assert!(chunk.contains("PAD1=\"A\""), "{chunk}");
        assert!(chunk.contains("SHIFT=\"LT\""), "{chunk}");
        assert!(chunk.contains("ALT=\"LT+RT\""), "{chunk}");
    }
}
