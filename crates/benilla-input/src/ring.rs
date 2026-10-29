//! The radial ring's pick: which slot a stick angle points at, and the entries the ring offers. The
//! shipped addon draws the ring and fires the entry on release; this is the part that is only
//! arithmetic, so it is tested without a UI.

/// The ring's entries, clockwise from the top: the label, then the binding command the release
/// runs (`RunBinding`). Every command is one the stock `Bindings.xml` declares, or one the shipped
/// addon does (`TOGGLEPADKEYBOARD`).
pub const ENTRIES: [(&str, &str); 8] = [
    ("Character", "TOGGLECHARACTER0"),
    ("Spellbook", "TOGGLESPELLBOOK"),
    ("Talents", "TOGGLETALENTS"),
    ("Quests", "TOGGLEQUESTLOG"),
    ("Bags", "OPENALLBAGS"),
    ("Map", "TOGGLEWORLDMAP"),
    ("Social", "TOGGLEFRIENDSTAB"),
    ("Keyboard", "TOGGLEPADKEYBOARD"),
];

/// How far the stick must be tilted, after its dead zone is rescaled away, to pick a slot.
pub const MIN_TILT: f32 = 0.5;

/// The slot (0-based, clockwise from the top) a stick at `(x, y)`, y up, points at in a ring of
/// `n` slots; `None` for a stick tilted less than [`MIN_TILT`], which picks nothing.
pub fn slot(x: f32, y: f32, n: usize) -> Option<usize> {
    if n == 0 || x * x + y * y < MIN_TILT * MIN_TILT {
        return None;
    }
    // Clockwise from up, in turns: up is 0, right is a quarter.
    let turns = x.atan2(y) / std::f32::consts::TAU;
    let slot = (turns * n as f32).round() as i64;
    Some(slot.rem_euclid(n as i64) as usize)
}

/// The Lua that seats the entries for the addon to read.
pub fn lua_table() -> String {
    let rows: Vec<String> = ENTRIES
        .iter()
        .map(|(label, command)| format!("{{\"{label}\",\"{command}\"}}"))
        .collect();
    format!("BenillaPadRingEntries={{{}}}", rows.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_eight_directions_pick_their_slots_clockwise_from_the_top() {
        let dirs = [
            (0.0, 1.0),
            (0.7, 0.7),
            (1.0, 0.0),
            (0.7, -0.7),
            (0.0, -1.0),
            (-0.7, -0.7),
            (-1.0, 0.0),
            (-0.7, 0.7),
        ];
        for (i, (x, y)) in dirs.into_iter().enumerate() {
            assert_eq!(slot(x, y, 8), Some(i), "({x}, {y})");
        }
    }

    #[test]
    fn a_slot_owns_the_arc_around_its_direction_and_the_seam_is_between_slots() {
        // 20 degrees either side of the top is still slot 0; 23 degrees right of it is slot 1.
        let at = |deg: f32| slot(deg.to_radians().sin(), deg.to_radians().cos(), 8);
        assert_eq!(at(20.0), Some(0));
        assert_eq!(at(-20.0), Some(0));
        assert_eq!(at(23.0), Some(1));
        assert_eq!(at(-23.0), Some(7));
        assert_eq!(at(179.0), Some(4));
        assert_eq!(at(-179.0), Some(4));
    }

    #[test]
    fn a_stick_short_of_the_tilt_picks_nothing() {
        assert_eq!(slot(0.0, 0.0, 8), None);
        assert_eq!(slot(0.0, 0.49, 8), None);
        assert_eq!(slot(0.0, 0.5, 8), Some(0));
        assert_eq!(slot(0.3, 0.3, 8), None, "0.42 of travel along the diagonal");
    }

    #[test]
    fn other_ring_sizes_divide_the_circle_evenly() {
        assert_eq!(slot(1.0, 0.0, 4), Some(1));
        assert_eq!(slot(0.0, -1.0, 4), Some(2));
        assert_eq!(slot(-1.0, 0.0, 4), Some(3));
        assert_eq!(slot(1.0, 0.0, 0), None);
        assert_eq!(slot(0.0, 1.0, 1), Some(0));
    }

    #[test]
    fn the_entries_are_distinct_and_the_table_lists_them_in_order() {
        for (i, (label, command)) in ENTRIES.iter().enumerate() {
            assert!(!ENTRIES[..i].iter().any(|(l, c)| l == label || c == command));
        }
        let t = lua_table();
        assert!(t.starts_with("BenillaPadRingEntries={{\"Character\",\"TOGGLECHARACTER0\"}"));
        assert!(t.ends_with("{\"Keyboard\",\"TOGGLEPADKEYBOARD\"}}"));
    }

    /// Every command the ring runs is one the stock `Bindings.xml` declares, or the addon does;
    /// a misspelled one would run nothing and say so nowhere.
    #[test]
    fn every_entry_command_is_declared_by_the_stock_bindings_file_or_the_addon() {
        let data = benilla_formats::wow_data_or_skip!();
        let chain = benilla_formats::Chain::open(&data).expect("the install's archives open");
        let bytes = chain
            .read(r"Interface\FrameXML\Bindings.xml")
            .expect("the chain has Bindings.xml");
        let declared: std::collections::HashSet<String> =
            benilla_ui::bindings_xml::parse(&benilla_ui::source::decode(&bytes))
                .expect("parses")
                .into_iter()
                .chain(benilla_ui::bindings_xml::parse(crate::addon::BINDINGS).expect("parses"))
                .map(|b| b.name)
                .collect();
        let unknown: Vec<_> = ENTRIES
            .iter()
            .map(|(_, c)| *c)
            .filter(|c| !declared.contains(*c))
            .collect();
        assert!(unknown.is_empty(), "undeclared commands: {unknown:?}");
    }
}
