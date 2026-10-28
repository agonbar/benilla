//! `WOW_PAD_TRACE=1`: what a pad press resolves to, read off the live binding table the way the
//! dispatch resolves a chord (exact, then once with the leftmost modifier dropped). The trace names
//! the table's answer; a UI frame that takes the press first (a capture window, a focused box) is
//! not modelled.

use crate::layers::Modifiers;

/// A binding key split into its modifiers and base token.
fn split(key: &str) -> (Modifiers, &str) {
    let mut m = Modifiers::default();
    let mut rest = key;
    loop {
        if let Some(r) = rest.strip_prefix("ALT-") {
            m.alt = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("CTRL-") {
            m.ctrl = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("SHIFT-") {
            m.shift = true;
            rest = r;
        } else {
            return (m, rest);
        }
    }
}

/// The command `token` pressed with `mods` runs in `keys` (the live `(key, command)` table).
pub fn resolve(keys: &[(String, String)], mods: Modifiers, token: &str) -> Option<String> {
    let find = |m: Modifiers| {
        keys.iter()
            .find(|(k, _)| split(k) == (m, token))
            .map(|(_, c)| c.clone())
    };
    if let Some(c) = find(mods) {
        return Some(c);
    }
    let fallback = if mods.alt {
        Modifiers { alt: false, ..mods }
    } else if mods.ctrl {
        Modifiers {
            ctrl: false,
            ..mods
        }
    } else if mods.shift {
        Modifiers {
            shift: false,
            ..mods
        }
    } else {
        return None;
    };
    find(fallback)
}

/// The chord as the table spells it: `ALT-CTRL-SHIFT-` prefixes, then the token.
pub fn chord_text(mods: Modifiers, token: &str) -> String {
    format!(
        "{}{}{}{token}",
        if mods.alt { "ALT-" } else { "" },
        if mods.ctrl { "CTRL-" } else { "" },
        if mods.shift { "SHIFT-" } else { "" },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Vec<(String, String)> {
        [
            ("PAD1", "A"),
            ("SHIFT-PAD1", "B"),
            ("CTRL-SHIFT-PAD2", "C"),
            ("PAD2", "D"),
        ]
        .map(|(k, c)| (k.to_owned(), c.to_owned()))
        .to_vec()
    }

    #[test]
    fn exact_then_leftmost_modifier_dropped() {
        let shift = Modifiers {
            shift: true,
            ..Default::default()
        };
        let alt = Modifiers {
            alt: true,
            ..Default::default()
        };
        assert_eq!(resolve(&table(), shift, "PAD1").as_deref(), Some("B"));
        assert_eq!(resolve(&table(), alt, "PAD1").as_deref(), Some("A"));
        let cs = Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(resolve(&table(), cs, "PAD2").as_deref(), Some("C"));
        assert_eq!(resolve(&table(), shift, "PAD3"), None);
    }

    #[test]
    fn chord_text_is_alt_ctrl_shift() {
        let m = Modifiers {
            alt: true,
            ctrl: true,
            shift: true,
        };
        assert_eq!(chord_text(m, "PAD1"), "ALT-CTRL-SHIFT-PAD1");
        assert_eq!(chord_text(Modifiers::default(), "PAD1"), "PAD1");
    }
}
