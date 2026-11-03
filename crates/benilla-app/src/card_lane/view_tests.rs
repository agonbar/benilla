//! The card view against the screen view, on the same quads: what `rebuild_ui_mesh::<true>` builds
//! for the cards and what `rebuild_ui_mesh::<false>` builds for the screen.

use bevy::camera::visibility::RenderLayers;
use bevy::math::Rect;
use bevy::mesh::{Indices, Mesh, MeshTag};
use bevy::prelude::*;

use crate::ui_pass::tests::{material_state, rebuild_app};
use crate::ui_pass::{rebuild_ui_mesh, ui_render_layers, UiQuad, UiQuadMaterial, UiQuads, UvRect};

fn lane_on() -> crate::card_lane::CardLane {
    crate::card_lane::CardLane {
        enabled: true,
        ..default()
    }
}

/// A bubble's quads, the one scene both views draw: solid, textured, tinted per corner,
/// additive, circular, desaturated, rotated and clipped, over three paint orders.
fn bubble_scene(at: Vec2, card: Option<u64>) -> Vec<UiQuad> {
    let rect =
        |x: f32, y: f32, w: f32, h: f32| Rect::new(at.x + x, at.y + y, at.x + x + w, at.y + y + h);
    let base = UiQuad { card, ..default() };
    vec![
        UiQuad {
            rect: rect(0.0, 0.0, 200.0, 80.0),
            z_key: 1,
            color: [0.1, 0.2, 0.3, 0.9],
            ..base.clone()
        },
        UiQuad {
            rect: rect(8.0, 8.0, 64.0, 32.0),
            z_key: 2,
            texture: Some(Handle::default()),
            uv: UvRect::from_rect(Rect::new(0.0, 0.25, 1.0, 0.75)),
            corner_colors: Some([
                [1.0, 0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0, 1.0],
                [0.0, 0.0, 1.0, 1.0],
                [1.0; 4],
            ]),
            ..base.clone()
        },
        UiQuad {
            rect: rect(80.0, 8.0, 48.0, 48.0),
            z_key: 2,
            texture: Some(Handle::default()),
            additive: true,
            circular: true,
            desaturated: true,
            ..base.clone()
        },
        UiQuad {
            rect: rect(140.0, 8.0, 40.0, 40.0),
            z_key: 3,
            rotation: 0.5,
            clip: Some(rect(150.0, 0.0, 60.0, 30.0)),
            ..base
        },
    ]
}

