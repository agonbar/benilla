//! The head rig as plain math: where the tracking root, the head and the culling camera sit, with
//! no OpenXR or ECS in it, so the headless tests below cover it.

use bevy::prelude::*;

/// One eye as OpenXR reports it: its pose in tracking space and its four field-of-view angles in
/// radians, `left` and `down` negative.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Eye {
    pub pose: Transform,
    pub left: f32,
    pub right: f32,
    pub up: f32,
    pub down: f32,
}

/// The game's yaw alone: pitch and roll are dropped, the head provides them.
pub fn yaw_only(rotation: Quat) -> Quat {
    Quat::from_rotation_y(rotation.to_euler(EulerRot::YXZ).0)
}

/// The tracking root's pose for the game camera at `game`: its position and yaw.
pub fn root_pose(game: &Transform) -> Transform {
    Transform {
        translation: game.translation,
        rotation: yaw_only(game.rotation),
        scale: Vec3::ONE,
    }
}

/// The head's centre in tracking space: between the eyes, facing as they do.
pub fn head_centre(eyes: &[Eye; 2]) -> Transform {
    Transform {
        translation: eyes[0].pose.translation.midpoint(eyes[1].pose.translation),
        rotation: eyes[0].pose.rotation.slerp(eyes[1].pose.rotation, 0.5),
        scale: Vec3::ONE,
    }
}

/// Margin on every side of the eyes' view that the culling camera keeps at rest: the eyes sit
/// 3 cm either side of the camera, 1.8° at one metre.
pub const MARGIN_AT_REST: f32 = 3.0 * DEG;

/// Frames of head motion the margin covers. The culls read the views the runtime located in the
/// previous frame, and the readers of the `Frustum` component one frame before that; the eyes
/// draw with views located again in the render world. The turn the last frame took stands in for
/// the one to come.
const LAG_FRAMES: f32 = 3.0;

/// The most the margin grows: a hitch or a snap turn must not blow the cull open for good.
const MARGIN_MAX: f32 = 25.0 * DEG;

/// The widest half-angle the culling camera takes; past it the tangent runs away.
const HALF_ANGLE_MAX: f32 = 85.0 * DEG;

const DEG: f32 = std::f32::consts::PI / 180.0;

/// The margin for a head that turned `turned` radians in the last frame.
pub fn margin(turned: f32) -> f32 {
    (MARGIN_AT_REST + LAG_FRAMES * turned).min(MARGIN_MAX)
}

/// The largest horizontal and vertical half-angle tangents of either eye's view, measured from
/// the head's forward axis `head` (a rotation in tracking space). Each eye is a pyramid, so its
/// corners hold the extremes; an eye turned in its pose (canted lenses) is counted at its turn.
pub fn half_tans(eyes: &[Eye; 2], head: Quat) -> (f32, f32) {
    let to_head = head.inverse();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    for eye in eyes {
        for tx in [eye.left.tan(), eye.right.tan()] {
            for ty in [eye.down.tan(), eye.up.tan()] {
                let dir = to_head * (eye.pose.rotation * Vec3::new(tx, ty, -1.0));
                // A corner at or behind the head's plane has no finite tangent.
                let ahead = (-dir.z).max(1e-3);
                x = x.max(dir.x.abs() / ahead);
                y = y.max(dir.y.abs() / ahead);
            }
        }
    }
    (x, y)
}

