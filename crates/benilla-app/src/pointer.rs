//! The pointer seam: where the cursor and the world ray come from when the OS mouse is not the
//! only source. 1.12.1 has the mouse and nothing else, so the seam is empty in stock benilla and
//! every read falls through to the window's cursor, as it did before. A crate on top fills
//! [`Pointer`]: a gamepad's virtual cursor, and later a VR laser.
//!
//! Priority: the VR laser, then the pad's cursor, then the OS mouse, then the headless hover probe.
//! The cursor is in the window's logical pixels, y down, what `Window::cursor_position` answers.
//! Nothing here moves the OS cursor, so a trackpad and a pad's cursor can both be live.
//!
//! A tracked laser owns the pointer outright: off the panel it has no UI cursor, and the other
//! sources are not read, so a stale OS cursor cannot hover a window behind the player's back.

use std::sync::RwLock;

use bevy::prelude::*;

use crate::target::hover_probe_point;

/// What a tracked laser aims at: the cursor on a UI panel it hits, if any, and its ray into the
/// world, in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aim {
    pub cursor: Option<Vec2>,
    pub ray: Option<Ray3d>,
}

/// The sources other than the OS mouse. A source clears its field when it lets go of the pointer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pointer {
    pub vr: Option<Aim>,
    pub pad: Option<Vec2>,
}

impl Pointer {
    /// The cursor of the highest source that has one. A laser that is tracked has the pointer:
    /// its cursor is the panel hit, and none off the panel.
    fn cursor(&self) -> Option<Vec2> {
        match self.vr {
            Some(aim) => aim.cursor,
            None => self.pad,
        }
    }

    /// Whether a laser's ray, not the cursor, aims the world picks.
    fn aims(&self) -> bool {
        self.vr.is_some_and(|a| a.ray.is_some())
    }
}

/// Process-wide, like the binding-token registry: the readers are systems all over the client and
/// a pointer source is a crate on top, so the seam is not a resource each reader has to ask for.
static SOURCES: RwLock<Pointer> = RwLock::new(Pointer {
    vr: None,
    pad: None,
});

fn sources() -> Pointer {
    *SOURCES.read().unwrap_or_else(|e| e.into_inner())
}

fn update(f: impl FnOnce(&mut Pointer)) {
    f(&mut SOURCES.write().unwrap_or_else(|e| e.into_inner()));
}

/// The pad's virtual cursor, in logical window px, or `None` when the pad lets go of the pointer.
pub fn set_pad(cursor: Option<Vec2>) {
    update(|p| p.pad = cursor);
}

/// The laser's aim, or `None` when it lets go of the pointer.
pub fn set_vr(aim: Option<Aim>) {
    update(|p| p.vr = aim);
}

/// The first thing a laser meets in the world: how far along the ray, and the unit or object there
/// if it is one the player can act on (the hover).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub distance: f32,
    pub guid: Option<u64>,
}

static HIT: RwLock<Option<Hit>> = RwLock::new(None);

/// The laser's world hit, or `None` when its ray meets nothing. Written by the target code each
/// frame; a crate with a laser reads it for the beam's length and for a haptic pulse.
pub fn set_hit(hit: Option<Hit>) {
    *HIT.write().unwrap_or_else(|e| e.into_inner()) = hit;
}

pub fn hit() -> Option<Hit> {
    *HIT.read().unwrap_or_else(|e| e.into_inner())
}

/// Whether a laser's ray aims the world picks. The reference's mouselook blanks the mouseover
/// (`0x483e80`) because its cursor is hidden at the screen's middle; a laser points independently
/// of the look, so its hover stands through one.
pub fn laser_aims() -> bool {
    sources().aims()
}

/// The UI's cursor: the seam's, else the window's, else the hover probe's aim. A tracked laser
/// alone answers when it is there.
pub fn ui_cursor(window: &Window) -> Option<Vec2> {
    resolve(&sources(), window.cursor_position(), hover_probe_point())
}

fn resolve(p: &Pointer, os: Option<Vec2>, probe: Option<Vec2>) -> Option<Vec2> {
    if p.vr.is_some() {
        return p.cursor();
    }
    p.cursor().or(os).or(probe)
}

/// The cursor in physical pixels, for the viewport test. The probe stands in for no one here.
pub fn physical_cursor(window: &Window) -> Option<Vec2> {
    physical(
        &sources(),
        window.scale_factor(),
        window.physical_cursor_position(),
        window.physical_size().as_vec2(),
    )
}

/// A laser aiming at the world has no pixel of its own, and a press counts as a world press only
/// inside the viewport: the window's middle stands in for it.
fn physical(p: &Pointer, scale: f32, os: Option<Vec2>, size: Vec2) -> Option<Vec2> {
    if p.vr.is_some() {
        return p
            .cursor()
            .map(|c| c * scale)
            .or(p.aims().then_some(size / 2.0));
    }
    p.cursor().map(|c| c * scale).or(os)
}

/// The ray the world picks along: a laser's own, else the camera's through [`ui_cursor`].
pub fn world_ray(
    camera: &Camera,
    camera_transform: &GlobalTransform,
    window: &Window,
) -> Option<Ray3d> {
    pick_ray(&sources(), ui_cursor(window), |c| {
        camera.viewport_to_world(camera_transform, c).ok()
    })
}

/// The laser's ray if it has one, else `through` the cursor.
fn pick_ray(
    p: &Pointer,
    cursor: Option<Vec2>,
    through: impl FnOnce(Vec2) -> Option<Ray3d>,
) -> Option<Ray3d> {
    p.vr.and_then(|a| a.ray).or_else(|| through(cursor?))
}

