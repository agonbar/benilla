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
//! ```

use bevy::prelude::*;
use serde::Deserialize;

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
    fn a_malformed_file_is_the_defaults_with_a_problem() {
        let (cfg, problems) = PadConfig::parse("device = [");
        assert_eq!(cfg, PadConfig::default());
        assert_eq!(problems.len(), 1);
    }
}
