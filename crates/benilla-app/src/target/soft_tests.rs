//! The soft target: its scoring, its hysteresis, and the wire rule that it sends nothing until a
//! lock.

use bevy::ecs::system::RunSystemOnce;
use crossbeam_channel::Receiver;

use super::*;
use crate::net::{ClientCommand, NetCommands, SelfGuid};
use crate::target::Selection;

fn cfg() -> SoftTargetConfig {
    SoftTargetConfig::default()
}

fn raw(kind: Kind, dist: f32, angle_deg: f32, blocked: bool) -> Raw {
    Raw {
        entity: Entity::PLACEHOLDER,
        guid: 1,
        kind,
        dist,
        angle: angle_deg.to_radians(),
        blocked,
    }
}

fn cand(guid: u64, kind: Kind, score: f32) -> Cand {
    Cand {
        entity: Entity::from_raw_u32(guid as u32).expect("a valid index"),
        guid,
        kind,
        score,
    }
}

#[test]
fn range_arc_and_walls_decide_who_is_a_candidate() {
    let c = cfg();
    assert!(score(&raw(Kind::Enemy, 40.0, 10.0, false), &c).is_some());
    assert!(
        score(&raw(Kind::Enemy, 41.5, 0.0, false), &c).is_none(),
        "out of range"
    );
    assert!(
        score(&raw(Kind::Enemy, 10.0, 41.0, false), &c).is_none(),
        "out of the arc"
    );
    assert!(score(&raw(Kind::Enemy, 10.0, 39.0, false), &c).is_some());
    assert!(
        score(&raw(Kind::Enemy, 10.0, 0.0, true), &c).is_none(),
        "behind a wall"
    );
    // An object or a corpse is only worth it within an interact's reach.
    assert!(score(&raw(Kind::Object, 9.0, 0.0, false), &c).is_some());
    assert!(score(&raw(Kind::Object, 20.0, 0.0, false), &c).is_none());
    assert!(score(&raw(Kind::Loot, 20.0, 0.0, false), &c).is_none());
}

#[test]
fn the_angle_decides_first_and_the_distance_second() {
    let c = cfg();
    let near_but_aside = score(&raw(Kind::Enemy, 3.0, 30.0, false), &c).unwrap();
    let far_but_ahead = score(&raw(Kind::Enemy, 38.0, 2.0, false), &c).unwrap();
    assert!(far_but_ahead.score < near_but_aside.score);
    // At one angle the nearer wins.
    let near = score(&raw(Kind::Enemy, 5.0, 10.0, false), &c).unwrap();
    let far = score(&raw(Kind::Enemy, 20.0, 10.0, false), &c).unwrap();
    assert!(near.score < far.score);
}

fn holder(state: &SoftState) -> Option<u64> {
    state.guid()
}

#[test]
fn a_challenger_needs_a_clear_lead_for_long_enough() {
    let c = cfg();
    let mut s = SoftState::default();
    s.step(vec![cand(1, Kind::Enemy, 0.40)], 0.0, &c);
    assert_eq!(holder(&s), Some(1));

    // 2 leads by 37 percent, but only for 0.10 s: 1 stays.
    for t in [0.0_f32, 0.05, 0.10] {
        s.step(
            vec![cand(1, Kind::Enemy, 0.40), cand(2, Kind::Enemy, 0.25)],
            1.0 + t,
            &c,
        );
        assert_eq!(holder(&s), Some(1), "at +{t}");
    }
    // 0.16 s after the lead began, 2 takes over.
    s.step(
        vec![cand(1, Kind::Enemy, 0.40), cand(2, Kind::Enemy, 0.25)],
        1.16,
        &c,
    );
    assert_eq!(holder(&s), Some(2));
}

#[test]
fn a_lead_under_the_margin_never_wins() {
    let c = cfg();
    let mut s = SoftState::default();
    s.step(vec![cand(1, Kind::Enemy, 0.40)], 0.0, &c);
    for i in 0..40 {
        // 2 scores 12 percent better, less than the 25 percent.
        s.step(
            vec![cand(1, Kind::Enemy, 0.40), cand(2, Kind::Enemy, 0.35)],
            0.1 * i as f32,
            &c,
        );
        assert_eq!(holder(&s), Some(1));
    }
}

