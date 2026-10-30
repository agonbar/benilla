//! The comfort vignette: a black ring in front of each eye that closes in on the view while the
//! camera moves or smooth-turns ([`crate::comfort::vignette_target`]).
//!
//! The ring is a child of the eye's overlay camera ([`crate::panel::PanelCamera`]), so it rides the
//! head, and it draws on a render layer of its own per eye: both eyes see only their own ring, at
//! the same place in the image, which is what keeps it from doubling. It is unlit and blended, so
//! the overlay's premultiplied blit lays it over the world and the panel alike. It is a mesh of
//! radius 1 at the opening, so the aperture is its scale.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

/// The render layer of eye 0's ring; eye 1's is the next. Above the panel (60) and the mirror (61).
const LAYER_BASE: usize = 62;

/// How far in front of the eye the ring hangs, in metres. Past the near plane (0.1), inside the
/// panel (1.5), so the ring draws over it.
pub const DISTANCE: f32 = 0.35;

/// Segments around the ring.
const SEGMENTS: usize = 64;
/// The ring's soft edge runs from radius 1 (clear) to this (black), in units of the opening.
const SOFT_EDGE: f32 = 1.35;
/// Where the black stops, in units of the opening: far past any view's corner.
const OUTER: f32 = 12.0;

/// The render layer for the ring of eye `index`.
pub fn layer(index: u32) -> usize {
    LAYER_BASE + index as usize
}

/// A ring of the vignette.
#[derive(Component)]
pub(crate) struct Ring;

/// The ring's scale for an opening of half-angle `aperture` radians.
pub fn scale_for(aperture: f32) -> f32 {
    DISTANCE * aperture.tan()
}

/// The ring mesh: clear inside radius 1, black from [`SOFT_EDGE`] out to [`OUTER`], with the alpha
/// ramping between in the vertex colours.
pub fn ring_mesh() -> Mesh {
    let radii = [(1.0, 0.0), (SOFT_EDGE, 1.0), (OUTER, 1.0)];
    let mut positions = Vec::new();
    let mut colours = Vec::new();
    for (radius, alpha) in radii {
        for i in 0..SEGMENTS {
            let a = std::f32::consts::TAU * i as f32 / SEGMENTS as f32;
            positions.push([radius * a.cos(), radius * a.sin(), 0.0]);
            colours.push([0.0, 0.0, 0.0, alpha]);
        }
    }
    let mut indices = Vec::new();
    for band in 0..radii.len() - 1 {
        for i in 0..SEGMENTS {
            let j = (i + 1) % SEGMENTS;
            let (a, b) = ((band * SEGMENTS + i) as u32, (band * SEGMENTS + j) as u32);
            let (c, d) = (a + SEGMENTS as u32, b + SEGMENTS as u32);
            indices.extend([a, b, c, b, d, c]);
        }
    }
    let normals = vec![[0.0, 0.0, 1.0]; positions.len()];
    let uvs = vec![[0.0, 0.0]; positions.len()];
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colours)
    .with_inserted_indices(Indices::U32(indices))
}

/// The material every ring shares: black, unlit, blended, seen from either side.
pub fn material() -> StandardMaterial {
    StandardMaterial {
        base_color: Color::BLACK,
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        double_sided: true,
        fog_enabled: false,
        ..default()
    }
}

/// Hangs the ring for eye `index` on its overlay `camera`, hidden until the vignette shows.
pub fn attach(
    commands: &mut Commands,
    camera: Entity,
    index: u32,
    mesh: &Handle<Mesh>,
    material: &Handle<StandardMaterial>,
) {
    let ring = commands
        .spawn((
            Ring,
            Name::new("vr vignette"),
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            Transform::from_xyz(0.0, 0.0, -DISTANCE),
            Visibility::Hidden,
            RenderLayers::layer(layer(index)),
            NoFrustumCulling,
        ))
        .id();
    commands.entity(camera).add_child(ring);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn positions(mesh: &Mesh) -> Vec<[f32; 3]> {
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|a| a.as_float3())
            .unwrap()
            .to_vec()
    }

    #[test]
    fn the_ring_is_clear_at_radius_one_and_black_from_the_soft_edge_out() {
        let mesh = ring_mesh();
        let pos = positions(&mesh);
        assert_eq!(pos.len(), 3 * SEGMENTS);
        let radius = |p: [f32; 3]| (p[0] * p[0] + p[1] * p[1]).sqrt();
        assert!((radius(pos[0]) - 1.0).abs() < 1e-5);
        assert!((radius(pos[SEGMENTS]) - SOFT_EDGE).abs() < 1e-5);
        assert!((radius(pos[2 * SEGMENTS]) - OUTER).abs() < 1e-4);

        let colours = match mesh.attribute(Mesh::ATTRIBUTE_COLOR) {
            Some(bevy::mesh::VertexAttributeValues::Float32x4(c)) => c.clone(),
            other => panic!("colours: {other:?}"),
        };
        assert!(colours[..SEGMENTS].iter().all(|c| c[3] == 0.0));
        assert!(colours[SEGMENTS..].iter().all(|c| c[3] == 1.0));
        assert!(colours.iter().all(|c| c[..3] == [0.0, 0.0, 0.0]));
    }

    #[test]
    fn the_ring_has_two_bands_of_triangles_and_every_index_is_a_vertex() {
        let mesh = ring_mesh();
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("no indices");
        };
        assert_eq!(indices.len(), 2 * SEGMENTS * 6);
        assert!(indices.iter().all(|&i| (i as usize) < 3 * SEGMENTS));
    }

    #[test]
    fn each_eye_has_a_layer_of_its_own_above_the_panels() {
        assert_ne!(layer(0), layer(1));
        assert!(layer(0) > crate::panel::PANEL_RENDER_LAYER + 1);
    }

    #[test]
    fn the_scale_puts_the_opening_at_the_aperture_angle() {
        let angle = 40.0f32.to_radians();
        let radius = scale_for(angle);
        assert!(((radius / DISTANCE).atan() - angle).abs() < 1e-6);
        // Wider apertures are bigger rings.
        assert!(scale_for(1.2) > scale_for(0.5));
    }
}
