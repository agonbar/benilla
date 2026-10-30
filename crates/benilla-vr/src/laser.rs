//! The laser: a ray from the right controller's `aim` pose onto the UI panel, and what it does to
//! the pointer. A hit is the UI's cursor ([`benilla_app::pointer::set_vr`]) and sets
//! [`benilla_input::Laser`], which puts the pad in cursor mode with the right trigger as a click
//! and the right stick as the wheel. Off the panel the laser lets go of the pointer.
//!
//! Everything is in the tracking root's space: the hand's pose is located there and so is the
//! panel ([`panel::quad_in_root`]), so the ray needs no `GlobalTransform` and has no frame of lag
//! behind the character moving. The world ray is a later piece; the aim carries none.

use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::math::Affine3A;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy_mod_openxr::spaces::OxrSpaceLocationFlags;
use bevy_mod_xr::spaces::XrSpaceSyncSet;

use benilla_app::pointer::{self, Aim};
use benilla_input::{Laser, PadSystems};

use crate::hands::{self, Actions};
use crate::panel::{self, PanelScreen, PANEL_RENDER_LAYER};
use crate::session_running;

/// How long the beam is while it hits nothing, in the root's units.
pub const MISS_LENGTH: f32 = 0.4;
/// The beam's width.
const BEAM_WIDTH: f32 = 0.003;

/// Where a ray meets the panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelHit {
    /// The window cursor in logical pixels (origin top-left, y down), what
    /// `Window::cursor_position` answers for the same spot. Not clamped.
    pub cursor: Vec2,
    /// The hit on the quad, centred, in the quad's unit square (y up).
    pub local: Vec2,
    /// Along the ray, in the units of the ray's space.
    pub distance: f32,
}

/// Where `ray` meets the front of the unit quad `quad` (a transform of the unit square in the XY
/// plane facing +Z; any pose and scale), as the cursor of a window of `window` logical pixels.
/// The inverse of [`panel::marker_position`]. `None` for a miss: past the edge, behind the ray's
/// origin, parallel, or from the back.
pub fn ray_to_panel(ray: Ray3d, quad: &Affine3A, window: Vec2) -> Option<PanelHit> {
    let inverse = quad.inverse();
    let origin = inverse.transform_point3(ray.origin);
    let dir = inverse.transform_vector3(*ray.direction);
    // The front faces +Z, so a ray that hits it travels toward -Z.
    if dir.z >= 0.0 || dir.z.is_nan() {
        return None;
    }
    // The ray's parameter is the same in the quad's space, and the direction is a unit vector in
    // the ray's, so it is also the distance.
    let t = -origin.z / dir.z;
    if t <= 0.0 || t.is_nan() {
        return None;
    }
    let p = origin + dir * t;
    if p.x.abs() > 0.5 || p.y.abs() > 0.5 {
        return None;
    }
    Some(PanelHit {
        cursor: Vec2::new((p.x + 0.5) * window.x, (0.5 - p.y) * window.y),
        local: p.truncate(),
        distance: t,
    })
}

/// What the laser did this frame.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq)]
pub struct Aimed {
    /// The controller is located.
    pub tracked: bool,
    pub hit: Option<PanelHit>,
}

impl Aimed {
    /// The beam's length: to the hit, else a stub.
    pub fn length(&self) -> f32 {
        self.hit.map_or(MISS_LENGTH, |h| h.distance)
    }
}

/// The right hand's `aim` pose, a child of the tracking root.
#[derive(Component)]
pub(crate) struct AimHand;

/// The beam, a child of [`AimHand`].
#[derive(Component)]
struct Beam;

pub struct LaserPlugin;

impl Plugin for LaserPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Aimed>()
            .init_resource::<Laser>()
            .add_systems(
                PreUpdate,
                (
                    spawn_hand.run_if(session_running).run_if(hands::ready),
                    drop_hand.run_if(not_running),
                    aim_laser.after(XrSpaceSyncSet).before(PadSystems::Read),
                )
                    .chain(),
            )
            .add_systems(Update, shape_beam);
    }
}

fn not_running(state: Option<Res<bevy_mod_xr::session::XrState>>) -> bool {
    !matches!(
        state.as_deref(),
        Some(bevy_mod_xr::session::XrState::Running)
    )
}

