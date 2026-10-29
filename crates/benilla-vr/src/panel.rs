//! The 2D interface on a flat panel in the headset.
//!
//! While a session runs, the player-UI camera draws into an offscreen image the size of the
//! window instead of into the window. Two things show that image:
//!
//! * a mirror camera draws it full-window, so the monitor still shows the UI, and the window stays
//!   the input surface: the UI lays out and hit-tests from the primary window, which the image
//!   matches in size and scale factor, so the mouse stays lined up;
//! * a quad, a child of the tracking root, carries it in front of the player. One overlay camera
//!   per eye draws that quad over the world eye in the same swapchain view.
//!
//! The UI camera keeps its `FfxBackdrop`, with the claim on the world dropped each frame
//! ([`unclaim_world`]): the UI draws over transparent, not over the world. Removing the component
//! would leave the render world's `FfxBackdropView` holding the last claimed world view, which
//! nothing upstream clears.
//!
//! When the session ends, or never starts, every camera, target and order the module changed goes
//! back to what upstream spawned, and nothing is spawned.
//!
//! # Colour
//!
//! The image is `Rgba8UnormSrgb`. [`benilla_app`]'s gamma decode writes it through the sRGB view,
//! so it holds the bytes the reference would have shown, premultiplied by alpha (the quad shader
//! emits `(rgb·a, a)`). Both readers sample it through that same sRGB view, which decodes to
//! linear, and both write an sRGB target (the window surface, the swapchain), which encodes the
//! same value back. Opaque texels therefore land as the same byte on the monitor and in the
//! headset. The mirror reads the image as opaque: premultiplied over black is the colour itself.
//! The panel blends `result = src + (1 - a) · eye` (premultiplied), in linear light, over the
//! world; the reference blends in gamma bytes, so a translucent texel over the world (a docked
//! chat tab) reads a little lighter than on the monitor over a black background.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::camera::{
    CameraOutputMode, ClearColorConfig, ImageRenderTarget, ManualTextureViewHandle, RenderTarget,
    ScalingMode,
};
use bevy::core_pipeline::tonemapping::{DebandDither, Tonemapping};
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{
    BlendState, Extent3d, TextureDimension, TextureFormat, TextureUsages,
};
use bevy::sprite_render::{AlphaMode2d, ColorMaterial};
use bevy::ui::IsDefaultUiCamera;
use bevy::window::{CursorOptions, PrimaryWindow};

use benilla_world::ffx_glow::FfxBackdrop;

use crate::laser::Aimed;
use benilla_world::view::NEARCLIP_DEFAULT;
use bevy_mod_openxr::render::{update_views, XR_TEXTURE_INDEX};
use bevy_mod_openxr::resources::OxrFrameState;
use bevy_mod_xr::camera::{XrCamera, XrProjection};
use bevy_mod_xr::session::{XrState, XrTrackingRoot};

/// How far in front of the tracking root's origin the panel hangs, in the root's units (metres of
/// head motion; the world reads about 9% large against them).
pub const PANEL_DISTANCE: f32 = 1.5;
/// How far the panel's centre sits below the tracking root's origin, which is eye level when the
/// session starts.
pub const PANEL_DROP: f32 = 0.15;
/// The panel's width. Its height follows the window's aspect ratio ([`panel_size`]).
pub const PANEL_WIDTH: f32 = 1.6;
/// Radius of the cursor marker's white disc; the black disc behind it is [`MARKER_RIM`] wider.
pub const MARKER_RADIUS: f32 = 0.009;
/// How much wider the black disc behind the marker is, so the marker reads on any background.
pub const MARKER_RIM: f32 = 0.005;
/// Distance the marker floats in front of the panel, against depth fighting.
const MARKER_LIFT: f32 = 0.004;

/// The panel's render layer, and the mirror's. Upstream's highest is the UI-model cameras' block
/// near 20; these are well above it.
pub const PANEL_RENDER_LAYER: usize = 60;
const MIRROR_RENDER_LAYER: usize = 61;

