//! The addon benilla-input ships: pad glyphs on the bars, and the pad's cursor. It is written into
//! the `AddOns/` folder of [`benilla_app::config_dir`], where benilla loads addons from, and
//! loads as any addon does. Written against the 1.12 API only.

use std::path::Path;

pub const NAME: &str = "BenillaPad";
const TOC: &str = include_str!("../addon/BenillaPad.toc");
const LUA: &str = include_str!("../addon/BenillaPad.lua");

/// Write the addon under `config`, leaving a file alone when it already reads the same.
pub fn install(config: &Path) -> std::io::Result<()> {
    let dir = config.join("AddOns").join(NAME);
    std::fs::create_dir_all(&dir)?;
    for (file, text) in [("BenillaPad.toc", TOC), ("BenillaPad.lua", LUA)] {
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

    use super::*;
    use crate::config::PadConfig;

    /// The addon over a bare VM that has the stock `GetBindingText` stand-in, a bar and a binding.
    fn loaded() -> UiScript {
        let s = UiScript::new().expect("construct UiScript");
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
        s.run(LUA).expect("the addon loads");
        s.run(&crate::glyphs::lua_tables(&PadConfig::default()))
            .unwrap();
        s
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
