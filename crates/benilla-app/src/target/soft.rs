//! A client-side soft target for a pad that has no cursor: the unit, NPC, corpse or object nearest
//! the middle of the view, published as the mouseover so it gets the mouseover highlight.
//!
//! Deviation: 1.12 has no soft target; its mouselook hides the cursor and blanks the mouseover
//! (`0x483e80`). This is the gamepad layer's, and it keeps four rules so the server never sees a
//! selection that flickers (`HandleSetSelectionOpcode`, vmangos `MiscHandler.cpp:399`: a changed
//! selection clears combo points, and an empty or non-hostile one cancels Auto Shot):
//!
//! 1. A soft target is client-only. Gaining, losing or switching it sends nothing, and losing it
//!    never sends guid 0.
//! 2. `CMSG_SET_SELECTION` goes out only on an explicit lock ([`SoftTargetInput::Cycle`]), once per
//!    press, through [`SelectCommit`], the commit TAB uses.
//! 3. Casts act on the hard target. A soft target is never locked by a cast.
//! 4. Interact ([`SoftTargetInput::Interact`]) is the existing [`Interact`] message.
//!
//! A laser (VR) hovers for real, so the soft target does not publish under it, and its two
//! presses act on that hover: [`SoftTargetInput::LockHovered`] is the lock of rule 2 on the unit
//! under the laser and [`SoftTargetInput::InteractHovered`] is rule 4 on what a right click there
//! would act on. Hover itself sends nothing.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use avian3d::prelude::SpatialQuery;
use benilla_protocol::{guid, EntityKind};
use benilla_world::collision::PickOccluder;
use benilla_world::view::WorldCamera;

use super::cursor_mode::{go_highlightable, GoOverrides};
use super::relations::{can_assist, can_attack, can_interact, Assist};
use super::{
    go_is_nearest, CreatureTypes, Factions, Hovered, HoveredObject, Interact, PickOcclusion,
    SelectCommit,
};
use crate::names::NameCache;
use crate::net::{Guid, GuidIndex, NetEntity, ObjectStore, Reputations, SelfPlayer};
use crate::player::CameraControl;

/// Weight of the distance term against the angle term: angle first, distance second.
const DIST_WEIGHT: f32 = 0.25;
/// How far in front of a unit the line of sight may end, so its own hull does not block it.
const LOS_SLACK: f32 = 1.0;

/// The soft target's dials. The pad's settings (`pad.toml`) replace the resource at start.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct SoftTargetConfig {
    /// Yards a unit may be from us: `targetNearestDistance`, 41 like the TAB scan.
    pub range: f32,
    /// Yards a corpse or object may be from us, the reach of an interact.
    pub object_range: f32,
    /// Degrees either side of the camera's forward.
    pub arc_degrees: f32,
    /// How much better, as a fraction of the holder's score, a challenger must score.
    pub margin: f32,
    /// Seconds a challenger must keep that lead before it takes over.
    pub switch_secs: f32,
}

impl Default for SoftTargetConfig {
    fn default() -> Self {
        Self {
            range: 41.0,
            object_range: 10.0,
            arc_degrees: 40.0,
            margin: 0.25,
            switch_secs: 0.15,
        }
    }
}

/// What a pad asks of the soft target. `benilla-input` writes these.
#[derive(Message, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoftTargetInput {
    /// The soft target runs while the pad plays in gameplay mode and stops otherwise.
    Active(bool),
    /// Lock the next target on a side: the soft target when it is on that side and not yet locked,
    /// else the one after the current target in the side's list. One press, at most one packet.
    Cycle(SoftSide),
    /// Interact with the soft target, as a right click on it would.
    Interact,
    /// Lock the unit under the laser: one press, at most one packet, none for an empty hover.
    LockHovered,
    /// Interact with what a right click under the laser would act on.
    InteractHovered,
}

/// Whether the soft target has an NPC to talk to, a corpse to loot or an object to use in front,
/// which is when `INTERACTORJUMP` interacts and otherwise jumps. A friendly player or an enemy
/// alone does not count: a right click on either is not a service. `benilla-input` reads it.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoftTargetReady(pub bool);