/// The player-UI camera's order while its target is the image: below the world eyes (0) and the
/// mirror, so the image is drawn before anything reads it.
const UI_IMAGE_ORDER: isize = -1;
/// The mirror camera: above the UI image's camera, below the egui dev overlay (2) on the window.
const MIRROR_ORDER: isize = 1;
/// The overlay cameras: above the world eyes (0) on the same swapchain view.
const OVERLAY_ORDER: isize = 3;

/// An overlay camera, which draws the panel over one eye. It is an [`XrCamera`] so the runtime's
/// pose and projection reach it, and is told apart from the world eyes by this marker.
#[derive(Component)]
pub(crate) struct PanelCamera;

/// The quad that carries the UI image.
#[derive(Component)]
pub(crate) struct PanelScreen;

/// The cursor marker.
#[derive(Component)]
struct CursorMarker;

/// Everything the panel changed or spawned, held while a session runs.
struct Engaged {
    /// The player-UI camera, and what it was before the session.
    ui_camera: Entity,
    saved_target: RenderTarget,
    saved_order: isize,
    /// The UI image, and the physical size and scale factor it was stamped with.
    image: Handle<Image>,
    stamped: (UVec2, f32),
    panel_material: Handle<StandardMaterial>,
    mirror_material: Handle<ColorMaterial>,
    /// The panel, its cameras and the mirror.
    spawned: Vec<Entity>,
}

/// `Some` while the UI is on the panel.
#[derive(Resource, Default)]
struct UiPanel(Option<Engaged>);

/// The size in physical pixels and the scale factor the UI image takes for a window: the window's
/// own numbers, with a minimised window held at one pixel.
pub fn ui_image_spec(physical: UVec2, scale_factor: f32) -> (UVec2, f32) {
    (physical.max(UVec2::ONE), scale_factor)
}

/// The panel's size in the root's units for a window of `aspect` (width over height).
pub fn panel_size(aspect: f32) -> Vec2 {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        16.0 / 9.0
    };
    Vec2::new(PANEL_WIDTH, PANEL_WIDTH / aspect)
}

/// Where the cursor marker sits on a panel of size `panel`, centred on the panel with y up, for a
/// cursor at `cursor` logical pixels (origin top-left, y down) in a window of `window` logical
/// pixels. `None` while the window has no cursor or the OS cursor is hidden (mouse-look).
pub fn marker_position(
    cursor: Option<Vec2>,
    os_cursor_visible: bool,
    window: Vec2,
    panel: Vec2,
) -> Option<Vec2> {
    let cursor = cursor.filter(|_| os_cursor_visible && window.x > 0.0 && window.y > 0.0)?;
    let uv = cursor / window;
    Some(Vec2::new((uv.x - 0.5) * panel.x, (0.5 - uv.y) * panel.y))
}

/// The panel quad in the tracking root's space for a panel of `size`: the panel's place under the
/// root and the quad's scale, which `spawn_panel` and `shape_panel` give its two entities. The quad
/// faces +Z and is a unit square before the scale.
pub fn quad_in_root(size: Vec2) -> Transform {
    Transform::from_xyz(0.0, -PANEL_DROP, -PANEL_DISTANCE).with_scale(size.extend(1.0))
}

/// A fresh UI image at `size` physical pixels: a render target the camera clears each frame and
/// both readers sample, never read back by the CPU.
fn new_ui_image(size: UVec2) -> Image {
    let mut image = Image::new_uninit(
        Extent3d {
            width: size.x.max(1),
            height: size.y.max(1),
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_descriptor.usage =
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING;
    image.sampler = ImageSampler::linear();
    image
}

fn session_running(state: Option<&XrState>) -> bool {
    matches!(state, Some(XrState::Running))
}

pub struct PanelPlugin;

impl Plugin for PanelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiPanel>()
            .init_resource::<Aimed>()
            .add_systems(
                PostUpdate,
                (
                    sync_session,
                    (
                        track_window,
                        unclaim_world,
                        shape_panel,
                        place_marker,
                        activate_overlays,
                    ),
                )
                    .chain()
                    .before(update_views),
            );
    }
}

