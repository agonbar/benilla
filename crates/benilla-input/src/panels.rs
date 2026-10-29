//! Which windows are open, read out of the Lua UI: the cause of cursor mode. A window is a frame the
//! stock UI shows and hides itself, so the pad asks the VM instead of tracking a list of events.
//! Polled at [`POLL`] seconds; the chunk is defensive, since the frames it names come from the
//! player's own FrameXML and an addon may have removed one.

use std::collections::BTreeSet;

/// Seconds between polls.
pub const POLL: f32 = 0.1;

/// Returns the open windows' names joined with commas, then whether a targeting cursor is armed.
/// A window is a `UIPanelWindows` entry, a bag (`ContainerFrame1..13`), a static popup or the
/// pad's keyboard, shown.
const CHUNK: &str = r#"
local open = ""
local function note(name)
  local f = getglobal(name)
  if f and f.IsShown and f:IsShown() then open = open .. name .. "," end
end
if UIPanelWindows then
  for name in pairs(UIPanelWindows) do note(name) end
end
for i = 1, 13 do note("ContainerFrame" .. i) end
for i = 1, 4 do note("StaticPopup" .. i) end
note("BenillaPadKeyboard")
local targeting = 0
if SpellIsTargeting and SpellIsTargeting() then targeting = 1 end
return open, targeting
"#;

/// What the UI says now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reading {
    pub panels: BTreeSet<String>,
    pub targeting: bool,
}

pub fn read(script: &benilla_ui::script::UiScript) -> Option<Reading> {
    let (open, targeting): (String, i32) = script.eval(CHUNK).ok()?;
    Some(parse(&open, targeting != 0))
}

fn parse(open: &str, targeting: bool) -> Reading {
    Reading {
        panels: open
            .split(',')
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .collect(),
        targeting,
    }
}

#[cfg(test)]
mod tests {
    use benilla_ui::script::UiScript;

    use super::*;

    fn script() -> UiScript {
        let s = UiScript::new().expect("construct UiScript");
        s.run(
            r#"
            UIPanelWindows = { CharacterFrame = {}, SpellBookFrame = {}, GoneFrame = {} }
            CreateFrame("Frame", "CharacterFrame"):Hide()
            CreateFrame("Frame", "SpellBookFrame")
            CreateFrame("Frame", "ContainerFrame2")
            CreateFrame("Frame", "ContainerFrame14")
            CreateFrame("Frame", "ChatFrame1")
        "#,
        )
        .unwrap();
        s
    }

    #[test]
    fn shown_panels_bags_and_popups_are_read_and_hidden_or_missing_ones_are_not() {
        let s = script();
        let r = read(&s).expect("the chunk runs");
        let names: Vec<_> = r.panels.iter().map(String::as_str).collect();
        assert_eq!(names, ["ContainerFrame2", "SpellBookFrame"]);
        assert!(!r.targeting);
    }

    #[test]
    fn a_targeting_cursor_is_read() {
        let s = script();
        s.run("function SpellIsTargeting() return 1 end").unwrap();
        assert!(read(&s).unwrap().targeting);
        s.run("function SpellIsTargeting() return nil end").unwrap();
        assert!(!read(&s).unwrap().targeting);
    }

    #[test]
    fn a_vm_with_no_window_table_reads_empty() {
        let s = UiScript::new().expect("construct UiScript");
        assert_eq!(read(&s), Some(Reading::default()));
    }
}
