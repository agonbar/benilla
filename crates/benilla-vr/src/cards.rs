//! Chat bubbles as cards in the world.
//!
//! While a session runs the client draws each bubble's frames to an atlas cell instead of the
//! panel ([`benilla_app::card_lane`]); this module shows each cell as a quad over its speaker. A
//! card is a child of the tracking root, on the panel's render layer, so each eye's overlay camera
//! draws it once, at the speaker's depth, and the vignette ring still draws over it.
//!
//! * **Pose.** `root⁻¹ × anchor`, read from the root's `Transform` that `seat_head` wrote this
//!   frame, not from its `GlobalTransform`, which is a frame old. A snap turn, a step or a
//!   third-person orbit moves the root; the card stays on the speaker.
//! * **Facing.** See [`layout`]. **Size.** See [`unit_per_texel`]. **Own bubble.** See [`visible`].
//! * **Fade.** The card's material alpha. The atlas is drawn at alpha 1 and is not drawn again
//!   for a fade.
//!
//! Deviation: the reference draws a bubble on the screen (`0x4b0c30`, `0x4b157a`) and 1.12.1 has no headset, so
//! a card at the speaker's depth, in the world and not on the panel, is benilla's own.

use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::math::{Affine2, Rect};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use benilla_app::card_lane::{Card, CardLane, CardLaneSet, ATLAS_SIZE};

use bevy_mod_xr::session::XrTrackingRoot;

use crate::comfort::Person;
use crate::panel::{PANEL_DISTANCE, PANEL_RENDER_LAYER, PANEL_WIDTH};
use crate::steer::Comfort;
use crate::{rig, seat_head, session_running, Views};

/// The nearest head-to-anchor distance, in world units, a card keeps its angular size at. Nearer,
/// it keeps its size at this distance and so grows on the eye.
pub const NEAR: f32 = 2.0;
/// The farthest. Farther, it keeps its size at this distance and so shrinks on the eye.
pub const FAR: f32 = 20.0;

/// World units a texel of a card spans at head-to-anchor distance `distance`, for a window
/// `window_width` logical px wide.
///
/// Deviation: the reference draws a bubble at a constant size on the screen (`0x4b157a`); a card
/// keeps a constant angle between [`NEAR`] and [`FAR`] units from the head and a constant size
/// outside them, so a bubble at 20 yd stays readable and one up close does not fill the view.
///
/// The angle a texel spans is the panel's: the panel shows a window px at
/// `PANEL_WIDTH / PANEL_DISTANCE / window_width` radians, so a bubble's text reads as big as the
/// interface's. The distance is clamped to [`NEAR`]..=[`FAR`].
pub fn unit_per_texel(distance: f32, window_width: f32) -> f32 {
    let per_texel = PANEL_WIDTH / PANEL_DISTANCE / window_width.max(1.0);
    per_texel * distance.clamp(NEAR, FAR)
}

/// A card is shown unless its line is hidden, it has faded out, it has not been drawn yet, or it
/// is the local player's in first person.
///
/// Deviation: the reference never hides the local player's own bubble (`0x608ac0`, self is not
/// excluded). In first person the tracking root sits in the avatar's head and the anchor is a
/// metre above the model's top, so the card would hang over the eyes; it is hidden there and shown
/// in third person.
pub fn visible(card: &Card, person: Person) -> bool {
    card.shown && card.alpha > 0.0 && card.rect.is_some() && !(card.own && person == Person::First)
}

/// Where a card's quad goes and what it samples.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// In the root's space; the scale is the quad's size, which faces +Z before the rotation.
    pub transform: Transform,
    /// Takes the quad's unit uv to the card's rectangle of the atlas.
    pub uv: Affine2,
}

