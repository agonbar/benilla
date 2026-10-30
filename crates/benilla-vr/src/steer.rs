//! Comfort in the headset as systems ([`crate::comfort`] has the math and the settings): the right
//! stick turns the game camera by snap or smooth turn, both stick clicks held recentre, the zoom
//! verbs make the view first or third person, and the vignette closes in with the camera's motion.
//!
//! The turn is the pad's own look session fed by [`LookFeed`]: `benilla-input` keeps the
//! `MouselookStart` and `MouselookStop` and the loading cover, and this writes the one
//! `MouseMotion` a snap turn is (or the frame's worth of a smooth turn) in their place. The right
//! stick's vertical axis does nothing in the headset: the head looks up and down.
//!
//! `WOW_VR_INPUT_TRACE=1` prints one line for each snap turn, recentre, zoom and settings change,
//! and a vignette line when it shows and twice a second while it does.

use bevy::prelude::*;
use bevy_mod_xr::session::XrState;

use benilla_input::{LookFeed, Mode, PadMode, PadState, PadSystems, RoutedTrace, StickChord};
use benilla_ui::script::UiScript;
use benilla_world::view::Viewer;

use crate::comfort::{self, Fade, RecentreHold, Snap, VrConfig, Watch, Zoom};
use crate::{rig, session_running, vignette, GamePose, Views};

/// The settings in force and the recentre offset the player last asked for.
#[derive(Resource)]
pub(crate) struct Comfort {
    pub config: VrConfig,
    /// The root's offset in its own frame ([`rig::recentre_offset`]); identity until a recentre.
    pub recentre: Transform,
}

impl Default for Comfort {
    fn default() -> Self {
        Comfort {
            config: VrConfig::default(),
            recentre: Transform::IDENTITY,
        }
    }
}

/// Degrees a second the player is smooth-turning, for the vignette.
#[derive(Resource, Default)]
pub(crate) struct Turning(pub f32);

#[derive(Resource, Default)]
struct Watching(Watch);

pub struct ComfortPlugin;

impl Plugin for ComfortPlugin {
    fn build(&self, app: &mut App) {
        let dir = benilla_app::config_dir();
        let (config, problems) = comfort::load(dir.as_deref());
        for p in problems {
            warn!("vr: {p}");
        }
        info!("vr: comfort {}", config.describe());
        app.insert_resource(Comfort {
            config,
            recentre: Transform::IDENTITY,
        })
        .insert_resource(Watching(Watch::new(dir.as_deref())))
        .init_resource::<Turning>()
        .init_resource::<Viewer>()
        .init_resource::<PadState>()
        .init_resource::<PadMode>()
        .init_resource::<LookFeed>()
        .init_resource::<StickChord>()
        .init_resource::<RoutedTrace>()
        .add_systems(
            PreUpdate,
            (
                steer
                    .run_if(session_running)
                    .after(PadSystems::Read)
                    .before(PadSystems::Look),
                hand_back
                    .run_if(not(session_running))
                    .before(PadSystems::Look),
            ),
        )
        .add_systems(
            Update,
            (
                reload,
                apply_zoom,
                drive_vignette
                    .after(crate::seat_head)
                    .run_if(session_running),
            ),
        );
    }
}

fn traced(trace: &Option<Res<RoutedTrace>>) -> bool {
    trace.as_ref().is_some_and(|t| t.0)
}

/// The right stick as the camera's turn, and the chord of both stick clicks as the recentre.
#[allow(clippy::too_many_arguments)]
fn steer(
    time: Res<Time<Real>>,
    state: Res<PadState>,
    mode: Res<PadMode>,
    views: Res<Views>,
    trace: Option<Res<RoutedTrace>>,
    mut comfort: ResMut<Comfort>,
    mut feed: ResMut<LookFeed>,
    mut chord: ResMut<StickChord>,
    mut turning: ResMut<Turning>,
    mut snap: Local<Snap>,
    mut hold: Local<RecentreHold>,
) {
    let dt = time.delta_secs();
    if !feed.external {
        feed.external = true;
    }
    if !chord.0 {
        chord.0 = true;
    }
    // Cursor mode scrolls with the stick; the latch keeps its state, so a stick still pushed when
    // gameplay comes back does not turn.
    let turned = if mode.0 == Mode::Gameplay {
        comfort::turn(&comfort.config, &mut snap, state.right.x, dt)
    } else {
        comfort::Turned::default()
    };
    feed.motion = turned.motion;
    turning.0 = turned.rate;
    if turned.snapped != 0.0 && traced(&trace) {
        info!("{}", snap_line(turned.snapped, comfort.config.snap_degrees));
    }

    let both = state.is_down("PADLSTICK") && state.is_down("PADRSTICK");
    if hold.step(dt, both) {
        if let Some(eyes) = views.0 {
            let head = rig::head_centre(&eyes);
            comfort.recentre = rig::recentre_offset(&head);
            if traced(&trace) {
                info!("{}", recentre_line(&head));
            }
        }
    }
}

