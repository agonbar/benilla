//! Comfort in the headset as plain math and settings, with no OpenXR or ECS in it, so the headless
//! tests below cover it: `vr.toml`, snap and smooth turn, the vignette's curve and fade, the
//! recentre hold, and the zoom that makes the view first or third person.
//!
//! # First and third person
//!
//! First person is the default. It is the game camera at zero zoom, which is the framing pivot
//! inside the avatar's head (`CameraZoomIn(50)` at session start); the tracking root sits there.
//! Third person is the game camera at `third_person_distance` yards (`CameraZoomOut` from zero),
//! and the tracking root sits level behind the avatar's pivot at that distance
//! ([`crate::rig::third_person_root`]), so the head looks at the avatar and not down on it.
//!
//! Both turns rotate the game camera's yaw, which the avatar follows while the pad holds the
//! look session. In first person the camera is the head, so that turns the player on the spot.
//! In third person the camera orbits the avatar, so the root turns around the avatar and not
//! around the head. Head rotation is the player's in both: the root takes the game camera's yaw
//! alone. Locomotion is body-relative: the avatar walks along the game camera's yaw, which is the
//! root's heading.
//!
//! The vignette reads the camera's own speed and the smooth turn's rate, so it is the same signal
//! in both: running, falling, a mount, the orbit of a third-person turn, all move the camera.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use bevy::prelude::*;
use serde::Deserialize;

/// Mouse pixels per radian of game camera yaw at the default `mousespeed`: the camera turns
/// `cameraYawMoveSpeed (180) × 0.003 / 180` radians per pixel (`player/camera.rs`). A player who
/// changed `mousespeed` turns by that much more or less than the angle asked for.
pub const PIXELS_PER_RADIAN: f32 = 1.0 / 0.003;

/// The stick travel that fires a snap turn, and the travel it must fall back under to fire again.
pub const SNAP_FIRE: f32 = 0.7;
pub const SNAP_REARM: f32 = 0.3;

/// Seconds both stick clicks are held to recentre.
pub const RECENTRE_HOLD: f32 = 1.0;

/// The vignette fades in over this many seconds and out over [`FADE_OUT`].
pub const FADE_IN: f32 = 0.2;
pub const FADE_OUT: f32 = 0.4;

/// Camera speed, yards a second, at which the vignette is full: a run is 7.
pub const SPEED_FULL: f32 = 7.0;
/// Below this the camera is standing still: collision and smoothing nudge it by less.
pub const SPEED_DEAD: f32 = 0.5;
/// Smooth turn rate, degrees a second, at which the vignette is full, and the rate under which it
/// does not show.
pub const TURN_FULL: f32 = 90.0;
pub const TURN_DEAD: f32 = 10.0;

/// The vignette's aperture, as a half-angle from the view's axis: wide open it is outside any
/// headset's view; closed it is `CLOSED_WEAK` at strength 0 and `CLOSED_STRONG` at strength 1.
pub const OPEN_ANGLE: f32 = 80.0 * DEG;
pub const CLOSED_WEAK: f32 = 60.0 * DEG;
pub const CLOSED_STRONG: f32 = 25.0 * DEG;

const DEG: f32 = std::f32::consts::PI / 180.0;

/// The camera 1.12 starts at: the reference's `cameraDistance` default.
pub const THIRD_PERSON_DISTANCE: f32 = 5.55;

/// The view the player is in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Person {
    #[default]
    First,
    Third,
}

/// How the right stick turns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TurnStyle {
    #[default]
    Snap,
    Smooth,
}

/// `vr.toml`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VrConfig {
    pub person: Person,
    pub turn: TurnStyle,
    /// Degrees a snap turn turns.
    pub snap_degrees: f32,
    /// Degrees a second a smooth turn turns at full stick.
    pub smooth_degrees_per_second: f32,
    pub vignette: bool,
    /// 0 to 1: how far the vignette closes in at its fullest.
    pub vignette_strength: f32,
    /// Yards behind the avatar's pivot in third person.
    pub third_person_distance: f32,
}

