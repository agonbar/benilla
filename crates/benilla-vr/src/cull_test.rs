//! The CPU culls see everything either eye sees. The real systems run on the real schedule with
//! fake eye views; a stand-in for the game's `publish_camera_pose` (a root camera's `Transform`
//! into its `GlobalTransform`, in `CameraPoseSet`) and a probe ordered after `CameraPoseSet`, where
//! `compute_wmo_pvs`, the exterior windows and the model visibility read, take the matrix those
//! culls build: `Projection::get_clip_from_view` over the inverse `GlobalTransform`.
//!
//! The eyes are checked at the views the render world locates for the frame being drawn. The cull
//! reads the views located in the previous frame's `PostUpdate` (`bevy_mod_openxr`'s
//! `locate_views`), and the readers of the `Frustum` component the ones before that, since Bevy
//! recomputes it in `PostUpdate` from the pose `seat_head` seated one frame earlier.

use super::*;
use bevy::camera::primitives::{Frustum, Sphere};
use std::f32::consts::PI;

const DEG: f32 = PI / 180.0;

/// Which of the head's tilts a frame has: degrees of pitch and roll at a yaw.
#[derive(Clone, Copy)]
struct Pose {
    pitch: f32,
    roll: f32,
}

/// What a probe after `CameraPoseSet` saw.
#[derive(Resource, Default)]
struct Seen {
    /// The matrix the culls build this frame.
    clip_from_world: Option<Mat4>,
    /// The same, one frame earlier: what Bevy's `Frustum` component holds when `Update` reads it.
    frustum_clip_from_world: Option<Mat4>,
    /// The game camera's `Frustum` as the probe read it.
    frustum: Option<Frustum>,
}

fn publish_pose(mut cam: Query<(&Transform, &mut GlobalTransform), With<WorldCamera>>) {
    for (t, mut g) in &mut cam {
        *g = GlobalTransform::from(*t);
    }
}

fn probe(
    cam: Query<(&GlobalTransform, &Projection, &Frustum), With<WorldCamera>>,
    mut seen: ResMut<Seen>,
) {
    let (g, p, f) = cam.single().unwrap();
    seen.frustum_clip_from_world = seen.clip_from_world;
    seen.clip_from_world = Some(p.get_clip_from_view() * g.to_matrix().inverse());
    seen.frustum = Some(*f);
}

/// Bevy's own `update_frusta`, at the point it runs in the frame.
fn frusta(app: &mut App) {
    app.add_systems(PostUpdate, bevy::camera::visibility::update_frusta);
}

/// Eyes shaped like a Quest 3's: taller than wide, asymmetric, the left looking further out to
/// its left. Not read from a headset; `WOW_VR_CULL_TRACE` prints the real four angles.
fn fov(left_eye: bool) -> [f32; 4] {
    let (out, inn) = (-50.0 * DEG, 40.0 * DEG);
    let (up, down) = (48.0 * DEG, -52.0 * DEG);
    if left_eye {
        [out, inn, up, down]
    } else {
        [-inn, -out, up, down]
    }
}

/// The two eyes the runtime locates for the head at `pose`, `cant` radians turned outward each,
/// 6.4 cm apart at 1.6 m, with the head turned `yaw`.
fn located(pose: Pose, cant: f32) -> [openxr::View; 2] {
    let head = Quat::from_euler(EulerRot::YXZ, 0.4, pose.pitch * DEG, pose.roll * DEG);
    let view = |left_eye: bool| {
        let side = if left_eye { -1.0 } else { 1.0 };
        let q = head * Quat::from_rotation_y(-side * cant);
        let p = Vec3::new(0.0, 1.6, 0.0) + head * Vec3::new(0.032 * side, 0.0, 0.0);
        let [l, r, u, d] = fov(left_eye);
        openxr::View {
            pose: openxr::Posef {
                orientation: openxr::Quaternionf {
                    x: q.x,
                    y: q.y,
                    z: q.z,
                    w: q.w,
                },
                position: openxr::Vector3f {
                    x: p.x,
                    y: p.y,
                    z: p.z,
                },
            },
            fov: openxr::Fovf {
                angle_left: l,
                angle_right: r,
                angle_up: u,
                angle_down: d,
            },
        }
    };
    [view(true), view(false)]
}

struct Rig {
    app: App,
    root: Transform,
}