/// Puts the UI on the panel when a session is running and takes it back when it is not.
#[allow(clippy::too_many_arguments)]
fn sync_session(
    state: Option<Res<XrState>>,
    mut panel: ResMut<UiPanel>,
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut standard: ResMut<Assets<StandardMaterial>>,
    mut colour: ResMut<Assets<ColorMaterial>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    root: Query<Entity, With<XrTrackingRoot>>,
    ui_camera: Query<Entity, (With<FfxBackdrop>, With<IsDefaultUiCamera>)>,
    mut cameras: Query<(&mut RenderTarget, &mut Camera), Without<PanelCamera>>,
) {
    let running = session_running(state.as_deref());
    match (running, panel.0.is_some()) {
        (true, false) => {
            let (Ok(window), Ok(root), Ok(ui)) =
                (windows.single(), root.single(), ui_camera.single())
            else {
                return;
            };
            let Ok((mut target, mut camera)) = cameras.get_mut(ui) else {
                return;
            };
            let (size, factor) = ui_image_spec(
                UVec2::new(window.physical_width(), window.physical_height()),
                window.resolution.scale_factor(),
            );
            let image = images.add(new_ui_image(size));
            let panel_material = standard.add(StandardMaterial {
                base_color_texture: Some(image.clone()),
                unlit: true,
                // The image holds colour premultiplied by alpha: the shader passes it through
                // and the blend adds it, `src + (1 - a) · dst`.
                alpha_mode: AlphaMode::Premultiplied,
                cull_mode: None,
                double_sided: true,
                fog_enabled: false,
                ..default()
            });
            let mirror_material = colour.add(ColorMaterial {
                texture: Some(image.clone()),
                // Opaque: premultiplied over black is the colour itself, what the monitor shows.
                alpha_mode: AlphaMode2d::Opaque,
                ..default()
            });

            let saved_target = std::mem::replace(&mut *target, image_target(&image, factor));
            let saved_order = std::mem::replace(&mut camera.order, UI_IMAGE_ORDER);

            let mut spawned = Vec::new();
            let aspect = window.width() / window.height().max(1.0);
            let screen = spawn_panel(
                &mut commands,
                root,
                &mut meshes,
                &mut standard,
                &panel_material,
                panel_size(aspect),
            );
            spawned.push(screen);
            for index in 0..2 {
                spawned.push(spawn_overlay(&mut commands, index));
            }
            spawned.push(spawn_mirror(&mut commands, &mut meshes, &mirror_material));

            panel.0 = Some(Engaged {
                ui_camera: ui,
                saved_target,
                saved_order,
                image,
                stamped: (size, factor),
                panel_material,
                mirror_material,
                spawned,
            });
        }
        (false, true) => {
            let Some(engaged) = panel.0.take() else {
                return;
            };
            if let Ok((mut target, mut camera)) = cameras.get_mut(engaged.ui_camera) {
                *target = engaged.saved_target;
                camera.order = engaged.saved_order;
            }
            for entity in engaged.spawned {
                commands.entity(entity).try_despawn();
            }
            images.remove(&engaged.image);
            standard.remove(&engaged.panel_material);
            colour.remove(&engaged.mirror_material);
        }
        _ => {}
    }
}

fn image_target(image: &Handle<Image>, scale_factor: f32) -> RenderTarget {
    RenderTarget::Image(ImageRenderTarget {
        handle: image.clone(),
        scale_factor,
    })
}

