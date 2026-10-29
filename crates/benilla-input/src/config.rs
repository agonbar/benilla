//! `pad.toml` in `benilla_app::config_dir()`: the pad's own settings, never rows in benilla's CVar
//! table, which every addon reads. An absent or unreadable file is the defaults; the file is never
//! written. Retail's `GamePadEmulate*` CVars are `emulate_*` here.
//!
//! ```toml
//! device = "auto"            # or "28de:1205": only that vendor:product is read
//! move_dead_zone = 0.25      # left stick, radial; also where a direction presses
//! look_dead_zone = 0.15      # right stick, radial
//! trigger_press = 0.5        # a trigger or button counts as down from here
//! look_speed = 900.0         # mouse pixels per second at full deflection
//! look_invert_y = false
//! emulate_shift = "PADLTRIGGER"   # held, the buttons press as SHIFT-<token>; "" is off
//! emulate_ctrl = "PADRTRIGGER"
//! emulate_alt = "both"       # "both": shift and ctrl held together press as ALT- alone
//! presentation = "auto"      # "kbm" or "controller" pins what the bars show; auto follows input
//! cursor_stick = "right"     # the stick that moves the cursor in cursor mode; "left" makes the
//!                            # left stick the only cursor stick, and then no stick leaves cursor mode
//! cursor_speed = 1100.0      # cursor pixels per second at full deflection
//! ui_scale = "off"           # "deck" sets the UI scale for a 1280x800 screen, or a number
//! soft_range = 41.0          # yards: how far the soft target looks for a unit
//! soft_object_range = 10.0   # yards: how far for a corpse or an object
//! soft_arc = 40.0            # degrees either side of the camera's forward
//! soft_margin = 0.25         # a challenger must score this fraction better than the holder...
//! soft_switch_ms = 150       # ...for this long before the soft target moves to it
//! ```

use bevy::prelude::*;
use serde::Deserialize;

use benilla_app::SoftTargetConfig;

use crate::tokens;

/// Which pad is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    /// The pad that sent the last input; see [`crate::device`].
    Auto,
    Pinned {
        vendor: u16,
        product: u16,
    },
}

/// What holds one of the three emulated modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Off,
    /// A pad token held.
    Token(&'static str),
    /// Alt only: both the shift and the ctrl sources held.
    Both,
}

/// What the bars and prompts show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presentation {
    /// Follows the last input that is not bare pointer motion.
    Auto,
    Kbm,
    Controller,
}

/// The stick that moves the cursor in cursor mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorStick {
    /// The camera stick. The left stick walks, which leaves cursor mode.
    Right,
    Left,
}

/// The UI scale `ui_scale = "deck"` sets, for the Deck's 1280x800 screen.
pub const DECK_UI_SCALE: f32 = 1.1;

#[derive(Resource, Clone, Debug, PartialEq)]
pub struct PadConfig {
    pub device: Device,
    pub move_dead_zone: f32,
    pub look_dead_zone: f32,
    pub trigger_press: f32,
    pub look_speed: f32,
    pub look_invert_y: bool,
    pub shift: Source,
    pub ctrl: Source,
    pub alt: Source,
    pub presentation: Presentation,
    pub cursor_stick: CursorStick,
    pub cursor_speed: f32,
    /// The `uiScale` to set at world entry, if the player asked for one.
    pub ui_scale: Option<f32>,
    /// The soft target's dials, handed to `benilla-app` at start.
    pub soft: SoftTargetConfig,
}

