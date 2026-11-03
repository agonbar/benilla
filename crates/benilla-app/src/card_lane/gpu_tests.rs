//! The card lane on a real adapter: the same quads drawn once to a panel image (the screen view)
//! and once to the atlas (the card view) come out as the same bytes.
//!
//! Needs a Vulkan adapter; a software one (lavapipe) is enough, and the test skips, loudly, where
//! there is none. `CARD_LANE_DUMP=<dir>` writes both images as PNGs there (synthetic frames, not
//! the client's art), the instrument for the ticket's captures.

use std::sync::{Arc, Mutex};

use bevy::asset::RenderAssetUsages;
use bevy::camera::{CameraOutputMode, ClearColorConfig, ImageRenderTarget, RenderTarget};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::render::RenderPlugin;
use bevy::sprite_render::Material2dPlugin;
use bevy::window::{ExitCondition, PrimaryWindow};

use super::{CardLane, CardLanePlugin, CARD_ORIGIN};
use crate::ui_pass::{ui_render_layers, UiQuad, UiQuadMaterial, UiQuads};

const WINDOW: UVec2 = UVec2::new(640, 360);

fn adapter_available() -> bool {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..default()
    });
    bevy::tasks::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).is_ok()
}

/// A bubble's worth of quads at `at`, tagged for `card`: a translucent panel, a checker texture
/// tinted per corner, an additive glow and a rotated clipped chip, over three paint orders.
fn scene(at: Vec2, card: Option<u64>, checker: &Handle<Image>) -> Vec<UiQuad> {
    let rect =
        |x: f32, y: f32, w: f32, h: f32| Rect::new(at.x + x, at.y + y, at.x + x + w, at.y + y + h);
    let base = UiQuad { card, ..default() };
    vec![
        UiQuad {
            rect: rect(0.0, 0.0, 220.0, 100.0),
            z_key: 1,
            color: [0.12, 0.2, 0.55, 0.8],
            ..base.clone()
        },
        UiQuad {
            rect: rect(10.0, 10.0, 64.0, 64.0),
            z_key: 2,
            texture: Some(checker.clone()),
            corner_colors: Some([
                [1.0, 0.3, 0.3, 1.0],
                [0.3, 1.0, 0.3, 1.0],
                [0.3, 0.3, 1.0, 1.0],
                [1.0; 4],
            ]),
            ..base.clone()
        },
        UiQuad {
            rect: rect(90.0, 14.0, 56.0, 56.0),
            z_key: 3,
            color: [1.0, 0.8, 0.2, 0.7],
            additive: true,
            circular: true,
            ..base.clone()
        },
        UiQuad {
            rect: rect(160.0, 20.0, 44.0, 44.0),
            z_key: 4,
            color: [0.9, 0.9, 0.9, 1.0],
            rotation: 0.6,
            clip: Some(rect(150.0, 0.0, 70.0, 56.0)),
            ..base
        },
    ]
}

