//! Which element of the sky has stereo disparity in the headset. The sun, moon and glare quads, the
//! sky gradient dome, the cloud dome and the star patches all sit on the head centre and are
//! drawn by both eyes, which are 3 cm either side of it, so an element `d` metres out puts its
//! point about `0.064 / d` radians apart between the eyes. Past a few hundred metres that is under
//! what an eye can tell from infinity, and only the nearer elements read as near.
//!
//! [`sight`] is the measure, over an element's real vertices: the nearest one and the largest
//! disparity. `WOW_VR_SKY_TRACE=1` prints it once a second for the live sky, at the `far` of the
//! world camera the placement systems read; the tests below take the same measure at the placement
//! the `follow_*` systems give (`sun/follow.rs`, `sky.rs`, `clouds/layer.rs`).

use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;

use benilla_world::clouds::CloudMaterial;
use benilla_world::sky::SkyMaterial;
use benilla_world::sun::{CelestialMaterial, StarMaterial};
use benilla_world::view::WorldCamera;
use bevy_mod_openxr::helper_traits::ToTransform;
use bevy_mod_openxr::resources::OxrViews;
use bevy_mod_xr::session::XrTrackingRoot;

/// Disparity under which an element reads as at infinity: one arc-minute, about half a display
/// pixel on a Quest 3 (some 25 pixels per degree), where stereo acuity bottoms out too.
pub(crate) const AT_SKY_DEPTH: f32 = std::f32::consts::PI / 180.0 / 60.0;

/// What the eyes see of one element.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sighting {
    /// The nearest vertex to the head centre, in metres.
    pub nearest: f32,
    /// The angle between the two eyes' rays to the vertex with the most of it, in radians.
    pub disparity: f32,
}

/// The angle between the rays from each eye to `point`. In `f64`: past a kilometre the angle is
/// 1e-5 rad, and `f32`'s `acos` of two nearly parallel unit vectors is 1e-4 of noise.
pub(crate) fn disparity(eyes: &[Vec3; 2], point: Vec3) -> f32 {
    let ray = |eye: Vec3| (point.as_dvec3() - eye.as_dvec3()).normalize();
    let (a, b) = (ray(eyes[0]), ray(eyes[1]));
    // atan2 of the cross and dot products keeps small angles exact.
    a.cross(b).length().atan2(a.dot(b)) as f32
}

/// The element's vertices (local space, placed by `world_from_local`) as the two `eyes` see them
/// from `head`.
pub(crate) fn sight(
    eyes: &[Vec3; 2],
    head: Vec3,
    world_from_local: &GlobalTransform,
    vertices: impl IntoIterator<Item = Vec3>,
) -> Option<Sighting> {
    vertices
        .into_iter()
        .map(|v| world_from_local.transform_point(v))
        .map(|p| Sighting {
            nearest: p.distance(head),
            disparity: disparity(eyes, p),
        })
        .reduce(|a, b| Sighting {
            nearest: a.nearest.min(b.nearest),
            disparity: a.disparity.max(b.disparity),
        })
}

fn positions(mesh: &Mesh) -> Option<impl Iterator<Item = Vec3> + '_> {
    match mesh.attribute(Mesh::ATTRIBUTE_POSITION)? {
        VertexAttributeValues::Float32x3(p) => Some(p.iter().map(|&p| Vec3::from(p))),
        _ => None,
    }
}

type Sky<'w, 's, M> = Query<
    'w,
    's,
    (
        &'static GlobalTransform,
        &'static Mesh3d,
        &'static Visibility,
    ),
    With<MeshMaterial3d<M>>,
>;