/// The panel and its cursor marker, a child of the tracking root so it follows the character and
/// not the head. Returns the panel's root entity; its children despawn with it.
fn spawn_panel(
    commands: &mut Commands,
    root: Entity,
    meshes: &mut Assets<Mesh>,
    standard: &mut Assets<StandardMaterial>,
    material: &Handle<StandardMaterial>,
    size: Vec2,
) -> Entity {
    let layer = RenderLayers::layer(PANEL_RENDER_LAYER);
    let unlit = |standard: &mut Assets<StandardMaterial>, colour: Color| {
        standard.add(StandardMaterial {
            base_color: colour,
            unlit: true,
            fog_enabled: false,
            ..default()
        })
    };
    let rim = unlit(standard, Color::BLACK);
    let dot = unlit(standard, Color::WHITE);
    let rim_mesh = meshes.add(Circle::new(MARKER_RADIUS + MARKER_RIM));
    let dot_mesh = meshes.add(Circle::new(MARKER_RADIUS));
    let quad = meshes.add(Rectangle::new(1.0, 1.0));

    let panel = commands
        .spawn((
            Name::new("vr panel"),
            Transform::from_xyz(0.0, -PANEL_DROP, -PANEL_DISTANCE),
            Visibility::default(),
        ))
        .with_children(|p| {
            p.spawn((
                PanelScreen,
                Mesh3d(quad),
                MeshMaterial3d(material.clone()),
                Transform::from_scale(size.extend(1.0)),
                layer.clone(),
                NoFrustumCulling,
            ));
            p.spawn((
                CursorMarker,
                Transform::from_xyz(0.0, 0.0, MARKER_LIFT),
                Visibility::Hidden,
            ))
            .with_children(|m| {
                m.spawn((
                    Mesh3d(rim_mesh),
                    MeshMaterial3d(rim),
                    layer.clone(),
                    NoFrustumCulling,
                ));
                m.spawn((
                    Mesh3d(dot_mesh),
                    MeshMaterial3d(dot),
                    Transform::from_xyz(0.0, 0.0, MARKER_LIFT),
                    layer.clone(),
                    NoFrustumCulling,
                ));
            });
        })
        .id();
    commands.entity(root).add_child(panel);
    panel
}

/// One overlay camera on the swapchain view of eye `index`: it draws only the panel's layer,
/// above the world eye, non-HDR, with no tonemapping, no dither and no FFX component, so none of
/// the world's grading touches the panel. Its main texture clears to transparent, and the blit
/// into the swapchain blends premultiplied, `src + (1 - a) · eye`. That blit clears the view to
/// black only when it is the first write of the frame (the world eye is dark behind the glue
/// screens); otherwise it loads what the world eye drew.
fn spawn_overlay(commands: &mut Commands, index: u32) -> Entity {
    commands
        .spawn((
            PanelCamera,
            Name::new("vr panel camera"),
            XrCamera(index),
            Projection::custom(XrProjection {
                near: NEARCLIP_DEFAULT,
                ..default()
            }),
            RenderTarget::TextureView(ManualTextureViewHandle(XR_TEXTURE_INDEX + index)),
            RenderLayers::layer(PANEL_RENDER_LAYER),
            Msaa::Off,
            Tonemapping::None,
            DebandDither::Disabled,
            bevy::light::cluster::ClusterConfig::None,
            Camera {
                order: OVERLAY_ORDER,
                is_active: false,
                clear_color: ClearColorConfig::Custom(Color::NONE),
                msaa_writeback: bevy::camera::MsaaWriteback::Off,
                output_mode: CameraOutputMode::Write {
                    blend_state: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    clear_color: ClearColorConfig::Custom(Color::BLACK),
                },
                ..default()
            },
        ))
        .id()
}

/// The camera that draws the UI image on the desktop window: a 2×2 quad under a 2×2 orthographic
/// view fills the window whatever its size. Returns the camera; the quad is its child.
fn spawn_mirror(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    material: &Handle<ColorMaterial>,
) -> Entity {
    let layer = RenderLayers::layer(MIRROR_RENDER_LAYER);
    let mut projection = OrthographicProjection::default_2d();
    projection.scaling_mode = ScalingMode::Fixed {
        width: 2.0,
        height: 2.0,
    };
    let quad = meshes.add(Rectangle::new(2.0, 2.0));
    commands
        .spawn((
            Name::new("vr mirror camera"),
            Camera2d,
            Projection::Orthographic(projection),
            Msaa::Off,
            layer.clone(),
            Camera {
                order: MIRROR_ORDER,
                clear_color: ClearColorConfig::Custom(Color::BLACK),
                ..default()
            },
        ))
        .with_children(|c| {
            c.spawn((
                Mesh2d(quad),
                MeshMaterial2d(material.clone()),
                // In front of the camera's near plane in a 2D view.
                Transform::from_xyz(0.0, 0.0, -1.0),
                layer,
            ));
        })
        .id()
}

