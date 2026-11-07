//! gilrs, built here instead of by bevy's `GilrsPlugin`, which always turns force feedback on:
//! gilrs' force-feedback thread then uploads and plays a rumble effect on every pad's node every
//! 50 ms, at zero strength, and over Bluetooth each play is an output report to the pad. An Xbox
//! pad on a Deck fails them (`hid-microsoft`: "failed to send FF report"), resets, and Steam loses
//! it. Force feedback is on only with `rumble = true` in `pad.toml`.
//!
//! While a Steam Input pad is connected, the other pads gilrs lists are the raw nodes of the
//! controllers Steam reads: the device layer never sees them, as if they were disconnected, until
//! the last Steam pad goes. A pinned `device` turns this off. gilrs still opens every pad's node,
//! without grabbing it, so reading it takes nothing from Steam.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use bevy::input::gamepad::{
    GamepadAxis, GamepadButton, GamepadConnection, GamepadConnectionEvent,
    RawGamepadAxisChangedEvent, RawGamepadButtonChangedEvent, RawGamepadEvent,
};
use bevy::input::InputSystems;
use bevy::platform::cell::SyncCell;
use bevy::prelude::*;
use gilrs::ev::filter::axis_dpad_to_button;
use gilrs::{EventType, Filter, GamepadId, GilrsBuilder};

use crate::config::{Device, PadConfig};
use crate::device::PadInfo;

/// Valve's USB vendor id, which Steam Input's virtual pads report.
const STEAM_VENDOR: u16 = 0x28de;

/// Whether `pad` is a Steam Input pad: Valve's vendor id, or the `X-Box 360 pad` name Steam gives
/// the virtual pads it creates.
fn is_steam_input(pad: &PadInfo) -> bool {
    pad.vendor == Some(STEAM_VENDOR) || pad.name.contains("X-Box 360 pad")
}

/// Whether gilrs drives force feedback: only when the player turned rumble on.
fn force_feedback(cfg: &PadConfig) -> bool {
    cfg.rumble
}

/// The pads gilrs lists and which of them the device layer sees.
struct Selection<K> {
    /// A pinned `device` shows every pad and the device layer picks the pinned one.
    pinned: bool,
    pads: HashMap<K, PadInfo>,
    shown: HashSet<K>,
}

impl<K: Copy + Eq + Hash> Selection<K> {
    fn new(cfg: &PadConfig) -> Self {
        Selection {
            pinned: matches!(cfg.device, Device::Pinned { .. }),
            pads: HashMap::new(),
            shown: HashSet::new(),
        }
    }

    fn shown(&self, id: K) -> bool {
        self.shown.contains(&id)
    }

    /// Whether the connected `pad` is shown, given every pad connected.
    fn reads(&self, pad: &PadInfo) -> bool {
        self.pinned || is_steam_input(pad) || !self.pads.values().any(is_steam_input)
    }

    /// `id` connected as `info`, or gone (`None`): every pad whose visibility changed, and whether
    /// it is shown now.
    fn update(&mut self, id: K, info: Option<PadInfo>) -> Vec<(K, bool)> {
        match info {
            Some(info) => {
                self.pads.insert(id, info);
            }
            None => {
                self.pads.remove(&id);
            }
        }
        let mut changes: Vec<(K, bool)> = self
            .pads
            .iter()
            .map(|(&k, info)| (k, self.reads(info)))
            .filter(|&(k, read)| read != self.shown.contains(&k))
            .collect();
        if !self.pads.contains_key(&id) && self.shown.contains(&id) {
            changes.push((id, false));
        }
        for &(k, read) in &changes {
            if read {
                self.shown.insert(k);
            } else {
                self.shown.remove(&k);
            }
        }
        changes
    }
}

/// gilrs, its pads' entities, and which pads the device layer sees.
#[derive(Resource)]
struct Pads {
    gilrs: SyncCell<gilrs::Gilrs>,
    entities: HashMap<GamepadId, Entity>,
    selection: Selection<GamepadId>,
    trace: bool,
}

/// gilrs as a plugin, in place of bevy's `GilrsPlugin`, which stays disabled. It also loads
/// `pad.toml` and leaves it as a resource for [`crate::PadPlugin`].
pub(crate) struct GilrsBackend;