/// `WOW_VR_SKY_TRACE=1`, once a second after the glares were pushed out and the runtime located
/// this frame's views. One line per element:
///
/// `vr sky: <kind> <n> dist <nearest vertex> m disparity <deg> deg <AT SKY DEPTH | NEAR> ...`
///
/// `NEAR` is a disparity over [`AT_SKY_DEPTH`]. A disc or glare also prints its direction and the
/// width of its quad, which is what size tells the eye.
#[allow(clippy::type_complexity)]
pub(crate) fn trace_sky(
    time: Res<Time<Real>>,
    openxr: Option<Res<OxrViews>>,
    root: Query<&Transform, (With<XrTrackingRoot>, Without<WorldCamera>)>,
    camera: Query<(&GlobalTransform, &Projection), With<WorldCamera>>,
    meshes: Res<Assets<Mesh>>,
    celestial_materials: Res<Assets<CelestialMaterial>>,
    celestial: Query<(
        &GlobalTransform,
        &Mesh3d,
        &Visibility,
        &MeshMaterial3d<CelestialMaterial>,
    )>,
    stars: Sky<StarMaterial>,
    dome: Sky<SkyMaterial>,
    clouds: Sky<CloudMaterial>,
    mut next: Local<f32>,
) {
    let (Some(openxr), Ok(root), Ok((head, projection))) = (openxr, root.single(), camera.single())
    else {
        return;
    };
    let [l, r, ..] = openxr.as_slice() else {
        return;
    };
    if time.elapsed_secs() < *next {
        return;
    }
    *next = time.elapsed_secs() + 1.0;

    let eyes = [l, r].map(|v| root.mul_transform(v.pose.to_transform()).translation);
    let head = head.translation();
    let far = match projection {
        Projection::Perspective(p) => p.far,
        _ => f32::NAN,
    };
    info!(
        "vr sky: eyes {:.1} mm apart, world camera far {far:.0} m (the eye cameras' is infinite), \
         NEAR is over {:.4} deg of disparity",
        eyes[0].distance(eyes[1]) * 1000.0,
        AT_SKY_DEPTH.to_degrees(),
    );

    let line = |kind: String, global: &GlobalTransform, mesh: &Mesh3d, vis: &Visibility| {
        let Some(vertices) = meshes.get(&mesh.0).and_then(positions) else {
            info!("vr sky: {kind} has no CPU mesh");
            return;
        };
        let Some(s) = sight(&eyes, head, global, vertices) else {
            return;
        };
        let verdict = if s.disparity > AT_SKY_DEPTH {
            "NEAR"
        } else {
            "AT SKY DEPTH"
        };
        let centre = global.translation() - head;
        let width = 2.0 * (0.5 * global.compute_transform().scale.x / centre.length()).atan();
        let elevation = (centre.y / centre.length()).asin();
        info!(
            "vr sky: {kind:<12} dist {:>9.1} m disparity {:.5} deg {verdict} (elevation {:.1} deg, \
             quad width {:.2} deg, {vis:?})",
            s.nearest,
            s.disparity.to_degrees(),
            elevation.to_degrees(),
            width.to_degrees(),
        );
    };
    for (global, mesh, vis, material) in &celestial {
        let kind = match celestial_materials
            .get(&material.0)
            .map(|m| m.base.alpha_mode)
        {
            Some(AlphaMode::Add) => "glare",
            _ => "disc",
        };
        line(kind.into(), global, mesh, vis);
    }
    for (n, (global, mesh, vis)) in stars.iter().enumerate() {
        line(format!("stars {n}"), global, mesh, vis);
    }
    for (global, mesh, vis) in &dome {
        line("sky dome".into(), global, mesh, vis);
    }
    for (global, mesh, vis) in &clouds {
        line("cloud dome".into(), global, mesh, vis);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glare::{pushed_out, SKY_DISTANCE};
    use benilla_world::view::CAM_FAR;

    /// The eyes of a head at `head`, turned by `rotation`, `ipd` metres apart.
    fn eyes(head: Vec3, rotation: Quat, ipd: f32) -> [Vec3; 2] {
        let right = rotation * Vec3::X * (0.5 * ipd);
        [head - right, head + right]
    }

    /// A unit quad's corners and its centre, as `quad_mesh` (`sun/mesh.rs`) gives them.
    fn quad() -> Vec<Vec3> {
        vec![
            Vec3::new(-0.5, -0.5, 0.0),
            Vec3::new(0.5, -0.5, 0.0),
            Vec3::new(0.5, 0.5, 0.0),
            Vec3::new(-0.5, 0.5, 0.0),
        ]
    }

    /// Points on the unit sphere: the six axes, the eight corners and the twelve edge midpoints.
    fn sphere() -> Vec<Vec3> {
        let mut p = Vec::new();
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    if (x, y, z) != (0, 0, 0) {
                        p.push(Vec3::new(x as f32, y as f32, z as f32).normalize());
                    }
                }
            }
        }
        p
    }

    /// Every sky element as the placement systems give it, from the head at `head` and the world
    /// camera's `far`: `(name, transform, vertices)`.
    fn sky(head: Vec3, far: f32) -> Vec<(&'static str, GlobalTransform, Vec<Vec3>)> {
        let to_body = Vec3::new(0.3, 0.6, -0.7).normalize();
        // `follow_sun` / `follow_moons`: a disc at `0.85 far` facing the camera, its quad
        // `SUN_SIZE × scale × dist` wide.
        let disc = |size: f32| {
            let dist = far * 0.85;
            GlobalTransform::from(
                Transform::from_translation(head + to_body * dist)
                    .with_rotation(Quat::from_rotation_arc(Vec3::Z, -to_body))
                    .with_scale(Vec3::splat(0.0833 * size * dist)),
            )
        };
        // A glare as `follow_*` places it (radius 12), then as `push_glares_out` leaves it.
        let glare = |size: f32| {
            Transform::from_translation(head + to_body * 12.0)
                .with_rotation(Quat::from_rotation_arc(Vec3::Z, -to_body))
                .with_scale(Vec3::splat(size))
        };
        let pushed = |size: f32| GlobalTransform::from(pushed_out(head, &glare(size)).unwrap());
        // A dome: a unit sphere about the head, `follow_camera` and the like.
        let dome = |k: f32| {
            GlobalTransform::from(
                Transform::from_translation(head).with_scale(Vec3::splat(far * k)),
            )
        };
        vec![
            ("sun disc", disc(1.5), quad()),
            ("white moon disc", disc(1.75), quad()),
            ("moon02 disc", disc(1.0), quad()),
            ("sun glare", pushed(20.0), quad()),
            ("moon glare", pushed(2.0), quad()),
            ("sky dome", dome(0.9), sphere()),
            ("cloud dome", dome(0.87), sphere()),
            ("stars", dome(0.88), sphere()),
        ]
    }

    #[test]
    fn disparity_is_the_angle_between_the_eyes_rays() {
        let e = eyes(Vec3::ZERO, Quat::IDENTITY, 0.064);
        // 12 m dead ahead: 2·atan(0.032 / 12).
        let d = disparity(&e, Vec3::new(0.0, 0.0, -12.0));
        assert!((d.to_degrees() - 0.3056).abs() < 1e-3, "{}", d.to_degrees());
        // Along the baseline both rays are the same line.
        assert!(disparity(&e, Vec3::X * 100.0) < 1e-4);
    }

    /// The numbers behind the ticket, `cargo test -p benilla-vr every_sky_element -- --nocapture`.
    #[test]
    fn every_sky_element_is_at_the_sky_s_depth_at_the_real_far() {
        let head = Vec3::new(1234.0, 56.0, -789.0);
        let e = eyes(head, Quat::from_euler(EulerRot::YXZ, 0.7, -0.4, 0.1), 0.064);
        println!("far {CAM_FAR} m, sky at {SKY_DISTANCE} m, eyes 64 mm apart");
        println!(
            "{:<16} {:>10} {:>14} {:>12}",
            "element", "nearest m", "disparity deg", "stereo m"
        );
        for (name, global, vertices) in sky(head, CAM_FAR) {
            let s = sight(&e, head, &global, vertices).unwrap();
            // The distance a point straight ahead would have for this disparity.
            let stereo = 0.064 / (2.0 * (0.5 * s.disparity).tan());
            println!(
                "{name:<16} {:>10.1} {:>14.5} {:>12.0}",
                s.nearest,
                s.disparity.to_degrees(),
                stereo
            );
            assert!(
                s.disparity < AT_SKY_DEPTH,
                "{name} is near: {} deg of disparity",
                s.disparity.to_degrees()
            );
        }
    }

    #[test]
    fn the_measure_sees_a_near_element_when_there_is_one() {
        // The glare before `push_glares_out`: the control that the check can fail.
        let head = Vec3::new(0.0, 1.6, 0.0);
        let e = eyes(head, Quat::IDENTITY, 0.064);
        let dir = Vec3::new(0.0, 0.0, -1.0);
        let glare = GlobalTransform::from(
            Transform::from_translation(head + dir * 12.0).with_scale(Vec3::splat(20.0)),
        );
        let s = sight(&e, head, &glare, quad()).unwrap();
        assert!(
            s.disparity > 5.0 * AT_SKY_DEPTH,
            "{}",
            s.disparity.to_degrees()
        );
        assert!((11.0..20.0).contains(&s.nearest), "{}", s.nearest);
    }

    #[test]
    fn a_shorter_or_longer_far_plane_keeps_the_sky_at_its_depth() {
        for far in [1000.0, CAM_FAR, 5000.0] {
            let head = Vec3::new(-40.0, 3.0, 9.0);
            let e = eyes(head, Quat::from_rotation_y(2.0), 0.07);
            for (name, global, vertices) in sky(head, far) {
                let s = sight(&e, head, &global, vertices).unwrap();
                assert!(s.disparity < AT_SKY_DEPTH, "{name} at far {far}");
            }
        }
    }
}
