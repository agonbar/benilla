//! Stereo VR for benilla through OpenXR: the 3D world in the headset, with head tracking, from a
//! plugin on top of the client ([`benilla_app::run_with_group`]); 1.12.1 has no VR, so none of it
//! lives in benilla.
//!
//! The game camera (`WorldCamera`) keeps doing everything it does: follow, collision, zoom, mouse
//! look, the sound listener. The [`XrTrackingRoot`] takes its position and yaw every frame, two
//! eye cameras ride the root, and the head supplies pitch and roll. While a session runs the world
//! camera draws nothing; the eyes draw through the same FFX path a camera no UI camera claims takes
//! (`benilla_world::final_pass`), so no upstream render code changes.
//!
//! Without OpenXR (no loader, runtime or headset) [`run`] is the flat client.

mod panel;
mod rig;

use bevy::app::PluginGroupBuilder;
use bevy::camera::{CameraOutputMode, ManualTextureViewHandle, RenderTarget};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::prelude::*;
use bevy::render::view::Hdr;
use bevy::window::{PresentMode, PrimaryWindow};
use bevy::winit::WinitSettings;

use benilla_app::{AppExit, BuildId};
use benilla_world::ffx_glow::FfxGlow;
use benilla_world::schedule::WorldStage;
use benilla_world::view::{CameraPoseSet, MsaaSetting, WorldCamera, NEARCLIP_DEFAULT};
use bevy_mod_openxr::add_xr_plugins;
use bevy_mod_openxr::features::handtracking::HandTrackingPlugin;
use bevy_mod_openxr::helper_traits::ToTransform;
use bevy_mod_openxr::init::OxrInitPlugin;
use bevy_mod_openxr::reference_space::OxrReferenceSpacePlugin;
use bevy_mod_openxr::render::{update_views, OxrRenderPlugin, XR_TEXTURE_INDEX};
use bevy_mod_openxr::resources::{OxrFrameState, OxrViews};
use bevy_mod_xr::camera::{XrCamera, XrProjection};
use bevy_mod_xr::session::{XrSessionCreated, XrState, XrTrackingRoot};

use openxr::ReferenceSpaceType;

use rig::Eye;

/// The client with stereo VR: [`benilla_app::run_with_group`] with OpenXR around the default
/// plugin group and the rig on top. Starts flat when OpenXR is unavailable.
pub fn run(build: BuildId) -> AppExit {
    benilla_app::run_with_group(build, openxr_group, |app| {
        if matches!(
            app.world().get_resource::<XrState>(),
            Some(XrState::Unavailable) | None
        ) {
            // `OxrInitPlugin` has logged the OpenXR error that says which of the three it was.
            info!("vr: OpenXR is unavailable (no loader, runtime or headset), running flat");
        } else {
            app.add_plugins(VrPlugin);
        }
    })
}

/// The default plugin group with OpenXR in front of the renderer. A seated (`LOCAL`) reference
/// space and no cameras of its own. No optional OpenXR extension is requested, so a runtime that
/// lacks hand tracking starts as well as one that has it; hands are not in this piece.
fn openxr_group(group: PluginGroupBuilder) -> PluginGroupBuilder {
    add_xr_plugins(group)
        .disable::<HandTrackingPlugin>()
        .set(OxrInitPlugin {
            exts: default(),
            ..default()
        })
        .set(OxrReferenceSpacePlugin {
            default_primary_ref_space: ReferenceSpaceType::LOCAL,
        })
        .set(OxrRenderPlugin {
            spawn_cameras: false,
            ..default()
        })
}

/// The rig, added once OpenXR is up.
pub struct VrPlugin;

impl Plugin for VrPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Views>()
            .init_resource::<GamePose>()
            // Camera shake is off in VR: a view that moves with the head must not be shaken.
            .insert_resource(benilla_app::NoCameraShake)
            .add_plugins(panel::PanelPlugin)
            .add_systems(XrSessionCreated, spawn_eyes)
            // Before the controller: the game's own pose goes back, so it never reads the head's.
            .add_systems(First, restore_game_pose)
            .add_systems(
                Update,
                (
                    read_views,
                    seat_head
                        .after(read_views)
                        .after(WorldStage::Input)
                        .before(CameraPoseSet)
                        .run_if(session_running),
                    pace_window,
                ),
            )
            .add_systems(
                PostUpdate,
                (sync_eyes, hand_over_to_eyes.after(sync_eyes))
                    .before(update_views)
                    .run_if(session_running),
            );
    }
}

fn session_running(state: Option<Res<XrState>>) -> bool {
    matches!(state.as_deref(), Some(XrState::Running))
}

/// The eyes as the runtime last located them, in tracking space.
#[derive(Resource, Default)]
struct Views(Option<[Eye; 2]>);

/// The game camera's own pose and vertical field of view while the head's are seated on it.
#[derive(Resource, Default)]
struct GamePose(Option<(Transform, f32)>);