/// The card's quad for `card` drawn in `rect` of the atlas (texels, y down), with the tracking
/// root at `root` and the head at `head` in the root's space, for a window `window_width` logical
/// px wide. The card's seat, the text's bottom centre, sits on the anchor.
///
/// Deviation: the card is a billboard that turns about the vertical alone toward the head, so a
/// tilted head does not roll it; the reference's screen overlay has no facing.
pub fn layout(card: &Card, rect: Rect, root: &Transform, head: Vec3, window_width: f32) -> Layout {
    let anchor = root
        .compute_affine()
        .inverse()
        .transform_point3(card.anchor);
    let to_head = head - anchor;
    let flat = Vec3::new(to_head.x, 0.0, to_head.z);
    let rotation = if flat.length_squared() > 1e-6 {
        Quat::from_rotation_y(flat.x.atan2(flat.z))
    } else {
        Quat::IDENTITY
    };
    let unit = unit_per_texel(to_head.length(), window_width);
    // From the rectangle's centre to the seat, in the quad's axes: x right, y up (the atlas' y is
    // down).
    let seat = card.seat - rect.center();
    let to_seat = Vec3::new(seat.x, -seat.y, 0.0) * unit;
    let size = rect.size() * unit;
    let atlas = ATLAS_SIZE as f32;
    Layout {
        transform: Transform {
            translation: anchor - rotation * to_seat,
            rotation,
            scale: size.extend(1.0),
        },
        uv: Affine2::from_scale_angle_translation(rect.size() / atlas, 0.0, rect.min / atlas),
    }
}

/// A card's quad, one per live speaker.
#[derive(Component)]
pub(crate) struct CardQuad {
    pub key: u64,
}

/// The unit quad every card shares.
#[derive(Resource, Default)]
struct CardMesh(Option<Handle<Mesh>>);

pub struct CardsPlugin;

impl Plugin for CardsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CardMesh>().add_systems(
            Update,
            (
                switch_lane,
                place_cards
                    .after(seat_head)
                    .after(CardLaneSet)
                    .run_if(session_running),
            ),
        );
    }
}

/// The client draws bubbles to cards while a session runs and to the screen otherwise.
fn switch_lane(state: Option<Res<bevy_mod_xr::session::XrState>>, mut lane: ResMut<CardLane>) {
    let on = matches!(
        state.as_deref(),
        Some(bevy_mod_xr::session::XrState::Running)
    );
    if lane.enabled != on {
        lane.enabled = on;
    }
}

/// A card's material: the atlas, unlit, blended as the panel is, premultiplied. The atlas holds
/// colour premultiplied by alpha, so the fade scales every channel.
///
/// Deviation: nothing occludes a card. It draws on the overlay cameras, as the panel does, so a
/// bubble shows through a wall as the reference's screen overlay does through anything.
fn material(atlas: &Handle<Image>, layout: &Layout, alpha: f32) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::linear_rgba(alpha, alpha, alpha, alpha),
        base_color_texture: Some(atlas.clone()),
        uv_transform: layout.uv,
        unlit: true,
        alpha_mode: AlphaMode::Premultiplied,
        cull_mode: None,
        double_sided: true,
        fog_enabled: false,
        ..default()
    }
}