/// The pad's own camera again once the session is not running.
fn hand_back(mut feed: ResMut<LookFeed>, mut chord: ResMut<StickChord>) {
    if feed.external {
        feed.external = false;
        feed.motion = Vec2::ZERO;
    }
    if chord.0 {
        chord.0 = false;
    }
}

/// `vr.toml` re-read when it changes.
fn reload(
    time: Res<Time<Real>>,
    trace: Option<Res<RoutedTrace>>,
    mut watching: ResMut<Watching>,
    mut comfort: ResMut<Comfort>,
) {
    let Some((config, problems)) = watching.0.step(time.delta_secs()) else {
        return;
    };
    for p in problems {
        warn!("vr: {p}");
    }
    if config != comfort.config {
        comfort.config = config;
        if traced(&trace) {
            info!("vr comfort: reloaded vr.toml: {}", config.describe());
        }
    }
}

/// The zoom that makes the view first or third person, once the avatar is in the world and again
/// whenever the setting changes. The verbs run as a pad's `MouselookStart` does: through the UI's
/// Lua, which the bindings read.
fn apply_zoom(
    state: Option<Res<XrState>>,
    viewer: Res<Viewer>,
    comfort: Res<Comfort>,
    trace: Option<Res<RoutedTrace>>,
    script: Option<NonSend<UiScript>>,
    mut zoom: Local<Zoom>,
) {
    if !matches!(state.as_deref(), Some(XrState::Running)) || viewer.at.is_none() {
        zoom.reset();
        return;
    }
    let Some(script) = script else {
        return;
    };
    let Some(chunk) = zoom.step(&comfort.config) else {
        return;
    };
    match script.run(&chunk) {
        Ok(()) => {
            if traced(&trace) {
                info!("vr comfort: zoom {chunk}");
            }
        }
        Err(e) => warn!("vr: {chunk}: {e}"),
    }
}

/// Moves the ring: how far the camera moved and how fast the player is turning set the level, the
/// level and the strength set the opening.
#[allow(clippy::too_many_arguments)]
fn drive_vignette(
    time: Res<Time<Real>>,
    comfort: Res<Comfort>,
    pose: Res<GamePose>,
    turning: Res<Turning>,
    trace: Option<Res<RoutedTrace>>,
    mut rings: Query<(&mut Transform, &mut Visibility), With<vignette::Ring>>,
    mut fade: Local<Fade>,
    mut last: Local<Option<Vec3>>,
    mut since: Local<f32>,
) {
    let dt = time.delta_secs();
    // The game's own pose is the camera's, free of the head and of a recentre.
    let here = pose.0.map(|(t, _)| t.translation);
    let speed = here.map_or(0.0, |h| comfort::speed(*last, h, dt));
    *last = here;
    let target = if comfort.config.vignette {
        comfort::vignette_target(speed, turning.0)
    } else {
        0.0
    };
    let was = fade.level > SHOWN;
    let level = fade.step(target, dt);
    let shown = level > SHOWN;
    let aperture = comfort::aperture(comfort.config.vignette_strength, level);
    for (mut transform, mut visibility) in &mut rings {
        let want = if shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != want {
            *visibility = want;
        }
        if shown {
            transform.scale = Vec3::splat(vignette::scale_for(aperture));
        }
    }
    *since += dt;
    if traced(&trace) && shown && (!was || *since >= 0.5) {
        *since = 0.0;
        info!("{}", vignette_line(level, speed, turning.0, aperture));
    }
}

/// The level under which the ring is hidden: nothing of it shows.
const SHOWN: f32 = 0.01;

