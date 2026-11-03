//! The card lane: a headset crate asks for each chat bubble to be drawn to an atlas cell instead of
//! the screen, and shows the cell as a quad in the world. The reference draws a bubble on the
//! screen (`0x4b0c30`); 1.12.1 has no headset, so none of this is in the reference.
//!
//! With [`CardLane::enabled`] set, three things change and nothing else does:
//!
//! * The bubble drive ([`crate::chat_bubble`]) seats each bubble at its speaker's cell, in a
//!   virtual window of the atlas' size that starts at [`CARD_ORIGIN`], outside any real window,
//!   and publishes a [`Card`] with the speaker's world anchor.
//! * The script extract tags the bubble's quads with the speaker ([`crate::ui_pass::UiQuad::card`]).
//!   The screen view of [`crate::ui_pass`] skips them, the card view draws them into the atlas, in
//!   the pass' own colour path and gamma decode ([`crate::ui_gamma::UiGammaLane`]), so a card is the
//!   panel's pixels for the same frames.
//! * The atlas and its camera exist, and the camera runs only on a frame a card's quads changed,
//!   which a fade does not: the fade is the card's alpha, carried by [`Card::alpha`].
//!
//! With it off, no atlas, camera, batch or card exists.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{CameraOutputMode, ClearColorConfig, ImageRenderTarget, RenderTarget};
use bevy::image::{Image, ImageSampler};
use bevy::math::Rect;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};

use crate::ui_pass::UiQuad;

/// The atlas' side in texels, one texel a logical px of the virtual window.
pub const ATLAS_SIZE: u32 = 2048;

/// Where the atlas' top-left corner sits in the virtual window the cards are laid out in, far past
/// any real window so the bubble frames there take no mouse and no flat quad lands on them.
pub const CARD_ORIGIN: Vec2 = Vec2::new(16384.0, 16384.0);

/// The cards' render layer: the card camera's and its batches', disjoint from the player UI's (1)
/// and from the headset's panel, mirror and vignette layers (60 and up).
const CARD_RENDER_LAYER: usize = 58;

/// The card camera's order, before the headset's UI image camera (-1) that reads nothing of it.
const CARD_CAMERA_ORDER: isize = -2;

/// The gap kept inside a cell on every side, so a border piece never meets a neighbour's.
const CELL_PAD: f32 = 4.0;

pub(crate) fn card_render_layers() -> RenderLayers {
    RenderLayers::layer(CARD_RENDER_LAYER)
}

/// One speaker's bubble, as the headset crate needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    /// The speaker's GUID.
    pub key: u64,
    /// The world point the bubble hangs from: unit z + height + 0.7 (`0x4b0c30`), in the game's
    /// world space.
    pub anchor: Vec3,
    /// The fade, 0 to 1; the card's own frame stays at 1.
    pub alpha: f32,
    /// The line is shown: false while an out-of-range or undrawn speaker's bubble is hidden.
    pub shown: bool,
    /// The speaker is the local player.
    pub own: bool,
    /// The text's bottom centre in atlas texels, the point that sits on [`Self::anchor`].
    pub seat: Vec2,
    /// The bounds of the card's drawn quads in atlas texels (y down), once they have been drawn.
    pub rect: Option<Rect>,
}

/// The lane's state. A headset crate sets [`Self::enabled`] and reads [`Self::atlas`] and
/// [`Self::cards`]; the app owns the rest.
#[derive(Resource, Default)]
pub struct CardLane {
    /// Draw bubbles to cards. Set by the headset crate while it shows them.
    pub enabled: bool,
    /// This frame's live bubbles, farthest from the camera first.
    pub cards: Vec<Card>,
    atlas: Option<Handle<Image>>,
    /// The cell size the slots were cut for; a window resize changes it and re-seats every card.
    cell: UVec2,
    slots: Vec<Option<u64>>,
    /// The tagged quads the atlas was last drawn from.
    last: Vec<UiQuad>,
    rects: HashMap<u64, Rect>,
    /// The card camera must run this frame.
    raster: bool,
}

impl CardLane {
    /// The atlas image, once it exists (a frame after [`Self::enabled`] is set).
    pub fn atlas(&self) -> Option<&Handle<Image>> {
        self.atlas.as_ref()
    }

    fn cols(&self) -> u32 {
        ATLAS_SIZE / self.cell.x.max(1)
    }

