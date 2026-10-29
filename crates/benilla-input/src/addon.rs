//! The addon benilla-input ships: pad glyphs on the bars, the pad's cursor, the two interact
//! commands of the Key Bindings window, the radial ring and the on-screen keyboard. It is written into
//! the `AddOns/` folder of [`benilla_app::config_dir`], where benilla loads addons from, and
//! loads as any addon does. Written against the 1.12 API only.

use std::path::Path;

pub const NAME: &str = "BenillaPad";
const TOC: &str = include_str!("../addon/BenillaPad.toc");
const LUA: &str = include_str!("../addon/BenillaPad.lua");
const RING: &str = include_str!("../addon/Ring.lua");
const KEYBOARD: &str = include_str!("../addon/Keyboard.lua");
pub(crate) const BINDINGS: &str = include_str!("../addon/Bindings.xml");

/// Write the addon under `config`, leaving a file alone when it already reads the same.
pub fn install(config: &Path) -> std::io::Result<()> {
    let dir = config.join("AddOns").join(NAME);
    std::fs::create_dir_all(&dir)?;
    for (file, text) in [
        ("BenillaPad.toc", TOC),
        ("BenillaPad.lua", LUA),
        ("Ring.lua", RING),
        ("Keyboard.lua", KEYBOARD),
        ("Bindings.xml", BINDINGS),
    ] {
        let path = dir.join(file);
        if std::fs::read_to_string(&path).is_ok_and(|t| t == text) {
            continue;
        }
        std::fs::write(path, text)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use benilla_ui::script::UiScript;

    use bevy::input::keyboard::KeyCode;
    use bevy::math::Vec2;

    use super::*;
    use crate::config::PadConfig;
    use crate::host::Host;
    use crate::keyboard::Typed;

    /// The addon over a bare VM that has the stock `GetBindingText` stand-in, a bar and a binding.
    fn loaded() -> UiScript {
        loaded_with_host().0
    }

    /// [`loaded`], with the ring and the keyboard and the host functions they call.
    fn loaded_with_host() -> (UiScript, Host) {
        let s = UiScript::new().expect("construct UiScript");
        let host = Host::default();
        host.install(s.lua()).unwrap();
        s.run(&crate::ring::lua_table()).unwrap();
        s.run(
            r#"
            function GetBindingText(name, prefix, abbr) return "stock:" .. name end
            local keys = { ACTIONBUTTON1 = { "1", "PAD1" }, ACTIONBUTTON2 = { "SHIFT-PAD1" },
                           ACTIONBUTTON3 = { "2" } }
            function GetBindingKey(cmd)
              local k = keys[cmd]
              if k then return k[1], k[2] end
            end
            CreateFrame("Frame", "UIParent")
            for n = 1, 3 do
              local button = CreateFrame("Button", "ActionButton" .. n)
              local text = button:CreateFontString("ActionButton" .. n .. "HotKey")
              text:SetFont("Fonts\\FRIZQT__.TTF", 12)
            end
        "#,
        )
        .unwrap();
        s.run(
            r#"
            Calls = ""
            function RunBinding(command) Calls = Calls .. command .. " " end
        "#,
        )
        .unwrap();
        for lua in [LUA, RING, KEYBOARD] {
            s.run(lua).expect("the addon loads");
        }
        s.run(&crate::glyphs::lua_tables(&PadConfig::default()))
            .unwrap();
        (s, host)
    }

    fn text(s: &UiScript, name: &str) -> String {
        s.eval(&format!("return GetBindingText(\"{name}\", \"KEY_\", 1)"))
            .unwrap()
    }

    #[test]
    fn the_keyboard_presentation_reads_as_stock() {
        let s = loaded();
        s.run("BenillaPadState = { presentation = \"kbm\" }")
            .unwrap();
        assert_eq!(text(&s, "PAD1"), "stock:PAD1");
        assert_eq!(text(&s, "SHIFT-1"), "stock:SHIFT-1");
    }

    #[test]
    fn the_controller_presentation_reads_a_pad_binding_as_its_glyph_and_layers() {
        let s = loaded();
        s.run("BenillaPadState = { presentation = \"controller\" }")
            .unwrap();
        assert_eq!(text(&s, "PAD1"), "A");
        assert_eq!(text(&s, "SHIFT-PAD1"), "LT-A");
        assert_eq!(text(&s, "CTRL-PADDDOWN"), "RT-DD");
        assert_eq!(text(&s, "ALT-PAD4"), "LT+RT-Y");
        assert_eq!(
            text(&s, "SHIFT-1"),
            "stock:SHIFT-1",
            "a keyboard chord is stock's"
        );
        assert_eq!(text(&s, "PADUNKNOWN"), "stock:PADUNKNOWN");
    }

    #[test]
    fn the_bars_show_the_key_of_the_presentation() {
        let s = loaded();
        let hotkey = |n: u32| -> String {
            s.eval(&format!("return ActionButton{n}HotKey:GetText()"))
                .unwrap()
        };
        let tick = || {
            s.run("arg1 = 1 BenillaPadDriver:GetScript('OnUpdate')()")
                .unwrap()
        };
        s.run("BenillaPadState = { presentation = \"controller\" }")
            .unwrap();
        tick();
        assert_eq!(hotkey(1), "A", "the pad key, not the first key");
        assert_eq!(hotkey(2), "LT-A");
        assert_eq!(hotkey(3), "stock:2", "no pad key: the keyboard's");
        s.run("BenillaPadState = { presentation = \"kbm\" }")
            .unwrap();
        tick();
        assert_eq!(hotkey(1), "stock:1");
    }

    #[test]
    fn the_cursor_frame_follows_the_host_flag() {
        let s = loaded();
        let shown = || -> bool {
            s.eval("return BenillaPadCursor:IsShown() and true or false")
                .unwrap()
        };
        let tick = || {
            s.run("arg1 = 1 BenillaPadDriver:GetScript('OnUpdate')()")
                .unwrap()
        };
        assert!(!shown());
        s.run("BenillaPadState = { cursor = 1, presentation = \"kbm\" }")
            .unwrap();
        tick();
        assert!(shown());
        s.run("BenillaPadState = { presentation = \"kbm\" }")
            .unwrap();
        tick();
        assert!(!shown());
    }

    /// Both commands show in Key Bindings (a declared name and a label), and a body queues the ask
    /// `soft::asks` reads.
    #[test]
    fn the_two_interact_commands_are_declared_and_their_bodies_queue_an_ask() {
        use crate::soft::{asks, Ask, TAKE_ASKS};
        let s = loaded();
        let bindings = benilla_ui::bindings_xml::parse(BINDINGS).expect("well-formed");
        let names: Vec<_> = bindings.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "INTERACTTARGET",
                "INTERACTORJUMP",
                "RADIALRING",
                "TOGGLEPADKEYBOARD"
            ]
        );
        assert_eq!(bindings[0].header.as_deref(), Some("BENILLAPAD"));
        for name in names {
            let label: Option<String> = s.eval(&format!("return BINDING_NAME_{name}")).unwrap();
            assert!(label.is_some_and(|l| !l.is_empty()), "{name} has no label");
        }
        assert_eq!(
            s.eval::<Option<String>>("return BINDING_HEADER_BENILLAPAD")
                .unwrap()
                .as_deref(),
            Some("Benilla Pad")
        );
        for b in bindings.iter().take(2) {
            s.run(&b.body).unwrap();
        }
        let text: Option<String> = s.eval(TAKE_ASKS).unwrap();
        assert_eq!(
            asks(&text.unwrap()),
            [Ask::Interact, Ask::InteractOrJump],
            "the asks come out in press order"
        );
        let again: Option<String> = s.eval(TAKE_ASKS).unwrap();
        assert_eq!(again, None, "taking clears them");
    }

    /// The ring's binding, run as the dispatch does: `keystate` set, then the body.
    fn ring_binding(s: &UiScript, keystate: &str) {
        let bindings = benilla_ui::bindings_xml::parse(BINDINGS).unwrap();
        let ring = bindings.iter().find(|b| b.name == "RADIALRING").unwrap();
        assert!(ring.run_on_up, "the release has to reach the body");
        s.run(&format!("keystate = \"{keystate}\"")).unwrap();
        s.run(&ring.body).unwrap();
    }

    fn ring_frame(s: &UiScript) {
        s.run("arg1 = 0.016 BenillaPadRing:GetScript('OnUpdate')()")
            .unwrap();
    }

    fn calls(s: &UiScript) -> String {
        s.eval("return Calls").unwrap()
    }

    fn ring_shown(s: &UiScript) -> bool {
        s.eval("return BenillaPadRing:IsShown() and true or false")
            .unwrap()
    }

    #[test]
    fn the_ring_runs_the_picked_entry_once_on_release() {
        let (s, host) = loaded_with_host();
        ring_binding(&s, "down");
        assert!(ring_shown(&s) && host.ring_open());
        // The right stick points right: the third entry, Talents.
        host.set(Vec2::ZERO, Vec2::new(1.0, 0.0), false);
        ring_frame(&s);
        assert_eq!(calls(&s), "", "nothing runs while it is held");
        ring_binding(&s, "up");
        assert_eq!(calls(&s), "TOGGLETALENTS ");
        assert!(!ring_shown(&s) && !host.ring_open());
        ring_binding(&s, "up");
        assert_eq!(
            calls(&s),
            "TOGGLETALENTS ",
            "a second release finds it closed"
        );
    }

    #[test]
    fn the_ring_follows_the_stick_and_a_stick_at_rest_runs_nothing() {
        let (s, host) = loaded_with_host();
        ring_binding(&s, "down");
        host.set(Vec2::ZERO, Vec2::new(0.0, 1.0), false);
        ring_frame(&s);
        host.set(Vec2::ZERO, Vec2::new(0.0, -1.0), false);
        ring_frame(&s);
        host.set(Vec2::ZERO, Vec2::ZERO, false);
        ring_frame(&s);
        ring_binding(&s, "up");
        assert_eq!(calls(&s), "", "back at the centre is a cancel");
        // Opened and released with no frame between: no pick yet, so no entry.
        ring_binding(&s, "down");
        ring_binding(&s, "up");
        assert_eq!(calls(&s), "");
        // The entry under the stick on the last frame is the one that runs.
        ring_binding(&s, "down");
        host.set(Vec2::ZERO, Vec2::new(0.0, -1.0), false);
        ring_frame(&s);
        ring_binding(&s, "up");
        assert_eq!(calls(&s), "OPENALLBAGS ");
    }

    #[test]
    fn the_ring_does_not_open_in_cursor_mode_and_closes_without_firing_when_it_starts() {
        let (s, host) = loaded_with_host();
        host.set(Vec2::ZERO, Vec2::ZERO, true);
        ring_binding(&s, "down");
        assert!(!ring_shown(&s) && !host.ring_open());
        host.set(Vec2::ZERO, Vec2::ZERO, false);
        ring_binding(&s, "down");
        host.set(Vec2::ZERO, Vec2::new(1.0, 0.0), false);
        ring_frame(&s);
        host.set(Vec2::ZERO, Vec2::new(1.0, 0.0), true);
        ring_frame(&s);
        assert!(
            !ring_shown(&s) && !host.ring_open(),
            "a window opened under it"
        );
        ring_binding(&s, "up");
        assert_eq!(calls(&s), "");
    }

    fn key(s: &UiScript, name: &str) {
        s.run(&format!(
            "arg1 = \"LeftButton\" {name}:GetScript('OnClick')()"
        ))
        .unwrap();
    }

    fn key_text(s: &UiScript, name: &str) -> String {
        s.eval(&format!("return {name}.text:GetText()")).unwrap()
    }

    #[test]
    fn the_keyboard_toggles_and_its_keys_type_through_the_host() {
        let (s, host) = loaded_with_host();
        let shown = |s: &UiScript| -> bool {
            s.eval("return BenillaPadKeyboard:IsShown() and true or false")
                .unwrap()
        };
        assert!(!shown(&s));
        s.run("BenillaPadToggleKeyboard()").unwrap();
        assert!(shown(&s));
        key(&s, "BenillaPadKey2_1");
        key(&s, "BenillaPadKeySpace");
        key(&s, "BenillaPadKey4_10");
        key(&s, "BenillaPadKeyBack");
        key(&s, "BenillaPadKeyEnter");
        assert_eq!(
            host.take_typed(),
            [
                Typed::Text("q".into()),
                Typed::Text(" ".into()),
                Typed::Text("/".into()),
                Typed::Key(KeyCode::Backspace),
                Typed::Key(KeyCode::Enter),
            ],
            "a slash is on the first layer, for the commands"
        );
        key(&s, "BenillaPadKeyClose");
        assert!(!shown(&s));
    }

    #[test]
    fn shift_is_for_one_character_and_the_symbol_layer_has_its_own_keys() {
        let (s, host) = loaded_with_host();
        key(&s, "BenillaPadKeyShift");
        assert_eq!(key_text(&s, "BenillaPadKey2_1"), "Q");
        key(&s, "BenillaPadKey2_1");
        assert_eq!(key_text(&s, "BenillaPadKey2_1"), "q", "back to lower case");
        key(&s, "BenillaPadKey2_1");
        key(&s, "BenillaPadKeySymbols");
        assert_eq!(key_text(&s, "BenillaPadKey2_1"), "-");
        assert_eq!(key_text(&s, "BenillaPadKeySymbols"), "abc");
        key(&s, "BenillaPadKey2_1");
        key(&s, "BenillaPadKeySymbols");
        assert_eq!(key_text(&s, "BenillaPadKey2_1"), "q");
        let typed: Vec<_> = host
            .take_typed()
            .into_iter()
            .map(|t| match t {
                Typed::Text(t) => t,
                Typed::Key(_) => String::new(),
            })
            .collect();
        assert_eq!(typed, ["Q", "q", "-"]);
    }

    #[test]
    fn every_key_of_every_layer_has_one_character() {
        let (s, _) = loaded_with_host();
        for toggle in [
            "BenillaPadKeyShift",
            "BenillaPadKeyShift",
            "BenillaPadKeySymbols",
        ] {
            for r in 1..=4 {
                for c in 1..=10 {
                    let t = key_text(&s, &format!("BenillaPadKey{r}_{c}"));
                    assert_eq!(t.chars().count(), 1, "{toggle}: key {r}_{c} reads {t:?}");
                }
            }
            key(&s, toggle);
        }
    }

    #[test]
    fn the_chat_button_shows_in_the_controller_presentation_and_opens_the_line_and_the_keyboard() {
        let (s, _) = loaded_with_host();
        s.run(
            r#"
            CreateFrame("Frame", "ChatFrame1")
            Opened = 0
            function ChatFrame_OpenChat(text) Opened = Opened + 1 end
        "#,
        )
        .unwrap();
        let tick = || {
            s.run("arg1 = 1 BenillaPadKeyboardWatcher:GetScript('OnUpdate')()")
                .unwrap()
        };
        let button_shown = || -> bool {
            s.eval("return BenillaPadChatKeyboard:IsShown() and true or false")
                .unwrap()
        };
        s.run("BenillaPadState = { presentation = \"kbm\" }")
            .unwrap();
        tick();
        assert!(!button_shown());
        s.run("BenillaPadState = { presentation = \"controller\" }")
            .unwrap();
        tick();
        assert!(button_shown());
        s.run("arg1 = \"LeftButton\" BenillaPadChatKeyboard:GetScript('OnClick')()")
            .unwrap();
        assert_eq!(s.eval::<i32>("return Opened").unwrap(), 1);
        assert!(s
            .eval::<bool>("return BenillaPadKeyboard:IsShown() and true or false")
            .unwrap());
    }

    /// What the keyboard types reaches the focused EditBox as the keyboard's own text does: a
    /// character through the typed-character walk, Enter through the key walk.
    #[test]
    fn typed_text_reaches_the_focused_editbox_and_enter_sends_it() {
        use benilla_ui::script::KeyWalk;
        let (mut s, host) = loaded_with_host();
        s.run(
            r#"
            local box = CreateFrame("EditBox", "ChatBox", UIParent)
            box:SetWidth(200) box:SetHeight(20)
            box:SetPoint("CENTER", UIParent, "CENTER", 0, 0)
            box:SetScript("OnEnterPressed", function() Sent = this:GetText() end)
            box:SetFocus()
        "#,
        )
        .unwrap();
        s.resolve();
        s.run("BenillaPadToggleKeyboard()").unwrap();
        key(&s, "BenillaPadKey3_8");
        key(&s, "BenillaPadKey3_9");
        key(&s, "BenillaPadKeySpace");
        key(&s, "BenillaPadKeySymbols");
        key(&s, "BenillaPadKey1_1");
        key(&s, "BenillaPadKeyEnter");
        for t in host.take_typed() {
            match t {
                Typed::Text(text) => assert!(s.char_input(&text), "{text:?} not taken"),
                Typed::Key(KeyCode::Enter) => assert_eq!(s.key_down("ENTER"), KeyWalk::Box),
                Typed::Key(_) => unreachable!(),
            }
        }
        assert_eq!(
            s.eval::<String>("return ChatBox:GetText()").unwrap(),
            "kl 1"
        );
        assert_eq!(s.eval::<String>("return Sent").unwrap(), "kl 1");
    }

    #[test]
    fn install_writes_once_and_repairs_an_edit() {
        let dir = std::env::temp_dir().join(format!("benilla-pad-addon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        install(&dir).unwrap();
        let lua = dir.join("AddOns").join(NAME).join("BenillaPad.lua");
        assert_eq!(std::fs::read_to_string(&lua).unwrap(), LUA);
        std::fs::write(&lua, "-- edited").unwrap();
        install(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(&lua).unwrap(), LUA);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
