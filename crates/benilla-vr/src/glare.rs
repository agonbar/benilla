//! The sun and moon glares pushed out to the sky's depth. 1.12.1 places each on a sphere of radius
//! 12 around the camera (`sun/follow.rs`), pinned to the far depth in `sky_vertex.wgsl`. With two
//! eyes 6.4 cm apart that sphere is 12 units away in stereo: 0.33° of disparity, so the flare
//! reads as a sheet in front of the face and sits ±0.16° off the disc in each eye. The disc is at
//! `0.85 * CAM_FAR`; the glare goes there too, scaled up by the same factor, which looks the same
//! to one eye and is at infinity for two.

use bevy::prelude::*;

use benilla_world::sun::CelestialMaterial;
use benilla_world::view::{WorldCamera, CAM_FAR};

/// Where the sun and moon discs sit (`sun/follow.rs`): the glares go to this distance.
pub(super) const SKY_DISTANCE: f32 = 0.85 * CAM_FAR;

/// Nearer than this from the head is a glare (12 units); the discs, stars and domes are
/// kilometres out. A glare already moved is far past it, which is what makes the push idempotent.
const NEAR_BODY: f32 = 100.0;

/// `placed` moved along its ray from `head` to [`SKY_DISTANCE`] and scaled to match, or `None`
/// when it is not a near body.
pub(super) fn pushed_out(head: Vec3, placed: &Transform) -> Option<Transform> {
    let offset = placed.translation - head;
    let distance = offset.length();
    if !(1e-3..NEAR_BODY).contains(&distance) {
        return None;
    }
    let k = SKY_DISTANCE / distance;
    Some(Transform {
        translation: head + offset * k,
        rotation: placed.rotation,
        scale: placed.scale * k,
    })
}

/// After `follow_sun` and `follow_moons` have written this frame's glare, and before visibility.
pub(crate) fn push_glares_out(
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    mut bodies: Query<
        &mut GlobalTransform,
        (
            With<MeshMaterial3d<CelestialMaterial>>,
            Without<WorldCamera>,
        ),
    >,
) {
    let Ok(head) = camera.single() else { return };
    let head = head.translation();
    for mut global in &mut bodies {
        if let Some(out) = pushed_out(head, &global.compute_transform()) {
            *global = GlobalTransform::from(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glare_moves_out_along_its_ray_and_grows_to_match() {
        let head = Vec3::new(10.0, 5.0, -3.0);
        let dir = Vec3::new(0.3, 0.5, -0.8).normalize();
        let glare = Transform::from_translation(head + dir * 12.0).with_scale(Vec3::splat(20.0));
        let out = pushed_out(head, &glare).unwrap();
        assert!((out.translation - (head + dir * SKY_DISTANCE)).length() < 1e-2);
        // The same angular size: scale over distance is unchanged.
        assert!((out.scale.x / SKY_DISTANCE - 20.0 / 12.0).abs() < 1e-4);
        assert_eq!(out.rotation, glare.rotation);
    }

    #[test]
    fn a_body_already_in_the_sky_is_left_alone_so_the_push_is_idempotent() {
        let head = Vec3::ZERO;
        let disc = Transform::from_translation(Vec3::Y * SKY_DISTANCE);
        assert!(pushed_out(head, &disc).is_none());
        let glare = Transform::from_translation(Vec3::X * 12.0);
        let once = pushed_out(head, &glare).unwrap();
        assert!(pushed_out(head, &once).is_none());
    }

    #[test]
    fn a_body_on_the_head_is_skipped_rather_than_divided_by_zero() {
        assert!(pushed_out(Vec3::ONE, &Transform::from_translation(Vec3::ONE)).is_none());
    }
}