/// Two eye cameras on the headset's swapchain, each with what the world camera has
/// (`player/setup.rs`) except the `WorldCamera` marker: some 79 queries assume exactly one.
fn spawn_eyes(mut commands: Commands, msaa: Res<MsaaSetting>) {
    for index in 0..2 {
        commands.spawn((
            XrCamera(index),
            Projection::custom(XrProjection {
                near: NEARCLIP_DEFAULT,
                ..default()
            }),
            RenderTarget::TextureView(ManualTextureViewHandle(XR_TEXTURE_INDEX + index)),
            msaa.level(),
            Hdr,
            Tonemapping::None,
            FfxGlow::WORLD,
            Camera {
                output_mode: CameraOutputMode::Skip,
                ..default()
            },
            bevy::light::cluster::ClusterConfig::None,
        ));
    }
}

fn read_views(openxr: Option<Res<OxrViews>>, mut views: ResMut<Views>) {
    let Some(openxr) = openxr else { return };
    let eye = |v: &openxr::View| Eye {
        pose: v.pose.to_transform(),
        left: v.fov.angle_left,
        right: v.fov.angle_right,
        up: v.fov.angle_up,
        down: v.fov.angle_down,
    };
    views.0 = match openxr.as_slice() {
        [l, r, ..] => Some([eye(l), eye(r)]),
        _ => None,
    };
}

/// Puts the root on the game camera and the head's pose and field of view on the game camera, for
/// the CPU culls (portals, draw election, drift) that look through `WorldCamera`. The game's own
/// pose is kept for [`restore_game_pose`]. The controller reads nothing of its camera's `Transform`
/// back but the free-fly translation, which is why the restore must be exact.
fn seat_head(
    views: Res<Views>,
    mut game: ResMut<GamePose>,
    mut root: Query<&mut Transform, (With<XrTrackingRoot>, Without<WorldCamera>)>,
    mut camera: Query<(&mut Transform, &mut Projection), With<WorldCamera>>,
) {
    let (Some(eyes), Ok((mut transform, mut projection)), Ok(mut root)) =
        (views.0, camera.single_mut(), root.single_mut())
    else {
        return;
    };
    let Projection::Perspective(perspective) = &mut *projection else {
        return;
    };
    // A pose still held means the restore did not run; keep the first, which is the game's.
    let (game_pose, game_fov) = *game.0.get_or_insert((*transform, perspective.fov));

    *root = rig::root_pose(&game_pose);
    *transform = root.mul_transform(rig::head_centre(&eyes));
    perspective.fov = rig::covering_fov_y(&eyes, perspective.aspect_ratio).max(game_fov);
}

/// Gives the game camera back its own pose, before anything of the frame reads it.
fn restore_game_pose(
    mut game: ResMut<GamePose>,
    mut camera: Query<(&mut Transform, &mut Projection), With<WorldCamera>>,
) {
    let Some((pose, fov)) = game.0.take() else {
        return;
    };
    for (mut transform, mut projection) in &mut camera {
        *transform = pose;
        if let Projection::Perspective(p) = &mut *projection {
            p.fov = fov;
        }
    }
}

/// The eyes' near plane follows the world camera's, which `view::stamp_near_clip` keeps at the
/// live `nearclip`.
fn sync_eyes(
    camera: Query<&Projection, (With<WorldCamera>, Without<XrCamera>)>,
    mut eyes: Query<&mut Projection, With<XrCamera>>,
) {
    let Ok(Projection::Perspective(world)) = camera.single() else {
        return;
    };
    for mut eye in &mut eyes {
        if let Projection::Custom(custom) = &mut *eye {
            if let Some(xr) = custom.get_mut::<XrProjection>() {
                if xr.near != world.near {
                    xr.near = world.near;
                }
            }
        }
    }
}

/// The world camera stops drawing while a session runs, and the eyes draw when it would have and
/// the headset wants a frame. `gate_world_camera` has written the world camera's `is_active` this
/// frame (it rewrites it whenever this system last turned it off), so it is read as the gate.
fn hand_over_to_eyes(
    frame: Option<Res<OxrFrameState>>,
    mut world: Query<&mut Camera, (With<WorldCamera>, Without<XrCamera>)>,
    mut eyes: Query<&mut Camera, (With<XrCamera>, Without<panel::PanelCamera>)>,
) {
    let Ok(mut world) = world.single_mut() else {
        return;
    };
    let should_render = frame.is_none_or(|f| f.should_render);
    let active = world.is_active && should_render;
    for mut eye in &mut eyes {
        if eye.is_active != active {
            eye.is_active = active;
        }
    }
    if world.is_active {
        world.is_active = false;
    }
}