/// The two lists a shoulder button walks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoftSide {
    /// Players and friendly NPCs.
    Friend,
    /// What we may attack.
    Enemy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Enemy,
    /// A unit we may assist.
    Friend,
    /// A friendly NPC that offers a service.
    Npc,
    /// A dead unit with loot.
    Loot,
    /// A usable GameObject.
    Object,
}

impl Kind {
    fn on(self, side: SoftSide) -> bool {
        match side {
            SoftSide::Enemy => self == Kind::Enemy,
            SoftSide::Friend => matches!(self, Kind::Friend | Kind::Npc),
        }
    }

    fn reach(self, cfg: &SoftTargetConfig) -> f32 {
        match self {
            Kind::Enemy | Kind::Friend | Kind::Npc => cfg.range,
            Kind::Loot | Kind::Object => cfg.object_range,
        }
    }
}

/// One thing the soft target may be, with its place in the view.
#[derive(Clone, Copy, Debug)]
struct Raw {
    entity: Entity,
    guid: u64,
    kind: Kind,
    /// Yards from us.
    dist: f32,
    /// Radians between the camera's forward and the thing.
    angle: f32,
    /// Terrain or a wall stands between the camera and it.
    blocked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Cand {
    entity: Entity,
    guid: u64,
    kind: Kind,
    /// Lower is better.
    score: f32,
}

/// The score of a thing in view, `None` when it is out of range, out of the arc or behind a wall.
fn score(raw: &Raw, cfg: &SoftTargetConfig) -> Option<Cand> {
    let reach = raw.kind.reach(cfg);
    let arc = cfg.arc_degrees.to_radians();
    if raw.blocked || raw.dist > reach || raw.angle > arc {
        return None;
    }
    Some(Cand {
        entity: raw.entity,
        guid: raw.guid,
        kind: raw.kind,
        score: raw.angle / arc + DIST_WEIGHT * raw.dist / reach,
    })
}

/// What the pass remembers between frames.
#[derive(Resource, Default)]
pub(crate) struct SoftState {
    active: bool,
    /// Every candidate this frame, best first.
    ranked: Vec<Cand>,
    current: Option<Cand>,
    /// The challenger and the time it first led by the margin.
    challenger: Option<(u64, f32)>,
    /// The target the last lock selected: while it is still the hard target the next lock walks on
    /// from it, and otherwise a lock starts with the soft target.
    cycling: Option<u64>,
}

impl SoftState {
    /// This frame's ranking in, the holder out. A holder stays until a challenger scores `margin`
    /// better for `switch_secs`; a holder that left the ranking is replaced at once.
    fn step(&mut self, mut ranked: Vec<Cand>, now: f32, cfg: &SoftTargetConfig) {
        ranked.sort_by(|a, b| a.score.total_cmp(&b.score));
        self.ranked = ranked;
        let Some(best) = self.ranked.first().copied() else {
            self.current = None;
            self.challenger = None;
            return;
        };
        let Some(held) = self
            .current
            .and_then(|c| self.ranked.iter().find(|r| r.guid == c.guid).copied())
        else {
            self.current = Some(best);
            self.challenger = None;
            return;
        };
        // The holder's score is this frame's, so it follows the view.
        self.current = Some(held);
        if best.guid == held.guid || best.score > held.score * (1.0 - cfg.margin) {
            self.challenger = None;
            return;
        }
        let since = match self.challenger {
            Some((g, since)) if g == best.guid => since,
            _ => now,
        };
        if now - since >= cfg.switch_secs {
            self.current = Some(best);
            self.challenger = None;
        } else {
            self.challenger = Some((best.guid, since));
        }
    }

    /// The soft target.
    pub(crate) fn guid(&self) -> Option<u64> {
        self.current.map(|c| c.guid)
    }

    /// What a lock on `side` selects, given the hard target.
    fn lock_pick(&self, side: SoftSide, hard: Option<u64>) -> Option<Cand> {
        let mid_cycle = self.cycling.is_some() && self.cycling == hard;
        if let Some(soft) = self
            .current
            .filter(|c| !mid_cycle && c.kind.on(side) && Some(c.guid) != hard)
        {
            return Some(soft);
        }
        let list: Vec<&Cand> = self.ranked.iter().filter(|c| c.kind.on(side)).collect();
        let start = hard
            .and_then(|h| list.iter().position(|c| c.guid == h))
            .map_or(0, |i| i + 1);
        (0..list.len())
            .map(|i| list[(start + i) % list.len()])
            .find(|c| Some(c.guid) != hard)
            .copied()
    }

