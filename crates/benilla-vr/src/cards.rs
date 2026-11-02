//! Chat bubbles in the headset as cards: each live bubble is a quad in the world over its speaker,
//! showing the cell of the card atlas its frames were drawn to ([`benilla_app::card_lane`]). The
//! reference draws a bubble on the screen over the speaker's projection (`0x4b0940`); the panel
//! cannot put it at the speaker's depth, and 1.12.1 has no headset, so everything here is a
//! deviation.
//!
//! * Deviation: a card keeps the angular size of a panel texel between 2 and 20 world units from
//!   the head and the size it has at those distances beyond them ([`texel_size`]); the reference
//!   draws a bubble at a constant screen size.
//! * Deviation: a card is not occluded; it is drawn by the overlay cameras, which have a depth
//!   buffer of their own, so a bubble behind a wall shows through it as it does in the reference.
//! * Deviation: a card turns about the vertical toward the head ([`card_pose`]); the reference's
//!   bubble has no facing.
//! * Deviation: the local player's own bubble is not shown in first person, where its anchor
//!   (`0x4b0c30`) hangs about a metre over the eyes ([`shown`]); the reference does not exclude it.
//!
//! The cards are children of the tracking root, which `seat_head` rewrites every frame, so a card's
//! pose is `root⁻¹ × anchor`, computed from the root's `Transform` after that write and not from its
//! `GlobalTransform` (a frame behind). Both overlay cameras draw layer 60, so one card entity is
//! drawn once per eye, each with its own disparity.

use std::collections::HashMap;

use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::math::Affine2;
use bevy::prelude::*;
use bevy::transform::TransformSystems;

use benilla_app::card_lane::{Card, CardLane, ATLAS_SIZE};
use bevy_mod_openxr::render::update_views;
use bevy_mod_xr::session::{XrState, XrTrackingRoot};

use crate::comfort::Person;
use crate::panel::PANEL_RENDER_LAYER;
use crate::steer::Comfort;
use crate::{rig, Views};

/// The angle one atlas texel covers, in radians: a panel texel at 1280 px over 1.6 m at 1.5 m
/// distance is 0.83 mrad, and the bubble's text is made to be read at that size.
pub const TEXEL_ANGLE: f32 = 0.0008;

/// The distances, in world units from the head, between which a card keeps its angular size.
pub const NEAR: f32 = 2.0;
pub const FAR: f32 = 20.0;

/// World units one atlas texel covers on a card `distance` from the head: constant angular size
/// between [`NEAR`] and [`FAR`], and the size of those distances outside them, so a card does not
/// grow without end when far or shrink to nothing up close.
pub fn texel_size(distance: f32) -> f32 {
    TEXEL_ANGLE * distance.clamp(NEAR, FAR)
}

/// A card's place in the tracking root's space: its origin on the speaker's `anchor` and its front
/// turned about the vertical toward `head`, both in root space after `root` is taken off.
pub fn card_pose(root: &Transform, anchor: Vec3, head: Vec3) -> Transform {
    let at = root.compute_affine().inverse().transform_point3(anchor);
    let toward = head - at;
    let rotation = if toward.xz().length_squared() > 1e-8 {
        Quat::from_rotation_y(toward.x.atan2(toward.z))
    } else {
        Quat::IDENTITY
    };
    Transform::from_translation(at).with_rotation(rotation)
}

/// Whether a card is drawn: its line is shown, it has not faded out, it has drawn (`rect`), and it
/// is not the player's own in first person.
pub fn shown(card: &Card, person: Person) -> bool {
    card.shown && card.alpha > 0.0 && card.rect.is_some() && !(card.own && person == Person::First)
}

/// The quad of a card in its own space: scaled to the card's drawn rect at `texel` world units a
/// texel, and moved so the text's bottom centre (`card.seat`) is the card's origin. Atlas y points
/// down, the card's up.
pub fn quad_of(card: &Card, texel: f32) -> Option<Transform> {
    let rect = card.rect?;
    let offset = (rect.center() - card.seat) * texel;
    Some(
        Transform::from_xyz(offset.x, -offset.y, 0.0).with_scale((rect.size() * texel).extend(1.0)),
    )
}