/// Every batch the app drew, by paint order: the mesh's vertices, the material's state and the
/// run's tint.
#[allow(clippy::type_complexity)]
fn drawn(
    app: &mut App,
) -> Vec<(
    f32,
    Vec<[f32; 3]>,
    Vec<[f32; 2]>,
    Vec<[f32; 4]>,
    Vec<u32>,
    String,
    u32,
)> {
    let mut out = Vec::new();
    let mut q = app.world_mut().query::<(
        &Mesh2d,
        &MeshMaterial2d<UiQuadMaterial>,
        &MeshTag,
        &Transform,
    )>();
    for (mesh, material, tag, tf) in q.iter(app.world()) {
        let mesh = app.world().resource::<Assets<Mesh>>().get(&mesh.0).unwrap();
        let material = app
            .world()
            .resource::<Assets<UiQuadMaterial>>()
            .get(&material.0)
            .unwrap();
        let float3 = match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
            Some(bevy::mesh::VertexAttributeValues::Float32x3(v)) => v.clone(),
            other => panic!("{other:?}"),
        };
        let float2 = match mesh.attribute(Mesh::ATTRIBUTE_UV_0) {
            Some(bevy::mesh::VertexAttributeValues::Float32x2(v)) => v.clone(),
            other => panic!("{other:?}"),
        };
        let float4 = match mesh.attribute(Mesh::ATTRIBUTE_COLOR) {
            Some(bevy::mesh::VertexAttributeValues::Float32x4(v)) => v.clone(),
            other => panic!("{other:?}"),
        };
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("indices")
        };
        let state = material_state(material);
        out.push((
            tf.translation.z,
            float3,
            float2,
            float4,
            indices.clone(),
            state,
            tag.0,
        ));
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// The card view builds what the screen view builds for the same quads, apart from where it
/// puts them: the same runs, meshes (uv, tint, winding, clip, rotation), materials and run
/// tints, every vertex moved by one offset that only the two views' origins decide (to within the f32
/// step of a quad seated far out, 2⁻⁹ px). With the
/// atlas drawn by the same shader and the same gamma decode, a card's texels are the panel's.
#[test]
fn the_card_view_builds_the_screens_batches_for_the_same_quads() {
    let at = Vec2::new(20.0, 12.0);
    let mut screen = rebuild_app();
    {
        let mut q = screen.world_mut().resource_mut::<UiQuads>();
        q.quads = bubble_scene(at, None);
        q.dirty = true;
    }
    screen.update();

    let mut card = rebuild_app();
    card.insert_resource(lane_on()).add_systems(
        Update,
        rebuild_ui_mesh::<true>.before(rebuild_ui_mesh::<false>),
    );
    card.world_mut().resource_mut::<UiQuads>().quads =
        bubble_scene(at + crate::card_lane::CARD_ORIGIN, Some(1));
    card.world_mut().resource_mut::<UiQuads>().dirty = true;
    card.update();

    let (on_screen, on_card) = (drawn(&mut screen), drawn(&mut card));
    // The screen view runs in the card app too and skips the tagged quads, so what is drawn
    // there is the card view's.
    assert!(on_screen.len() >= 3, "{} batches", on_screen.len());
    assert_eq!(on_screen.len(), on_card.len());
    // The screen view maps px to world about the window's centre, the card view about the
    // atlas' centre from `CARD_ORIGIN`, one px a unit in both.
    let half = crate::card_lane::ATLAS_SIZE as f32 * 0.5;
    let (win_w, win_h) = (1280.0, 720.0);
    let delta = Vec2::new((win_w * 0.5) - half, half - win_h * 0.5);
    for (a, b) in on_screen.iter().zip(&on_card) {
        assert_eq!(a.5, b.5, "the material");
        assert_eq!(a.6, b.6, "the run's tint");
        assert_eq!(a.4, b.4, "the triangles");
        assert_eq!(a.2, b.2, "the uvs");
        assert_eq!(a.3, b.3, "the vertex tints");
        assert_eq!(a.1.len(), b.1.len());
        for (p, q) in a.1.iter().zip(&b.1) {
            // A quad seated 16384 px out has an f32 step of 2⁻⁹ px there.
            assert!((q[0] - p[0] - delta.x).abs() < 0.005, "{p:?} -> {q:?}");
            assert!((q[1] - p[1] - delta.y).abs() < 0.005, "{p:?} -> {q:?}");
            assert_eq!(q[2], p[2]);
        }
    }
}

/// Both views in one app split the quads by the tag: the screen takes the untagged, the card
/// view the tagged, and a lane that is off draws none and keeps no batch.
#[test]
fn each_view_draws_only_its_own_quads() {
    let mut app = rebuild_app();
    app.add_systems(
        Update,
        rebuild_ui_mesh::<true>.before(rebuild_ui_mesh::<false>),
    );
    app.insert_resource(lane_on());
    let mut mixed = bubble_scene(Vec2::ZERO, None);
    mixed.extend(bubble_scene(crate::card_lane::CARD_ORIGIN, Some(3)));
    {
        let mut q = app.world_mut().resource_mut::<UiQuads>();
        q.quads = mixed;
        q.dirty = true;
    }
    app.update();
    let layers = |app: &mut App, want: RenderLayers| {
        app.world_mut()
            .query::<&RenderLayers>()
            .iter(app.world())
            .filter(|l| **l == want)
            .count()
    };
    let (screen, card) = (
        layers(&mut app, ui_render_layers()),
        layers(&mut app, crate::card_lane::card_render_layers()),
    );
    assert!(screen >= 3 && card >= 3, "{screen} and {card}");
    assert_eq!(screen, card, "the same scene either side");

    app.world_mut()
        .resource_mut::<crate::card_lane::CardLane>()
        .enabled = false;
    app.world_mut().resource_mut::<UiQuads>().dirty = true;
    app.update();
    app.update();
    assert_eq!(layers(&mut app, crate::card_lane::card_render_layers()), 0);
}