impl Default for PadConfig {
    fn default() -> Self {
        PadConfig {
            device: Device::Auto,
            move_dead_zone: 0.25,
            look_dead_zone: 0.15,
            trigger_press: 0.5,
            look_speed: 900.0,
            look_invert_y: false,
            shift: Source::Token("PADLTRIGGER"),
            ctrl: Source::Token("PADRTRIGGER"),
            alt: Source::Both,
            presentation: Presentation::Auto,
            cursor_stick: CursorStick::Right,
            cursor_speed: 1100.0,
            ui_scale: None,
            soft: SoftTargetConfig::default(),
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct File {
    device: Option<String>,
    move_dead_zone: Option<f32>,
    look_dead_zone: Option<f32>,
    trigger_press: Option<f32>,
    look_speed: Option<f32>,
    look_invert_y: Option<bool>,
    emulate_shift: Option<String>,
    emulate_ctrl: Option<String>,
    emulate_alt: Option<String>,
    presentation: Option<String>,
    cursor_stick: Option<String>,
    cursor_speed: Option<f32>,
    ui_scale: Option<toml::Value>,
    soft_range: Option<f32>,
    soft_object_range: Option<f32>,
    soft_arc: Option<f32>,
    soft_margin: Option<f32>,
    soft_switch_ms: Option<f32>,
}

impl PadConfig {
    /// The file's text over the defaults. A bad value keeps its default and says so in `problems`.
    pub fn parse(text: &str) -> (PadConfig, Vec<String>) {
        let mut cfg = PadConfig::default();
        let mut problems = Vec::new();
        let file: File = match toml::from_str(text) {
            Ok(f) => f,
            Err(e) => {
                problems.push(format!("pad.toml: {e}"));
                return (cfg, problems);
            }
        };
        if let Some(d) = &file.device {
            match parse_device(d) {
                Some(d) => cfg.device = d,
                None => problems.push(format!("pad.toml: device \"{d}\" is not auto or vvvv:pppp")),
            }
        }
        let mut range = |name: &str, v: Option<f32>, lo: f32, hi: f32, out: &mut f32| {
            if let Some(v) = v {
                if (lo..=hi).contains(&v) {
                    *out = v;
                } else {
                    problems.push(format!("pad.toml: {name} {v} is outside {lo} to {hi}"));
                }
            }
        };
        range(
            "move_dead_zone",
            file.move_dead_zone,
            0.0,
            0.9,
            &mut cfg.move_dead_zone,
        );
        range(
            "look_dead_zone",
            file.look_dead_zone,
            0.0,
            0.9,
            &mut cfg.look_dead_zone,
        );
        range(
            "trigger_press",
            file.trigger_press,
            0.1,
            0.95,
            &mut cfg.trigger_press,
        );
        range(
            "look_speed",
            file.look_speed,
            1.0,
            10_000.0,
            &mut cfg.look_speed,
        );
        if let Some(v) = file.look_invert_y {
            cfg.look_invert_y = v;
        }
        range(
            "cursor_speed",
            file.cursor_speed,
            10.0,
            20_000.0,
            &mut cfg.cursor_speed,
        );
        range(
            "soft_range",
            file.soft_range,
            5.0,
            100.0,
            &mut cfg.soft.range,
        );
        range(
            "soft_object_range",
            file.soft_object_range,
            2.0,
            40.0,
            &mut cfg.soft.object_range,
        );
        range(
            "soft_arc",
            file.soft_arc,
            5.0,
            90.0,
            &mut cfg.soft.arc_degrees,
        );
        range(
            "soft_margin",
            file.soft_margin,
            0.0,
            0.9,
            &mut cfg.soft.margin,
        );
        if let Some(ms) = file.soft_switch_ms {
            let mut secs = cfg.soft.switch_secs;
            range("soft_switch_ms", Some(ms), 0.0, 2000.0, &mut secs);
            // `range` wrote the number as it was given, in milliseconds.
            if (0.0..=2000.0).contains(&ms) {
                cfg.soft.switch_secs = ms / 1000.0;
            }
        }
        if let Some(t) = &file.presentation {
            match t.to_ascii_lowercase().as_str() {
                "auto" => cfg.presentation = Presentation::Auto,
                "kbm" => cfg.presentation = Presentation::Kbm,
                "controller" => cfg.presentation = Presentation::Controller,
                _ => problems.push(format!(
                    "pad.toml: presentation \"{t}\" is not auto, kbm or controller"
                )),
            }
        }
        if let Some(t) = &file.cursor_stick {
            match t.to_ascii_lowercase().as_str() {
                "right" => cfg.cursor_stick = CursorStick::Right,
                "left" => cfg.cursor_stick = CursorStick::Left,
                _ => problems.push(format!(
                    "pad.toml: cursor_stick \"{t}\" is not right or left"
                )),
            }
        }
        match &file.ui_scale {
            None => {}
            Some(toml::Value::String(t)) if t.eq_ignore_ascii_case("off") => cfg.ui_scale = None,
            Some(toml::Value::String(t)) if t.eq_ignore_ascii_case("deck") => {
                cfg.ui_scale = Some(DECK_UI_SCALE)
            }
            Some(v) => match v.as_float().or_else(|| v.as_integer().map(|i| i as f64)) {
                Some(n) if (0.5..=1.5).contains(&n) => cfg.ui_scale = Some(n as f32),
                _ => problems.push(format!(
                    "pad.toml: ui_scale {v} is not off, deck or a number from 0.5 to 1.5"
                )),
            },
        }
        for (name, text, out, allow_both) in [
            ("emulate_shift", &file.emulate_shift, &mut cfg.shift, false),
            ("emulate_ctrl", &file.emulate_ctrl, &mut cfg.ctrl, false),
            ("emulate_alt", &file.emulate_alt, &mut cfg.alt, true),
        ] {
            let Some(text) = text else { continue };
            match parse_source(text, allow_both) {
                Some(s) => *out = s,
                None => problems.push(format!("pad.toml: {name} \"{text}\" is not a PAD token")),
            }
        }
        (cfg, problems)
    }

    /// `pad.toml` under the state folder, if there is one.
    pub fn load() -> PadConfig {
        let Some(path) = benilla_app::config_dir().map(|d| d.join("pad.toml")) else {
            return PadConfig::default();
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return PadConfig::default(),
            Err(e) => {
                warn!("pad: reading {}: {e}", path.display());
                return PadConfig::default();
            }
        };
        let (cfg, problems) = PadConfig::parse(&text);
        for p in problems {
            warn!("pad: {p}");
        }
        cfg
    }
}

fn parse_device(s: &str) -> Option<Device> {
    if s.eq_ignore_ascii_case("auto") {
        return Some(Device::Auto);
    }
    let (v, p) = s.split_once(':')?;
    Some(Device::Pinned {
        vendor: u16::from_str_radix(v.trim(), 16).ok()?,
        product: u16::from_str_radix(p.trim(), 16).ok()?,
    })
}

fn parse_source(s: &str, allow_both: bool) -> Option<Source> {
    let s = s.trim();
    if s.is_empty() || s.eq_ignore_ascii_case("none") {
        return Some(Source::Off);
    }
    if allow_both && s.eq_ignore_ascii_case("both") {
        return Some(Source::Both);
    }
    tokens::find(&s.to_ascii_uppercase()).map(Source::Token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_the_defaults() {
        assert_eq!(PadConfig::parse("").0, PadConfig::default());
    }

    #[test]
    fn values_override_and_bad_ones_keep_the_default() {
        let (cfg, problems) = PadConfig::parse(
            "device = \"28de:1205\"\nlook_speed = 500\nmove_dead_zone = 3\nemulate_alt = \"pad1\"\nemulate_ctrl = \"nope\"",
        );
        assert_eq!(
            cfg.device,
            Device::Pinned {
                vendor: 0x28de,
                product: 0x1205
            }
        );
        assert_eq!(cfg.look_speed, 500.0);
        assert_eq!(cfg.move_dead_zone, 0.25);
        assert_eq!(cfg.alt, Source::Token("PAD1"));
        assert_eq!(cfg.ctrl, PadConfig::default().ctrl);
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    #[test]
    fn the_cursor_and_presentation_settings_parse() {
        let (cfg, problems) = PadConfig::parse(
            "presentation = \"Controller\"\ncursor_stick = \"left\"\ncursor_speed = 700\nui_scale = \"deck\"",
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(cfg.presentation, Presentation::Controller);
        assert_eq!(cfg.cursor_stick, CursorStick::Left);
        assert_eq!(cfg.cursor_speed, 700.0);
        assert_eq!(cfg.ui_scale, Some(DECK_UI_SCALE));
        assert_eq!(PadConfig::parse("ui_scale = 1").0.ui_scale, Some(1.0));
        assert_eq!(PadConfig::parse("ui_scale = \"off\"").0.ui_scale, None);
        let (cfg, problems) =
            PadConfig::parse("presentation = \"x\"\ncursor_stick = \"up\"\nui_scale = 9");
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert_eq!(cfg.presentation, Presentation::Auto);
        assert_eq!(cfg.ui_scale, None);
    }

    #[test]
    fn the_soft_target_dials_parse_and_milliseconds_become_seconds() {
        assert_eq!(PadConfig::parse("").0.soft, SoftTargetConfig::default());
        let (cfg, problems) = PadConfig::parse(
            "soft_range = 30\nsoft_arc = 25\nsoft_margin = 0.4\nsoft_switch_ms = 250\nsoft_object_range = 1",
        );
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(cfg.soft.range, 30.0);
        assert_eq!(cfg.soft.arc_degrees, 25.0);
        assert_eq!(cfg.soft.margin, 0.4);
        assert_eq!(cfg.soft.switch_secs, 0.25);
        assert_eq!(
            cfg.soft.object_range,
            SoftTargetConfig::default().object_range
        );
        let (cfg, problems) = PadConfig::parse("soft_switch_ms = 99999");
        assert_eq!(problems.len(), 1);
        assert_eq!(
            cfg.soft.switch_secs,
            SoftTargetConfig::default().switch_secs
        );
    }

    #[test]
    fn a_malformed_file_is_the_defaults_with_a_problem() {
        let (cfg, problems) = PadConfig::parse("device = [");
        assert_eq!(cfg, PadConfig::default());
        assert_eq!(problems.len(), 1);
    }
}