    /// Cut the atlas into cells of `cell` texels and free the cells of speakers not in `live`.
    pub(crate) fn begin(&mut self, cell: UVec2, live: &[u64]) {
        if cell != self.cell {
            self.cell = cell;
            self.slots.clear();
        }
        let n = (self.cols() * (ATLAS_SIZE / self.cell.y.max(1))) as usize;
        self.slots.resize(n, None);
        for slot in &mut self.slots {
            if slot.is_some_and(|k| !live.contains(&k)) {
                *slot = None;
            }
        }
    }

    /// The speaker's cell in atlas texels: the one it has, else the first free one; `None` when
    /// every cell is taken.
    pub(crate) fn cell_of(&mut self, key: u64) -> Option<Rect> {
        let i = match self.slots.iter().position(|s| *s == Some(key)) {
            Some(i) => i,
            None => {
                let i = self.slots.iter().position(Option::is_none)?;
                self.slots[i] = Some(key);
                i
            }
        };
        Some(self.slot_rect(i))
    }

    /// Slot `i`'s cell in atlas texels.
    fn slot_rect(&self, i: usize) -> Rect {
        let (col, row) = (i as u32 % self.cols(), i as u32 / self.cols());
        let min = Vec2::new((col * self.cell.x) as f32, (row * self.cell.y) as f32);
        Rect::from_corners(min, min + self.cell.as_vec2())
    }

    /// Take the tagged quads in draw order; `true` when they differ from the ones the atlas was
    /// last drawn from, which also publishes their bounds and sets the camera to run.
    pub(crate) fn gather(&mut self, tagged: &[&UiQuad]) -> bool {
        let same =
            self.last.len() == tagged.len() && self.last.iter().zip(tagged).all(|(a, b)| a == *b);
        if same {
            return false;
        }
        self.last.clear();
        self.last.extend(tagged.iter().map(|q| (*q).clone()));
        self.rects.clear();
        for q in tagged {
            let Some(key) = q.card else { continue };
            let r = Rect::from_corners(q.rect.min - CARD_ORIGIN, q.rect.max - CARD_ORIGIN);
            // A cell is the card's room: what overflows it would be a neighbour's.
            let r = match self.cell_rect(key) {
                Some(cell) => r.intersect(cell),
                None => r,
            };
            if r.is_empty() {
                continue;
            }
            self.rects
                .entry(key)
                .and_modify(|b| *b = b.union(r))
                .or_insert(r);
        }
        self.publish_rects();
        self.raster = true;
        true
    }

    fn cell_rect(&self, key: u64) -> Option<Rect> {
        let i = self.slots.iter().position(|s| *s == Some(key))?;
        Some(self.slot_rect(i))
    }

    /// Replace the live cards, each with the bounds its quads last drew.
    pub(crate) fn publish(&mut self, cards: Vec<Card>) {
        self.cards = cards;
        self.publish_rects();
    }

    fn publish_rects(&mut self) {
        for c in &mut self.cards {
            c.rect = self.rects.get(&c.key).copied();
        }
    }
}

/// The cell size for a bubble geometry: the widest frame the text wraps to, its border either
/// side and the pad, and three quarters of that tall. `seam` takes units to logical px.
pub(crate) fn cell_size(g: &benilla_ui::script::BubbleGeometry, seam: f32) -> UVec2 {
    let w = (g.wrap_cap + 2.0 * g.margin + 2.0 * g.border) * seam + 2.0 * CELL_PAD;
    let w = ((w / 16.0).ceil() * 16.0).clamp(64.0, ATLAS_SIZE as f32 / 2.0);
    UVec2::new(w as u32, ((w * 0.75 / 16.0).ceil() * 16.0) as u32)
}

/// Where the text's bottom centre sits in `cell`: the lowest thing a bubble draws is its tail, a
/// border unit square hung three quarters below the frame, which hangs a margin below the text.
pub(crate) fn seat_in_cell(cell: Rect, g: &benilla_ui::script::BubbleGeometry, seam: f32) -> Vec2 {
    Vec2::new(
        cell.center().x,
        cell.max.y - CELL_PAD - (g.margin + 0.75 * g.border) * seam,
    )
}

/// The card view's rebuild runs in this set, so a headset crate orders its placement after it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CardLaneSet;

/// Marker on the card camera.
#[derive(Component)]
pub(crate) struct CardCamera;

pub struct CardLanePlugin;

impl Plugin for CardLanePlugin {
    fn build(&self, app: &mut App) {
        // `run_camera` is ordered in the chain of the two UI views ([`crate::ui_pass`]).
        app.init_resource::<CardLane>().add_systems(
            Update,
            sync_atlas
                .before(CardLaneSet)
                .before(crate::chat_bubble::BubbleSet),
        );
    }
}