/// While a session runs the desktop window must not pace the headset: no vsync, and a frame per
/// loop turn whether or not the window is focused.
fn pace_window(
    state: Option<Res<XrState>>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
    winit: Option<ResMut<WinitSettings>>,
    mut saved: Local<Option<(PresentMode, WinitSettings)>>,
) {
    let running = matches!(state.as_deref(), Some(XrState::Running));
    let (Ok(mut window), Some(mut winit)) = (window.single_mut(), winit) else {
        return;
    };
    match (running, saved.take()) {
        (true, None) => {
            *saved = Some((window.present_mode, winit.clone()));
            window.present_mode = PresentMode::AutoNoVsync;
            *winit = WinitSettings::continuous();
        }
        (true, held) => *saved = held,
        (false, Some((mode, settings))) => {
            window.present_mode = mode;
            *winit = settings;
        }
        (false, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    fn eye(x: f32) -> Eye {
        Eye {
            pose: Transform::from_xyz(x, 1.6, 0.0),
            left: -0.9,
            right: 0.9,
            up: 0.8,
            down: -0.8,
        }
    }

    /// A world camera pitched and rolled, a tracking root, and eyes 6 cm apart at head height.
    fn world() -> (App, Entity, Entity, Transform) {
        let mut app = App::new();
        app.insert_resource(XrState::Running)
            .init_resource::<GamePose>()
            .insert_resource(Views(Some([eye(-0.03), eye(0.03)])));
        let game = Transform::from_xyz(100.0, 20.0, -50.0).with_rotation(Quat::from_euler(
            EulerRot::YXZ,
            1.2,
            -0.5,
            0.1,
        ));
        let camera = app
            .world_mut()
            .spawn((
                WorldCamera,
                game,
                Projection::Perspective(PerspectiveProjection {
                    fov: 0.7,
                    aspect_ratio: 16.0 / 9.0,
                    ..default()
                }),
            ))
            .id();
        let root = app.world_mut().spawn(XrTrackingRoot).id();
        (app, camera, root, game)
    }

    fn fov(app: &App, camera: Entity) -> f32 {
        match app.world().get::<Projection>(camera) {
            Some(Projection::Perspective(p)) => p.fov,
            _ => panic!("no perspective projection"),
        }
    }

    #[test]
    fn the_head_is_seated_on_the_game_camera_and_the_root_follows_its_yaw() {
        let (mut app, camera, root, game) = world();
        app.world_mut().run_system_once(seat_head).unwrap();

        let root_t = *app.world().get::<Transform>(root).unwrap();
        assert_eq!(root_t, rig::root_pose(&game));
        let head = *app.world().get::<Transform>(camera).unwrap();
        let want = root_t.mul_transform(rig::head_centre(&[eye(-0.03), eye(0.03)]));
        assert!((head.translation - want.translation).length() < 1e-5);
        assert!(head.rotation.abs_diff_eq(want.rotation, 1e-5));
        assert!(
            fov(&app, camera) > 0.7,
            "the culling camera covers both eyes"
        );
    }

    #[test]
    fn the_game_pose_comes_back_exactly() {
        let (mut app, camera, _, game) = world();
        app.world_mut().run_system_once(seat_head).unwrap();
        assert_ne!(*app.world().get::<Transform>(camera).unwrap(), game);

        app.world_mut().run_system_once(restore_game_pose).unwrap();
        assert_eq!(*app.world().get::<Transform>(camera).unwrap(), game);
        assert_eq!(fov(&app, camera), 0.7);
        assert!(app.world().resource::<GamePose>().0.is_none());
    }

    #[test]
    fn a_missed_restore_keeps_the_first_pose_as_the_game_s() {
        let (mut app, camera, _, game) = world();
        app.world_mut().run_system_once(seat_head).unwrap();
        app.world_mut().run_system_once(seat_head).unwrap();

        app.world_mut().run_system_once(restore_game_pose).unwrap();
        assert_eq!(*app.world().get::<Transform>(camera).unwrap(), game);
        assert_eq!(fov(&app, camera), 0.7);
    }

    #[test]
    fn nothing_is_seated_before_the_runtime_has_located_the_eyes() {
        let (mut app, camera, _, game) = world();
        app.insert_resource(Views(None));
        app.world_mut().run_system_once(seat_head).unwrap();
        assert_eq!(*app.world().get::<Transform>(camera).unwrap(), game);
        assert!(app.world().resource::<GamePose>().0.is_none());
    }

    #[test]
    fn the_world_camera_hands_over_to_the_eyes() {
        let mut app = App::new();
        let world_cam = app.world_mut().spawn((WorldCamera, Camera::default())).id();
        let eye = app
            .world_mut()
            .spawn((
                XrCamera(0),
                Camera {
                    is_active: false,
                    ..default()
                },
            ))
            .id();
        app.world_mut().run_system_once(hand_over_to_eyes).unwrap();
        assert!(!app.world().get::<Camera>(world_cam).unwrap().is_active);
        assert!(app.world().get::<Camera>(eye).unwrap().is_active);

        // The game's gate (glue screens) turns the world camera off: the eye stays dark.
        app.world_mut()
            .get_mut::<Camera>(world_cam)
            .unwrap()
            .is_active = false;
        app.world_mut().run_system_once(hand_over_to_eyes).unwrap();
        assert!(!app.world().get::<Camera>(eye).unwrap().is_active);
    }
}