#[test]
fn a_lead_that_flickers_restarts_the_clock() {
    let c = cfg();
    let mut s = SoftState::default();
    s.step(vec![cand(1, Kind::Enemy, 0.40)], 0.0, &c);
    let both = |lead: f32| vec![cand(1, Kind::Enemy, 0.40), cand(2, Kind::Enemy, lead)];
    s.step(both(0.25), 1.00, &c);
    s.step(both(0.25), 1.10, &c);
    // The lead lapses for a frame.
    s.step(both(0.39), 1.12, &c);
    s.step(both(0.25), 1.14, &c);
    s.step(both(0.25), 1.26, &c);
    assert_eq!(holder(&s), Some(1), "0.12 s since the lead came back");
    s.step(both(0.25), 1.30, &c);
    assert_eq!(holder(&s), Some(2));
}

#[test]
fn a_holder_that_left_the_view_is_replaced_at_once() {
    let c = cfg();
    let mut s = SoftState::default();
    s.step(
        vec![cand(1, Kind::Enemy, 0.1), cand(2, Kind::Enemy, 0.5)],
        0.0,
        &c,
    );
    assert_eq!(holder(&s), Some(1));
    s.step(vec![cand(2, Kind::Enemy, 0.5)], 0.01, &c);
    assert_eq!(holder(&s), Some(2));
    s.step(Vec::new(), 0.02, &c);
    assert_eq!(holder(&s), None);
}

#[test]
fn a_lock_takes_the_soft_target_then_walks_the_side_past_the_hard_one() {
    let c = cfg();
    let mut s = SoftState::default();
    let list = vec![
        cand(1, Kind::Enemy, 0.1),
        cand(5, Kind::Friend, 0.2),
        cand(2, Kind::Enemy, 0.3),
        cand(3, Kind::Enemy, 0.4),
        cand(6, Kind::Npc, 0.5),
    ];
    s.step(list, 0.0, &c);
    // `mid` is a walk the last lock began, so the soft target is not locked again.
    let pick = |s: &mut SoftState, side, hard: Option<u64>, mid: bool| {
        s.cycling = hard.filter(|_| mid);
        s.lock_pick(side, hard).map(|c| c.guid)
    };
    // The soft target is 1, an enemy: the first press locks what the highlight shows.
    assert_eq!(pick(&mut s, SoftSide::Enemy, None, false), Some(1));
    // Locked, the next presses walk the list from it, and the list wraps.
    assert_eq!(pick(&mut s, SoftSide::Enemy, Some(1), true), Some(2));
    assert_eq!(pick(&mut s, SoftSide::Enemy, Some(2), true), Some(3));
    assert_eq!(pick(&mut s, SoftSide::Enemy, Some(3), true), Some(1));
    // A target the mouse picked, not a lock of ours: the soft target is not it, so it is locked.
    assert_eq!(pick(&mut s, SoftSide::Enemy, Some(3), false), Some(1));
    // The soft target is not a friend, so the friend side starts at its best.
    assert_eq!(pick(&mut s, SoftSide::Friend, None, false), Some(5));
    assert_eq!(
        pick(&mut s, SoftSide::Friend, Some(5), true),
        Some(6),
        "an NPC is a friend"
    );
    assert_eq!(pick(&mut s, SoftSide::Friend, Some(6), true), Some(5));
    // The soft target is the hard one already: the press walks on.
    assert_eq!(pick(&mut s, SoftSide::Enemy, Some(1), false), Some(2));
    // A hard target off the list starts the walk at the top.
    assert_eq!(pick(&mut s, SoftSide::Enemy, Some(99), true), Some(1));
}

#[test]
fn a_lock_with_nothing_new_to_lock_does_nothing() {
    let c = cfg();
    let mut s = SoftState::default();
    s.step(vec![cand(1, Kind::Enemy, 0.1)], 0.0, &c);
    assert!(
        s.lock_pick(SoftSide::Enemy, Some(1)).is_none(),
        "only the hard target"
    );
    assert!(s.lock_pick(SoftSide::Friend, None).is_none(), "no friends");
    assert!(SoftState::default()
        .lock_pick(SoftSide::Enemy, None)
        .is_none());
}

#[test]
fn an_interact_skips_an_enemy_for_a_service_beside_it() {
    let c = cfg();
    let mut s = SoftState::default();
    s.step(
        vec![
            cand(1, Kind::Enemy, 0.1),
            cand(2, Kind::Object, 0.3),
            cand(3, Kind::Npc, 0.2),
        ],
        0.0,
        &c,
    );
    assert_eq!(s.interact_pick().map(|c| c.guid), Some(3));
    // Only an enemy: a right click on it attacks, and so does this.
    s.step(vec![cand(1, Kind::Enemy, 0.1)], 1.0, &c);
    assert_eq!(s.interact_pick().map(|c| c.guid), Some(1));
    // A friend as the soft target is the interact's.
    s.step(
        vec![cand(4, Kind::Loot, 0.1), cand(1, Kind::Enemy, 0.0)],
        2.0,
        &c,
    );
    assert_eq!(s.interact_pick().map(|c| c.guid), Some(4));
}