/// The fade as the quad's colour: the atlas holds colour premultiplied by alpha, so the fade
/// scales all four channels.
fn fade(alpha: f32) -> Color {
    Color::linear_rgba(alpha, alpha, alpha, alpha)
}

/// The cell of the atlas a rect covers, as the transform from the quad's 0..1 uv.
fn uv_of(rect: Rect) -> Affine2 {
    let atlas = ATLAS_SIZE as f32;
    Affine2::from_scale_angle_translation(rect.size() / atlas, 0.0, rect.min / atlas)
}

struct Live {
    /// The card: at the anchor, facing the head.
    card: Entity,
    quad: Entity,
    material: Handle<StandardMaterial>,
}

/// The entities of the live cards by speaker.
#[derive(Resource, Default)]
struct Cards(HashMap<u64, Live>);

pub struct CardsPlugin;

impl Plugin for CardsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Cards>()
            .add_systems(Update, switch_lane)
            .add_systems(
                PostUpdate,
                place_cards
                    .before(update_views)
                    .before(TransformSystems::Propagate),
            );
    }
}

/// The lane draws bubbles to cards while a session runs, and to the screen when it does not.
fn switch_lane(state: Option<Res<XrState>>, mut lane: ResMut<CardLane>) {
    let running = session_running_state(state.as_deref());
    if lane.enabled != running {
        lane.enabled = running;
    }
}

fn session_running_state(state: Option<&XrState>) -> bool {
    matches!(state, Some(XrState::Running))
}

/// Seats a card for each of the lane's live bubbles under the tracking root, and takes down the
/// ones that ended. Writes a component only when its value changes.
#[allow(clippy::too_many_arguments)]
fn place_cards(
    lane: Res<CardLane>,
    comfort: Res<Comfort>,
    views: Res<Views>,
    mut live: ResMut<Cards>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    root: Query<(Entity, &Transform), With<XrTrackingRoot>>,
    mut poses: Query<(&mut Transform, &mut Visibility), (Without<XrTrackingRoot>, Without<Quad>)>,
    mut quads: Query<&mut Transform, (With<Quad>, Without<XrTrackingRoot>)>,
    mut quad_mesh: Local<Option<Handle<Mesh>>>,
) {
    let (Ok((root_entity, root)), Some(atlas)) = (root.single(), lane.atlas()) else {
        retire(&mut live, &mut commands, &mut materials, |_| false);
        return;
    };
    let head = views
        .0
        .map(|eyes| rig::head_centre(&eyes).translation)
        .unwrap_or(Vec3::ZERO);
    let person = comfort.config.person;

    retire(&mut live, &mut commands, &mut materials, |key| {
        lane.cards.iter().any(|c| c.key == key)
    });
    for card in &lane.cards {
        let pose = card_pose(root, card.anchor, head);
        let texel = texel_size(pose.translation.distance(head));
        let (Some(quad), visible) = (quad_of(card, texel), shown(card, person)) else {
            // Not drawn yet: no rect to size a quad by, so the card waits hidden.
            if let Some(l) = live.0.get(&card.key) {
                if let Ok((_, mut visibility)) = poses.get_mut(l.card) {
                    if *visibility != Visibility::Hidden {
                        *visibility = Visibility::Hidden;
                    }
                }
            }
            continue;
        };
        let rect = card.rect.expect("quad_of has a rect");
        let want = StandardMaterial {
            base_color_texture: Some(atlas.clone()),
            base_color: fade(card.alpha),
            uv_transform: uv_of(rect),
            unlit: true,
            // The atlas holds colour premultiplied by alpha, as the panel's image does.
            alpha_mode: AlphaMode::Premultiplied,
            cull_mode: None,
            double_sided: true,
            fog_enabled: false,
            ..default()
        };
        let visibility = if visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        match live.0.get(&card.key) {
            Some(l) => {
                if let Ok((mut transform, mut vis)) = poses.get_mut(l.card) {
                    if *transform != pose {
                        *transform = pose;
                    }
                    if *vis != visibility {
                        *vis = visibility;
                    }
                }
                if let Ok(mut transform) = quads.get_mut(l.quad) {
                    if *transform != quad {
                        *transform = quad;
                    }
                }
                let stale = materials.get(&l.material).is_none_or(|m| {
                    m.base_color != want.base_color
                        || m.uv_transform != want.uv_transform
                        || m.base_color_texture != want.base_color_texture
                });
                if stale {
                    if let Some(m) = materials.get_mut(&l.material) {
                        *m = want;
                    }
                }
            }
            None => {
                let mesh = quad_mesh
                    .get_or_insert_with(|| meshes.add(Rectangle::new(1.0, 1.0)))
                    .clone();
                let material = materials.add(want);
                let layer = RenderLayers::layer(PANEL_RENDER_LAYER);
                let quad_entity = commands
                    .spawn((
                        Quad,
                        Mesh3d(mesh),
                        MeshMaterial3d(material.clone()),
                        quad,
                        layer,
                        NoFrustumCulling,
                    ))
                    .id();
                let entity = commands
                    .spawn((Name::new("vr chat card"), pose, visibility))
                    .add_child(quad_entity)
                    .id();
                commands.entity(root_entity).add_child(entity);
                live.0.insert(
                    card.key,
                    Live {
                        card: entity,
                        quad: quad_entity,
                        material,
                    },
                );
            }
        }
    }
}