impl Plugin for GilrsBackend {
    fn build(&self, app: &mut App) {
        let cfg = PadConfig::load();
        let ff = force_feedback(&cfg);
        info!(
            "pad: force feedback {}",
            if ff { "on" } else { "off (pad.toml rumble)" }
        );
        // Everything but force feedback as bevy's `GilrsPlugin` builds it.
        match GilrsBuilder::new()
            .with_default_filters(false)
            .set_update_state(false)
            .with_force_feedback(ff)
            .build()
        {
            Ok(gilrs) => {
                app.insert_resource(Pads {
                    gilrs: SyncCell::new(gilrs),
                    entities: HashMap::new(),
                    selection: Selection::new(&cfg),
                    trace: crate::pad_trace(),
                })
                .add_systems(PreStartup, connect_listed)
                .add_systems(PreUpdate, read_gilrs.before(InputSystems));
            }
            Err(e) => error!("pad: gilrs did not start: {e}"),
        }
        app.insert_resource(cfg);
    }
}

/// The messages bevy's `GilrsPlugin` writes for gilrs' events.
#[derive(bevy::ecs::system::SystemParam)]
struct Out<'w> {
    raw: MessageWriter<'w, RawGamepadEvent>,
    connections: MessageWriter<'w, GamepadConnectionEvent>,
    buttons: MessageWriter<'w, RawGamepadButtonChangedEvent>,
    axes: MessageWriter<'w, RawGamepadAxisChangedEvent>,
}

/// The pads gilrs found at start.
fn connect_listed(mut commands: Commands, mut pads: ResMut<Pads>, mut out: Out) {
    let Pads {
        gilrs,
        entities,
        selection,
        trace,
    } = &mut *pads;
    let gilrs = gilrs.get();
    let listed: Vec<GamepadId> = gilrs.gamepads().map(|(id, _)| id).collect();
    for id in listed {
        connect(
            &mut commands,
            gilrs,
            entities,
            selection,
            *trace,
            id,
            &mut out,
        );
    }
}

/// gilrs' events since last frame, for the pads shown.
fn read_gilrs(mut commands: Commands, mut pads: ResMut<Pads>, mut out: Out) {
    let Pads {
        gilrs,
        entities,
        selection,
        trace,
    } = &mut *pads;
    let gilrs = gilrs.get();
    while let Some(ev) = gilrs.next_event().filter_ev(&axis_dpad_to_button, gilrs) {
        gilrs.update(&ev);
        let entity = entities.get(&ev.id).copied();
        match ev.event {
            EventType::Connected => connect(
                &mut commands,
                gilrs,
                entities,
                selection,
                *trace,
                ev.id,
                &mut out,
            ),
            EventType::Disconnected => {
                let changes = selection.update(ev.id, None);
                announce(entities, selection, *trace, &changes, &mut out);
            }
            EventType::ButtonChanged(b, value, _) if selection.shown(ev.id) => {
                let (Some(e), Some(button)) = (entity, convert_button(b)) else {
                    continue;
                };
                out.raw
                    .write(RawGamepadButtonChangedEvent::new(e, button, value).into());
                out.buttons
                    .write(RawGamepadButtonChangedEvent::new(e, button, value));
            }
            EventType::AxisChanged(a, value, _) if selection.shown(ev.id) => {
                let (Some(e), Some(axis)) = (entity, convert_axis(a)) else {
                    continue;
                };
                out.raw
                    .write(RawGamepadAxisChangedEvent::new(e, axis, value).into());
                out.axes
                    .write(RawGamepadAxisChangedEvent::new(e, axis, value));
            }
            _ => {}
        }
    }
    gilrs.inc();
}

/// gilrs lists pad `id`: give it an entity and show it, or say it is ignored.
fn connect(
    commands: &mut Commands,
    gilrs: &gilrs::Gilrs,
    entities: &mut HashMap<GamepadId, Entity>,
    selection: &mut Selection<GamepadId>,
    trace: bool,
    id: GamepadId,
    out: &mut Out,
) {
    let pad = gilrs.gamepad(id);
    let info = PadInfo {
        name: pad.name().to_owned(),
        vendor: pad.vendor_id(),
        product: pad.product_id(),
    };
    entities
        .entry(id)
        .or_insert_with(|| commands.spawn_empty().id());
    let changes = selection.update(id, Some(info));
    if !selection.shown(id) && !changes.iter().any(|&(k, _)| k == id) {
        ignored(selection, trace, id);
    }
    announce(entities, selection, trace, &changes, out);
}