    /// What an interact acts on: the soft target, or when that is an enemy (a right click would
    /// attack it) the best thing beside it that is not.
    fn interact_pick(&self) -> Option<Cand> {
        let current = self.current?;
        if current.kind != Kind::Enemy {
            return Some(current);
        }
        self.ranked
            .iter()
            .find(|c| c.kind != Kind::Enemy)
            .copied()
            .or(Some(current))
    }
}

impl SoftState {
    /// An interact has a service to act on.
    fn has_service(&self) -> bool {
        self.interact_pick()
            .is_some_and(|c| matches!(c.kind, Kind::Npc | Kind::Loot | Kind::Object))
    }
}

fn trace_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("WOW_PAD_TRACE").is_some_and(|v| v != "0"))
}

/// Everything the candidates are read from.
#[derive(SystemParam)]
#[allow(clippy::type_complexity)] // one bundled param, the app's convention for big query sets
struct SoftScan<'w, 's> {
    units: Query<
        'w,
        's,
        (
            Entity,
            &'static NetEntity,
            &'static Guid,
            &'static GlobalTransform,
            Option<&'static ObjectStore>,
        ),
        Without<SelfPlayer>,
    >,
    me: Query<'w, 's, (&'static GlobalTransform, Option<&'static ObjectStore>), With<SelfPlayer>>,
    camera: Query<'w, 's, &'static GlobalTransform, (With<WorldCamera>, Without<SelfPlayer>)>,
    factions: Option<Res<'w, Factions>>,
    reputations: Res<'w, Reputations>,
    names: Res<'w, NameCache>,
    creature_types: Option<Res<'w, CreatureTypes>>,
    /// Every store, for `CanAssist`'s owner chase.
    stores: Query<'w, 's, &'static ObjectStore>,
    index: Option<Res<'w, GuidIndex>>,
    spatial: SpatialQuery<'w, 's>,
    occluders: Query<'w, 's, (), With<PickOccluder>>,
}

impl SoftScan<'_, '_> {
    /// What `entity` is to the soft target: a kind, or `None` for what is never one.
    fn kind_of(
        &self,
        net: &NetEntity,
        guid_: u64,
        store: Option<&ObjectStore>,
        me: Option<&ObjectStore>,
    ) -> Option<Kind> {
        match net.kind {
            EntityKind::Unit | EntityKind::Player => {
                let store = store?;
                if store.0.unit_reads_dead() {
                    return store.0.unit_lootable().then_some(Kind::Loot);
                }
                // The critter gate TAB keeps; an unresolved type passes.
                if guid::is_creature_or_pet(guid_) {
                    if let (Some(types), Some(ty)) = (
                        self.creature_types.as_deref(),
                        Some(store)
                            .and_then(|s| s.0.object_entry())
                            .filter(|&e| e != 0)
                            .and_then(|e| self.names.creature_type(e)),
                    ) {
                        if types.0.no_tab_target(ty) {
                            return None;
                        }
                    }
                }
                let factions = self.factions.as_deref();
                if can_attack(Some(store), factions, &self.reputations, me) {
                    Some(Kind::Enemy)
                } else if store.0.unit_npc_flags() != 0
                    && can_interact(Some(store), factions, &self.reputations, me)
                {
                    // Ahead of `CanAssist`: a faction town's service NPC carries `UNIT_FLAG_PVP`,
                    // which `CanAssist` accepts, and it must stay an NPC for the interact button.
                    Some(Kind::Npc)
                } else if can_assist(
                    Some(store),
                    factions,
                    &self.reputations,
                    me,
                    |owner| self.store_of(owner).cloned(),
                    Assist::PlayerToUnit,
                ) {
                    Some(Kind::Friend)
                } else {
                    None
                }
            }
            EntityKind::GameObject => store
                .is_some_and(|s| go_highlightable(s, None, GoOverrides::default()))
                .then_some(Kind::Object),
            _ => None,
        }
    }

    fn store_of(&self, guid_: u64) -> Option<&ObjectStore> {
        let entity = *self.index.as_ref()?.0.get(&guid_)?;
        self.stores.get(entity).ok()
    }