/// The vertical field of view of a camera of `aspect` (width over height) facing `head` that sees
/// everything either eye sees, and `margin` radians more on every side.
pub fn covering_fov_y(eyes: &[Eye; 2], head: Quat, aspect: f32, margin: f32) -> f32 {
    let (x, y) = half_tans(eyes, head);
    let widen = |tan: f32| (tan.atan() + margin).min(HALF_ANGLE_MAX).tan();
    2.0 * widen(y).max(widen(x) / aspect).atan()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

    fn eye(x: f32, fov: [f32; 4]) -> Eye {
        Eye {
            pose: Transform::from_xyz(x, 0.0, 0.0),
            left: fov[0],
            right: fov[1],
            up: fov[2],
            down: fov[3],
        }
    }

    #[test]
    fn the_root_keeps_yaw_and_drops_pitch_and_roll() {
        let game = Transform::from_xyz(1.0, 2.0, 3.0).with_rotation(Quat::from_euler(
            EulerRot::YXZ,
            0.7,
            -0.4,
            0.2,
        ));
        let root = root_pose(&game);
        assert_eq!(root.translation, game.translation);
        let (yaw, pitch, roll) = root.rotation.to_euler(EulerRot::YXZ);
        assert!((yaw - 0.7).abs() < 1e-5);
        assert!(pitch.abs() < 1e-5 && roll.abs() < 1e-5);
    }

    #[test]
    fn the_root_faces_where_the_game_camera_faces() {
        let game =
            Transform::from_xyz(0.0, 5.0, 0.0).looking_to(Vec3::new(1.0, -1.0, 0.0), Vec3::Y);
        let facing = root_pose(&game).rotation * Vec3::NEG_Z;
        assert!((facing - Vec3::X).length() < 1e-5, "{facing}");
    }

    #[test]
    fn the_head_is_between_the_eyes() {
        let eyes = [eye(-0.03, [-0.8; 4]), eye(0.03, [0.8; 4])];
        let head = head_centre(&eyes);
        assert!(head.translation.length() < 1e-6);
        assert!(head.rotation.angle_between(Quat::IDENTITY) < 1e-6);
    }

    #[test]
    fn the_head_in_the_world_turns_with_the_root() {
        let eyes = [eye(0.0, [-0.8; 4]), eye(0.0, [0.8; 4])];
        let mut eyes = eyes;
        eyes[0].pose.translation = Vec3::new(0.0, 0.0, -1.0);
        eyes[1].pose.translation = Vec3::new(0.0, 0.0, -1.0);
        let game =
            Transform::from_xyz(10.0, 0.0, 0.0).with_rotation(Quat::from_rotation_y(FRAC_PI_2));
        let world = root_pose(&game).mul_transform(head_centre(&eyes));
        // One unit ahead of the head, with the root turned 90° left: that is -X from the root.
        assert!((world.translation - Vec3::new(9.0, 0.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn the_fov_covers_the_taller_eye_in_both_directions() {
        // Symmetric 90° vertical at 1:1, 90° horizontal: the union is 90°.
        let eyes = [
            eye(-0.03, [-FRAC_PI_4, FRAC_PI_4, FRAC_PI_4, -FRAC_PI_4]),
            eye(0.03, [-FRAC_PI_4, FRAC_PI_4, FRAC_PI_4, -FRAC_PI_4]),
        ];
        assert!((covering_fov_y(&eyes, Quat::IDENTITY, 1.0, 0.0) - FRAC_PI_2).abs() < 1e-5);
        // Asymmetric: one eye looks further up, the other further down.
        let eyes = [
            eye(-0.03, [-0.5, 0.5, 0.9, -0.3]),
            eye(0.03, [-0.5, 0.5, 0.3, -0.9]),
        ];
        let want = 2.0 * 0.9f32.tan().atan();
        assert!((covering_fov_y(&eyes, Quat::IDENTITY, 1.0, 0.0) - want).abs() < 1e-5);
    }

    #[test]
    fn a_wide_view_widens_the_vertical_fov_of_a_narrower_camera() {
        let eyes = [
            eye(-0.03, [-1.0, 0.8, 0.4, -0.4]),
            eye(0.03, [-0.8, 1.0, 0.4, -0.4]),
        ];
        // Horizontal half-tangent tan(1.0) at 16:9 needs a taller vertical half than tan(0.4).
        let fov = covering_fov_y(&eyes, Quat::IDENTITY, 16.0 / 9.0, 0.0);
        let want = 2.0 * (1.0f32.tan() / (16.0 / 9.0)).atan();
        assert!((fov - want).abs() < 1e-5);
        assert!(fov > 2.0 * 0.4f32.tan().atan());
    }

    #[test]
    fn the_margin_widens_every_side_by_its_angle() {
        let eyes = [
            eye(-0.03, [-FRAC_PI_4, FRAC_PI_4, FRAC_PI_4, -FRAC_PI_4]),
            eye(0.03, [-FRAC_PI_4, FRAC_PI_4, FRAC_PI_4, -FRAC_PI_4]),
        ];
        let fov = covering_fov_y(&eyes, Quat::IDENTITY, 1.0, 0.1);
        assert!((fov - (FRAC_PI_2 + 0.2)).abs() < 1e-5, "{fov}");
    }

    #[test]
    fn canted_eyes_widen_the_view_by_their_cant() {
        // Each eye 90° wide and turned 10° outward: the pair spans 110°, which 90° misses.
        let cant = 10.0f32.to_radians();
        let mut eyes = [
            eye(-0.03, [-FRAC_PI_4, FRAC_PI_4, FRAC_PI_4, -FRAC_PI_4]),
            eye(0.03, [-FRAC_PI_4, FRAC_PI_4, FRAC_PI_4, -FRAC_PI_4]),
        ];
        eyes[0].pose.rotation = Quat::from_rotation_y(cant);
        eyes[1].pose.rotation = Quat::from_rotation_y(-cant);
        let (x, _) = half_tans(&eyes, head_centre(&eyes).rotation);
        let want = (FRAC_PI_4 + cant).tan();
        assert!((x - want).abs() < 1e-4, "{x} against {want}");
        // At 1:1 the vertical angle is then the horizontal one.
        let fov = covering_fov_y(&eyes, head_centre(&eyes).rotation, 1.0, 0.0);
        assert!((fov - 2.0 * (FRAC_PI_4 + cant)).abs() < 1e-4, "{fov}");
    }

    #[test]
    fn the_margin_grows_with_the_turn_and_stops() {
        assert_eq!(margin(0.0), MARGIN_AT_REST);
        assert!(margin(0.02) > margin(0.01));
        assert_eq!(margin(10.0), MARGIN_MAX);
    }
}