/// `INTERACTORJUMP` interacts only for a service in front: an NPC, a corpse or an object. An
/// enemy alone, or a friendly player, is a jump.
#[test]
fn only_a_service_in_front_makes_the_contextual_button_interact() {
    let c = cfg();
    let mut s = SoftState::default();
    assert!(!s.has_service(), "nothing in view");
    for (kind, ready) in [
        (Kind::Enemy, false),
        (Kind::Friend, false),
        (Kind::Npc, true),
        (Kind::Loot, true),
        (Kind::Object, true),
    ] {
        s = SoftState::default();
        s.step(vec![cand(1, kind, 0.1)], 0.0, &c);
        assert_eq!(s.has_service(), ready, "{kind:?}");
    }
    // An enemy ahead with a service beside it: the service.
    s.step(
        vec![cand(1, Kind::Enemy, 0.1), cand(2, Kind::Npc, 0.3)],
        5.0,
        &c,
    );
    assert!(s.has_service());
}

const ME: u64 = 0x10;
const MOB: u64 = 0x31;
const OTHER: u64 = 0x32;

struct Fix {
    app: App,
    rx: Receiver<ClientCommand>,
    mob: Entity,
    other: Entity,
}

/// The whole client, headless, with us at the origin facing +z, a camera behind us, and two mobs.
fn fix() -> Fix {
    let mut app = crate::game_plugins::schedule_tests::headless_client();
    let (tx, rx) = crossbeam_channel::unbounded();
    app.insert_resource(NetCommands(tx));
    let world = app.world_mut();
    let store =
        |pairs: &[(u16, u32)]| ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs));
    // `UNIT_FLAG_PVP_ATTACKABLE`: with no faction table a plain unit is attackable.
    let me = world
        .spawn((
            SelfPlayer,
            Guid(ME),
            Transform::default(),
            GlobalTransform::default(),
            store(&[(2, 0x19), (22, 100), (28, 100), (46, 0x8)]),
        ))
        .id();
    let mut unit = |guid: u64, z: f32| {
        world
            .spawn((
                Guid(guid),
                NetEntity {
                    kind: EntityKind::Unit,
                    display_id: None,
                    scale: 1.0,
                },
                Transform::from_xyz(0.0, 0.0, z),
                GlobalTransform::from_translation(Vec3::new(0.0, 0.0, z)),
                store(&[(22, 100), (28, 100)]),
            ))
            .id()
    };
    let mob = unit(MOB, 10.0);
    let other = unit(OTHER, 20.0);
    world.spawn((
        WorldCamera,
        GlobalTransform::from(Transform::from_xyz(0.0, 2.0, -5.0).looking_to(Vec3::Z, Vec3::Y)),
    ));
    let index = &mut world.resource_mut::<GuidIndex>().0;
    index.extend([(ME, me), (MOB, mob), (OTHER, other)]);
    world.resource_mut::<SelfGuid>().0 = Some(ME);
    Fix {
        app,
        rx,
        mob,
        other,
    }
}

impl Fix {
    fn send(&mut self, msg: SoftTargetInput) {
        self.app.world_mut().write_message(msg);
    }

    fn selections(&self) -> Vec<u64> {
        self.rx
            .try_iter()
            .filter_map(|c| match c {
                ClientCommand::SetSelection { guid } => Some(guid),
                _ => None,
            })
            .collect()
    }
}

/// The pass reads what is in front of the camera, in range, and ranks it.
#[test]
fn the_scan_finds_what_is_in_front_and_in_range() {
    let mut f = fix();
    f.send(SoftTargetInput::Active(true));
    f.app
        .world_mut()
        .run_system_once(update_soft)
        .expect("the pass runs");
    let state = f.app.world().resource::<SoftState>();
    assert_eq!(state.guid(), Some(MOB), "the nearer on the same line");
    assert_eq!(
        state.ranked.iter().map(|c| c.guid).collect::<Vec<_>>(),
        [MOB, OTHER]
    );
    // Turn the camera away: nothing is in the arc.
    let mut cam = f
        .app
        .world_mut()
        .query_filtered::<&mut GlobalTransform, With<WorldCamera>>();
    *cam.single_mut(f.app.world_mut()).unwrap() =
        GlobalTransform::from(Transform::from_xyz(0.0, 2.0, -5.0).looking_to(Vec3::NEG_Z, Vec3::Y));
    f.app
        .world_mut()
        .run_system_once(update_soft)
        .expect("the pass runs");
    assert_eq!(f.app.world().resource::<SoftState>().guid(), None);
}