    /// Every candidate in view, scored.
    fn gather(&self, cfg: &SoftTargetConfig) -> Vec<Cand> {
        let (Ok((me_tf, me_store)), Ok(cam)) = (self.me.single(), self.camera.single()) else {
            return Vec::new();
        };
        let (eye, forward) = (cam.translation(), cam.forward());
        let mut out = Vec::new();
        for (entity, net, guid_c, tf, store) in &self.units {
            let Some(kind) = self.kind_of(net, guid_c.0, store, me_store) else {
                continue;
            };
            let at = tf.translation();
            let dist = at.distance(me_tf.translation());
            if dist > kind.reach(cfg) {
                continue;
            }
            let chest = at + Vec3::Y;
            let to = chest - eye;
            let (angle, blocked) = match Dir3::new(to) {
                Ok(dir) => (
                    forward.angle_between(*dir),
                    self.spatial
                        .cast_ray_predicate(
                            eye,
                            dir,
                            to.length(),
                            true,
                            &benilla_world::collision::WorldCollision::body_filter(),
                            &|e| self.occluders.contains(e),
                        )
                        .is_some_and(|hit| hit.distance < to.length() - LOS_SLACK),
                ),
                Err(_) => (0.0, false),
            };
            out.extend(score(
                &Raw {
                    entity,
                    guid: guid_c.0,
                    kind,
                    dist,
                    angle,
                    blocked,
                },
                cfg,
            ));
        }
        out
    }
}

/// Read the pad's switch, then rank what is in view and apply the hysteresis.
fn update_soft(
    mut input: MessageReader<SoftTargetInput>,
    cfg: Res<SoftTargetConfig>,
    time: Res<Time<Real>>,
    scan: SoftScan,
    mut state: ResMut<SoftState>,
    mut ready: ResMut<SoftTargetReady>,
) {
    for msg in input.read() {
        if let SoftTargetInput::Active(on) = msg {
            state.active = *on;
        }
    }
    if !state.active {
        *state = SoftState::default();
        ready.set_if_neq(SoftTargetReady(false));
        return;
    }
    let before = state.guid();
    let ranked = scan.gather(&cfg);
    state.step(ranked, time.elapsed_secs(), &cfg);
    ready.set_if_neq(SoftTargetReady(state.has_service()));
    if trace_on() && before != state.guid() {
        info!(
            "pad: soft target {} -> {} ({} candidates)",
            before.map_or("-".into(), |g| format!("{g:#x}")),
            state.guid().map_or("-".into(), |g| format!(
                "{g:#x} \"{}\"",
                scan.names.peek(g).unwrap_or("?")
            )),
            state.ranked.len()
        );
    }
}

/// The soft target as the mouseover, which the highlight, the tooltip and `"mouseover"` read. It
/// runs only in mouselook, where the reference's own hover is blank.
///
/// It also tells a laser's crate what the laser meets this frame, which is this pass's one other
/// job: it has the picks in hand, and a pass of its own would be one more system unordered against
/// the colliders' exclusive `finish_colliders`.
fn publish_soft(
    occlusion: Res<PickOcclusion>,
    state: Res<SoftState>,
    rig: Option<Res<CameraControl>>,
    me: Query<&GlobalTransform, With<SelfPlayer>>,
    at: Query<&GlobalTransform>,
    mut hovered: ResMut<Hovered>,
    mut object: ResMut<HoveredObject>,
) {
    let aims = crate::pointer::laser_aims();
    crate::pointer::set_hit(laser_hit(aims, &hovered, &object, &occlusion));
    let Some(soft) = state.current else {
        return;
    };
    if aims {
        return;
    }
    let distance = match (me.single(), at.get(soft.entity)) {
        (Ok(me), Ok(at)) => me.translation().distance(at.translation()),
        _ => 0.0,
    };
    publish(
        &state,
        soft,
        distance,
        rig.is_some_and(|r| r.is_looking()),
        &mut hovered,
        &mut object,
    );
}