/// A connect for each pad now shown and a disconnect for each now hidden.
fn announce(
    entities: &HashMap<GamepadId, Entity>,
    selection: &Selection<GamepadId>,
    trace: bool,
    changes: &[(GamepadId, bool)],
    out: &mut Out,
) {
    for &(id, shown) in changes {
        let Some(&entity) = entities.get(&id) else {
            continue;
        };
        let connection = match selection.pads.get(&id) {
            Some(info) if shown => GamepadConnection::Connected {
                name: info.name.clone(),
                vendor_id: info.vendor,
                product_id: info.product,
            },
            _ => GamepadConnection::Disconnected,
        };
        if !shown {
            ignored(selection, trace, id);
        }
        let event = GamepadConnectionEvent::new(entity, connection);
        out.raw.write(event.clone().into());
        out.connections.write(event);
    }
}

/// `WOW_PAD_TRACE=1`: a connected pad the device layer does not see.
fn ignored(selection: &Selection<GamepadId>, trace: bool, id: GamepadId) {
    let Some(info) = selection.pads.get(&id).filter(|_| trace) else {
        return;
    };
    let hex = |v: Option<u16>| v.map_or("?".to_owned(), |v| format!("{v:04x}"));
    info!(
        "pad: ignored \"{}\" vendor={} product={}: a Steam Input pad is connected",
        info.name,
        hex(info.vendor),
        hex(info.product)
    );
}

fn convert_button(button: gilrs::Button) -> Option<GamepadButton> {
    use gilrs::Button as B;
    Some(match button {
        B::South => GamepadButton::South,
        B::East => GamepadButton::East,
        B::North => GamepadButton::North,
        B::West => GamepadButton::West,
        B::C => GamepadButton::C,
        B::Z => GamepadButton::Z,
        B::LeftTrigger => GamepadButton::LeftTrigger,
        B::LeftTrigger2 => GamepadButton::LeftTrigger2,
        B::RightTrigger => GamepadButton::RightTrigger,
        B::RightTrigger2 => GamepadButton::RightTrigger2,
        B::Select => GamepadButton::Select,
        B::Start => GamepadButton::Start,
        B::Mode => GamepadButton::Mode,
        B::LeftThumb => GamepadButton::LeftThumb,
        B::RightThumb => GamepadButton::RightThumb,
        B::DPadUp => GamepadButton::DPadUp,
        B::DPadDown => GamepadButton::DPadDown,
        B::DPadLeft => GamepadButton::DPadLeft,
        B::DPadRight => GamepadButton::DPadRight,
        B::Unknown => return None,
    })
}

/// `axis_dpad_to_button` turns the d-pad axes into buttons, so they have no axis here.
fn convert_axis(axis: gilrs::Axis) -> Option<GamepadAxis> {
    use gilrs::Axis as A;
    Some(match axis {
        A::LeftStickX => GamepadAxis::LeftStickX,
        A::LeftStickY => GamepadAxis::LeftStickY,
        A::LeftZ => GamepadAxis::LeftZ,
        A::RightStickX => GamepadAxis::RightStickX,
        A::RightStickY => GamepadAxis::RightStickY,
        A::RightZ => GamepadAxis::RightZ,
        A::Unknown | A::DPadX | A::DPadY => return None,
    })
}

#[cfg(test)]
mod tests {
    use bevy::input::gamepad::GamepadConnectionEvent;

    use super::*;
    use crate::device::PadDevice;

    /// gilrs' pads by id through [`Selection`] into a [`PadDevice`], as [`read_gilrs`] feeds it.
    struct Rig {
        selection: Selection<u32>,
        device: PadDevice,
    }

    impl Rig {
        fn new(cfg: PadConfig) -> Self {
            Rig {
                selection: Selection::new(&cfg),
                device: PadDevice::new(cfg),
            }
        }

        fn entity(id: u32) -> Entity {
            Entity::from_raw_u32(id).expect("a valid index")
        }

        fn apply(&mut self, changes: Vec<(u32, bool)>, t: f32) {
            for (id, shown) in changes {
                let connection = match self.selection.pads.get(&id) {
                    Some(i) if shown => GamepadConnection::Connected {
                        name: i.name.clone(),
                        vendor_id: i.vendor,
                        product_id: i.product,
                    },
                    _ => GamepadConnection::Disconnected,
                };
                self.device.handle(
                    &RawGamepadEvent::Connection(GamepadConnectionEvent::new(
                        Self::entity(id),
                        connection,
                    )),
                    t,
                );
            }
        }