/// A window cursor (logical px, y down) in the VM's root units, y up: where the UI sees it.
pub fn window_to_ui(window_height: f32, cursor: Vec2) -> Vec2 {
    crate::ui_script::window_to_ui(window_height, cursor)
}

/// A window cursor in logical px for a point in the VM's root units, y up: the inverse of
/// [`crate::ui_script::window_to_ui`], for a crate that moves the cursor to a widget.
pub fn ui_to_window(window_height: f32, ui: Vec2) -> Vec2 {
    let s = crate::ui_script::seam_scale(window_height);
    Vec2::new(ui.x * s, window_height - ui.y * s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LASER: Aim = Aim {
        cursor: Some(Vec2::new(5.0, 5.0)),
        ray: Some(Ray3d {
            origin: Vec3::ZERO,
            direction: Dir3::X,
        }),
    };

    #[test]
    fn the_cursor_is_vr_then_pad_then_os_then_probe() {
        let (os, probe) = (Some(Vec2::new(1.0, 1.0)), Some(Vec2::new(9.0, 9.0)));
        let pad = Vec2::new(2.0, 2.0);
        let all = Pointer {
            vr: Some(LASER),
            pad: Some(pad),
        };
        assert_eq!(resolve(&all, os, probe), Some(Vec2::new(5.0, 5.0)));
        let pad_only = Pointer {
            vr: None,
            pad: Some(pad),
        };
        assert_eq!(resolve(&pad_only, os, probe), Some(pad));
        let none = Pointer::default();
        assert_eq!(resolve(&none, os, probe), os);
        assert_eq!(resolve(&none, None, probe), probe);
        assert_eq!(resolve(&none, None, None), None);
    }

    #[test]
    fn a_laser_off_the_panel_keeps_the_pointer_and_has_no_cursor() {
        let off = Pointer {
            vr: Some(Aim {
                cursor: None,
                ..LASER
            }),
            pad: Some(Vec2::new(2.0, 2.0)),
        };
        assert_eq!(resolve(&off, Some(Vec2::ONE), Some(Vec2::ONE)), None);
        assert!(off.aims());
        assert!(!Pointer::default().aims());
    }

    #[test]
    fn a_laser_in_the_world_is_inside_the_viewport_and_a_panel_hit_is_where_it_hits() {
        let size = Vec2::new(1000.0, 600.0);
        let world = Pointer {
            vr: Some(Aim {
                cursor: None,
                ..LASER
            }),
            pad: None,
        };
        assert_eq!(
            physical(&world, 2.0, Some(Vec2::new(9.0, 9.0)), size),
            Some(size / 2.0),
            "the OS cursor is not read"
        );
        let panel = Pointer {
            vr: Some(LASER),
            pad: None,
        };
        assert_eq!(
            physical(&panel, 2.0, None, size),
            Some(Vec2::new(10.0, 10.0))
        );
    }

    #[test]
    fn the_world_ray_is_the_lasers_else_through_the_cursor() {
        let seen = std::cell::Cell::new(None);
        let through = |c: Vec2| {
            seen.set(Some(c));
            Some(Ray3d {
                origin: Vec3::Y,
                direction: Dir3::Z,
            })
        };
        let laser = Pointer {
            vr: Some(LASER),
            pad: Some(Vec2::new(2.0, 2.0)),
        };
        let ray = pick_ray(&laser, Some(Vec2::new(5.0, 5.0)), through).unwrap();
        assert_eq!(ray.direction, Dir3::X);
        assert_eq!(seen.get(), None, "the camera is not asked");
        let pad = Pointer {
            vr: None,
            pad: Some(Vec2::new(2.0, 2.0)),
        };
        let ray = pick_ray(&pad, Some(Vec2::new(2.0, 2.0)), through).unwrap();
        assert_eq!(ray.direction, Dir3::Z);
        assert_eq!(seen.get(), Some(Vec2::new(2.0, 2.0)));
        assert!(
            pick_ray(&Pointer::default(), None, through).is_none(),
            "no cursor, no ray"
        );
        // A laser tracked with no world ray (its root unknown) over the panel falls back to the
        // camera's ray through the panel's cursor; off the panel there is nothing to cast through.
        let on_panel = Pointer {
            vr: Some(Aim { ray: None, ..LASER }),
            pad: None,
        };
        let ray = pick_ray(&on_panel, Some(Vec2::new(5.0, 5.0)), through).unwrap();
        assert_eq!(ray.direction, Dir3::Z);
        let off_panel = Pointer {
            vr: Some(Aim {
                cursor: None,
                ray: None,
            }),
            pad: None,
        };
        assert!(pick_ray(&off_panel, resolve(&off_panel, None, None), through).is_none());
    }

    #[test]
    fn a_virtual_cursor_is_scaled_to_physical_pixels_and_the_os_one_is_not() {
        let pad = Pointer {
            vr: None,
            pad: Some(Vec2::new(10.0, 20.0)),
        };
        let os = Some(Vec2::new(7.0, 7.0));
        let size = Vec2::new(100.0, 100.0);
        assert_eq!(physical(&pad, 2.0, os, size), Some(Vec2::new(20.0, 40.0)));
        assert_eq!(physical(&Pointer::default(), 2.0, os, size), os);
    }

    #[test]
    fn ui_units_and_window_px_are_inverses() {
        for h in [768.0, 800.0, 1080.0] {
            let c = Vec2::new(321.0, 123.0);
            let back = ui_to_window(h, crate::ui_script::window_to_ui(h, c));
            assert!((back - c).length() < 1e-3, "{back} at {h}");
        }
    }
}