/// Keeps the image at the window's physical size and scale factor, so the UI lays out and
/// hit-tests as it does with no headset. A resize publishes a new asset and removes the old one,
/// never writing through the old handle: `AssetId<Image>` names a GPU texture.
fn track_window(
    mut panel: ResMut<UiPanel>,
    mut images: ResMut<Assets<Image>>,
    mut standard: ResMut<Assets<StandardMaterial>>,
    mut colour: ResMut<Assets<ColorMaterial>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut cameras: Query<&mut RenderTarget>,
) {
    let (Some(engaged), Ok(window)) = (panel.0.as_mut(), windows.single()) else {
        return;
    };
    let want = ui_image_spec(
        UVec2::new(window.physical_width(), window.physical_height()),
        window.resolution.scale_factor(),
    );
    if want == engaged.stamped {
        return;
    }
    if want.0 != engaged.stamped.0 {
        let retired = std::mem::replace(&mut engaged.image, images.add(new_ui_image(want.0)));
        images.remove(&retired);
        if let Some(material) = standard.get_mut(&engaged.panel_material) {
            material.base_color_texture = Some(engaged.image.clone());
        }
        if let Some(material) = colour.get_mut(&engaged.mirror_material) {
            material.texture = Some(engaged.image.clone());
        }
    }
    engaged.stamped = want;
    if let Ok(mut target) = cameras.get_mut(engaged.ui_camera) {
        *target = image_target(&engaged.image, want.1);
    }
}

/// Drops the UI camera's claim on the world camera after `claim_backdrop` (upstream, `Update`)
/// has set it, before the frame is extracted: with no source the camera's own clear, transparent,
/// is the ground. Writes only when the claim is set.
fn unclaim_world(panel: Res<UiPanel>, mut backdrops: Query<&mut FfxBackdrop>) {
    let Some(engaged) = panel.0.as_ref() else {
        return;
    };
    if let Ok(mut backdrop) = backdrops.get_mut(engaged.ui_camera) {
        if backdrop.source.is_some() {
            backdrop.source = None;
        }
    }
}

/// The panel takes the window's aspect ratio.
fn shape_panel(
    windows: Query<&Window, With<PrimaryWindow>>,
    mut screen: Query<&mut Transform, With<PanelScreen>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let size = panel_size(window.width() / window.height().max(1.0)).extend(1.0);
    for mut transform in &mut screen {
        if transform.scale != size {
            transform.scale = size;
        }
    }
}

/// The cursor marker follows the laser where it hits the panel, else `Window::cursor_position`,
/// and hides with the OS cursor.
fn place_marker(
    windows: Query<(&Window, &CursorOptions), With<PrimaryWindow>>,
    screen: Query<&Transform, (With<PanelScreen>, Without<CursorMarker>)>,
    aimed: Res<Aimed>,
    mut marker: Query<(&mut Transform, &mut Visibility), With<CursorMarker>>,
) {
    let (Ok((window, cursor)), Ok(screen)) = (windows.single(), screen.single()) else {
        return;
    };
    let laser = aimed.hit.map(|h| h.cursor);
    let at = marker_position(
        laser.or(window.cursor_position()),
        laser.is_some() || cursor.visible,
        Vec2::new(window.width(), window.height()),
        screen.scale.truncate(),
    );
    for (mut transform, mut visibility) in &mut marker {
        match at {
            Some(at) => {
                transform.translation = at.extend(MARKER_LIFT);
                *visibility = Visibility::Inherited;
            }
            None => *visibility = Visibility::Hidden,
        }
    }
}