/// Marker on a card's quad.
#[derive(Component)]
struct Quad;

/// Takes down every card whose speaker `keep` refuses.
fn retire(
    live: &mut Cards,
    commands: &mut Commands,
    materials: &mut Assets<StandardMaterial>,
    keep: impl Fn(u64) -> bool,
) {
    live.0.retain(|&key, l| {
        let kept = keep(key);
        if !kept {
            commands.entity(l.card).try_despawn();
            materials.remove(&l.material);
        }
        kept
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_app::card_lane::CardLanePlugin;
    use bevy::asset::AssetPlugin;
    use bevy::transform::TransformPlugin;

    fn card(key: u64, anchor: Vec3) -> Card {
        Card {
            key,
            anchor,
            alpha: 1.0,
            shown: true,
            own: false,
            seat: Vec2::new(150.0, 200.0),
            rect: Some(Rect::from_corners(
                Vec2::new(10.0, 20.0),
                Vec2::new(290.0, 210.0),
            )),
        }
    }

    /// An app with a tracking root, the lane on and its atlas up.
    fn app(person: Person) -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((AssetPlugin::default(), TransformPlugin, CardLanePlugin))
            .init_asset::<Image>()
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_resource::<Cards>()
            .init_resource::<Views>()
            .init_resource::<Comfort>()
            .add_systems(PostUpdate, place_cards.before(TransformSystems::Propagate));
        app.world_mut().resource_mut::<Comfort>().config.person = person;
        app.world_mut().resource_mut::<CardLane>().enabled = true;
        let root = app
            .world_mut()
            .spawn((XrTrackingRoot, Transform::IDENTITY))
            .id();
        // The atlas is up after one frame.
        app.update();
        (app, root)
    }

    fn set_cards(app: &mut App, cards: Vec<Card>) {
        app.world_mut().resource_mut::<CardLane>().cards = cards;
    }

    fn card_entity(app: &mut App) -> Option<Entity> {
        app.world_mut()
            .query_filtered::<Entity, (
                With<Name>,
                Without<Quad>,
                Without<XrTrackingRoot>,
                With<ChildOf>,
            )>()
            .iter(app.world())
            .find(|e| {
                app.world()
                    .get::<Name>(*e)
                    .is_some_and(|n| n.as_str() == "vr chat card")
            })
    }

    fn world_of(app: &App, e: Entity) -> GlobalTransform {
        *app.world().get::<GlobalTransform>(e).unwrap()
    }

    /// The size law at 1, 2, 10, 20 and 40 units: the clamp holds a card at the near and far
    /// distances' size, and between them the size is proportional to the distance.
    #[test]
    fn the_size_law_clamps_between_two_and_twenty_units() {
        assert_eq!(texel_size(1.0), texel_size(2.0));
        assert_eq!(texel_size(2.0), TEXEL_ANGLE * 2.0);
        assert_eq!(texel_size(10.0), TEXEL_ANGLE * 10.0);
        assert_eq!(texel_size(20.0), TEXEL_ANGLE * 20.0);
        assert_eq!(texel_size(40.0), texel_size(20.0));
        // Constant angular size inside the band: size over distance does not move.
        let ratio = |d: f32| texel_size(d) / d;
        assert!((ratio(5.0) - ratio(15.0)).abs() < 1e-9);
    }

    /// The root moves and turns between frames (a step, a snap turn, a third-person orbit) and the
    /// card's world pose stays on the anchor.
    #[test]
    fn a_card_stays_on_its_anchor_while_the_root_moves_and_turns() {
        let (mut app, root) = app(Person::Third);
        let anchor = Vec3::new(30.0, 4.0, -12.0);
        set_cards(&mut app, vec![card(7, anchor)]);
        app.update();
        let e = card_entity(&mut app).expect("the card is up");
        assert!(world_of(&app, e).translation().distance(anchor) < 1e-3);

        for (at, yaw) in [
            (Vec3::new(1.0, 0.0, 2.0), 0.7),
            (Vec3::new(-5.0, 1.0, 0.3), -2.4),
            (Vec3::new(28.0, 0.0, -9.0), 3.1),
        ] {
            *app.world_mut().get_mut::<Transform>(root).unwrap() =
                Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw));
            app.update();
            let w = world_of(&app, e);
            assert!(
                w.translation().distance(anchor) < 1e-3,
                "root at {at} turned {yaw}: card at {}",
                w.translation()
            );
        }
        assert_eq!(app.world().resource::<Cards>().0.len(), 1, "one card, kept");
    }

    /// The card turns about the vertical toward the head and never rolls or pitches.
    #[test]
    fn a_card_faces_the_head_by_yaw_alone() {
        let root = Transform::from_translation(Vec3::new(3.0, 0.0, 1.0));
        let anchor = Vec3::new(13.0, 9.0, 1.0);
        let head = Vec3::new(0.0, 0.1, 0.0);
        let pose = card_pose(&root, anchor, head);
        let (yaw, pitch, roll) = pose.rotation.to_euler(EulerRot::YXZ);
        assert!(pitch.abs() < 1e-5 && roll.abs() < 1e-5);
        // The front (+Z) points at the head on the ground plane.
        let front = pose.rotation * Vec3::Z;
        let to_head = (head - pose.translation).with_y(0.0).normalize();
        assert!(front.dot(to_head) > 0.9999, "yaw {yaw}");
    }

    /// The player's own bubble is hidden in first person and shown in third; a speaker's is shown
    /// in both.
    #[test]
    fn the_own_bubble_is_hidden_in_first_person_and_shown_in_third() {
        let mut own = card(1, Vec3::new(5.0, 2.0, 5.0));
        own.own = true;
        let other = card(2, Vec3::new(5.0, 2.0, -5.0));
        assert!(!shown(&own, Person::First));
        assert!(shown(&own, Person::Third));
        assert!(shown(&other, Person::First) && shown(&other, Person::Third));

        for (person, own_visible) in [(Person::First, false), (Person::Third, true)] {
            let (mut app, _) = app(person);
            set_cards(&mut app, vec![own.clone(), other.clone()]);
            app.update();
            let vis: Vec<(String, bool)> = app
                .world_mut()
                .query_filtered::<(&Name, &Visibility), Without<Quad>>()
                .iter(app.world())
                .filter(|(n, _)| n.as_str() == "vr chat card")
                .map(|(_, v)| (String::new(), *v != Visibility::Hidden))
                .collect();
            assert_eq!(vis.len(), 2, "both cards exist");
            assert_eq!(
                vis.iter().filter(|(_, v)| *v).count(),
                if own_visible { 2 } else { 1 },
                "{person:?}"
            );
        }
    }

    /// A fade writes the card's material alpha and nothing else: no entity is spawned, the uv rect
    /// holds, and the pose stays. (That a fade changes no quad is the extract's, tested in
    /// `benilla-ui`.)
    #[test]
    fn a_fade_changes_the_material_alpha_only() {
        let (mut app, _) = app(Person::Third);
        let mut c = card(9, Vec3::new(8.0, 3.0, 8.0));
        set_cards(&mut app, vec![c.clone()]);
        app.update();
        let (e, handle) = {
            let live = &app.world().resource::<Cards>().0[&9];
            (live.card, live.material.clone())
        };
        let before = app
            .world()
            .resource::<Assets<StandardMaterial>>()
            .get(&handle)
            .unwrap()
            .clone();
        let pose = world_of(&app, e);
        assert_eq!(before.base_color, Color::linear_rgba(1.0, 1.0, 1.0, 1.0));

        c.alpha = 0.25;
        set_cards(&mut app, vec![c.clone()]);
        app.update();
        let after = app
            .world()
            .resource::<Assets<StandardMaterial>>()
            .get(&handle)
            .unwrap();
        assert_eq!(after.base_color, Color::linear_rgba(0.25, 0.25, 0.25, 0.25));
        assert_eq!(after.uv_transform, before.uv_transform);
        assert_eq!(world_of(&app, e), pose);
        assert_eq!(app.world().resource::<Cards>().0[&9].card, e, "same entity");

        c.alpha = 0.0;
        set_cards(&mut app, vec![c]);
        app.update();
        let hidden = app.world().get::<Visibility>(e).unwrap();
        assert_eq!(*hidden, Visibility::Hidden, "a faded-out card is not drawn");
    }

    /// The quad is the card's drawn rect, the text's bottom centre is its origin, and the uv is
    /// that rect's place in the atlas.
    #[test]
    fn the_quad_covers_the_drawn_rect_with_the_seat_at_the_origin() {
        let c = card(1, Vec3::ZERO);
        let t = 0.01;
        let q = quad_of(&c, t).unwrap();
        assert_eq!(q.scale.truncate(), Vec2::new(280.0, 190.0) * t);
        // The rect's centre is (150, 115); the seat (150, 200) is 85 texels below it.
        assert!((q.translation.x).abs() < 1e-6);
        assert!((q.translation.y - 0.85).abs() < 1e-6);
        let uv = uv_of(c.rect.unwrap());
        let atlas = ATLAS_SIZE as f32;
        assert_eq!(
            uv.transform_point2(Vec2::ZERO),
            Vec2::new(10.0, 20.0) / atlas
        );
        assert_eq!(
            uv.transform_point2(Vec2::ONE),
            Vec2::new(290.0, 210.0) / atlas
        );
    }

    /// A card whose line ended is taken down with its material, and a lane switched off takes
    /// them all.
    #[test]
    fn a_card_that_ends_is_taken_down() {
        let (mut app, _) = app(Person::Third);
        set_cards(&mut app, vec![card(1, Vec3::X), card(2, Vec3::Z)]);
        app.update();
        assert_eq!(app.world().resource::<Cards>().0.len(), 2);
        set_cards(&mut app, vec![card(2, Vec3::Z)]);
        app.update();
        app.update();
        assert_eq!(app.world().resource::<Cards>().0.len(), 1);
        assert_eq!(
            app.world().resource::<Assets<StandardMaterial>>().len(),
            1,
            "the ended card's material is released"
        );
        app.world_mut().resource_mut::<CardLane>().enabled = false;
        app.update();
        app.update();
        assert!(app.world().resource::<Cards>().0.is_empty());
    }
}