/// A fresh atlas: a render target the card camera draws and the headset samples, never read back
/// by the CPU.
fn new_atlas() -> Image {
    let mut image = Image::new_uninit(
        Extent3d {
            width: ATLAS_SIZE,
            height: ATLAS_SIZE,
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

/// Builds the atlas and its camera when the lane is switched on and takes them down when it is
/// switched off, so flat play allocates nothing.
fn sync_atlas(
    mut lane: ResMut<CardLane>,
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    camera: Query<Entity, With<CardCamera>>,
) {
    match (lane.enabled, lane.atlas.is_some()) {
        (true, false) => {
            let atlas = images.add(new_atlas());
            commands.spawn((
                CardCamera,
                Name::new("card-lane camera"),
                Camera2d,
                Msaa::Off,
                // The same decode the screen's lane ends in: the quads composite in gamma bytes.
                crate::ui_gamma::UiGammaLane::default(),
                card_render_layers(),
                RenderTarget::Image(ImageRenderTarget {
                    handle: atlas.clone(),
                    scale_factor: 1.0,
                }),
                Camera {
                    order: CARD_CAMERA_ORDER,
                    output_mode: CameraOutputMode::Skip,
                    clear_color: ClearColorConfig::Custom(Color::NONE),
                    msaa_writeback: bevy::camera::MsaaWriteback::Off,
                    ..default()
                },
            ));
            lane.atlas = Some(atlas);
            // The first draw clears the new target.
            lane.raster = true;
        }
        (false, true) => {
            for e in &camera {
                commands.entity(e).despawn();
            }
            if let Some(atlas) = lane.atlas.take() {
                images.remove(&atlas);
            }
            lane.cards.clear();
            lane.slots.clear();
            lane.cell = UVec2::ZERO;
            lane.last.clear();
            lane.rects.clear();
            lane.raster = false;
        }
        _ => {}
    }
}

/// The camera draws the whole atlas when it runs, so it runs only on a frame the cards changed.
pub(crate) fn run_camera(
    mut lane: ResMut<CardLane>,
    mut camera: Query<&mut Camera, With<CardCamera>>,
) {
    let want = std::mem::take(&mut lane.raster);
    for mut cam in &mut camera {
        if cam.is_active != want {
            cam.is_active = want;
        }
    }
}

#[cfg(test)]
mod gpu_tests;
#[cfg(test)]
mod view_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use benilla_ui::script::BubbleGeometry;

    fn geometry() -> BubbleGeometry {
        BubbleGeometry {
            border: 16.0,
            margin: 13.0,
            wrap_cap: 256.0,
            text_height: 13.0,
            device_px: 1.0,
        }
    }

    fn quad(key: u64, min: Vec2, max: Vec2) -> UiQuad {
        UiQuad {
            rect: Rect::from_corners(min + CARD_ORIGIN, max + CARD_ORIGIN),
            card: Some(key),
            ..default()
        }
    }

    /// A speaker keeps its cell for as long as it speaks, cells never overlap, a freed cell is
    /// reused, and a lane with every cell taken gives none.
    #[test]
    fn a_speaker_keeps_a_cell_and_cells_never_overlap() {
        let cell = cell_size(&geometry(), 1.0);
        let mut lane = CardLane::default();
        lane.begin(cell, &[1, 2, 3]);
        let a = lane.cell_of(1).unwrap();
        let b = lane.cell_of(2).unwrap();
        assert!(a.intersect(b).is_empty(), "{a:?} and {b:?} overlap");
        lane.begin(cell, &[2, 1]);
        assert_eq!(lane.cell_of(1), Some(a), "kept while it speaks");
        assert_eq!(lane.cell_of(2), Some(b));
        assert_eq!(
            lane.cell_of(3).map(|c| c.min),
            Some(lane.cell_of(3).unwrap().min)
        );
        lane.begin(cell, &[4]);
        assert_eq!(lane.cell_of(4), Some(a), "the first free cell is reused");

        let all: Vec<u64> = (0..1000).collect();
        lane.begin(cell, &all);
        let given = all.iter().filter_map(|&k| lane.cell_of(k)).count();
        assert_eq!(given, lane.slots.len());
        assert!(lane.cell_of(5000).is_none());
        for c in all.iter().filter_map(|&k| lane.cell_of(k)) {
            assert!(c.max.x <= ATLAS_SIZE as f32 && c.max.y <= ATLAS_SIZE as f32);
        }
    }

    /// A window resize cuts new cells, so the old slots go.
    #[test]
    fn a_new_cell_size_frees_every_slot() {
        let mut lane = CardLane::default();
        lane.begin(UVec2::new(320, 240), &[1]);
        lane.cell_of(1).unwrap();
        lane.begin(UVec2::new(400, 304), &[1]);
        assert_eq!(lane.cell_of(1).unwrap().size(), Vec2::new(400.0, 304.0));
    }

    /// The seat sits so the tail, the lowest thing a bubble draws, ends a pad above the cell's
    /// bottom, centred.
    #[test]
    fn the_seat_leaves_room_for_the_tail() {
        let g = geometry();
        let cell = Rect::from_corners(Vec2::new(300.0, 0.0), Vec2::new(600.0, 240.0));
        let seat = seat_in_cell(cell, &g, 1.0);
        assert_eq!(seat.x, 450.0);
        let tail_bottom = seat.y + g.margin + 0.75 * g.border;
        assert_eq!(tail_bottom, cell.max.y - CELL_PAD);
    }

    /// The gather publishes each card's bounds in atlas texels, keeps what overflows its cell out
    /// of them, and reports unchanged quads as unchanged, so the camera stays idle.
    #[test]
    fn gather_bounds_each_card_in_its_cell_and_gates_on_change() {
        let cell = UVec2::new(320, 240);
        let mut lane = CardLane::default();
        lane.begin(cell, &[1, 2]);
        let c1 = lane.cell_of(1).unwrap();
        let c2 = lane.cell_of(2).unwrap();
        lane.publish(vec![
            Card {
                key: 1,
                anchor: Vec3::ZERO,
                alpha: 1.0,
                shown: true,
                own: false,
                seat: Vec2::ZERO,
                rect: None,
            },
            Card {
                key: 2,
                anchor: Vec3::ZERO,
                alpha: 1.0,
                shown: true,
                own: true,
                seat: Vec2::ZERO,
                rect: None,
            },
        ]);
        let quads = [
            quad(1, c1.min + 10.0, c1.min + 50.0),
            quad(1, c1.min + 40.0, c1.min + 90.0),
            // Overflows into the next cell.
            quad(2, c2.min + 100.0, c2.max + 500.0),
        ];
        let refs: Vec<&UiQuad> = quads.iter().collect();
        assert!(lane.gather(&refs));
        assert!(lane.raster, "the camera runs for new quads");
        assert_eq!(
            lane.cards[0].rect,
            Some(Rect::from_corners(c1.min + 10.0, c1.min + 90.0))
        );
        assert_eq!(
            lane.cards[1].rect,
            Some(Rect::from_corners(c2.min + 100.0, c2.max))
        );
        lane.raster = false;
        assert!(!lane.gather(&refs), "the same quads are not a change");
        assert!(!lane.raster);
    }

    /// The camera runs on the frame it is told to and sleeps after; a fade (no quad change) wakes
    /// nothing, and the lane takes the atlas and camera down when it is switched off.
    #[test]
    fn the_atlas_and_camera_exist_only_while_enabled_and_the_camera_runs_on_change() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>()
            .init_resource::<CardLane>()
            .configure_sets(Update, CardLaneSet)
            .add_systems(
                Update,
                (
                    sync_atlas.before(CardLaneSet),
                    run_camera.after(CardLaneSet),
                ),
            );
        // Camera2d needs its required components' assets; none of the lane's tests render.
        let cameras = |app: &mut App| {
            app.world_mut()
                .query_filtered::<&Camera, With<CardCamera>>()
                .iter(app.world())
                .map(|c| c.is_active)
                .collect::<Vec<_>>()
        };
        app.update();
        assert!(cameras(&mut app).is_empty(), "flat play spawns nothing");
        assert!(app.world().resource::<CardLane>().atlas().is_none());

        app.world_mut().resource_mut::<CardLane>().enabled = true;
        app.update();
        assert_eq!(
            cameras(&mut app),
            [true],
            "the first frame clears the atlas"
        );
        app.update();
        assert_eq!(cameras(&mut app), [false], "then it sleeps");
        app.world_mut().resource_mut::<CardLane>().raster = true;
        app.update();
        assert_eq!(cameras(&mut app), [true], "a change wakes it for one frame");
        app.update();
        assert_eq!(cameras(&mut app), [false]);

        app.world_mut().resource_mut::<CardLane>().enabled = false;
        app.update();
        app.update();
        assert!(cameras(&mut app).is_empty());
        assert!(app.world().resource::<CardLane>().atlas().is_none());
    }
}