impl Rig {
    fn new(aspect: f32) -> Self {
        let mut app = App::new();
        app.insert_resource(XrState::Running)
            .init_resource::<Seen>()
            .insert_resource(OxrViews(Vec::new()))
            .add_plugins(CullCameraPlugin)
            .add_systems(Update, publish_pose.in_set(CameraPoseSet))
            .add_systems(Update, probe.after(CameraPoseSet));
        frusta(&mut app);
        let game = Transform::from_xyz(100.0, 20.0, -50.0).with_rotation(Quat::from_euler(
            EulerRot::YXZ,
            1.2,
            -0.5,
            0.1,
        ));
        app.world_mut().spawn((
            WorldCamera,
            game,
            GlobalTransform::default(),
            Frustum::default(),
            Projection::Perspective(PerspectiveProjection {
                fov: benilla_world::view::CAM_FOVY,
                aspect_ratio: aspect,
                near: NEARCLIP_DEFAULT,
                ..default()
            }),
        ));
        app.world_mut().spawn(XrTrackingRoot);
        Self {
            app,
            root: rig::root_pose(&game, &Transform::IDENTITY),
        }
    }

    /// One frame: the runtime has located `cull` since the last one, and the eyes will draw with
    /// `eyes`. Returns the worst overshoot (in NDC half-widths, positive when an eye sees what the
    /// cull does not) for the matrix the culls build and for the one `Frustum` was built from.
    fn frame(&mut self, cull: [openxr::View; 2], eyes: [openxr::View; 2]) -> (f32, f32) {
        self.app
            .world_mut()
            .insert_resource(OxrViews(cull.to_vec()));
        self.app.update();
        let seen = self.app.world().resource::<Seen>();
        let points = self.eye_points(eyes);
        // The `Frustum` component is the stale matrix's volume: Bevy built it from that pose.
        for p in &points {
            let o = seen.frustum_clip_from_world.map(|c| overshoot(&c, *p));
            if let Some(o) = o.filter(|o| o.abs() > 1e-3 && o.is_finite()) {
                assert_eq!(
                    self.frustum_admits(*p),
                    o < 0.0,
                    "Frustum vs matrix at {p}: {o}"
                );
            }
        }
        let worst = |clip: Option<Mat4>| {
            clip.map_or(f32::NEG_INFINITY, |clip| {
                points
                    .iter()
                    .map(|p| overshoot(&clip, *p))
                    .fold(f32::NEG_INFINITY, f32::max)
            })
        };
        (
            worst(seen.clip_from_world),
            worst(seen.frustum_clip_from_world),
        )
    }

    /// Points the eyes see, in world space: the corners, edge middles and centre of each eye's
    /// view at 1.5 m (a doorway) out to 1 km.
    fn eye_points(&self, views: [openxr::View; 2]) -> Vec<Vec3> {
        let mut out = Vec::new();
        for v in &views {
            let q = v.pose.orientation;
            let pose = Transform {
                translation: Vec3::new(v.pose.position.x, v.pose.position.y, v.pose.position.z),
                rotation: Quat::from_xyzw(q.x, q.y, q.z, q.w),
                scale: Vec3::ONE,
            };
            let world = self.root.mul_transform(pose);
            let f = v.fov;
            let xs = [
                f.angle_left.tan(),
                (f.angle_left.tan() + f.angle_right.tan()) / 2.0,
                f.angle_right.tan(),
            ];
            let ys = [
                f.angle_down.tan(),
                (f.angle_down.tan() + f.angle_up.tan()) / 2.0,
                f.angle_up.tan(),
            ];
            for x in xs {
                for y in ys {
                    for d in [1.5, 10.0, 100.0, 1000.0] {
                        out.push(world.transform_point(Vec3::new(x, y, -1.0) * d));
                    }
                }
            }
        }
        out
    }

    /// What `trace_cull` reports after `frame(cull, ..)`: the shortfall of the seated cull camera
    /// against the views of `eyes`, in radians per axis.
    fn short(&mut self, eyes: [openxr::View; 2]) -> (f32, f32) {
        let world = self.app.world_mut();
        let (t, p) = world
            .query_filtered::<(&Transform, &Projection), With<WorldCamera>>()
            .single(world)
            .unwrap();
        let Projection::Perspective(p) = p else {
            panic!("not a perspective")
        };
        let fresh = eyes.each_ref().map(|v| {
            let q = v.pose.orientation;
            let pose = Transform {
                translation: Vec3::new(v.pose.position.x, v.pose.position.y, v.pose.position.z),
                rotation: Quat::from_xyzw(q.x, q.y, q.z, q.w),
                scale: Vec3::ONE,
            };
            Eye {
                pose: self.root.mul_transform(pose),
                left: v.fov.angle_left,
                right: v.fov.angle_right,
                up: v.fov.angle_up,
                down: v.fov.angle_down,
            }
        });
        rig::shortfall(&fresh, t.rotation, p.fov, p.aspect_ratio)
    }

    fn frustum_admits(&self, p: Vec3) -> bool {
        let seen = self.app.world().resource::<Seen>();
        seen.frustum.as_ref().is_some_and(|f| {
            f.intersects_sphere(
                &Sphere {
                    center: p.into(),
                    radius: 0.0,
                },
                false,
            )
        })
    }
}