/// The trace line for a snap turn.
pub fn snap_line(direction: f32, degrees: f32) -> String {
    format!(
        "vr comfort: snap turn {} {degrees:.0} deg",
        if direction > 0.0 { "right" } else { "left" }
    )
}

/// The trace line for a recentre: where the head was, in tracking space, when it was put back.
pub fn recentre_line(head: &Transform) -> String {
    let (yaw, ..) = head.rotation.to_euler(EulerRot::YXZ);
    format!(
        "vr comfort: recentre head at ({:.2}, {:.2}, {:.2}) yaw {:.0} deg",
        head.translation.x,
        head.translation.y,
        head.translation.z,
        yaw.to_degrees()
    )
}

/// The trace line for the vignette.
pub fn vignette_line(level: f32, speed: f32, turn_rate: f32, aperture: f32) -> String {
    format!(
        "vr comfort: vignette {level:.2} (camera {speed:.1} yd/s, turn {turn_rate:.0} deg/s, \
         opening {:.0} deg)",
        aperture.to_degrees()
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::ecs::system::RunSystemOnce;

    use super::*;
    use crate::comfort::TurnStyle;
    use crate::rig::Eye;
    use crate::touch::{self, Changes, Touch};
    use benilla_input::{PadConfig, PadDevice};

    /// The pad state the Touch reading `t` makes, through the same events the headset sends.
    fn pad(t: Touch) -> PadState {
        let entity = Entity::from_raw_u32(7).unwrap();
        let mut device = PadDevice::new(PadConfig::default());
        device.handle(&touch::connection(entity, true), 0.0);
        for (input, value) in Changes::default().step(&t) {
            device.handle(&touch::raw(entity, input, value), 0.1);
        }
        device.state().clone()
    }

    fn flick(x: f32) -> PadState {
        pad(Touch {
            right_stick: Vec2::new(x, 0.0),
            ..default()
        })
    }

    fn both_clicks() -> PadState {
        pad(Touch {
            left_stick_click: true,
            right_stick_click: true,
            ..default()
        })
    }

    fn eye(x: f32, y: f32, z: f32) -> Eye {
        Eye {
            pose: Transform::from_xyz(x, y, z),
            left: -0.9,
            right: 0.9,
            up: 0.8,
            down: -0.8,
        }
    }

    fn app() -> App {
        let mut app = App::new();
        app.insert_resource(Time::<Real>::default())
            .insert_resource(Comfort::default())
            .insert_resource(PadMode(Mode::Gameplay))
            .insert_resource(Views(Some([eye(-0.03, 0.0, 0.0), eye(0.03, 0.0, 0.0)])))
            .init_resource::<PadState>()
            .init_resource::<LookFeed>()
            .init_resource::<StickChord>()
            .init_resource::<Turning>()
            .init_resource::<RoutedTrace>()
            .add_systems(Update, steer);
        app
    }

    fn frame(app: &mut App, dt: f32) {
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_secs_f32(dt));
        app.update();
    }

    fn set(app: &mut App, state: PadState) {
        *app.world_mut().resource_mut::<PadState>() = state;
    }

    #[test]
    fn a_flick_snaps_the_angle_once_and_takes_the_camera_from_the_stick() {
        let mut app = app();
        set(&mut app, flick(1.0));
        frame(&mut app, 0.016);
        let feed = *app.world().resource::<LookFeed>();
        assert!(feed.external, "the pad's own stick look steps aside");
        let want = 30.0f32.to_radians() / 0.003;
        assert!((feed.motion.x - want).abs() < 1e-2, "{:?}", feed.motion);
        assert_eq!(feed.motion.y, 0.0, "the head looks up and down");
        assert!(app.world().resource::<StickChord>().0);

        frame(&mut app, 0.016);
        assert_eq!(
            app.world().resource::<LookFeed>().motion,
            Vec2::ZERO,
            "held"
        );
        set(&mut app, flick(0.0));
        frame(&mut app, 0.016);
        set(&mut app, flick(-1.0));
        frame(&mut app, 0.016);
        assert!(
            app.world().resource::<LookFeed>().motion.x < 0.0,
            "and left"
        );
    }

    #[test]
    fn the_snap_angle_comes_from_the_settings() {
        let mut app = app();
        app.world_mut()
            .resource_mut::<Comfort>()
            .config
            .snap_degrees = 45.0;
        set(&mut app, flick(1.0));
        frame(&mut app, 0.016);
        let want = 45.0f32.to_radians() / 0.003;
        let got = app.world().resource::<LookFeed>().motion.x;
        assert!((got - want).abs() < 1e-2, "{got}");
    }

    #[test]
    fn a_smooth_turn_follows_the_stick_and_shows_its_rate_to_the_vignette() {
        let mut app = app();
        {
            let mut c = app.world_mut().resource_mut::<Comfort>();
            c.config.turn = TurnStyle::Smooth;
            c.config.smooth_degrees_per_second = 100.0;
        }
        set(&mut app, flick(1.0));
        frame(&mut app, 0.1);
        let got = app.world().resource::<LookFeed>().motion.x;
        // 10° in 0.1 s at full stick.
        assert!((got - 10.0f32.to_radians() / 0.003).abs() < 1e-2, "{got}");
        assert_eq!(app.world().resource::<Turning>().0, 100.0);
        frame(&mut app, 0.1);
        assert!(
            (app.world().resource::<LookFeed>().motion.x - got).abs() < 1e-2,
            "held"
        );
        set(&mut app, flick(0.0));
        frame(&mut app, 0.1);
        assert_eq!(app.world().resource::<Turning>().0, 0.0);
    }

    #[test]
    fn the_cursor_mode_stick_scrolls_and_turns_nothing() {
        let mut app = app();
        app.world_mut().insert_resource(PadMode(Mode::Cursor));
        set(&mut app, flick(1.0));
        frame(&mut app, 0.016);
        assert_eq!(app.world().resource::<LookFeed>().motion, Vec2::ZERO);
        // Back in gameplay mode with the stick still over, it has not fired yet: it fires now.
        app.world_mut().insert_resource(PadMode(Mode::Gameplay));
        frame(&mut app, 0.016);
        assert!(app.world().resource::<LookFeed>().motion.x > 0.0);
    }

    #[test]
    fn both_clicks_held_for_a_second_recentre_on_the_head_where_it_is() {
        let mut app = app();
        // The head 0.4 m right, 0.3 forward and 0.5 up of where the session began.
        let eyes = [eye(0.37, 0.5, -0.3), eye(0.43, 0.5, -0.3)];
        app.world_mut().insert_resource(Views(Some(eyes)));
        set(&mut app, both_clicks());
        for _ in 0..8 {
            frame(&mut app, 0.1);
        }
        assert_eq!(
            app.world().resource::<Comfort>().recentre,
            Transform::IDENTITY
        );
        for _ in 0..4 {
            frame(&mut app, 0.1);
        }
        let offset = app.world().resource::<Comfort>().recentre;
        let head = rig::head_centre(&eyes);
        assert_eq!(offset, rig::recentre_offset(&head));
        // With the offset the head is at the root.
        let at = offset.mul_transform(head);
        assert!(at.translation.length() < 1e-5, "{}", at.translation);

        // Still held, the head moved: it does not fire again until let go and held again.
        app.world_mut()
            .insert_resource(Views(Some([eye(1.0, 0.5, 0.0), eye(1.06, 0.5, 0.0)])));
        for _ in 0..15 {
            frame(&mut app, 0.1);
        }
        assert_eq!(app.world().resource::<Comfort>().recentre, offset);
        set(&mut app, flick(0.0));
        frame(&mut app, 0.1);
        set(&mut app, both_clicks());
        for _ in 0..12 {
            frame(&mut app, 0.1);
        }
        assert_ne!(app.world().resource::<Comfort>().recentre, offset);
    }

    #[test]
    fn a_hand_back_returns_the_camera_and_the_clicks_to_the_pad() {
        let mut app = app();
        set(&mut app, flick(1.0));
        frame(&mut app, 0.016);
        assert!(app.world().resource::<LookFeed>().external);
        app.world_mut().resource_mut::<LookFeed>().motion = Vec2::X;
        app.world_mut().run_system_once(hand_back).unwrap();
        assert!(!app.world().resource::<LookFeed>().external);
        assert_eq!(app.world().resource::<LookFeed>().motion, Vec2::ZERO);
        assert!(!app.world().resource::<StickChord>().0);
    }

    fn vignette_app() -> (App, Entity) {
        let mut app = App::new();
        app.insert_resource(Time::<Real>::default())
            .insert_resource(Comfort::default())
            .init_resource::<GamePose>()
            .init_resource::<Turning>()
            .init_resource::<RoutedTrace>()
            .add_systems(Update, drive_vignette);
        let ring = app
            .world_mut()
            .spawn((vignette::Ring, Transform::default(), Visibility::Hidden))
            .id();
        (app, ring)
    }

    fn ring(app: &App, ring: Entity) -> (Visibility, f32) {
        (
            *app.world().get::<Visibility>(ring).unwrap(),
            app.world().get::<Transform>(ring).unwrap().scale.x,
        )
    }

    /// One frame with the game camera at `x` yards.
    fn camera_at(app: &mut App, x: f32, dt: f32) {
        app.world_mut().resource_mut::<GamePose>().0 =
            Some((Transform::from_xyz(x, 0.0, 0.0), 0.8));
        frame(app, dt);
    }

    #[test]
    fn the_ring_shows_while_the_camera_runs_and_closes_in_and_hides_when_it_stops() {
        let (mut app, ring_id) = vignette_app();
        for i in 0..5 {
            camera_at(&mut app, 0.0, 0.05 * (i as f32 + 1.0).min(1.0));
        }
        assert_eq!(ring(&app, ring_id).0, Visibility::Hidden, "standing still");

        // A run: 7 yd/s, 0.7 yd every 0.1 s. A fifth of a second takes it to full.
        let mut x = 0.0;
        let mut scales = Vec::new();
        for _ in 0..4 {
            x += 0.7;
            camera_at(&mut app, x, 0.1);
            scales.push(ring(&app, ring_id));
        }
        assert_eq!(scales[0].0, Visibility::Inherited);
        assert!(scales[3].1 < scales[0].1, "it closes in: {scales:?}");
        let closed = comfort::aperture(0.5, 1.0);
        assert!((scales[3].1 - vignette::scale_for(closed)).abs() < 1e-4);

        // Stopped: out over 0.4 s.
        for _ in 0..3 {
            camera_at(&mut app, x, 0.1);
        }
        assert_eq!(ring(&app, ring_id).0, Visibility::Inherited, "still fading");
        for _ in 0..2 {
            camera_at(&mut app, x, 0.1);
        }
        assert_eq!(ring(&app, ring_id).0, Visibility::Hidden);
    }

    #[test]
    fn the_vignette_off_never_shows_and_a_smooth_turn_shows_it_standing_still() {
        let (mut app, ring_id) = vignette_app();
        app.world_mut().resource_mut::<Comfort>().config.vignette = false;
        let mut x = 0.0;
        for _ in 0..5 {
            x += 0.7;
            camera_at(&mut app, x, 0.1);
        }
        assert_eq!(ring(&app, ring_id).0, Visibility::Hidden);

        app.world_mut().resource_mut::<Comfort>().config.vignette = true;
        app.world_mut().resource_mut::<Turning>().0 = 90.0;
        for _ in 0..3 {
            camera_at(&mut app, x, 0.1);
        }
        assert_eq!(ring(&app, ring_id).0, Visibility::Inherited);
    }

    #[test]
    fn a_stronger_vignette_closes_further() {
        let scale_at = |strength: f32| {
            let (mut app, ring_id) = vignette_app();
            app.world_mut()
                .resource_mut::<Comfort>()
                .config
                .vignette_strength = strength;
            app.world_mut().resource_mut::<Turning>().0 = 90.0;
            for _ in 0..4 {
                camera_at(&mut app, 0.0, 0.1);
            }
            ring(&app, ring_id).1
        };
        assert!(scale_at(1.0) < scale_at(0.0));
    }

    #[test]
    fn the_trace_lines_name_what_happened() {
        assert_eq!(snap_line(1.0, 30.0), "vr comfort: snap turn right 30 deg");
        assert_eq!(snap_line(-1.0, 45.0), "vr comfort: snap turn left 45 deg");
        let head = Transform::from_xyz(0.1, 0.5, -0.25)
            .with_rotation(Quat::from_rotation_y(20.0f32.to_radians()));
        assert_eq!(
            recentre_line(&head),
            "vr comfort: recentre head at (0.10, 0.50, -0.25) yaw 20 deg"
        );
        assert_eq!(
            vignette_line(0.5, 3.5, 0.0, 70.0f32.to_radians()),
            "vr comfort: vignette 0.50 (camera 3.5 yd/s, turn 0 deg/s, opening 70 deg)"
        );
    }
}