impl Default for VrConfig {
    fn default() -> Self {
        VrConfig {
            person: Person::First,
            turn: TurnStyle::Snap,
            snap_degrees: 30.0,
            smooth_degrees_per_second: 120.0,
            vignette: true,
            vignette_strength: 0.5,
            third_person_distance: THIRD_PERSON_DISTANCE,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct File {
    person: Option<String>,
    turn: Option<String>,
    snap_degrees: Option<f32>,
    smooth_degrees_per_second: Option<f32>,
    vignette: Option<bool>,
    vignette_strength: Option<f32>,
    third_person_distance: Option<f32>,
}

const SNAP_RANGE: std::ops::RangeInclusive<f32> = 5.0..=180.0;
const SMOOTH_RANGE: std::ops::RangeInclusive<f32> = 15.0..=360.0;
const DISTANCE_RANGE: std::ops::RangeInclusive<f32> = 1.0..=15.0;

impl VrConfig {
    /// The file's text over the defaults. A bad value keeps its default and says so in `problems`;
    /// a file that is not TOML, or has a key this does not know, is the defaults and one problem.
    pub fn parse(text: &str) -> (VrConfig, Vec<String>) {
        let mut cfg = VrConfig::default();
        let mut problems = Vec::new();
        let file: File = match toml::from_str(text) {
            Ok(f) => f,
            Err(e) => {
                problems.push(format!("vr.toml: {e}"));
                return (cfg, problems);
            }
        };
        match file.person.as_deref() {
            None => {}
            Some("first") => cfg.person = Person::First,
            Some("third") => cfg.person = Person::Third,
            Some(other) => problems.push(format!(
                "vr.toml: person \"{other}\" is not \"first\" or \"third\""
            )),
        }
        match file.turn.as_deref() {
            None => {}
            Some("snap") => cfg.turn = TurnStyle::Snap,
            Some("smooth") => cfg.turn = TurnStyle::Smooth,
            Some(other) => problems.push(format!(
                "vr.toml: turn \"{other}\" is not \"snap\" or \"smooth\""
            )),
        }
        let mut number = |name: &str,
                          value: Option<f32>,
                          range: &std::ops::RangeInclusive<f32>,
                          out: &mut f32| {
            match value {
                None => {}
                Some(v) if range.contains(&v) => *out = v,
                Some(v) => problems.push(format!(
                    "vr.toml: {name} {v} is outside {} to {}",
                    range.start(),
                    range.end()
                )),
            }
        };
        number(
            "snap_degrees",
            file.snap_degrees,
            &SNAP_RANGE,
            &mut cfg.snap_degrees,
        );
        number(
            "smooth_degrees_per_second",
            file.smooth_degrees_per_second,
            &SMOOTH_RANGE,
            &mut cfg.smooth_degrees_per_second,
        );
        number(
            "vignette_strength",
            file.vignette_strength,
            &(0.0..=1.0),
            &mut cfg.vignette_strength,
        );
        number(
            "third_person_distance",
            file.third_person_distance,
            &DISTANCE_RANGE,
            &mut cfg.third_person_distance,
        );
        if let Some(v) = file.vignette {
            cfg.vignette = v;
        }
        (cfg, problems)
    }

    /// The file a first run writes: every setting at its default, with what it does.
    pub fn default_text() -> String {
        let d = VrConfig::default();
        format!(
            "# Comfort in the headset. benilla re-reads this file about once a second while a\n\
             # session runs, so a change shows without restarting.\n\
             \n\
             # \"first\" (the camera is your head) or \"third\" (the camera sits behind the avatar).\n\
             person = \"first\"\n\
             \n\
             # \"snap\" turns in steps when the right stick is flicked, \"smooth\" turns while it is held.\n\
             turn = \"snap\"\n\
             snap_degrees = {}\n\
             smooth_degrees_per_second = {}\n\
             \n\
             # The ring that closes in on the view while the camera moves or smooth-turns.\n\
             vignette = {}\n\
             # 0 closes in a little, 1 closes in a lot.\n\
             vignette_strength = {}\n\
             \n\
             # Yards behind the avatar in third person, {} to {}.\n\
             third_person_distance = {}\n",
            d.snap_degrees,
            d.smooth_degrees_per_second,
            d.vignette,
            d.vignette_strength,
            DISTANCE_RANGE.start(),
            DISTANCE_RANGE.end(),
            d.third_person_distance,
        )
    }

    /// One line for the trace.
    pub fn describe(&self) -> String {
        format!(
            "person {} turn {} snap {:.0} smooth {:.0}/s vignette {} strength {:.2} \
             third-person distance {:.2}",
            match self.person {
                Person::First => "first",
                Person::Third => "third",
            },
            match self.turn {
                TurnStyle::Snap => "snap",
                TurnStyle::Smooth => "smooth",
            },
            self.snap_degrees,
            self.smooth_degrees_per_second,
            if self.vignette { "on" } else { "off" },
            self.vignette_strength,
            self.third_person_distance,
        )
    }
}

/// `vr.toml` under `dir`, written with the defaults when it is not there yet. A file that cannot
/// be read is the defaults.
pub fn load(dir: Option<&Path>) -> (VrConfig, Vec<String>) {
    let Some(path) = dir.map(|d| d.join("vr.toml")) else {
        return (VrConfig::default(), Vec::new());
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => VrConfig::parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let problems = std::fs::create_dir_all(dir.unwrap_or(Path::new(".")))
                .and_then(|()| std::fs::write(&path, VrConfig::default_text()))
                .err()
                .map(|e| format!("vr.toml: writing {}: {e}", path.display()))
                .into_iter()
                .collect();
            (VrConfig::default(), problems)
        }
        Err(e) => (
            VrConfig::default(),
            vec![format!("vr.toml: reading {}: {e}", path.display())],
        ),
    }
}

/// Re-reads `vr.toml` when its modification time or size changes, checked once a second.
#[derive(Default)]
pub struct Watch {
    path: Option<PathBuf>,
    seen: Option<(SystemTime, u64)>,
    since: f32,
}

impl Watch {
    /// Watches `dir/vr.toml`, taking its present state as already seen.
    pub fn new(dir: Option<&Path>) -> Watch {
        let path = dir.map(|d| d.join("vr.toml"));
        let seen = path.as_deref().and_then(stamp);
        Watch {
            path,
            seen,
            since: 0.0,
        }
    }

    /// One frame: the new settings and their problems, if the file changed in the last second.
    pub fn step(&mut self, dt: f32) -> Option<(VrConfig, Vec<String>)> {
        self.since += dt;
        if self.since < 1.0 {
            return None;
        }
        self.since = 0.0;
        let path = self.path.as_deref()?;
        let now = stamp(path);
        if now == self.seen {
            return None;
        }
        self.seen = now;
        let text = std::fs::read_to_string(path).ok()?;
        Some(VrConfig::parse(&text))
    }
}

fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// What the right stick asked of the camera this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Turned {
    /// Mouse motion in pixels, for the look session.
    pub motion: Vec2,
    /// Degrees a second the player is smooth-turning, signed, right positive. Zero for a snap,
    /// which is a step and not a rate, so it never shows in the vignette.
    pub rate: f32,
    /// The direction of a snap turn this frame: 1 right, -1 left, 0 none.
    pub snapped: f32,
}

/// The snap turn's latch: it fires once per flick and re-arms when the stick comes back.
#[derive(Debug)]
pub struct Snap {
    armed: bool,
}

impl Default for Snap {
    fn default() -> Self {
        Snap { armed: true }
    }
}

impl Snap {
    /// The direction to turn for a stick at `x`, right positive: 0 unless this frame fires.
    pub fn step(&mut self, x: f32) -> f32 {
        if x.abs() < SNAP_REARM {
            self.armed = true;
            0.0
        } else if self.armed && x.abs() >= SNAP_FIRE {
            self.armed = false;
            x.signum()
        } else {
            0.0
        }
    }
}

/// The turn for a right stick at `x` over `dt` seconds. Stick right turns right, which is the
/// mouse moving right: positive x motion.
pub fn turn(cfg: &VrConfig, snap: &mut Snap, x: f32, dt: f32) -> Turned {
    match cfg.turn {
        TurnStyle::Snap => {
            let dir = snap.step(x);
            Turned {
                motion: Vec2::new(dir * cfg.snap_degrees.to_radians() * PIXELS_PER_RADIAN, 0.0),
                rate: 0.0,
                snapped: dir,
            }
        }
        TurnStyle::Smooth => {
            let rate = x * cfg.smooth_degrees_per_second;
            Turned {
                motion: Vec2::new(rate.to_radians() * dt * PIXELS_PER_RADIAN, 0.0),
                rate,
                snapped: 0.0,
            }
        }
    }
}

/// How hard the vignette wants to close, 0 to 1, for a camera moving at `speed` yards a second and
/// smooth-turning at `turn_rate` degrees a second. The stronger of the two, each ramping linearly
/// from its dead zone to its full value.
pub fn vignette_target(speed: f32, turn_rate: f32) -> f32 {
    let ramp = |v: f32, dead: f32, full: f32| ((v - dead) / (full - dead)).clamp(0.0, 1.0);
    ramp(speed, SPEED_DEAD, SPEED_FULL).max(ramp(turn_rate.abs(), TURN_DEAD, TURN_FULL))
}

/// The vignette's level, moved toward its target at the fade-in and fade-out rates.
#[derive(Debug, Default)]
pub struct Fade {
    pub level: f32,
}

impl Fade {
    pub fn step(&mut self, target: f32, dt: f32) -> f32 {
        let target = target.clamp(0.0, 1.0);
        self.level = if target > self.level {
            (self.level + dt / FADE_IN).min(target)
        } else {
            (self.level - dt / FADE_OUT).max(target)
        };
        self.level
    }
}

/// The half-angle of the vignette's opening at `level` (0 open, 1 closed) and `strength`.
pub fn aperture(strength: f32, level: f32) -> f32 {
    let closed = CLOSED_WEAK + (CLOSED_STRONG - CLOSED_WEAK) * strength.clamp(0.0, 1.0);
    OPEN_ANGLE + (closed - OPEN_ANGLE) * level.clamp(0.0, 1.0)
}

/// Holding both stick clicks for [`RECENTRE_HOLD`] seconds fires once; letting go re-arms.
#[derive(Debug, Default)]
pub struct RecentreHold {
    held: f32,
    fired: bool,
}

impl RecentreHold {
    pub fn step(&mut self, dt: f32, both: bool) -> bool {
        if !both {
            *self = RecentreHold::default();
            return false;
        }
        self.held += dt;
        if self.held >= RECENTRE_HOLD && !self.fired {
            self.fired = true;
            return true;
        }
        false
    }
}

/// The zoom verbs that make the view first or third person. The binding pass sums a frame's zoom
/// amounts, so zooming in all the way and then out must be two frames, not one chunk.
#[derive(Debug, Default)]
pub struct Zoom {
    done: Option<(Person, f32)>,
    out: Option<f32>,
}

/// More than the zoom range, so it ends at the floor.
const ZOOM_ALL_THE_WAY: f32 = 50.0;

impl Zoom {
    /// One frame: the Lua to run, if any, for the view `cfg` asks for.
    pub fn step(&mut self, cfg: &VrConfig) -> Option<String> {
        let want = (cfg.person, cfg.third_person_distance);
        if self.done != Some(want) {
            self.done = Some(want);
            self.out = (cfg.person == Person::Third).then_some(cfg.third_person_distance);
            return Some(format!("CameraZoomIn({ZOOM_ALL_THE_WAY})"));
        }
        self.out.take().map(|d| format!("CameraZoomOut({d})"))
    }