        fn connect(&mut self, id: u32, name: &str, vendor: u16, product: u16, t: f32) {
            let info = PadInfo {
                name: name.into(),
                vendor: Some(vendor),
                product: Some(product),
            };
            let changes = self.selection.update(id, Some(info));
            self.apply(changes, t);
        }

        fn disconnect(&mut self, id: u32, t: f32) {
            let changes = self.selection.update(id, None);
            self.apply(changes, t);
        }

        fn press(&mut self, id: u32, button: GamepadButton, value: f32, t: f32) {
            if self.selection.shown(id) {
                self.device.handle(
                    &RawGamepadEvent::Button(RawGamepadButtonChangedEvent::new(
                        Self::entity(id),
                        button,
                        value,
                    )),
                    t,
                );
            }
        }

        /// The Deck docked with an Xbox pad over Bluetooth, as gilrs lists them under Steam.
        fn deck_with_xbox_pad(cfg: PadConfig) -> Self {
            let mut rig = Rig::new(cfg);
            rig.connect(XBOX, "Xbox Wireless Controller", 0x045e, 0x0b13, 0.0);
            rig.connect(DECK, "Microsoft X-Box 360 pad 0", 0x28de, 0x11ff, 0.0);
            rig.connect(STEAM_XBOX, "Microsoft X-Box 360 pad 1", 0x28de, 0x11ff, 0.0);
            rig
        }
    }

    const XBOX: u32 = 1;
    const DECK: u32 = 2;
    const STEAM_XBOX: u32 = 3;

    #[test]
    fn steam_input_pads_shadow_the_raw_pad_until_they_go() {
        let mut rig = Rig::deck_with_xbox_pad(PadConfig::default());
        rig.press(XBOX, GamepadButton::South, 1.0, 1.0);
        assert_eq!(rig.device.active(), None, "the raw pad is never read");
        assert!(!rig.device.state().is_down("PAD1"));

        rig.press(STEAM_XBOX, GamepadButton::East, 1.0, 2.0);
        assert_eq!(rig.device.active(), Some(Rig::entity(STEAM_XBOX)));
        assert!(rig.device.state().is_down("PAD2"));
        rig.press(STEAM_XBOX, GamepadButton::East, 0.0, 2.1);
        rig.press(XBOX, GamepadButton::South, 1.0, 9.0);
        assert!(
            !rig.device.state().is_down("PAD1"),
            "not even after silence"
        );

        // The Deck's own controls still play with the Xbox pad off.
        rig.disconnect(STEAM_XBOX, 10.0);
        rig.press(DECK, GamepadButton::North, 1.0, 10.1);
        assert_eq!(rig.device.active(), Some(Rig::entity(DECK)));

        // Steam gone: the raw pad is a pad again.
        rig.disconnect(DECK, 11.0);
        rig.press(XBOX, GamepadButton::South, 1.0, 11.1);
        assert_eq!(rig.device.active(), Some(Rig::entity(XBOX)));
        assert!(rig.device.state().is_down("PAD1"));
    }

    #[test]
    fn a_raw_pad_in_play_lets_go_when_steam_input_arrives() {
        let mut rig = Rig::new(PadConfig::default());
        rig.connect(XBOX, "Xbox Wireless Controller", 0x045e, 0x0b13, 0.0);
        rig.press(XBOX, GamepadButton::South, 1.0, 1.0);
        assert!(rig.device.state().is_down("PAD1"));
        rig.connect(STEAM_XBOX, "Microsoft X-Box 360 pad 1", 0x28de, 0x11ff, 1.5);
        assert_eq!(rig.device.active(), None);
        assert!(!rig.device.state().is_down("PAD1"), "nothing stays held");
        // Steam's pad plays at once, with no silence to wait out.
        rig.press(STEAM_XBOX, GamepadButton::East, 1.0, 1.6);
        assert_eq!(rig.device.active(), Some(Rig::entity(STEAM_XBOX)));
    }