/// Writes `soft` into the mouseover slots unless a real hover (a plate, a unit or object under the
/// mouse) already holds them.
fn publish(
    state: &SoftState,
    soft: Cand,
    distance: f32,
    looking: bool,
    hovered: &mut Hovered,
    object: &mut HoveredObject,
) {
    if !state.active || !looking {
        return;
    }
    if hovered.any().is_some() || hovered.plate || hovered.refused || object.target.is_some() {
        return;
    }
    if soft.kind == Kind::Object {
        object.target = Some(soft.entity);
        object.guid = Some(soft.guid);
        object.distance = distance;
    } else {
        hovered.target = Some(soft.entity);
        hovered.guid = Some(soft.guid);
        hovered.distance = distance;
    }
}

/// The nearer of what a right click would act on and the terrain or wall on the ray.
fn laser_hit(
    aims: bool,
    hovered: &Hovered,
    object: &HoveredObject,
    occlusion: &PickOcclusion,
) -> Option<crate::pointer::Hit> {
    if !aims {
        return None;
    }
    let pick = laser_pick(hovered, object);
    let distance = pick.map_or(f32::INFINITY, |p| p.2).min(occlusion.distance);
    distance.is_finite().then(|| crate::pointer::Hit {
        distance,
        guid: pick.map(|p| p.1),
    })
}

/// What a right click under the pointer acts on: the nearer of the character pick (a unit or a
/// corpse) and the GameObject, with its guid and ray distance.
fn laser_pick(hovered: &Hovered, object: &HoveredObject) -> Option<(Entity, u64, f32)> {
    if go_is_nearest(hovered, object) {
        return Some((object.target?, object.guid?, object.distance));
    }
    let (entity, guid) = hovered
        .target
        .zip(hovered.guid)
        .or(hovered.corpse.zip(hovered.corpse_guid))?;
    Some((entity, guid, hovered.distance))
}

/// The pad's presses: a lock goes out through the commit TAB uses, an interact as the message.
fn apply_soft_input(
    mut input: MessageReader<SoftTargetInput>,
    mut state: ResMut<SoftState>,
    mut commit: SelectCommit,
    mut interact: MessageWriter<Interact>,
    hovered: Res<Hovered>,
    object: Res<HoveredObject>,
) {
    for msg in input.read() {
        match msg {
            SoftTargetInput::Active(_) => {}
            SoftTargetInput::Cycle(side) => {
                let Some(pick) = state.lock_pick(*side, commit.selection.guid) else {
                    continue;
                };
                if trace_on() {
                    info!("pad: lock {side:?} -> {:#x}", pick.guid);
                }
                commit.commit(pick.entity, pick.guid);
                state.cycling = Some(pick.guid);
            }
            SoftTargetInput::Interact => {
                let Some(pick) = state.interact_pick() else {
                    continue;
                };
                if trace_on() {
                    info!("pad: interact {:?} -> {:#x}", pick.kind, pick.guid);
                }
                interact.write(Interact(pick.entity));
            }
            SoftTargetInput::LockHovered => {
                // A unit only: a corpse or a GameObject is not a selection.
                let Some((entity, guid)) = hovered.target.zip(hovered.guid) else {
                    continue;
                };
                if trace_on() {
                    info!("pad: lock hovered -> {guid:#x}");
                }
                commit.commit(entity, guid);
            }
            SoftTargetInput::InteractHovered => {
                let Some((entity, guid, _)) = laser_pick(&hovered, &object) else {
                    continue;
                };
                if trace_on() {
                    info!("pad: interact hovered -> {guid:#x}");
                }
                interact.write(Interact(entity));
            }
        }
    }
}

/// Registers the soft target.
pub(super) struct SoftTargetPlugin;

impl Plugin for SoftTargetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SoftTargetConfig>()
            .init_resource::<SoftState>()
            .init_resource::<SoftTargetReady>()
            .add_message::<SoftTargetInput>()
            .add_systems(
                Update,
                (
                    (update_soft, publish_soft)
                        .chain()
                        .after(super::hover::update_hovered_object)
                        .before(super::cursor_mode::classify_cursor),
                    // With the other selection writers, after the clicks and before the walk
                    // they owe, so an interact is acted on this frame.
                    apply_soft_input
                        .after(crate::spell::targeting::commit_object_cast_on_click)
                        .before(super::click::act_on_arrival),
                )
                    .in_set(super::TargetUpdate),
            );
    }
}

#[cfg(test)]
#[path = "soft_tests.rs"]
mod tests;