    /// Forget what was applied: the session ended, or the avatar left the world.
    pub fn reset(&mut self) {
        *self = Zoom::default();
    }
}

/// The camera's speed between two frames, yards a second.
pub fn speed(last: Option<Vec3>, now: Vec3, dt: f32) -> f32 {
    match last {
        Some(last) if dt > 1e-4 => last.distance(now) / dt,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_is_the_defaults_and_the_written_file_says_the_same() {
        let (cfg, problems) = VrConfig::parse("");
        assert_eq!(cfg, VrConfig::default());
        assert!(problems.is_empty());
        let (cfg, problems) = VrConfig::parse(&VrConfig::default_text());
        assert_eq!(cfg, VrConfig::default());
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn the_defaults_are_first_person_snap_30_and_the_vignette_on() {
        let d = VrConfig::default();
        assert_eq!(d.person, Person::First);
        assert_eq!(d.turn, TurnStyle::Snap);
        assert_eq!(d.snap_degrees, 30.0);
        assert!(d.vignette);
    }

    #[test]
    fn values_override_and_bad_ones_keep_the_default_and_say_so() {
        let (cfg, problems) = VrConfig::parse(
            "person = \"third\"\nturn = \"smooth\"\nsnap_degrees = 45\n\
             smooth_degrees_per_second = 90\nvignette = false\nvignette_strength = 0.8\n\
             third_person_distance = 8",
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            cfg,
            VrConfig {
                person: Person::Third,
                turn: TurnStyle::Smooth,
                snap_degrees: 45.0,
                smooth_degrees_per_second: 90.0,
                vignette: false,
                vignette_strength: 0.8,
                third_person_distance: 8.0,
            }
        );
        let (cfg, problems) = VrConfig::parse(
            "person = \"sideways\"\nsnap_degrees = 400\nvignette_strength = 2\n\
             third_person_distance = 0.1\nturn = \"snap\"",
        );
        assert_eq!(cfg, VrConfig::default());
        assert_eq!(problems.len(), 4, "{problems:?}");
    }

    #[test]
    fn a_misspelt_key_or_a_malformed_file_is_the_defaults_and_a_problem() {
        let (cfg, problems) = VrConfig::parse("snap_degree = 45");
        assert_eq!(cfg, VrConfig::default());
        assert_eq!(problems.len(), 1);
        let (cfg, problems) = VrConfig::parse("snap_degrees = ");
        assert_eq!(cfg, VrConfig::default());
        assert_eq!(problems.len(), 1);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("benilla-vr-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_first_run_writes_the_file_and_reads_it_back() {
        let dir = scratch("first-run");
        let (cfg, problems) = load(Some(&dir));
        assert_eq!(cfg, VrConfig::default());
        assert!(problems.is_empty(), "{problems:?}");
        assert!(dir.join("vr.toml").is_file());
        std::fs::write(dir.join("vr.toml"), "snap_degrees = 45").unwrap();
        assert_eq!(load(Some(&dir)).0.snap_degrees, 45.0);
        assert_eq!(load(None).0, VrConfig::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_watch_reloads_once_when_the_file_changes_and_not_before_a_second() {
        let dir = scratch("watch");
        std::fs::write(dir.join("vr.toml"), "snap_degrees = 45").unwrap();
        let mut watch = Watch::new(Some(&dir));
        assert!(
            watch.step(1.0).is_none(),
            "present at start is already seen"
        );

        // A longer file is a change whatever the clock's granularity.
        std::fs::write(dir.join("vr.toml"), "snap_degrees = 60\n\n").unwrap();
        assert!(watch.step(0.4).is_none(), "not yet a second");
        let (cfg, problems) = watch.step(0.7).expect("changed");
        assert_eq!(cfg.snap_degrees, 60.0);
        assert!(problems.is_empty());
        assert!(watch.step(1.0).is_none(), "seen once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_snap_fires_once_per_flick_and_rearms_when_the_stick_comes_back() {
        let mut snap = Snap::default();
        assert_eq!(snap.step(0.5), 0.0, "short of the flick");
        assert_eq!(snap.step(0.9), 1.0, "right");
        assert_eq!(snap.step(1.0), 0.0, "held, however long");
        assert_eq!(snap.step(0.5), 0.0, "not back far enough to re-arm");
        assert_eq!(snap.step(0.9), 0.0);
        assert_eq!(snap.step(0.1), 0.0);
        assert_eq!(snap.step(-0.8), -1.0, "left, once it came back");
        assert_eq!(snap.step(-0.1), 0.0);
        assert_eq!(snap.step(0.8), 1.0);
    }

    #[test]
    fn a_snap_turns_the_angle_from_the_settings() {
        let mut snap = Snap::default();
        let mut cfg = VrConfig::default();
        let t = turn(&cfg, &mut snap, 1.0, 0.016);
        assert_eq!(t.snapped, 1.0);
        assert_eq!(t.rate, 0.0, "a step is not a rate");
        // 30° at 0.003 rad a pixel.
        let want = 30.0f32.to_radians() / 0.003;
        assert!((t.motion.x - want).abs() < 1e-3, "{}", t.motion.x);
        assert_eq!(t.motion.y, 0.0);

        cfg.snap_degrees = 45.0;
        let _ = turn(&cfg, &mut snap, 0.0, 0.016);
        let t = turn(&cfg, &mut snap, -1.0, 0.016);
        assert!((t.motion.x + 45.0f32.to_radians() / 0.003).abs() < 1e-3);
        assert!(t.motion.x < 0.0, "left is the mouse moving left");
    }

    #[test]
    fn a_smooth_turn_follows_the_stick_at_the_rate_from_the_settings() {
        let cfg = VrConfig {
            turn: TurnStyle::Smooth,
            smooth_degrees_per_second: 100.0,
            ..default()
        };
        let mut snap = Snap::default();
        let t = turn(&cfg, &mut snap, 0.5, 0.1);
        assert_eq!(t.rate, 50.0);
        assert_eq!(t.snapped, 0.0);
        // 5° this frame.
        assert!((t.motion.x - 5.0f32.to_radians() / 0.003).abs() < 1e-3);
        let t = turn(&cfg, &mut snap, 0.0, 0.1);
        assert_eq!(t, Turned::default());
        // Half the frames, the same turn per second.
        let a: f32 = (0..10)
            .map(|_| turn(&cfg, &mut snap, 1.0, 0.1).motion.x)
            .sum();
        let b: f32 = (0..20)
            .map(|_| turn(&cfg, &mut snap, 1.0, 0.05).motion.x)
            .sum();
        assert!((a - b).abs() < 1e-2, "{a} {b}");
    }

    #[test]
    fn the_vignette_is_off_at_rest_full_at_a_run_and_a_turn_counts_too() {
        assert_eq!(vignette_target(0.0, 0.0), 0.0);
        assert_eq!(
            vignette_target(SPEED_DEAD, 0.0),
            0.0,
            "a nudge is not a move"
        );
        assert_eq!(vignette_target(SPEED_FULL, 0.0), 1.0);
        assert_eq!(
            vignette_target(30.0, 0.0),
            1.0,
            "a mount does not overshoot"
        );
        let walk = vignette_target(3.5, 0.0);
        assert!(walk > 0.0 && walk < 1.0);
        assert!(vignette_target(5.0, 0.0) > walk);
        assert_eq!(vignette_target(0.0, TURN_DEAD), 0.0);
        assert_eq!(vignette_target(0.0, -TURN_FULL), 1.0, "either way");
        assert_eq!(vignette_target(7.0, 90.0), 1.0);
        let both = vignette_target(3.5, 45.0);
        assert_eq!(
            both,
            walk.max(vignette_target(0.0, 45.0)),
            "the stronger, not the sum"
        );
    }

    #[test]
    fn the_vignette_fades_in_over_a_fifth_of_a_second_and_out_over_two_fifths() {
        let mut fade = Fade::default();
        assert!((fade.step(1.0, 0.1) - 0.5).abs() < 1e-6);
        assert!((fade.step(1.0, 0.1) - 1.0).abs() < 1e-6);
        assert_eq!(fade.step(1.0, 0.1), 1.0, "no overshoot");
        assert!((fade.step(0.0, 0.2) - 0.5).abs() < 1e-6);
        assert!((fade.step(0.0, 0.2) - 0.0).abs() < 1e-6);
        // A partial target stops there from either side.
        assert!((fade.step(0.3, 1.0) - 0.3).abs() < 1e-6);
        fade.level = 1.0;
        assert!((fade.step(0.3, 10.0) - 0.3).abs() < 1e-6);
    }

    #[test]
    fn the_aperture_closes_further_at_a_higher_strength_and_never_opens_past_wide() {
        assert_eq!(aperture(0.5, 0.0), OPEN_ANGLE);
        assert!((aperture(0.0, 1.0) - CLOSED_WEAK).abs() < 1e-6);
        assert!((aperture(1.0, 1.0) - CLOSED_STRONG).abs() < 1e-6);
        assert!(aperture(1.0, 1.0) < aperture(0.5, 1.0));
        assert!(aperture(0.5, 1.0) < aperture(0.5, 0.5));
        assert_eq!(
            aperture(9.0, 1.0),
            aperture(1.0, 1.0),
            "strength is clamped"
        );
        // Closed at the default, the ring is still outside the eye's own pole.
        assert!(aperture(0.5, 1.0) > 20.0 * DEG);
    }

    #[test]
    fn both_clicks_held_for_a_second_recentre_once() {
        let mut hold = RecentreHold::default();
        let mut fired = 0;
        for _ in 0..200 {
            fired += usize::from(hold.step(0.01, true));
        }
        assert_eq!(fired, 1, "two seconds is still one recentre");
        assert!(!hold.step(0.01, false));
        let mut at = None;
        for i in 0..150 {
            if hold.step(0.01, true) {
                at = Some(i);
                break;
            }
        }
        assert!(
            matches!(at, Some(98..=100)),
            "a second after letting go, again: {at:?}"
        );
    }

    #[test]
    fn letting_go_early_resets_the_hold() {
        let mut hold = RecentreHold::default();
        for _ in 0..90 {
            assert!(!hold.step(0.01, true));
        }
        assert!(!hold.step(0.01, false));
        for _ in 0..90 {
            assert!(!hold.step(0.01, true), "starts over");
        }
    }

    #[test]
    fn first_person_zooms_all_the_way_in_once() {
        let cfg = VrConfig::default();
        let mut zoom = Zoom::default();
        assert_eq!(zoom.step(&cfg).as_deref(), Some("CameraZoomIn(50)"));
        assert_eq!(zoom.step(&cfg), None);
        assert_eq!(zoom.step(&cfg), None);
    }

    #[test]
    fn third_person_zooms_in_and_then_out_on_the_next_frame() {
        let cfg = VrConfig {
            person: Person::Third,
            third_person_distance: 8.0,
            ..default()
        };
        let mut zoom = Zoom::default();
        assert_eq!(zoom.step(&cfg).as_deref(), Some("CameraZoomIn(50)"));
        assert_eq!(zoom.step(&cfg).as_deref(), Some("CameraZoomOut(8)"));
        assert_eq!(zoom.step(&cfg), None);
    }

    #[test]
    fn a_change_of_person_or_distance_zooms_again_and_a_change_mid_way_drops_the_pending_out() {
        let mut zoom = Zoom::default();
        let first = VrConfig::default();
        let third = VrConfig {
            person: Person::Third,
            ..default()
        };
        zoom.step(&first);
        assert_eq!(zoom.step(&third).as_deref(), Some("CameraZoomIn(50)"));
        assert_eq!(zoom.step(&third).as_deref(), Some("CameraZoomOut(5.55)"));
        // Back to first before the out went.
        assert!(zoom.step(&first).is_some());
        let again = VrConfig {
            third_person_distance: 9.0,
            ..third
        };
        assert!(zoom.step(&again).is_some());
        assert!(zoom.step(&first).is_some());
        assert_eq!(zoom.step(&first), None, "the stale out was dropped");
        zoom.reset();
        assert!(zoom.step(&first).is_some(), "a new session zooms again");
    }

    #[test]
    fn speed_is_the_distance_over_the_time_and_zero_with_no_last_frame() {
        assert_eq!(speed(None, Vec3::X, 0.1), 0.0);
        assert!((speed(Some(Vec3::ZERO), Vec3::new(0.7, 0.0, 0.0), 0.1) - 7.0).abs() < 1e-5);
        assert_eq!(speed(Some(Vec3::ZERO), Vec3::X, 0.0), 0.0);
    }
}