/// The tracking root: its entity and the `Transform` `seat_head` just wrote.
type RootQuery = (Entity, &'static Transform);
type RootFilter = (With<XrTrackingRoot>, Without<CardQuad>);
/// A card's quad: what a frame rewrites.
type QuadQuery = (
    Entity,
    &'static CardQuad,
    &'static mut Transform,
    &'static mut Visibility,
    &'static MeshMaterial3d<StandardMaterial>,
);

/// Seats a quad for each live card and takes down the quads of speakers that are gone. Runs after
/// `seat_head`, which wrote the root's `Transform` this frame.
#[allow(clippy::too_many_arguments)]
fn place_cards(
    lane: Res<CardLane>,
    views: Res<Views>,
    comfort: Res<Comfort>,
    mut mesh: ResMut<CardMesh>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    root: Query<RootQuery, RootFilter>,
    mut quads: Query<QuadQuery, Without<XrTrackingRoot>>,
) {
    let (Some(eyes), Ok((root_entity, root)), Ok(window), Some(atlas)) =
        (views.0, root.single(), windows.single(), lane.atlas())
    else {
        return;
    };
    let head = rig::head_centre(&eyes).translation;
    let person = comfort.config.person;
    let width = window.width();

    let mut seen: Vec<u64> = Vec::with_capacity(lane.cards.len());
    for card in &lane.cards {
        let Some(rect) = card.rect else { continue };
        seen.push(card.key);
        let show = visible(card, person);
        let place = layout(card, rect, root, head, width);
        let want = if show {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        match quads.iter_mut().find(|(_, q, ..)| q.key == card.key) {
            Some((_, _, mut transform, mut visibility, handle)) => {
                if *visibility != want {
                    *visibility = want;
                }
                if !show {
                    continue;
                }
                if *transform != place.transform {
                    *transform = place.transform;
                }
                // Read before writing, so a card at rest raises no asset change. The atlas handle
                // is part of it: a session restart drops the atlas and the lane makes a new one.
                let fresh = material(atlas, &place, card.alpha);
                let same = materials.get(&handle.0).is_some_and(|m| {
                    m.base_color == fresh.base_color
                        && m.uv_transform == fresh.uv_transform
                        && m.base_color_texture == fresh.base_color_texture
                });
                if !same {
                    if let Some(m) = materials.get_mut(&handle.0) {
                        m.base_color = fresh.base_color;
                        m.uv_transform = fresh.uv_transform;
                        m.base_color_texture = fresh.base_color_texture;
                    }
                }
            }
            None => {
                let mesh = mesh
                    .0
                    .get_or_insert_with(|| meshes.add(Rectangle::new(1.0, 1.0)))
                    .clone();
                let material = materials.add(material(atlas, &place, card.alpha));
                let quad = commands
                    .spawn((
                        CardQuad { key: card.key },
                        Name::new("vr bubble card"),
                        Mesh3d(mesh),
                        MeshMaterial3d(material),
                        place.transform,
                        want,
                        RenderLayers::layer(PANEL_RENDER_LAYER),
                        NoFrustumCulling,
                    ))
                    .id();
                commands.entity(root_entity).add_child(quad);
            }
        }
    }
    for (entity, quad, ..) in &quads {
        if !seen.contains(&quad.key) {
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> Card {
        Card {
            key: 7,
            anchor: Vec3::new(10.0, 3.0, -4.0),
            alpha: 1.0,
            shown: true,
            own: false,
            seat: Vec2::new(160.0, 200.0),
            rect: Some(Rect::from_corners(
                Vec2::new(40.0, 20.0),
                Vec2::new(280.0, 220.0),
            )),
        }
    }

    const WINDOW: f32 = 1600.0;

    /// The size law: a texel spans the panel's angle between 2 and 20 units and a fixed size
    /// outside, so a card is as big on the eye at 2, 10 and 20 and keeps its size at 1 and 40.
    #[test]
    fn the_size_is_constant_in_angle_between_the_clamps_and_in_size_outside() {
        let at = |d: f32| unit_per_texel(d, WINDOW);
        let angle = |d: f32| at(d) / d;
        // The panel's own angle: 1.6 units at 1.5 over 1600 px.
        let panel = PANEL_WIDTH / PANEL_DISTANCE / WINDOW;
        for d in [2.0, 10.0, 20.0] {
            assert!(
                (angle(d) - panel).abs() < 1e-7,
                "{d}: {} vs {panel}",
                angle(d)
            );
        }
        assert_eq!(at(1.0), at(2.0), "nearer than 2 keeps the size it has at 2");
        assert_eq!(
            at(40.0),
            at(20.0),
            "farther than 20 keeps the size it has at 20"
        );
        assert!(angle(1.0) > angle(2.0), "so it grows on the eye up close");
        assert!(angle(40.0) < angle(20.0), "and shrinks on the eye far off");
        assert!(
            (at(20.0) / at(2.0) - 10.0).abs() < 1e-4,
            "a tenth the distance, a tenth the size"
        );
    }

    /// The own bubble is hidden in first person and shown in third; another speaker's is shown in
    /// both; a hidden, faded or undrawn card is never shown.
    #[test]
    fn the_own_bubble_is_hidden_in_first_person_and_shown_in_third() {
        let own = Card {
            own: true,
            ..card()
        };
        assert!(!visible(&own, Person::First));
        assert!(visible(&own, Person::Third));
        assert!(visible(&card(), Person::First));
        assert!(visible(&card(), Person::Third));
        for gone in [
            Card {
                shown: false,
                ..card()
            },
            Card {
                alpha: 0.0,
                ..card()
            },
            Card {
                rect: None,
                ..card()
            },
        ] {
            assert!(!visible(&gone, Person::Third), "{gone:?}");
        }
    }

    /// The card's seat is on the anchor, whatever the root does: for a root moved and turned
    /// between frames, `root × card pose` puts the seat at the same world point.
    #[test]
    fn the_card_stays_on_its_anchor_when_the_root_moves_and_turns() {
        let c = card();
        let rect = c.rect.unwrap();
        let seat_world = |root: &Transform, head_world: Vec3| {
            let head = root.compute_affine().inverse().transform_point3(head_world);
            let l = layout(&c, rect, root, head, WINDOW);
            // The seat is `to_seat` from the quad's centre, in the quad's axes.
            let unit = l.transform.scale.x / rect.size().x;
            let s = c.seat - rect.center();
            let local =
                l.transform.translation + l.transform.rotation * (Vec3::new(s.x, -s.y, 0.0) * unit);
            root.compute_affine().transform_point3(local)
        };
        let head_world = Vec3::new(2.0, 1.6, 3.0);
        let still = Transform::from_xyz(1.0, 0.0, 1.0);
        let moved = Transform::from_xyz(-6.0, 0.5, 8.0).with_rotation(Quat::from_rotation_y(2.1));
        for root in [still, moved] {
            let at = seat_world(&root, head_world);
            assert!(
                at.distance(c.anchor) < 1e-4,
                "{root:?}: {at} vs {}",
                c.anchor
            );
        }
    }

    /// The quad faces the head with yaw alone and its up is the root's: a head above or below the
    /// anchor does not tip it, and the head straight above gives the identity rather than NaN.
    #[test]
    fn the_card_faces_the_head_by_yaw_alone() {
        let c = card();
        let rect = c.rect.unwrap();
        let root = Transform::IDENTITY;
        for head in [
            c.anchor + Vec3::new(5.0, 0.0, 0.0),
            c.anchor + Vec3::new(-3.0, 4.0, 7.0),
            c.anchor + Vec3::new(0.2, -2.0, -9.0),
        ] {
            let l = layout(&c, rect, &root, head, WINDOW);
            let facing = l.transform.rotation * Vec3::Z;
            let want = Vec3::new(head.x - c.anchor.x, 0.0, head.z - c.anchor.z).normalize();
            assert!(facing.distance(want) < 1e-5, "{facing} vs {want}");
            assert!((l.transform.rotation * Vec3::Y).distance(Vec3::Y) < 1e-6);
        }
        let above = layout(&c, rect, &root, c.anchor + Vec3::Y * 3.0, WINDOW);
        assert_eq!(above.transform.rotation, Quat::IDENTITY);
    }

    /// The uv takes the unit quad's corners to the card's rectangle of the atlas.
    #[test]
    fn the_uv_maps_the_quad_onto_the_cards_rectangle() {
        let c = card();
        let rect = c.rect.unwrap();
        let l = layout(&c, rect, &Transform::IDENTITY, Vec3::ZERO, WINDOW);
        let atlas = ATLAS_SIZE as f32;
        let tl = l.uv.transform_point2(Vec2::ZERO);
        let br = l.uv.transform_point2(Vec2::ONE);
        assert!((tl - rect.min / atlas).length() < 1e-6);
        assert!((br - rect.max / atlas).length() < 1e-6);
    }

    mod world {
        use super::*;
        use bevy::asset::AssetPlugin;
        use bevy::window::WindowResolution;
        use bevy_mod_xr::session::XrState;

        use benilla_app::card_lane::CardLanePlugin;

        use crate::rig::Eye;

        /// An app with a running session, a root, a window, a lane with one atlas and one card.
        fn app(person: Person) -> App {
            let mut app = App::new();
            app.add_plugins((MinimalPlugins, AssetPlugin::default(), CardLanePlugin))
                .init_asset::<Image>()
                .init_asset::<Mesh>()
                .init_asset::<StandardMaterial>()
                .init_resource::<CardMesh>()
                .insert_resource(XrState::Running)
                .insert_resource(Views(Some([eye(-0.03), eye(0.03)])))
                .add_systems(Update, place_cards);
            let mut comfort = Comfort::default();
            comfort.config.person = person;
            app.insert_resource(comfort);
            app.world_mut().spawn((
                Window {
                    resolution: WindowResolution::new(1600, 900),
                    ..default()
                },
                PrimaryWindow,
            ));
            app.world_mut()
                .spawn((XrTrackingRoot, Transform::IDENTITY, Visibility::default()));
            // The lane builds its atlas the frame after it is switched on.
            app.world_mut().resource_mut::<CardLane>().enabled = true;
            app.update();
            assert!(app.world().resource::<CardLane>().atlas().is_some());
            app
        }

        fn eye(x: f32) -> Eye {
            Eye {
                pose: Transform::from_xyz(x, 0.0, 0.0),
                left: -0.8,
                right: 0.8,
                up: 0.8,
                down: -0.8,
            }
        }

        fn show(app: &mut App, cards: Vec<Card>) {
            app.world_mut().resource_mut::<CardLane>().cards = cards;
            app.update();
        }

        fn quads(app: &mut App) -> Vec<(u64, Transform, Visibility, Handle<StandardMaterial>)> {
            app.world_mut()
                .query::<(
                    &CardQuad,
                    &Transform,
                    &Visibility,
                    &MeshMaterial3d<StandardMaterial>,
                )>()
                .iter(app.world())
                .map(|(q, t, v, m)| (q.key, *t, *v, m.0.clone()))
                .collect()
        }

        /// One quad per speaker, on the panel's layer under the root; a speaker that stops is
        /// taken down.
        #[test]
        fn a_card_is_a_quad_under_the_root_on_the_panel_layer() {
            let mut app = app(Person::Third);
            show(&mut app, vec![card(), Card { key: 9, ..card() }]);
            assert_eq!(quads(&mut app).len(), 2);
            let root = app
                .world_mut()
                .query_filtered::<Entity, With<XrTrackingRoot>>()
                .single(app.world())
                .unwrap();
            let kids = app.world().get::<Children>(root).unwrap();
            assert_eq!(kids.len(), 2);
            let layers = app
                .world_mut()
                .query_filtered::<&RenderLayers, With<CardQuad>>()
                .iter(app.world())
                .cloned()
                .collect::<Vec<_>>();
            assert!(layers
                .iter()
                .all(|l| *l == RenderLayers::layer(PANEL_RENDER_LAYER)));
            show(&mut app, vec![Card { key: 9, ..card() }]);
            app.update();
            let left: Vec<u64> = quads(&mut app).iter().map(|q| q.0).collect();
            assert_eq!(left, [9]);
        }

        /// A fade writes the card's material alpha and nothing else: the same material asset, the
        /// same uv, the same pose, premultiplied so every channel scales.
        #[test]
        fn a_fade_changes_the_material_alpha_and_nothing_else() {
            let mut app = app(Person::Third);
            show(&mut app, vec![card()]);
            let before = quads(&mut app).remove(0);
            let uv = |app: &App, h: &Handle<StandardMaterial>| {
                app.world()
                    .resource::<Assets<StandardMaterial>>()
                    .get(h)
                    .unwrap()
                    .uv_transform
            };
            let uv0 = uv(&app, &before.3);
            show(
                &mut app,
                vec![Card {
                    alpha: 0.25,
                    ..card()
                }],
            );
            let after = quads(&mut app).remove(0);
            assert_eq!(after.3, before.3, "the same material");
            assert_eq!(after.1, before.1, "the same pose");
            assert_eq!(uv(&app, &after.3), uv0);
            let m = app
                .world()
                .resource::<Assets<StandardMaterial>>()
                .get(&after.3)
                .unwrap();
            assert_eq!(m.base_color, Color::linear_rgba(0.25, 0.25, 0.25, 0.25));
            assert_eq!(m.alpha_mode, AlphaMode::Premultiplied);
            assert_eq!(
                app.world().resource::<Assets<Image>>().len(),
                1,
                "only the atlas: the fade made no image"
            );
        }

        /// The root moves and turns between frames; the card's world pose follows the anchor
        /// frame by frame (the root's `Transform`, as `seat_head` writes it).
        #[test]
        fn the_card_follows_the_root_between_frames() {
            let mut app = app(Person::Third);
            let c = card();
            let rect = c.rect.unwrap();
            let seat_world = |app: &mut App| -> Vec3 {
                let (root, quad) = {
                    let w = app.world_mut();
                    let root = *w
                        .query_filtered::<&Transform, With<XrTrackingRoot>>()
                        .single(w)
                        .unwrap();
                    let quad = quads(app).remove(0).1;
                    (root, quad)
                };
                let unit = quad.scale.x / rect.size().x;
                let s = c.seat - rect.center();
                let local = quad.translation + quad.rotation * (Vec3::new(s.x, -s.y, 0.0) * unit);
                root.compute_affine().transform_point3(local)
            };
            show(&mut app, vec![c.clone()]);
            let first = seat_world(&mut app);
            assert!(first.distance(c.anchor) < 1e-3);
            for (pos, yaw) in [
                (Vec3::new(5.0, 0.0, -2.0), 0.7),
                (Vec3::new(-9.0, 1.0, 4.0), -2.4),
            ] {
                *app.world_mut()
                    .query_filtered::<&mut Transform, With<XrTrackingRoot>>()
                    .single_mut(app.world_mut())
                    .unwrap() =
                    Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw));
                show(&mut app, vec![c.clone()]);
                let at = seat_world(&mut app);
                assert!(at.distance(c.anchor) < 1e-3, "{at} vs {}", c.anchor);
            }
        }

        /// The own bubble's quad is hidden in first person and visible in third.
        #[test]
        fn the_own_card_is_hidden_in_first_person_and_shown_in_third() {
            for (person, want) in [
                (Person::First, Visibility::Hidden),
                (Person::Third, Visibility::Inherited),
            ] {
                let mut app = app(person);
                show(
                    &mut app,
                    vec![Card {
                        own: true,
                        ..card()
                    }],
                );
                assert_eq!(quads(&mut app)[0].2, want, "{person:?}");
            }
        }

        /// The headset idles and the session restarts: the lane drops its atlas and builds a new
        /// one, and a speaker who kept talking keeps a card drawn from the new atlas.
        #[test]
        fn a_card_survives_a_session_restart_on_the_new_atlas() {
            let mut app = app(Person::Third);
            show(&mut app, vec![card()]);
            let first = app.world().resource::<CardLane>().atlas().cloned().unwrap();
            app.world_mut().resource_mut::<CardLane>().enabled = false;
            app.update();
            assert!(app.world().resource::<CardLane>().atlas().is_none());
            app.world_mut().resource_mut::<CardLane>().enabled = true;
            app.update();
            let second = app.world().resource::<CardLane>().atlas().cloned().unwrap();
            assert_ne!(second, first, "a new atlas image");
            show(&mut app, vec![card()]);
            let quad = quads(&mut app).remove(0);
            let m = app
                .world()
                .resource::<Assets<StandardMaterial>>()
                .get(&quad.3)
                .unwrap();
            assert_eq!(m.base_color_texture.as_ref(), Some(&second));
        }

        /// With no atlas yet, no head or no drawn card, nothing is spawned.
        #[test]
        fn nothing_is_placed_before_the_atlas_and_the_card_are_drawn() {
            let mut app = app(Person::Third);
            show(
                &mut app,
                vec![Card {
                    rect: None,
                    ..card()
                }],
            );
            assert!(quads(&mut app).is_empty());
            app.insert_resource(Views(None));
            show(&mut app, vec![card()]);
            assert!(quads(&mut app).is_empty());
        }
    }
}