/// The hand and its beam, once the actions are attached and the session runs.
fn spawn_hand(
    mut commands: Commands,
    actions: Res<Actions>,
    session: Res<bevy_mod_openxr::session::OxrSession>,
    hand: Query<(), With<AimHand>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !hand.is_empty() {
        return;
    }
    let Some(space) = actions.aim_space(&session) else {
        return;
    };
    let beam = (
        Beam,
        Mesh3d(meshes.add(Cuboid::new(BEAM_WIDTH, BEAM_WIDTH, 1.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.55, 0.85, 1.0),
            unlit: true,
            fog_enabled: false,
            ..default()
        })),
        Transform::default(),
        Visibility::Hidden,
        RenderLayers::layer(PANEL_RENDER_LAYER),
        NoFrustumCulling,
    );
    commands
        .spawn((Name::new("vr laser hand"), AimHand, space))
        .with_child(beam);
}

/// The session ended: the hand's space went with it.
fn drop_hand(mut commands: Commands, hand: Query<Entity, With<AimHand>>) {
    for hand in &hand {
        commands.entity(hand).despawn();
    }
}

/// The ray from the hand onto the panel, written to the pointer and to [`Laser`].
fn aim_laser(
    hand: Query<(&Transform, &OxrSpaceLocationFlags), With<AimHand>>,
    screen: Query<(), With<PanelScreen>>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut aimed: ResMut<Aimed>,
    mut laser: ResMut<Laser>,
    mut sent: Local<Option<Aim>>,
) {
    let tracked = hand
        .single()
        .ok()
        .filter(|(_, flags)| flags.pos_valid() && flags.rot_valid());
    let hit = tracked.and_then(|(pose, _)| {
        let window = window.single().ok()?;
        screen.single().ok()?;
        let size = Vec2::new(window.width(), window.height());
        let quad = panel::quad_in_root(panel::panel_size(size.x / size.y.max(1.0)));
        let ray = Ray3d::new(
            pose.translation,
            Dir3::new(pose.rotation * Vec3::NEG_Z).ok()?,
        );
        ray_to_panel(ray, &quad.compute_affine(), size).map(|mut hit| {
            // The last pixel is inside the window; the edge of the quad is one past it.
            hit.cursor = hit
                .cursor
                .clamp(Vec2::ZERO, (size - Vec2::ONE).max(Vec2::ZERO));
            hit
        })
    });
    let now = Aimed {
        tracked: tracked.is_some(),
        hit,
    };
    if *aimed != now {
        *aimed = now;
    }
    let on_panel = hit.is_some();
    if laser.on_panel != on_panel {
        laser.on_panel = on_panel;
    }
    let aim = hit.map(|h| Aim {
        cursor: Some(h.cursor),
        ray: None,
    });
    if *sent != aim {
        pointer::set_vr(aim);
        *sent = aim;
    }
}