/// The overlay cameras draw whenever the headset wants a frame; the glue screens have no world,
/// and the panel is all there is.
fn activate_overlays(
    frame: Option<Res<OxrFrameState>>,
    mut overlays: Query<&mut Camera, With<PanelCamera>>,
) {
    let active = frame.is_none_or(|f| f.should_render);
    for mut camera in &mut overlays {
        if camera.is_active != active {
            camera.is_active = active;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::window::WindowResolution;

    use crate::laser::PanelHit;

    const START_ORDER: isize = 1;

    fn app(state: XrState) -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(AssetPlugin::default())
            .init_asset::<Image>()
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_asset::<ColorMaterial>()
            .init_resource::<UiPanel>()
            .init_resource::<Aimed>()
            .insert_resource(state);
        app.world_mut().spawn((
            Window {
                resolution: WindowResolution::new(1280, 720).with_scale_factor_override(1.5),
                ..default()
            },
            CursorOptions::default(),
            PrimaryWindow,
        ));
        app.world_mut().spawn(XrTrackingRoot);
        let ui = app
            .world_mut()
            .spawn((
                FfxBackdrop::default(),
                IsDefaultUiCamera,
                Camera {
                    order: START_ORDER,
                    ..default()
                },
                RenderTarget::default(),
            ))
            .id();
        let window = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .unwrap();
        (app, ui, window)
    }

    fn step(app: &mut App) {
        app.world_mut().run_system_once(sync_session).unwrap();
        app.world_mut().run_system_once(track_window).unwrap();
        app.update();
    }

    fn count<F: bevy::ecs::query::QueryFilter>(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<(), F>()
            .iter(app.world())
            .count()
    }

    fn alive(app: &mut App) -> usize {
        app.world_mut().query::<Entity>().iter(app.world()).count()
    }

    fn image_of(app: &App, ui: Entity) -> (Handle<Image>, f32) {
        match app.world().get::<RenderTarget>(ui) {
            Some(RenderTarget::Image(t)) => (t.handle.clone(), t.scale_factor),
            other => panic!("the UI camera does not target an image: {other:?}"),
        }
    }

    #[test]
    fn the_image_has_the_windows_physical_size_and_scale_factor() {
        let (mut app, ui, _) = app(XrState::Running);
        step(&mut app);

        let (handle, factor) = image_of(&app, ui);
        assert_eq!(factor, 1.5);
        let image = app
            .world()
            .resource::<Assets<Image>>()
            .get(&handle)
            .unwrap();
        assert_eq!(image.texture_descriptor.size.width, 1280);
        assert_eq!(image.texture_descriptor.size.height, 720);
        assert_eq!(
            image.texture_descriptor.format,
            TextureFormat::Rgba8UnormSrgb
        );
    }

    #[test]
    fn the_ui_draws_over_transparent_not_over_the_world() {
        let (mut app, ui, _) = app(XrState::Running);
        step(&mut app);
        // `claim_backdrop` (upstream, `Update`) claims the drawing world camera every frame.
        let world_camera = app.world_mut().spawn_empty().id();
        app.world_mut().get_mut::<FfxBackdrop>(ui).unwrap().source = Some(world_camera);
        app.world_mut().run_system_once(unclaim_world).unwrap();
        assert_eq!(app.world().get::<FfxBackdrop>(ui).unwrap().source, None);
    }

    #[test]
    fn a_resize_rebuilds_the_image_as_a_new_asset_and_every_reader_follows() {
        let (mut app, ui, window) = app(XrState::Running);
        step(&mut app);
        let (old, _) = image_of(&app, ui);

        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution
            .set_physical_resolution(2400, 1000);
        step(&mut app);

        let (new, factor) = image_of(&app, ui);
        assert_ne!(new.id(), old.id());
        assert_eq!(factor, 1.5);
        let images = app.world().resource::<Assets<Image>>();
        assert!(images.get(&old).is_none(), "the retired texture is removed");
        let size = images.get(&new).unwrap().texture_descriptor.size;
        assert_eq!((size.width, size.height), (2400, 1000));
        let panel = app.world().resource::<UiPanel>().0.as_ref().unwrap();
        let standard = app.world().resource::<Assets<StandardMaterial>>();
        assert_eq!(
            standard
                .get(&panel.panel_material)
                .unwrap()
                .base_color_texture,
            Some(new.clone())
        );
        let colour = app.world().resource::<Assets<ColorMaterial>>();
        assert_eq!(
            colour.get(&panel.mirror_material).unwrap().texture,
            Some(new)
        );
    }

    #[test]
    fn the_panel_size_follows_the_aspect_ratio() {
        let wide = panel_size(16.0 / 9.0);
        assert_eq!(wide.x, PANEL_WIDTH);
        assert!((wide.y - PANEL_WIDTH * 9.0 / 16.0).abs() < 1e-6);
        let square = panel_size(1.0);
        assert_eq!(square, Vec2::splat(PANEL_WIDTH));
        assert!(panel_size(21.0 / 9.0).y < wide.y);
        // A window with no height yet must not make the panel infinite or flat.
        assert_eq!(panel_size(f32::INFINITY).x, PANEL_WIDTH);
        assert!(panel_size(0.0).y > 0.0);
    }

    #[test]
    fn the_cursor_maps_onto_the_panel() {
        let window = Vec2::new(1280.0, 720.0);
        let panel = Vec2::new(1.6, 0.9);
        let at = |c: Vec2| marker_position(Some(c), true, window, panel).unwrap();
        assert_eq!(at(window / 2.0), Vec2::ZERO);
        assert_eq!(at(Vec2::ZERO), Vec2::new(-0.8, 0.45));
        assert_eq!(at(window), Vec2::new(0.8, -0.45));
        assert_eq!(at(Vec2::new(960.0, 180.0)), Vec2::new(0.4, 0.225));
    }

    #[test]
    fn the_marker_hides_with_the_os_cursor_and_outside_the_window() {
        let (window, panel) = (Vec2::new(1280.0, 720.0), Vec2::new(1.6, 0.9));
        assert_eq!(marker_position(Some(Vec2::ONE), false, window, panel), None);
        assert_eq!(marker_position(None, true, window, panel), None);
    }

    #[test]
    fn the_marker_system_follows_the_window_cursor() {
        let (mut app, _, window) = app(XrState::Running);
        step(&mut app);
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .set_cursor_position(Some(Vec2::new(0.0, 0.0)));
        app.world_mut().run_system_once(shape_panel).unwrap();
        app.world_mut().run_system_once(place_marker).unwrap();
        app.update();

        let (transform, visibility) = app
            .world_mut()
            .query_filtered::<(&Transform, &Visibility), With<CursorMarker>>()
            .single(app.world())
            .unwrap();
        let panel = panel_size(1280.0 / 720.0);
        assert_eq!(
            transform.translation.truncate(),
            Vec2::new(-panel.x / 2.0, panel.y / 2.0)
        );
        assert_eq!(*visibility, Visibility::Inherited);

        app.world_mut()
            .get_mut::<CursorOptions>(window)
            .unwrap()
            .visible = false;
        app.world_mut().run_system_once(place_marker).unwrap();
        let visibility = app
            .world_mut()
            .query_filtered::<&Visibility, With<CursorMarker>>()
            .single(app.world())
            .unwrap();
        assert_eq!(*visibility, Visibility::Hidden);
    }

    #[test]
    fn the_marker_follows_the_laser_even_with_the_os_cursor_hidden() {
        let (mut app, _, window) = app(XrState::Running);
        step(&mut app);
        app.world_mut()
            .get_mut::<CursorOptions>(window)
            .unwrap()
            .visible = false;
        app.world_mut().run_system_once(shape_panel).unwrap();
        // Three quarters across and a quarter down the window, whatever its scale factor makes it.
        let logical = {
            let w = app.world().get::<Window>(window).unwrap();
            Vec2::new(w.width(), w.height())
        };
        *app.world_mut().resource_mut::<Aimed>() = Aimed {
            tracked: true,
            hit: Some(PanelHit {
                cursor: logical * Vec2::new(0.75, 0.25),
                local: Vec2::ZERO,
                distance: 1.5,
            }),
        };
        app.world_mut().run_system_once(place_marker).unwrap();

        let (transform, visibility) = app
            .world_mut()
            .query_filtered::<(&Transform, &Visibility), With<CursorMarker>>()
            .single(app.world())
            .unwrap();
        let panel = panel_size(1280.0 / 720.0);
        assert_eq!(
            transform.translation.truncate(),
            Vec2::new(0.25 * panel.x, 0.25 * panel.y)
        );
        assert_eq!(*visibility, Visibility::Inherited);

        // The laser lets go and the OS cursor is still hidden: so is the marker.
        *app.world_mut().resource_mut::<Aimed>() = Aimed::default();
        app.world_mut().run_system_once(place_marker).unwrap();
        let visibility = app
            .world_mut()
            .query_filtered::<&Visibility, With<CursorMarker>>()
            .single(app.world())
            .unwrap();
        assert_eq!(*visibility, Visibility::Hidden);
    }

    #[test]
    fn a_session_that_does_not_run_changes_nothing() {
        for state in [XrState::Unavailable, XrState::Idle] {
            let (mut app, ui, _) = app(state);
            let before = alive(&mut app);
            step(&mut app);
            step(&mut app);

            assert!(app.world().entity(ui).contains::<FfxBackdrop>());
            assert!(matches!(
                app.world().get::<RenderTarget>(ui),
                Some(RenderTarget::Window(_))
            ));
            assert_eq!(app.world().get::<Camera>(ui).unwrap().order, START_ORDER);
            assert_eq!(alive(&mut app), before, "nothing is spawned");
            assert!(app.world().resource::<UiPanel>().0.is_none());
            assert!(app.world().resource::<Assets<Image>>().is_empty());
        }
    }

    #[test]
    fn the_panel_is_one_quad_under_the_root_with_a_camera_per_eye() {
        let (mut app, _, _) = app(XrState::Running);
        step(&mut app);

        assert_eq!(count::<With<PanelScreen>>(&mut app), 1);
        assert_eq!(count::<With<PanelCamera>>(&mut app), 2);
        assert_eq!(count::<With<CursorMarker>>(&mut app), 1);
        let screen = app
            .world_mut()
            .query_filtered::<Entity, With<PanelScreen>>()
            .single(app.world())
            .unwrap();
        let panel = app.world().get::<ChildOf>(screen).unwrap().parent();
        let root = app.world().get::<ChildOf>(panel).unwrap().parent();
        assert!(app.world().entity(root).contains::<XrTrackingRoot>());
        let at = app.world().get::<Transform>(panel).unwrap().translation;
        assert_eq!(at, Vec3::new(0.0, -PANEL_DROP, -PANEL_DISTANCE));
        // The quad the laser hits is the quad the two entities compose.
        let (panel_t, screen_t) = (
            *app.world().get::<Transform>(panel).unwrap(),
            *app.world().get::<Transform>(screen).unwrap(),
        );
        assert_eq!(
            panel_t.mul_transform(screen_t),
            quad_in_root(panel_size(1280.0 / 720.0))
        );
        let layers = app.world().get::<RenderLayers>(screen).unwrap();
        assert_eq!(*layers, RenderLayers::layer(PANEL_RENDER_LAYER));
    }

    #[test]
    fn the_end_of_the_session_gives_upstream_back_what_it_spawned() {
        let (mut app, ui, _) = app(XrState::Running);
        step(&mut app);
        let world_camera = app.world_mut().spawn_empty().id();
        app.world_mut().get_mut::<FfxBackdrop>(ui).unwrap().source = Some(world_camera);
        let entities = alive(&mut app);

        app.insert_resource(XrState::Idle);
        step(&mut app);

        app.world_mut().run_system_once(unclaim_world).unwrap();
        assert_eq!(
            app.world().get::<FfxBackdrop>(ui).unwrap().source,
            Some(world_camera),
            "upstream's claim is left alone"
        );
        assert!(matches!(
            app.world().get::<RenderTarget>(ui),
            Some(RenderTarget::Window(_))
        ));
        assert_eq!(app.world().get::<Camera>(ui).unwrap().order, START_ORDER);
        assert_eq!(count::<With<PanelScreen>>(&mut app), 0);
        assert_eq!(count::<With<PanelCamera>>(&mut app), 0);
        assert_eq!(count::<With<Camera2d>>(&mut app), 0);
        assert!(alive(&mut app) < entities);
        assert!(app.world().resource::<Assets<Image>>().is_empty());
    }
}