/// How far past the cull volume `p` is: 0 on its edge, positive outside, in half-widths of the
/// wider of the two axes. Behind the camera counts as far outside.
fn overshoot(clip_from_world: &Mat4, p: Vec3) -> f32 {
    let c = *clip_from_world * p.extend(1.0);
    if c.w <= 0.0 {
        return f32::INFINITY;
    }
    (c.x.abs().max(c.y.abs()) - c.w) / c.w
}

/// A triangle wave between `-amp` and `amp`, `step` per frame.
fn wave(frame: usize, amp: f32, step: f32) -> f32 {
    let period = 4.0 * amp / step;
    let t = (frame as f32 % period) / period;
    let tri = if t < 0.25 {
        4.0 * t
    } else if t < 0.75 {
        2.0 - 4.0 * t
    } else {
        4.0 * t - 4.0
    };
    amp * tri
}

fn pose_at(frame: usize, pitch_step: f32, roll_step: f32) -> Pose {
    Pose {
        pitch: wave(frame, 60.0, pitch_step),
        roll: wave(frame, 30.0, roll_step),
    }
}

/// Worst overshoot over a sweep of the head through ±60° of pitch and ±30° of roll at
/// `pitch_step` and `roll_step` degrees a frame, the cull always a frame behind the eyes.
fn sweep(aspect: f32, cant: f32, pitch_step: f32, roll_step: f32) -> (f32, f32) {
    let mut rig = Rig::new(aspect);
    let (mut cull_worst, mut frustum_worst) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for k in 2..400 {
        let at = |k: usize| located(pose_at(k, pitch_step, roll_step), cant);
        let (c, f) = rig.frame(at(k - 1), at(k));
        // The first frames fill the history the `Frustum` lags by.
        if k > 3 {
            cull_worst = cull_worst.max(c);
            frustum_worst = frustum_worst.max(f);
        }
    }
    (cull_worst, frustum_worst)
}

/// Worst overshoot with the head held still at each pitch and roll of a grid.
fn still(aspect: f32, cant: f32) -> f32 {
    let mut rig = Rig::new(aspect);
    let mut worst = f32::NEG_INFINITY;
    for pitch in [-60.0, -30.0, 0.0, 30.0, 60.0] {
        for roll in [-30.0, 0.0, 30.0] {
            let views = located(Pose { pitch, roll }, cant);
            for _ in 0..4 {
                worst = worst.max(rig.frame(views, views).0);
            }
        }
    }
    worst
}

const ASPECTS: [f32; 3] = [16.0 / 9.0, 16.0 / 10.0, 1.0];

#[test]
fn a_still_head_is_covered_at_every_pitch_and_roll() {
    for aspect in ASPECTS {
        for cant in [0.0, 6.0 * DEG] {
            let worst = still(aspect, cant);
            assert!(
                worst <= 0.0,
                "aspect {aspect:.2}, cant {:.0}°: an eye sees {worst:.4} half-widths past the cull",
                cant / DEG
            );
        }
    }
}

#[test]
fn a_moving_head_is_covered_at_every_pitch_and_roll() {
    // 2° a frame is 144°/s at 72 Hz; 1° of roll alongside it.
    for aspect in ASPECTS {
        for cant in [0.0, 6.0 * DEG] {
            let (cull, frustum) = sweep(aspect, cant, 2.0, 1.0);
            assert!(
                cull <= 0.0 && frustum <= 0.0,
                "aspect {aspect:.2}, cant {:.0}°: past the cull by {cull:.4} half-widths, past \
                 the Frustum component's by {frustum:.4}",
                cant / DEG
            );
        }
    }
}

#[test]
fn the_trace_measure_goes_above_zero_when_the_head_outruns_the_margin() {
    for aspect in ASPECTS {
        let mut rig = Rig::new(aspect);
        let still = located(
            Pose {
                pitch: 0.0,
                roll: 0.0,
            },
            0.0,
        );
        for _ in 0..3 {
            rig.frame(still, still);
        }
        // The eyes drew one frame later at 2° of pitch: inside the margin, so under zero.
        rig.frame(still, still);
        let near = located(
            Pose {
                pitch: 2.0,
                roll: 0.0,
            },
            0.0,
        );
        let (x, y) = rig.short(near);
        assert!(
            x < 0.0 && y < 0.0,
            "aspect {aspect:.2}: {x} {y} inside the margin"
        );
        // A 40° snap between the located views and the drawn ones is past any margin.
        let far = located(
            Pose {
                pitch: 40.0,
                roll: 0.0,
            },
            0.0,
        );
        let (_, y) = rig.short(far);
        assert!(
            y > 10.0 * DEG,
            "aspect {aspect:.2}: short y {:.1}° for a 40° snap",
            y / DEG
        );
    }
}