    /// At startup gilrs may list the raw pad before Steam's: in one frame it is shown and hidden
    /// again, each as a raw and a `GamepadConnectionEvent`, as [`announce`] writes them.
    /// `read_pad` reads connects from the latter first, and the raw pad still stays out.
    #[test]
    fn a_raw_pad_listed_before_steams_at_startup_stays_out() {
        let mut app = App::new();
        app.add_message::<RawGamepadEvent>()
            .add_message::<GamepadConnectionEvent>()
            .insert_resource(PadDevice::new(PadConfig::default()))
            .init_resource::<crate::device::PadState>()
            .insert_resource(crate::Trace(false))
            .insert_resource(Time::<bevy::time::Real>::default())
            .add_systems(PreUpdate, crate::read_pad);
        let mut selection = Selection::new(&PadConfig::default());
        let pads = [
            (XBOX, "Xbox Wireless Controller", 0x045e, 0x0b13),
            (STEAM_XBOX, "Microsoft X-Box 360 pad 1", 0x28de, 0x11ff),
        ];
        for (id, name, vendor, product) in pads {
            let info = PadInfo {
                name: name.into(),
                vendor: Some(vendor),
                product: Some(product),
            };
            for (k, shown) in selection.update(id, Some(info.clone())) {
                let connection = if shown {
                    let i = &selection.pads[&k];
                    GamepadConnection::Connected {
                        name: i.name.clone(),
                        vendor_id: i.vendor,
                        product_id: i.product,
                    }
                } else {
                    GamepadConnection::Disconnected
                };
                let event = GamepadConnectionEvent::new(Rig::entity(k), connection);
                app.world_mut()
                    .write_message(RawGamepadEvent::from(event.clone()));
                app.world_mut().write_message(event);
            }
        }
        app.update();
        let press = |app: &mut App, id| {
            app.world_mut().write_message(RawGamepadEvent::Button(
                RawGamepadButtonChangedEvent::new(Rig::entity(id), GamepadButton::South, 1.0),
            ));
            app.update();
            app.world().resource::<PadDevice>().active()
        };
        assert_eq!(press(&mut app, XBOX), None, "the raw pad is out");
        assert_eq!(press(&mut app, STEAM_XBOX), Some(Rig::entity(STEAM_XBOX)));
    }

    #[test]
    fn a_pinned_device_wins_over_steam_input() {
        let (cfg, _) = PadConfig::parse("device = \"045e:0b13\"");
        let mut rig = Rig::deck_with_xbox_pad(cfg);
        rig.press(STEAM_XBOX, GamepadButton::East, 1.0, 1.0);
        assert_eq!(rig.device.active(), None);
        rig.press(XBOX, GamepadButton::South, 1.0, 1.1);
        assert_eq!(rig.device.active(), Some(Rig::entity(XBOX)));
        assert!(rig.device.state().is_down("PAD1"));
    }

    #[test]
    fn without_steam_input_every_pad_is_read_as_before() {
        let mut rig = Rig::new(PadConfig::default());
        rig.connect(1, "Xbox Wireless Controller", 0x045e, 0x0b13, 0.0);
        rig.connect(
            2,
            "Sony Interactive Entertainment Wireless Controller",
            0x054c,
            0x09cc,
            0.0,
        );
        assert!(rig.selection.shown(1) && rig.selection.shown(2));
        rig.press(2, GamepadButton::South, 1.0, 1.0);
        assert_eq!(rig.device.active(), Some(Rig::entity(2)));
    }

    #[test]
    fn steam_input_is_valves_vendor_or_steams_pad_name() {
        let pad = |name: &str, vendor| PadInfo {
            name: name.into(),
            vendor: Some(vendor),
            product: Some(1),
        };
        assert!(is_steam_input(&pad("Steam Deck", 0x28de)));
        assert!(is_steam_input(&pad("Microsoft X-Box 360 pad 0", 0x045e)));
        assert!(!is_steam_input(&pad("Xbox Wireless Controller", 0x045e)));
    }

    #[test]
    fn rumble_off_builds_gilrs_without_force_feedback() {
        assert!(!force_feedback(&PadConfig::default()));
        assert!(!force_feedback(&PadConfig::parse("rumble = false").0));
        assert!(force_feedback(&PadConfig::parse("rumble = true").0));
    }

    /// bevy's `GilrsPlugin` builds gilrs with force feedback on whatever `pad.toml` says, so the
    /// hook never turns it on.
    #[test]
    fn the_hook_adds_this_backend_and_never_bevys_gilrs() {
        let group = bevy::DefaultPlugins
            .build()
            .disable::<bevy::gilrs::GilrsPlugin>();
        let group = crate::enable_gilrs(group);
        assert!(!group.enabled::<bevy::gilrs::GilrsPlugin>());
        assert!(group.enabled::<GilrsBackend>());
    }
}