/// The soft target is published as the mouseover, and nothing is sent.
#[test]
fn the_soft_target_is_the_mouseover_and_only_in_mouselook() {
    let mut f = fix();
    f.send(SoftTargetInput::Active(true));
    f.app.world_mut().run_system_once(update_soft).unwrap();
    // The cursor is free: the reference's hover stands, the soft target does not write.
    f.app.world_mut().run_system_once(publish_soft).unwrap();
    assert_eq!(f.app.world().resource::<Hovered>().guid, None);
    // In mouselook the hover is blank and the soft target fills it.
    let state = f.app.world().resource::<SoftState>();
    let soft = state.current.expect("the pass found a target");
    let (mut hovered, mut object) = (Hovered::default(), HoveredObject::default());
    publish(state, soft, 10.0, true, &mut hovered, &mut object);
    assert_eq!((hovered.target, hovered.guid), (Some(f.mob), Some(MOB)));
    assert_eq!(object.target, None);
    // A real hover is not overwritten.
    let mut plate = Hovered {
        plate: true,
        ..Hovered::default()
    };
    publish(state, soft, 10.0, true, &mut plate, &mut object);
    assert_eq!(plate.target, None);
    assert!(f.selections().is_empty());
}

/// The wire rule: a run of soft-target changes sends no `CMSG_SET_SELECTION`, one lock sends
/// exactly one, and losing the soft target sends nothing, least of all guid 0.
#[test]
fn the_wire_sees_one_selection_per_lock_and_none_from_the_soft_target() {
    let mut f = fix();
    let c = cfg();
    f.app.world_mut().resource_mut::<SoftState>().active = true;
    let (mob, other) = (f.mob, f.other);
    let on = |e: Entity, g: u64, s: f32| Cand {
        entity: e,
        guid: g,
        kind: Kind::Enemy,
        score: s,
    };
    // Walk past a group: the best one changes, appears and disappears.
    let frames: Vec<(f32, Vec<Cand>)> = vec![
        (0.0, vec![on(mob, MOB, 0.3)]),
        (0.1, vec![on(mob, MOB, 0.3), on(other, OTHER, 0.1)]),
        (0.3, vec![on(mob, MOB, 0.3), on(other, OTHER, 0.1)]),
        (0.4, vec![on(other, OTHER, 0.1)]),
        (0.5, vec![]),
        (0.6, vec![on(mob, MOB, 0.2)]),
        (0.7, vec![]),
    ];
    for (now, ranked) in frames {
        f.app
            .world_mut()
            .resource_mut::<SoftState>()
            .step(ranked, now, &c);
        f.app.world_mut().run_system_once(publish_soft).unwrap();
    }
    assert!(
        f.selections().is_empty(),
        "no soft target change is a selection"
    );
    assert_eq!(f.app.world().resource::<Selection>().guid, None);

    // One press, one packet.
    f.app
        .world_mut()
        .resource_mut::<SoftState>()
        .step(vec![on(mob, MOB, 0.2)], 1.0, &c);
    f.send(SoftTargetInput::Cycle(SoftSide::Enemy));
    f.app
        .world_mut()
        .run_system_once(apply_soft_input)
        .expect("the lock runs");
    assert_eq!(f.selections(), [MOB]);
    assert_eq!(f.app.world().resource::<Selection>().guid, Some(MOB));

    // The soft target goes away again: the hard target and the wire stay as they were.
    f.app
        .world_mut()
        .resource_mut::<SoftState>()
        .step(Vec::new(), 2.0, &c);
    f.app.world_mut().run_system_once(publish_soft).unwrap();
    assert!(f.selections().is_empty(), "losing it sends no guid 0");
    assert_eq!(f.app.world().resource::<Selection>().guid, Some(MOB));
}

/// An interact on the soft target is the `Interact` message and no selection.
#[test]
fn an_interact_is_the_interact_message_and_sends_no_selection() {
    let mut f = fix();
    f.app.world_mut().resource_mut::<SoftState>().step(
        vec![cand_for(f.mob, MOB, Kind::Npc)],
        0.0,
        &cfg(),
    );
    f.send(SoftTargetInput::Interact);
    f.app.world_mut().run_system_once(apply_soft_input).unwrap();
    let sent: Vec<_> = f
        .app
        .world_mut()
        .resource_mut::<Messages<Interact>>()
        .drain()
        .collect();
    assert_eq!(sent, [Interact(f.mob)]);
    assert!(f.selections().is_empty());
}

fn cand_for(entity: Entity, guid: u64, kind: Kind) -> Cand {
    Cand {
        entity,
        guid,
        kind,
        score: 0.1,
    }
}