fn checker() -> Image {
    let mut data = Vec::new();
    for y in 0..8u8 {
        for x in 0..8u8 {
            let on = (x + y) % 2 == 0;
            data.extend_from_slice(&if on {
                [240, 240, 240, 255]
            } else {
                [40, 40, 90, 200]
            });
        }
    }
    let mut img = Image::new(
        Extent3d {
            width: 8,
            height: 8,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    img.sampler = bevy::image::ImageSampler::nearest();
    img
}

fn target(size: UVec2) -> Image {
    let mut img = Image::new_uninit(
        Extent3d {
            width: size.x,
            height: size.y,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.texture_descriptor.usage =
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC;
    img
}

type Shot = Arc<Mutex<Option<Image>>>;

fn shoot(app: &mut App, image: Handle<Image>) -> Shot {
    let shot: Shot = Arc::default();
    let sink = shot.clone();
    app.world_mut()
        .spawn(Screenshot::image(image))
        .observe(move |s: On<ScreenshotCaptured>| {
            *sink.lock().unwrap() = Some(s.image.clone());
        });
    shot
}

fn texel(img: &Image, x: u32, y: u32) -> [u8; 4] {
    let w = img.texture_descriptor.size.width as usize;
    let i = (y as usize * w + x as usize) * 4;
    img.data.as_ref().expect("a readback has data")[i..i + 4]
        .try_into()
        .unwrap()
}

fn dump(dir: &str, name: &str, img: &Image, crop: URect) {
    let (w, h) = (crop.width(), crop.height());
    let mut out = image::RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            out.put_pixel(
                x,
                y,
                image::Rgba(texel(img, crop.min.x + x, crop.min.y + y)),
            );
        }
    }
    std::fs::create_dir_all(dir).unwrap();
    out.save(std::path::Path::new(dir).join(name)).unwrap();
}

/// The card view of a scene is the screen view's, byte for byte.
#[test]
fn a_card_is_the_panels_pixels_for_the_same_quads() {
    if !adapter_available() {
        // Counted by the gates with the data-gated skips, but never a failure: a machine with the
        // install and no adapter has nothing to render this with.
        eprintln!("SKIPPED: no Vulkan adapter (lavapipe would do)");
        if let Some(log) = std::env::var_os("BENILLA_SKIP_LOG").filter(|p| !p.is_empty()) {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log)
            {
                let _ = writeln!(f, "no Vulkan adapter (card lane render)");
            }
        }
        return;
    }
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .build()
            .disable::<bevy::winit::WinitPlugin>()
            .disable::<bevy::log::LogPlugin>()
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..default()
            })
            .set(RenderPlugin {
                synchronous_pipeline_compilation: true,
                ..default()
            }),
    );
    app.add_plugins(crate::shaders::plugin);
    app.add_plugins((
        Material2dPlugin::<UiQuadMaterial>::default(),
        crate::ui_gamma::UiGammaPlugin,
        CardLanePlugin,
    ));
    app.init_resource::<UiQuads>()
        .init_resource::<crate::ui_pass::UiMeshCost>()
        .init_resource::<crate::ui_script::UiCostWanted>()
        .add_systems(Update, crate::ui_pass::rebuild_views());
    while app.plugins_state() == bevy::app::PluginsState::Adding {
        bevy::tasks::tick_global_task_pools_on_main_thread();
    }
    app.finish();
    app.cleanup();

    app.world_mut().spawn((
        Window {
            resolution: bevy::window::WindowResolution::new(WINDOW.x, WINDOW.y),
            ..default()
        },
        PrimaryWindow,
    ));
    let white = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new_fill(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[255; 4],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        ));
    app.insert_resource(crate::ui_pass::UiWhiteTexture(white));

    // The screen view's camera, aimed at an image the size of the window: the panel's image.
    let panel = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(target(WINDOW));
    app.world_mut().spawn((
        Camera2d,
        Msaa::Off,
        Tonemapping::None,
        crate::ui_gamma::UiGammaLane::default(),
        ui_render_layers(),
        RenderTarget::Image(ImageRenderTarget {
            handle: panel.clone(),
            scale_factor: 1.0,
        }),
        Camera {
            order: -1,
            output_mode: CameraOutputMode::Skip,
            clear_color: ClearColorConfig::Custom(Color::NONE),
            msaa_writeback: bevy::camera::MsaaWriteback::Off,
            ..default()
        },
    ));

    let checker = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(checker());
    // The same scene twice: on the screen at `at`, and on the speaker's card, whose cell is the
    // atlas' first, at the same offset from the cell's corner.
    let at = Vec2::new(24.0, 16.0);
    let key = 7;
    app.world_mut().resource_mut::<CardLane>().enabled = true;
    {
        let cell = UVec2::new(320, 240);
        let mut lane = app.world_mut().resource_mut::<CardLane>();
        lane.begin(cell, &[key]);
        lane.cell_of(key).unwrap();
    }
    {
        let mut quads = app.world_mut().resource_mut::<UiQuads>();
        quads.quads = scene(at, None, &checker);
        quads
            .quads
            .extend(scene(at + CARD_ORIGIN, Some(key), &checker));
        quads.dirty = true;
    }
    for _ in 0..30 {
        app.update();
    }
    let atlas = app
        .world()
        .resource::<CardLane>()
        .atlas()
        .expect("the lane built its atlas")
        .clone();
    // `Screenshot::image` returns what renders during the capture: the card camera draws.
    app.world_mut().resource_mut::<CardLane>().raster = true;
    app.world_mut().resource_mut::<UiQuads>().dirty = true;
    let (panel_shot, atlas_shot) = (shoot(&mut app, panel), shoot(&mut app, atlas));
    for _ in 0..60 {
        app.update();
        if panel_shot.lock().unwrap().is_some() && atlas_shot.lock().unwrap().is_some() {
            break;
        }
    }
    let panel = panel_shot
        .lock()
        .unwrap()
        .take()
        .expect("the panel image came back");
    let atlas = atlas_shot
        .lock()
        .unwrap()
        .take()
        .expect("the atlas came back");

    let (a, b) = (at.as_uvec2(), (at + Vec2::new(220.0, 100.0)).as_uvec2());
    let mut drawn = 0;
    for y in a.y..b.y {
        for x in a.x..b.x {
            let (p, c) = (texel(&panel, x, y), texel(&atlas, x, y));
            assert_eq!(p, c, "texel ({x}, {y}): panel {p:?}, card {c:?}");
            drawn += usize::from(p[3] != 0);
        }
    }
    assert!(drawn > 20_000, "the scene drew {drawn} texels");
    // What the card did not draw is still clear: nothing of the screen scene leaked in beside it.
    assert_eq!(texel(&atlas, 600, 300), [0; 4]);

    if let Ok(dir) = std::env::var("CARD_LANE_DUMP") {
        let crop = URect::new(a.x, a.y, b.x, b.y);
        dump(&dir, "panel-lane.png", &panel, crop);
        dump(&dir, "card-lane.png", &atlas, crop);
    }
}