/// The beam runs from the hand to the hit, or a stub where it hits nothing, and hides with the
/// controller.
fn shape_beam(aimed: Res<Aimed>, mut beam: Query<(&mut Transform, &mut Visibility), With<Beam>>) {
    let length = aimed.length();
    for (mut transform, mut visibility) in &mut beam {
        // A unit-long beam along -Z, scaled to `length` and moved so it starts at the hand.
        transform.scale = Vec3::new(1.0, 1.0, length);
        transform.translation = Vec3::new(0.0, 0.0, -length / 2.0);
        let want = if aimed.tracked {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != want {
            *visibility = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::window::WindowResolution;

    use super::*;
    use crate::panel::{marker_position, panel_size, quad_in_root};

    const WINDOW: Vec2 = Vec2::new(1280.0, 720.0);

    /// A quad turned, tilted, moved and scaled, so the inverse is not the identity.
    fn tilted() -> Affine3A {
        Transform::from_xyz(0.3, -0.2, -1.5)
            .with_rotation(Quat::from_rotation_y(0.4) * Quat::from_rotation_x(-0.2))
            .with_scale(Vec3::new(1.6, 0.9, 1.0))
            .compute_affine()
    }

    fn ray_through(quad: &Affine3A, from: Vec3, at: Vec3) -> Ray3d {
        let origin = quad.transform_point3(from);
        let to = quad.transform_point3(at);
        Ray3d::new(origin, Dir3::new(to - origin).unwrap())
    }

    #[test]
    fn the_ray_hits_where_the_marker_shows() {
        let quad = tilted();
        let panel = Vec2::new(1.6, 0.9);
        // The corners are a hundredth of a pixel inside: a ray through the exact edge is a coin
        // toss in floating point.
        let (lo, hi) = (Vec2::splat(0.01), WINDOW - Vec2::splat(0.01));
        let cursors = [
            lo,
            Vec2::new(hi.x, lo.y),
            Vec2::new(lo.x, hi.y),
            hi,
            WINDOW / 2.0,
            Vec2::new(960.0, 180.0),
            Vec2::new(17.0, 701.0),
        ];
        for c in cursors {
            // The marker's place on the panel, and a ray from a hand in front of it onto that spot.
            let m = marker_position(Some(c), true, WINDOW, panel).unwrap();
            let ray = ray_through(&quad, Vec3::new(0.1, -0.3, 2.0), (m / panel).extend(0.0));
            let hit = ray_to_panel(ray, &quad, WINDOW).unwrap_or_else(|| panic!("{c} missed"));
            assert!((hit.cursor - c).length() < 0.05, "{c}: {}", hit.cursor);
            assert!((hit.local * panel - m).length() < 1e-4, "{c}");
        }
    }

    #[test]
    fn the_distance_is_along_the_ray() {
        let quad = Transform::from_xyz(0.0, 0.0, -2.0)
            .with_scale(Vec3::new(1.6, 0.9, 1.0))
            .compute_affine();
        let ray = Ray3d::new(Vec3::new(0.0, 0.0, 1.0), Dir3::NEG_Z);
        let hit = ray_to_panel(ray, &quad, WINDOW).unwrap();
        assert_eq!(hit.cursor, WINDOW / 2.0);
        assert!((hit.distance - 3.0).abs() < 1e-5);
    }

    #[test]
    fn a_ray_past_the_edge_misses() {
        let quad = tilted();
        for local in [
            Vec3::new(0.51, 0.0, 0.0),
            Vec3::new(-0.51, 0.0, 0.0),
            Vec3::new(0.0, 0.51, 0.0),
            Vec3::new(0.0, -0.51, 0.0),
        ] {
            let ray = ray_through(&quad, Vec3::new(0.0, 0.0, 2.0), local);
            assert_eq!(ray_to_panel(ray, &quad, WINDOW), None, "{local}");
        }
        // The edge itself is on the panel.
        let ray = ray_through(&quad, Vec3::new(0.0, 0.0, 2.0), Vec3::new(0.5, -0.5, 0.0));
        assert!(ray_to_panel(ray, &quad, WINDOW).is_some());
    }

    #[test]
    fn a_ray_with_the_panel_behind_it_or_parallel_misses() {
        let quad = tilted();
        // In front of the panel, pointing away from it.
        let away = Ray3d::new(
            quad.transform_point3(Vec3::new(0.0, 0.0, 1.0)),
            Dir3::new(quad.transform_vector3(Vec3::Z)).unwrap(),
        );
        assert_eq!(ray_to_panel(away, &quad, WINDOW), None);
        // Along the panel's plane.
        let parallel = Ray3d::new(
            quad.transform_point3(Vec3::new(-2.0, 0.0, 1.0)),
            Dir3::new(quad.transform_vector3(Vec3::X)).unwrap(),
        );
        assert_eq!(ray_to_panel(parallel, &quad, WINDOW), None);
    }

    #[test]
    fn the_back_of_the_panel_is_not_pointable() {
        let quad = tilted();
        let from_behind = ray_through(&quad, Vec3::new(0.0, 0.0, -2.0), Vec3::ZERO);
        assert_eq!(ray_to_panel(from_behind, &quad, WINDOW), None);
        // Past the panel and still going: the hit is behind the origin.
        let past = Ray3d::new(
            quad.transform_point3(Vec3::new(0.0, 0.0, -1.0)),
            Dir3::new(quad.transform_vector3(Vec3::NEG_Z)).unwrap(),
        );
        assert_eq!(ray_to_panel(past, &quad, WINDOW), None);
    }

    #[test]
    fn the_panel_in_the_headset_is_hit_straight_ahead() {
        // A hand at the origin of the tracking space, pointing down -Z: the middle of the panel,
        // a little above the centre line because the panel hangs below eye level.
        let quad = quad_in_root(panel_size(16.0 / 9.0)).compute_affine();
        let size = Vec2::new(1920.0, 1080.0);
        let ray = Ray3d::new(Vec3::ZERO, Dir3::NEG_Z);
        let hit = ray_to_panel(ray, &quad, size).unwrap();
        assert!((hit.cursor.x - 960.0).abs() < 1e-3);
        assert!(hit.cursor.y < 540.0, "{}", hit.cursor.y);
        assert!((hit.distance - panel::PANEL_DISTANCE).abs() < 1e-5);
    }

    /// An app with the hand, the panel and the window, and the systems that connect them.
    fn world() -> (App, Entity) {
        let mut app = App::new();
        app.init_resource::<Aimed>()
            .init_resource::<Laser>()
            .add_systems(Update, aim_laser);
        app.world_mut().spawn((
            Window {
                resolution: WindowResolution::new(1280, 720),
                ..default()
            },
            PrimaryWindow,
        ));
        app.world_mut().spawn(PanelScreen);
        let hand = app
            .world_mut()
            .spawn((
                AimHand,
                Transform::default(),
                OxrSpaceLocationFlags(
                    openxr::SpaceLocationFlags::POSITION_VALID
                        | openxr::SpaceLocationFlags::ORIENTATION_VALID,
                ),
            ))
            .id();
        (app, hand)
    }

    /// Pointer sources are process-wide, so the scenarios that write them are one test.
    #[test]
    fn a_hit_is_the_cursor_and_sets_the_laser_and_a_miss_lets_go() {
        let (mut app, hand) = world();
        app.update();
        let aimed = *app.world().resource::<Aimed>();
        let hit = aimed.hit.expect("straight ahead hits the panel");
        assert!(aimed.tracked && app.world().resource::<Laser>().on_panel);
        assert_eq!(
            pointer::ui_cursor(&Window::default()),
            Some(hit.cursor),
            "the laser is the UI's cursor"
        );

        // Turned away: no hit, the laser is off and the pointer is the window's again.
        app.world_mut().get_mut::<Transform>(hand).unwrap().rotation =
            Quat::from_rotation_y(std::f32::consts::PI);
        app.update();
        assert_eq!(app.world().resource::<Aimed>().hit, None);
        assert!(app.world().resource::<Aimed>().tracked);
        assert!(!app.world().resource::<Laser>().on_panel);
        assert_eq!(pointer::ui_cursor(&Window::default()), None);

        // A controller the runtime lost is not pointing, whatever pose it kept.
        app.world_mut().get_mut::<Transform>(hand).unwrap().rotation = Quat::IDENTITY;
        app.world_mut()
            .entity_mut(hand)
            .insert(OxrSpaceLocationFlags::default());
        app.update();
        assert_eq!(*app.world().resource::<Aimed>(), Aimed::default());
        assert!(!app.world().resource::<Laser>().on_panel);
        assert_eq!(pointer::ui_cursor(&Window::default()), None);
    }

    #[test]
    fn the_beam_runs_to_the_hit_and_hides_with_the_controller() {
        let mut app = App::new();
        app.init_resource::<Aimed>().add_systems(Update, shape_beam);
        let beam = app
            .world_mut()
            .spawn((Beam, Transform::default(), Visibility::Hidden))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Transform>(beam).unwrap().scale.z,
            MISS_LENGTH
        );
        assert_eq!(
            *app.world().get::<Visibility>(beam).unwrap(),
            Visibility::Hidden
        );
        *app.world_mut().resource_mut::<Aimed>() = Aimed {
            tracked: true,
            hit: Some(PanelHit {
                cursor: Vec2::ZERO,
                local: Vec2::ZERO,
                distance: 1.5,
            }),
        };
        app.update();
        let t = app.world().get::<Transform>(beam).unwrap();
        assert_eq!((t.scale.z, t.translation.z), (1.5, -0.75));
        assert_eq!(
            *app.world().get::<Visibility>(beam).unwrap(),
            Visibility::Inherited
        );
    }
}
