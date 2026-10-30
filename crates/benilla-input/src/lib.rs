//! Gamepad input for benilla: a plugin on top of the client ([`benilla_app::run_with_group`]);
//! 1.12.1 has no gamepad support, so none of it lives in benilla.
//!
//! gilrs' events become one [`PadState`] ([`device`]): sticks with dead zones, triggers, and every
//! input as a `PAD*` token ([`tokens`]), digital stick directions included. The tokens enter the
//! binding store through `benilla_app::register_key_tokens` and reach the dispatch as
//! [`benilla_app::BindingKeyInput`] messages carrying the layer's modifiers ([`layers`]), so a
//! pad button is bound, rebound and saved like a key. The pad plays in one of two modes
//! ([`mode`]): gameplay, where the right stick feeds the camera as a held mouselook ([`look`]), and
//! cursor, where a virtual cursor ([`cursor`]) clicks, snaps and scrolls through the UI by synthetic
//! mouse input ([`actions`]) and the pointer seam of `benilla_app::pointer`. In gameplay mode the
//! bumpers lock the next friend or enemy and interact through the soft target ([`soft`]). The bars show pad
//! glyphs ([`glyphs`], the addon of [`addon`]) by the presentation setting ([`presentation`]). The
//! addon also draws a radial ring ([`ring`]) and an on-screen keyboard ([`keyboard`]), which reach
//! the pad through `BenillaPad_*` functions ([`host`]); a long press of `PADBACK` ([`hold`]) shows
//! the keyboard. The pad's own settings are `pad.toml` ([`config`]).
//!
//! Gamepad input is off in stock benilla (`benilla-world`'s `boot.rs` disables `GilrsPlugin`);
//! [`run`] turns it back on from the plugin group's hook, so stock benilla stays as it is.

mod actions;
mod addon;
mod config;
mod cursor;
mod device;
mod glyphs;
mod hold;
mod host;
mod keyboard;
mod layers;
mod look;
mod mode;
mod panels;
mod presentation;
mod ring;
mod soft;
mod tokens;
mod trace;

use std::collections::{BTreeSet, HashMap};

use bevy::app::PluginGroupBuilder;
use bevy::input::gamepad::RawGamepadEvent;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::{MouseButtonInput, MouseMotion, MouseScrollUnit, MouseWheel};
use bevy::input::{ButtonState, InputSystems};
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};

use benilla_app::{AppExit, BindingKeyInput, BuildId, CoverUp, SoftTargetInput, SoftTargetReady};
use benilla_ui::script::UiScript;

use actions::Act;
use config::CursorStick;
use host::Host;

pub use config::{Device, PadConfig, Presentation, Source};
pub use device::{PadDevice, PadInfo, PadState};
pub use layers::Modifiers;
pub use mode::Mode;

/// The pad's input systems, for a crate that is another source of pad input to order against.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PadSystems {
    /// Reads the raw pad events into [`PadState`]. A source writes its `RawGamepadEvent`s, and
    /// sets [`Laser`], before it.
    Read,
    /// Opens and closes the look session and feeds the camera. A source that turns the camera
    /// itself ([`LookFeed`]) writes its motion after [`PadSystems::Read`] and before this.
    Look,
}

/// Who moves the camera while gameplay mode holds the look session. The right stick does by
/// default ([`look::motion`]). A source with a turn of its own (VR's snap and smooth turn) sets
/// `external` and writes the mouse motion, in pixels, that it wants this frame to `motion`; the
/// session, the loading cover and the ring's hold on the camera stay the pad's.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq)]
pub struct LookFeed {
    pub external: bool,
    pub motion: Vec2,
}

/// Set by a crate that gives both stick clicks held together a meaning of its own (VR's recentre):
/// while the left click is down, the right click goes to no binding, so the chord does not also
/// toggle autorun.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct StickChord(pub bool);

/// Whether the stick-click chord takes `token`: the right click while the left is down.
fn chord_takes(token: &str, down: &BTreeSet<&'static str>) -> bool {
    token == "PADRSTICK" && down.contains("PADLSTICK")
}

/// The mode the pad plays in, kept current for a source that reports it (VR's trace).
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PadMode(pub Mode);

/// Set to have [`PadPlugin`] write a [`PadRouted`] for every token it routes.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct RoutedTrace(pub bool);

/// Where a pad token went: `what` is the command a binding resolved to, or the cursor act or
/// soft-target press that took it instead. Written only while [`RoutedTrace`] is on.
#[derive(Message, Clone, Debug)]
pub struct PadRouted {
    pub token: &'static str,
    pub pressed: bool,
    pub what: String,
}

/// A laser pointer, set by a crate that has one (VR). While it is `tracked` the right trigger
/// clicks in cursor mode, and in gameplay mode a tap of either trigger acts on what the laser
/// hovers ([`soft::Tap`]). While `on_panel`, the pad is in cursor mode and the right stick scrolls
/// instead of moving the pad's cursor ([`actions::scroll`]).
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Laser {
    pub on_panel: bool,
    pub tracked: bool,
}

impl Laser {
    /// The laser has the pointer: it clicks, wherever it points.
    fn pointing(&self) -> bool {
        self.on_panel || self.tracked
    }
}

/// The client with a gamepad: [`benilla_app::run_with_group`] with gilrs on and [`PadPlugin`] added.
pub fn run(build: BuildId) -> AppExit {
    benilla_app::run_with_group(build, enable_gilrs, |app| {
        app.add_plugins(PadPlugin);
    })
}

/// The plugin group hook that turns gilrs on. A launcher that composes more hooks (VR) calls it.
pub fn enable_gilrs(group: PluginGroupBuilder) -> PluginGroupBuilder {
    group.enable::<bevy::gilrs::GilrsPlugin>()
}

/// Pad input: the device layer, the tokens and default bindings, the modes, the cursor and the
/// camera stick.
pub struct PadPlugin;

impl Plugin for PadPlugin {
    fn build(&self, app: &mut App) {
        benilla_app::register_key_tokens(&tokens::ALL);
        let defaults = layers::default_bindings();
        let pairs: Vec<(&str, &str)> = defaults
            .iter()
            .map(|(k, c)| (k.as_str(), c.as_str()))
            .collect();
        benilla_app::register_default_bindings(&pairs);

        if let Some(dir) = benilla_app::config_dir() {
            if let Err(e) = addon::install(&dir) {
                warn!("pad: writing the {} addon: {e}", addon::NAME);
            }
        }

        let cfg = PadConfig::load();
        app.insert_resource(PadDevice::new(cfg.clone()))
            .insert_resource(cfg.soft)
            .insert_resource(cfg)
            .init_resource::<PadState>()
            .init_resource::<Laser>()
            .init_resource::<LookFeed>()
            .init_resource::<StickChord>()
            .init_resource::<PadMode>()
            .init_resource::<RoutedTrace>()
            .init_resource::<Runtime>()
            .init_resource::<Host>()
            .insert_resource(Trace(
                std::env::var_os("WOW_PAD_TRACE").is_some_and(|v| v != "0"),
            ))
            .add_message::<BindingKeyInput>()
            .add_message::<SoftTargetInput>()
            .add_message::<PadRouted>()
            // Before `InputSystems`: the motion, buttons and keys the pad writes are accumulated
            // there, and the `BindingKeyInput`s are read by the dispatch in `Update`.
            .add_systems(
                PreUpdate,
                (
                    read_pad.in_set(PadSystems::Read),
                    read_world,
                    step_mode,
                    drive_soft,
                    route_keys,
                    take_asks,
                    drive_cursor,
                    drive_text,
                    drive_look.in_set(PadSystems::Look),
                    push_state,
                )
                    .chain()
                    .before(InputSystems),
            );
    }
}

/// `WOW_PAD_TRACE=1`.
#[derive(Resource)]
struct Trace(bool);

/// What the pad's systems share across a frame: each one's answer is the next one's input.
#[derive(Resource, Default)]
struct Runtime {
    machine: mode::Machine,
    cursor: cursor::VirtualCursor,
    /// The windows and targeting cursor the UI showed at the last poll.
    reading: panels::Reading,
    /// FrameXML's UI is loaded, so there is a world to play in.
    world: bool,
    poll: f32,
    auto: presentation::Auto,
    /// A real mouse moved or clicked since the last frame.
    real_mouse: bool,
    /// Inputs the pad wrote last frame, which its readers must not take for the player's.
    own: Own,
    back: hold::BackHold,
    /// A trigger pressed under the laser that may still be a tap.
    tap: soft::Tap,
    /// `PADBACK` was held long: the keyboard is to be shown or hidden.
    toggle_keyboard: bool,
    left_was: bool,
    prev_down: BTreeSet<&'static str>,
    /// How each pressed token was routed, for its release.
    routed: HashMap<&'static str, Routed>,
    acts: Vec<Act>,
    /// Wheel travel the right stick has built up while the laser is on the panel.
    scroll: f32,
    /// The OS cursor is hidden because the pad's is drawn.
    hid: bool,
    /// The soft target was last told to run.
    soft_on: bool,
    pushed: Pushed,
}

#[derive(Default)]
struct Own {
    motion: usize,
    buttons: usize,
    keys: usize,
}

#[derive(Clone, Copy)]
enum Routed {
    /// Sent to the bindings, so its release goes there too.
    Binding,
    /// Taken by the cursor; a button's release undoes its press.
    Cursor(Option<Act>),
    /// A targeting press ([`soft`]): nothing is held, so its release does nothing.
    Soft,
    /// `PADBACK` in gameplay mode: the mode and the keyboard read it ([`hold`]), not the bindings.
    Back,
    /// The right stick click of a [`StickChord`]: nothing is held, so its release does nothing.
    Chord,
}

#[derive(Default)]
struct Pushed {
    session: Option<u64>,
    state: String,
    scale_done: bool,
}

/// The raw events since last frame into [`PadDevice`], and its state out to [`PadState`].
fn read_pad(
    mut raw: MessageReader<RawGamepadEvent>,
    time: Res<Time<Real>>,
    mut device: ResMut<PadDevice>,
    mut state: ResMut<PadState>,
    trace: Res<Trace>,
) {
    let now = time.elapsed_secs();
    for ev in raw.read() {
        device.handle(ev, now);
    }
    for pad in device.take_connects() {
        if trace.0 {
            let id = |v: Option<u16>| v.map_or("?".to_owned(), |v| format!("{v:04x}"));
            info!(
                "pad: connected \"{}\" vendor={} product={}",
                pad.name,
                id(pad.vendor),
                id(pad.product)
            );
        }
    }
    if *state != *device.state() {
        *state = device.state().clone();
    }
}

/// What the UI says (the open windows, a targeting cursor), what the player's own hands did (a real
/// mouse, a key), and what that means for the presentation.
fn read_world(
    state: Res<PadState>,
    time: Res<Time<Real>>,
    script: Option<NonSend<UiScript>>,
    mut rt: ResMut<Runtime>,
    mut motion: MessageReader<MouseMotion>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut keys: MessageReader<KeyboardInput>,
) {
    rt.poll += time.delta_secs();
    if let Some(script) = script {
        if rt.poll >= panels::POLL {
            rt.poll = 0.0;
            match panels::read(&script) {
                Some(r) => {
                    rt.world = script
                        .eval::<bool>("return UIPanelWindows ~= nil")
                        .unwrap_or(false);
                    rt.reading = r;
                }
                None => rt.world = false,
            }
        }
    } else {
        rt.world = false;
    }

    // Their own are the ones the pad wrote last frame, which come back through these readers.
    let real_motion = motion.read().count().saturating_sub(rt.own.motion);
    let real_buttons = buttons
        .read()
        .filter(|b| b.state == ButtonState::Pressed)
        .count()
        .saturating_sub(rt.own.buttons);
    let real_keys = keys
        .read()
        .filter(|k| k.state == ButtonState::Pressed)
        .count()
        .saturating_sub(rt.own.keys);
    rt.own = Own::default();
    rt.real_mouse = real_motion > 0 || real_buttons > 0;
    // Bare pointer motion is not input for the presentation: a trackpad hover must not flip it.
    if real_keys > 0 || real_buttons > 0 {
        rt.auto.kbm_input();
    }
    if state.is_changed()
        && (!state.down().is_empty() || state.left != Vec2::ZERO || state.right != Vec2::ZERO)
    {
        rt.auto.pad_input();
    }
}

/// `PADBACK`, the open windows and the left stick into the mode.
fn step_mode(
    state: Res<PadState>,
    cfg: Res<PadConfig>,
    laser: Res<Laser>,
    trace: Res<Trace>,
    time: Res<Time<Real>>,
    mut rt: ResMut<Runtime>,
    mut current: ResMut<PadMode>,
) {
    let back = rt.back.step(state.is_down("PADBACK"), time.delta_secs());
    rt.toggle_keyboard |= back == hold::Back::Long;
    let left = state.left != Vec2::ZERO;
    let signals = mode::Signals {
        panels: &rt.reading.panels.clone(),
        targeting: rt.reading.targeting,
        back: back == hold::Back::Tap,
        // Pushing the stick again is a walk; a stick already held when a window opened is not.
        walk: left && !rt.left_was && cfg.cursor_stick == CursorStick::Right,
        pointing: laser.on_panel,
    };
    let before = rt.machine.mode();
    let after = rt.machine.step(signals);
    rt.left_was = left;
    if current.0 != after {
        current.0 = after;
    }
    if trace.0 && before != after {
        info!(
            "pad: mode {} (windows: {}, targeting: {})",
            if after == Mode::Cursor {
                "cursor"
            } else {
                "gameplay"
            },
            rt.reading
                .panels
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(","),
            rt.reading.targeting
        );
    }
}

/// Tells the soft target when the pad plays in gameplay mode, which is when it runs.
fn drive_soft(
    device: Res<PadDevice>,
    trace: Res<Trace>,
    mut rt: ResMut<Runtime>,
    mut out: MessageWriter<SoftTargetInput>,
) {
    let on = wants_look(rt.world, rt.machine.mode(), device.connected());
    if on != rt.soft_on {
        rt.soft_on = on;
        out.write(SoftTargetInput::Active(on));
        if trace.0 {
            info!("pad: soft target {}", if on { "on" } else { "off" });
        }
    }
}

/// The interact commands, `INTERACTTARGET` and `INTERACTORJUMP`, whose bodies in the shipped addon
/// queue an ask ([`soft::asks`]). A key or a pad button bound to either lands here, one frame after
/// the binding ran. An interact goes to the soft target; the contextual one with nothing in front
/// runs the `JUMP` command, so a rebound or edited jump is the player's own.
fn take_asks(
    script: Option<NonSend<UiScript>>,
    ready: Option<Res<SoftTargetReady>>,
    trace: Res<Trace>,
    mut out: MessageWriter<SoftTargetInput>,
) {
    let Some(script) = script else {
        return;
    };
    let Ok(Some(text)) = script.eval::<Option<String>>(soft::TAKE_ASKS) else {
        return;
    };
    let ready = ready.is_some_and(|r| r.0);
    for ask in soft::asks(&text) {
        let action = soft::ask_action(ask, ready);
        if trace.0 {
            info!("pad: {ask:?} (service in front: {ready}) -> {action:?}");
        }
        match action {
            Some(input) => {
                out.write(input);
            }
            None => {
                if let Err(e) = script.execute_binding("JUMP", true) {
                    warn!("pad: JUMP: {e}");
                }
            }
        }
    }
}

/// The tokens that went down and up between two states, in token order.
fn edges(
    prev: &BTreeSet<&'static str>,
    now: &BTreeSet<&'static str>,
) -> (Vec<&'static str>, Vec<&'static str>) {
    (
        prev.difference(now).copied().collect(),
        now.difference(prev).copied().collect(),
    )
}

/// One [`BindingKeyInput`] per input that changed, releases first, a press carrying the layer. In
/// cursor mode the buttons the cursor takes ([`actions`]) become acts for [`drive_cursor`] instead,
/// and a release always goes where its press went.
fn route_keys(
    state: Res<PadState>,
    cfg: Res<PadConfig>,
    laser: Res<Laser>,
    chord: Res<StickChord>,
    trace: Res<Trace>,
    routed_on: Res<RoutedTrace>,
    script: Option<NonSend<UiScript>>,
    mut rt: ResMut<Runtime>,
    mut out: MessageWriter<BindingKeyInput>,
    mut soft_out: MessageWriter<SoftTargetInput>,
    mut routed_out: MessageWriter<PadRouted>,
) {
    let rt = &mut *rt;
    if rt.prev_down == *state.down() {
        return;
    }
    let cursor_mode = rt.machine.mode() == Mode::Cursor;
    let left_is_cursor = cfg.cursor_stick == CursorStick::Left;
    let (up, down) = edges(&rt.prev_down, state.down());
    let mods = layers::modifiers(&cfg, &state);
    for token in up {
        let tapped = rt.tap.up(token).filter(|_| laser.tracked);
        if let Some(input) = tapped {
            soft_out.write(input);
            if trace.0 {
                info!("pad: {token} tap -> {input:?}");
            }
            if routed_on.0 {
                routed_out.write(PadRouted {
                    token,
                    pressed: false,
                    what: format!("{input:?}"),
                });
            }
        }
        let routed = rt.routed.remove(token);
        let command = match routed {
            Some(Routed::Binding) => {
                out.write(BindingKeyInput {
                    token,
                    pressed: false,
                    alt: mods.alt,
                    ctrl: mods.ctrl,
                    shift: mods.shift,
                });
                "binding"
            }
            Some(Routed::Cursor(act)) => {
                rt.acts.extend(act.and_then(actions::release));
                "cursor"
            }
            Some(Routed::Soft) => "soft",
            Some(Routed::Back) => "mode",
            Some(Routed::Chord) => "chord",
            // Pressed while the pad was not looking (a stick direction the dead zone made up).
            None => continue,
        };
        if trace.0 {
            info!("pad: {token} up -> {command}");
        }
        if routed_on.0 {
            routed_out.write(PadRouted {
                token,
                pressed: false,
                what: command.into(),
            });
        }
    }
    for token in down {
        if chord.0 && chord_takes(token, state.down()) {
            rt.routed.insert(token, Routed::Chord);
            if routed_on.0 {
                routed_out.write(PadRouted {
                    token,
                    pressed: true,
                    what: "chord".into(),
                });
            }
            continue;
        }
        if !cursor_mode && laser.tracked {
            rt.tap.down(token);
        }
        if cursor_mode && actions::consumed(token, left_is_cursor, laser.pointing()) {
            let act = actions::press(token, laser.pointing());
            rt.acts.extend(act);
            rt.routed.insert(token, Routed::Cursor(act));
            if trace.0 {
                info!("pad: {token} down -> cursor:{}", act_name(act));
            }
            if routed_on.0 {
                routed_out.write(PadRouted {
                    token,
                    pressed: true,
                    what: format!("cursor:{}", act_name(act)),
                });
            }
            continue;
        }
        if token == "PADBACK" {
            rt.routed.insert(token, Routed::Back);
            if trace.0 {
                info!("pad: {token} down -> mode");
            }
            if routed_on.0 {
                routed_out.write(PadRouted {
                    token,
                    pressed: true,
                    what: "mode".into(),
                });
            }
            continue;
        }
        if !cursor_mode {
            if let Some(press) = soft::press(token, mods, state.down()) {
                match press {
                    soft::Press::Soft(input) => {
                        soft_out.write(input);
                    }
                    party => {
                        if let (Some(lua), Some(script)) = (party.lua(), script.as_ref()) {
                            if let Err(e) = script.run_binding(&lua) {
                                warn!("pad: {lua}: {e}");
                            }
                        }
                    }
                }
                rt.routed.insert(token, Routed::Soft);
                if trace.0 {
                    info!("pad: {} down -> {press:?}", trace::chord_text(mods, token));
                }
                if routed_on.0 {
                    routed_out.write(PadRouted {
                        token,
                        pressed: true,
                        what: format!("{press:?}"),
                    });
                }
                continue;
            }
        }
        out.write(BindingKeyInput {
            token,
            pressed: true,
            alt: mods.alt,
            ctrl: mods.ctrl,
            shift: mods.shift,
        });
        rt.routed.insert(token, Routed::Binding);
        if trace.0 || routed_on.0 {
            let command = script
                .as_ref()
                .and_then(|s| trace::resolve(&s.binding_keys(), mods, token))
                .unwrap_or_else(|| "-".into());
            if trace.0 {
                info!("pad: {} down -> {command}", trace::chord_text(mods, token));
            }
            if routed_on.0 {
                routed_out.write(PadRouted {
                    token,
                    pressed: true,
                    what: command,
                });
            }
        }
    }
    rt.prev_down = state.down().clone();
}

fn act_name(act: Option<Act>) -> String {
    match act {
        Some(Act::Button { button, shift, .. }) => {
            format!("{}{button:?}", if shift { "shift-" } else { "" })
        }
        Some(Act::Wheel(n)) => format!("wheel {n}"),
        Some(Act::Close) => "close".into(),
        Some(Act::Snap(d)) => format!("snap {d:?}"),
        None => "-".into(),
    }
}

/// The virtual cursor and the synthetic mouse and key input its acts make.
#[allow(clippy::too_many_arguments)]
fn drive_cursor(
    state: Res<PadState>,
    cfg: Res<PadConfig>,
    laser: Res<Laser>,
    trace: Res<Trace>,
    time: Res<Time<Real>>,
    script: Option<NonSend<UiScript>>,
    mut rt: ResMut<Runtime>,
    mut window: Query<(Entity, &mut Window, &mut CursorOptions), With<PrimaryWindow>>,
    mut buttons: MessageWriter<MouseButtonInput>,
    mut wheel: MessageWriter<MouseWheel>,
    mut keys: MessageWriter<KeyboardInput>,
) {
    let rt = &mut *rt;
    let mut acts = std::mem::take(&mut rt.acts);
    let Ok((entity, mut window, mut opts)) = window.single_mut() else {
        return;
    };
    // The laser has the cursor; the right stick scrolls under it and leaves the pad's cursor alone.
    let pointing = laser.on_panel && rt.machine.mode() == Mode::Cursor;
    if pointing {
        let (notches, held) = actions::scroll(rt.scroll, state.right.y, time.delta_secs());
        rt.scroll = held;
        if notches != 0.0 {
            acts.push(Act::Wheel(notches));
        }
    } else {
        rt.scroll = 0.0;
    }
    if rt.machine.mode() == Mode::Cursor {
        let size = Vec2::new(window.width(), window.height());
        let mut input = cursor::Input {
            size,
            os: window.cursor_position(),
            stick: match cfg.cursor_stick {
                _ if pointing => Vec2::ZERO,
                CursorStick::Right => state.right,
                CursorStick::Left => state.left,
            },
            dt: time.delta_secs(),
            speed: cfg.cursor_speed,
            real: rt.real_mouse,
            ..default()
        };
        for act in &acts {
            match act {
                Act::Snap(dir) => {
                    let from = if rt.cursor.owner == cursor::Owner::Pad {
                        rt.cursor.pos
                    } else {
                        input.os.unwrap_or(size / 2.0)
                    };
                    let at = benilla_app::pointer::window_to_ui(size.y, from);
                    let to = script
                        .as_ref()
                        .and_then(|s| s.snap_target((at.x, at.y), *dir));
                    if let Some((x, y)) = to {
                        input.snap =
                            Some(benilla_app::pointer::ui_to_window(size.y, Vec2::new(x, y)));
                    }
                    if trace.0 {
                        info!("pad: snap {dir:?} from {from} -> {to:?}");
                    }
                }
                // A click at the laser is not the pad taking the cursor.
                Act::Button { down: true, .. } if !laser.pointing() => input.pad_action = true,
                _ => {}
            }
        }
        let was = rt.cursor.owner;
        if rt.cursor.step(&input) {
            // Move the OS pointer with it: the world picks (hover, click) read the window's cursor.
            window.set_cursor_position(Some(rt.cursor.pos));
        }
        let pad_owns = rt.cursor.owner == cursor::Owner::Pad;
        benilla_app::pointer::set_pad(pad_owns.then_some(rt.cursor.pos));
        if pad_owns {
            hide_os_cursor(rt, &mut opts);
        } else {
            show_os_cursor(rt, &mut opts);
        }
        if trace.0 && was != rt.cursor.owner {
            info!("pad: cursor -> {:?} at {}", rt.cursor.owner, rt.cursor.pos);
        }
    } else {
        // The OS pointer is the player's again; the pad's cursor starts over from it next time.
        rt.cursor.owner = cursor::Owner::Os;
        benilla_app::pointer::set_pad(None);
        show_os_cursor(rt, &mut opts);
    }

    // A button's release is written whatever the mode is now: its press went down in cursor mode.
    for act in acts {
        match act {
            Act::Button {
                button,
                shift,
                down,
            } => {
                let state = if down {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                let mut key = |state| {
                    keys.write(KeyboardInput {
                        key_code: KeyCode::ShiftLeft,
                        logical_key: Key::Shift,
                        state,
                        text: None,
                        repeat: false,
                        window: entity,
                    });
                    if state == ButtonState::Pressed {
                        rt.own.keys += 1;
                    }
                };
                // Shift is down before the click lands and up after it.
                if shift && down {
                    key(ButtonState::Pressed);
                }
                buttons.write(MouseButtonInput {
                    button,
                    state,
                    window: entity,
                });
                if state == ButtonState::Pressed {
                    rt.own.buttons += 1;
                }
                if shift && !down {
                    key(ButtonState::Released);
                }
                if trace.0 {
                    info!("pad: {button:?} {state:?} at {}", rt.cursor.pos);
                }
            }
            Act::Wheel(y) => {
                wheel.write(MouseWheel {
                    unit: MouseScrollUnit::Line,
                    x: 0.0,
                    y,
                    window: entity,
                });
            }
            Act::Close => {
                // Escape closes the windows and cancels a targeting cursor; with neither it would
                // open the game menu, which is not a close.
                if rt.reading.panels.is_empty() && !rt.reading.targeting {
                    continue;
                }
                for state in [ButtonState::Pressed, ButtonState::Released] {
                    keys.write(KeyboardInput {
                        key_code: KeyCode::Escape,
                        logical_key: Key::Escape,
                        state,
                        text: None,
                        repeat: false,
                        window: entity,
                    });
                }
                rt.own.keys += 1;
            }
            Act::Snap(_) => {}
        }
    }
}

fn hide_os_cursor(rt: &mut Runtime, opts: &mut CursorOptions) {
    if !rt.hid {
        opts.visible = false;
        rt.hid = true;
    }
}

fn show_os_cursor(rt: &mut Runtime, opts: &mut CursorOptions) {
    if rt.hid {
        opts.visible = true;
        rt.hid = false;
    }
}

/// The on-screen keyboard: shown or hidden by a long press of `PADBACK`, and what its keys type
/// written as keyboard input ([`keyboard::Typed`]). The pad's own key presses are counted, so
/// typing does not read as the player's keyboard to the presentation setting.
fn drive_text(
    host: Res<Host>,
    trace: Res<Trace>,
    script: Option<NonSend<UiScript>>,
    mut rt: ResMut<Runtime>,
    window: Query<Entity, With<PrimaryWindow>>,
    mut keys: MessageWriter<KeyboardInput>,
) {
    if std::mem::take(&mut rt.toggle_keyboard) {
        if let Some(script) = script.as_ref() {
            match script.run("BenillaPadToggleKeyboard()") {
                Ok(()) if trace.0 => info!("pad: keyboard toggled"),
                Ok(()) => {}
                Err(e) => warn!("pad: keyboard: {e}"),
            }
        }
    }
    let typed = host.take_typed();
    let Ok(entity) = window.single() else {
        return;
    };
    for t in typed {
        if trace.0 {
            info!("pad: type {t:?}");
        }
        for event in t.events(entity) {
            if event.state == ButtonState::Pressed {
                rt.own.keys += 1;
            }
            keys.write(event);
        }
    }
}

/// Whether the pad holds a mouselook session: in gameplay mode with a pad to play it. With no pad
/// connected the mouse owns the camera and the cursor.
fn wants_look(world: bool, mode: Mode, connected: bool) -> bool {
    world && connected && mode == Mode::Gameplay
}

/// The right stick as a held mouselook session ([`look`]), open for all of gameplay mode.
fn drive_look(
    state: Res<PadState>,
    cfg: Res<PadConfig>,
    device: Res<PadDevice>,
    trace: Res<Trace>,
    time: Res<Time<Real>>,
    cover: CoverUp,
    host: Res<Host>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut feed: ResMut<LookFeed>,
    mut rt: ResMut<Runtime>,
    mut session: Local<look::Session>,
    mut script: Option<NonSendMut<UiScript>>,
    mut motion: MessageWriter<MouseMotion>,
) {
    let dt = time.delta_secs();
    let covered = cover.get();
    let want = wants_look(rt.world, rt.machine.mode(), device.connected());
    if let Some(call) = session.step(want, covered, mouse.pressed(MouseButton::Right)) {
        if let Some(s) = script.as_mut() {
            let (chunk, open) = match call {
                look::Call::Start => ("MouselookStart()", true),
                look::Call::Stop => ("MouselookStop()", false),
            };
            match s.run(chunk) {
                Ok(()) => {
                    session.open = open;
                    if trace.0 {
                        info!("pad: {chunk}");
                    }
                }
                Err(e) => warn!("pad: {chunk}: {e}"),
            }
        }
    }
    // The ring's stick picks an entry; the camera stays where it is while the ring is open.
    let delta = look::feed(&mut feed, state.right, &cfg, dt);
    if session.open && want && !covered && !host.ring_open() && delta != Vec2::ZERO {
        motion.write(MouseMotion { delta });
        rt.own.motion += 1;
    }
}

/// The state the addon reads, the glyph tables with it, and the UI scale the player asked for.
fn push_state(
    cfg: Res<PadConfig>,
    device: Res<PadDevice>,
    trace: Res<Trace>,
    host: Res<Host>,
    state: Res<PadState>,
    script: Option<NonSend<UiScript>>,
    mut rt: ResMut<Runtime>,
) {
    host.set(state.left, state.right, rt.machine.mode() == Mode::Cursor);
    let Some(script) = script else {
        return;
    };
    let rt = &mut *rt;
    if rt.pushed.session != Some(script.session()) {
        if let Err(e) = host.install(script.lua()) {
            warn!("pad: BenillaPad_* functions: {e}");
            return;
        }
        match script.run(&format!(
            "{} {}",
            glyphs::lua_tables(&cfg),
            ring::lua_table()
        )) {
            Ok(()) => {
                rt.pushed = Pushed {
                    session: Some(script.session()),
                    ..default()
                }
            }
            Err(e) => {
                warn!("pad: glyph tables: {e}");
                return;
            }
        }
    }
    let look = rt.auto.resolve(cfg.presentation, device.connected());
    let text = format!(
        "BenillaPadState = {{mode=\"{}\",cursor={},presentation=\"{}\"}}",
        if rt.machine.mode() == Mode::Cursor {
            "cursor"
        } else {
            "gameplay"
        },
        u8::from(rt.machine.mode() == Mode::Cursor && rt.cursor.owner == cursor::Owner::Pad),
        look.name()
    );
    if text != rt.pushed.state && script.run(&text).is_ok() {
        if trace.0 && !rt.pushed.state.ends_with(look.name()) {
            info!("pad: presentation {}", look.name());
        }
        rt.pushed.state = text;
    }
    if let (Some(scale), true, false) = (cfg.ui_scale, rt.world, rt.pushed.scale_done) {
        let chunk = format!("SetCVar(\"useUiScale\", \"1\") SetCVar(\"uiScale\", \"{scale}\")");
        if script.run(&chunk).is_ok() {
            rt.pushed.scale_done = true;
            if trace.0 {
                info!("pad: uiScale {scale}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::mouse::MouseButton;

    /// The systems that read a frame's input and route the pad's, with no gilrs and no UI.
    fn app() -> App {
        let mut app = App::new();
        app.add_message::<BindingKeyInput>()
            .add_message::<MouseMotion>()
            .add_message::<MouseButtonInput>()
            .add_message::<MouseWheel>()
            .add_message::<KeyboardInput>()
            .add_message::<SoftTargetInput>()
            .init_resource::<PadState>()
            .init_resource::<Laser>()
            .init_resource::<StickChord>()
            .init_resource::<PadMode>()
            .init_resource::<RoutedTrace>()
            .add_message::<PadRouted>()
            .init_resource::<Runtime>()
            .init_resource::<ButtonInput<MouseButton>>()
            .insert_resource(PadConfig::default())
            .insert_resource(Trace(false))
            .insert_resource(Time::<Real>::default())
            .add_systems(Update, (read_world, step_mode, route_keys).chain());
        app.world_mut()
            .spawn((Window::default(), PrimaryWindow, CursorOptions::default()));
        app
    }

    fn hold(app: &mut App, down: &[&'static str]) {
        *app.world_mut().resource_mut::<PadState>() = device::state_with(PadState::default(), down);
    }

    fn open(app: &mut App, panels: &[&str]) {
        app.world_mut().resource_mut::<Runtime>().reading.panels =
            panels.iter().map(|p| (*p).to_owned()).collect();
    }

    fn bindings(app: &App) -> Vec<(&'static str, bool)> {
        let m = app.world().resource::<Messages<BindingKeyInput>>();
        m.iter_current_update_messages()
            .map(|b| (b.token, b.pressed))
            .collect()
    }

    fn clicks(app: &App) -> Vec<(MouseButton, ButtonState)> {
        let m = app.world().resource::<Messages<MouseButtonInput>>();
        m.iter_current_update_messages()
            .map(|b| (b.button, b.state))
            .collect()
    }

    #[test]
    fn the_stick_chord_takes_the_right_click_only_while_the_left_is_down() {
        let down = |t: &[&'static str]| t.iter().copied().collect::<BTreeSet<_>>();
        assert!(chord_takes("PADRSTICK", &down(&["PADLSTICK", "PADRSTICK"])));
        assert!(!chord_takes("PADRSTICK", &down(&["PADRSTICK"])));
        assert!(!chord_takes(
            "PADLSTICK",
            &down(&["PADLSTICK", "PADRSTICK"])
        ));

        let mut off = app();
        hold(&mut off, &["PADLSTICK", "PADRSTICK"]);
        off.update();
        let both: Vec<_> = bindings(&off).into_iter().map(|b| b.0).collect();
        assert_eq!(both, ["PADLSTICK", "PADRSTICK"], "off, both are bindings");

        let mut on = app();
        on.world_mut().insert_resource(StickChord(true));
        hold(&mut on, &["PADLSTICK", "PADRSTICK"]);
        on.update();
        assert_eq!(bindings(&on), [("PADLSTICK", true)], "on, jump only");
        hold(&mut on, &[]);
        on.update();
        assert_eq!(
            bindings(&on),
            [("PADLSTICK", false)],
            "the chord's release sends nothing"
        );
    }

    #[test]
    fn no_pad_means_no_look_session_and_a_disconnect_closes_it() {
        let mut session = look::Session::default();
        let mut frame = |connected: bool| {
            let want = wants_look(true, Mode::Gameplay, connected);
            let call = session.step(want, false, false);
            match call {
                Some(look::Call::Start) => session.open = true,
                Some(look::Call::Stop) => session.open = false,
                None => {}
            }
            call
        };
        assert_eq!(frame(false), None, "no pad, no session");
        assert_eq!(frame(false), None);
        assert_eq!(frame(true), Some(look::Call::Start));
        assert_eq!(frame(true), None);
        assert_eq!(frame(false), Some(look::Call::Stop), "the pad went away");
        assert_eq!(frame(false), None);
        assert!(
            !wants_look(false, Mode::Gameplay, true),
            "no world, no look"
        );
        assert!(
            !wants_look(true, Mode::Cursor, true),
            "cursor mode, no look"
        );
    }

    /// One scenario, in order: the pad globals the seam holds are shared, so it is one test.
    #[test]
    fn the_same_button_presses_an_action_in_gameplay_and_a_click_in_cursor_mode() {
        let mut app = app();
        // The cursor writes the process-wide pointer seam, which no other test may share.
        app.add_systems(Update, drive_cursor.after(route_keys));
        // Gameplay mode: PAD1 is the first action slot.
        hold(&mut app, &["PAD1"]);
        app.update();
        assert_eq!(bindings(&app), [("PAD1", true)]);
        assert!(clicks(&app).is_empty());

        // A window opens with PAD1 still held: its release still goes to the bindings.
        open(&mut app, &["ContainerFrame1"]);
        hold(&mut app, &[]);
        app.update();
        assert_eq!(
            app.world().resource::<Runtime>().machine.mode(),
            Mode::Cursor
        );
        assert_eq!(
            bindings(&app),
            [("PAD1", false)],
            "a press is released where it went"
        );
        assert!(clicks(&app).is_empty());

        // Cursor mode: PAD1 is a right click at the cursor, down and up, and not an action.
        hold(&mut app, &["PAD1"]);
        app.update();
        assert!(bindings(&app).is_empty());
        assert_eq!(clicks(&app), [(MouseButton::Right, ButtonState::Pressed)]);
        assert_eq!(
            app.world().resource::<Runtime>().cursor.owner,
            cursor::Owner::Pad
        );
        hold(&mut app, &[]);
        app.update();
        assert_eq!(clicks(&app), [(MouseButton::Right, ButtonState::Released)]);

        // PAD4 is a shift-left-click: the shift is down for the click and up after it.
        hold(&mut app, &["PAD4"]);
        app.update();
        assert_eq!(clicks(&app), [(MouseButton::Left, ButtonState::Pressed)]);
        let keys = |app: &App| -> Vec<(KeyCode, ButtonState)> {
            app.world()
                .resource::<Messages<KeyboardInput>>()
                .iter_current_update_messages()
                .map(|k| (k.key_code, k.state))
                .collect()
        };
        assert_eq!(keys(&app), [(KeyCode::ShiftLeft, ButtonState::Pressed)]);
        hold(&mut app, &[]);
        app.update();
        assert_eq!(keys(&app), [(KeyCode::ShiftLeft, ButtonState::Released)]);

        // The mode's other buttons still work: the menu button is a binding in cursor mode.
        hold(&mut app, &["PADFORWARD"]);
        app.update();
        assert_eq!(bindings(&app), [("PADFORWARD", true)]);
        hold(&mut app, &[]);
        app.update();

        // The cursor stick moves the pad's cursor, which the seam reports; a real mouse takes
        // the cursor back without leaving the mode.
        app.world_mut().resource_mut::<PadState>().right = Vec2::new(1.0, 0.0);
        app.update();
        assert!(benilla_app::pointer::ui_cursor(&Window::default()).is_some());
        app.world_mut()
            .write_message(MouseMotion { delta: Vec2::X });
        app.update();
        assert_eq!(
            app.world().resource::<Runtime>().machine.mode(),
            Mode::Cursor
        );
        assert_eq!(
            app.world().resource::<Runtime>().cursor.owner,
            cursor::Owner::Os
        );
        assert_eq!(benilla_app::pointer::ui_cursor(&Window::default()), None);

        // A click held while the window closes is still released.
        hold(&mut app, &["PAD3"]);
        app.update();
        assert_eq!(clicks(&app), [(MouseButton::Left, ButtonState::Pressed)]);
        open(&mut app, &[]);
        hold(&mut app, &[]);
        app.update();
        assert_eq!(
            clicks(&app),
            [(MouseButton::Left, ButtonState::Released)],
            "no stuck button when the mode ends under a press"
        );

        // The window closes: gameplay mode, and the pad lets go of the pointer.
        app.world_mut().resource_mut::<PadState>().right = Vec2::ZERO;
        open(&mut app, &[]);
        app.update();
        assert_eq!(
            app.world().resource::<Runtime>().machine.mode(),
            Mode::Gameplay
        );
        assert_eq!(benilla_app::pointer::ui_cursor(&Window::default()), None);

        // A ground spell: the armed targeting cursor is cursor mode, a left click commits it at the
        // cursor, and putting the cursor away is gameplay mode again.
        app.world_mut().resource_mut::<Runtime>().reading.targeting = true;
        app.update();
        assert_eq!(
            app.world().resource::<Runtime>().machine.mode(),
            Mode::Cursor
        );
        hold(&mut app, &["PAD3"]);
        app.update();
        assert_eq!(clicks(&app), [(MouseButton::Left, ButtonState::Pressed)]);
        assert!(bindings(&app).is_empty(), "a click, not the bar slot");
        hold(&mut app, &[]);
        app.update();
        assert_eq!(clicks(&app), [(MouseButton::Left, ButtonState::Released)]);
        app.world_mut().resource_mut::<Runtime>().reading.targeting = false;
        app.update();
        assert_eq!(
            app.world().resource::<Runtime>().machine.mode(),
            Mode::Gameplay
        );

        // A laser on the panel is cursor mode with nothing open: the right trigger is a left click
        // held for a drag, not the control layer, and the right stick scrolls and moves no cursor.
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(std::time::Duration::from_millis(100));
        app.world_mut().resource_mut::<Laser>().on_panel = true;
        app.update();
        assert_eq!(
            app.world().resource::<Runtime>().machine.mode(),
            Mode::Cursor
        );
        hold(&mut app, &["PADRTRIGGER"]);
        app.update();
        assert!(bindings(&app).is_empty(), "a click, not a layer");
        assert_eq!(clicks(&app), [(MouseButton::Left, ButtonState::Pressed)]);
        hold(&mut app, &[]);
        app.world_mut().resource_mut::<PadState>().right = Vec2::new(0.0, 1.0);
        app.update();
        assert_eq!(clicks(&app), [(MouseButton::Left, ButtonState::Released)]);
        let wheel = |app: &App| -> Vec<f32> {
            app.world()
                .resource::<Messages<MouseWheel>>()
                .iter_current_update_messages()
                .map(|w| w.y)
                .collect()
        };
        // 0.1 s at full travel is 1.2 notches: one now, the rest kept.
        assert_eq!(wheel(&app), [1.0]);
        assert_eq!(
            app.world().resource::<Runtime>().cursor.owner,
            cursor::Owner::Os,
            "the stick that scrolls does not take the cursor"
        );
        assert_eq!(benilla_app::pointer::ui_cursor(&Window::default()), None);
        // A stick at rest drops the 0.2 left over, then down scrolls down.
        app.world_mut().resource_mut::<PadState>().right = Vec2::ZERO;
        app.update();
        assert!(wheel(&app).is_empty());
        app.world_mut().resource_mut::<PadState>().right = Vec2::new(0.0, -1.0);
        app.update();
        assert_eq!(wheel(&app), [-1.0]);

        // The same stick off the panel is the cursor again.
        app.world_mut().resource_mut::<Laser>().on_panel = false;
        app.world_mut().resource_mut::<PadState>().right = Vec2::new(1.0, 0.0);
        app.world_mut().resource_mut::<Runtime>().machine = mode::Machine::default();
        app.world_mut().resource_mut::<Runtime>().reading.targeting = true;
        app.update();
        assert!(wheel(&app).is_empty());
        assert_eq!(
            app.world().resource::<Runtime>().cursor.owner,
            cursor::Owner::Pad
        );

        // A tracked laser off the panel with the ground spell still armed: the right trigger is a
        // left click, which the world reads as the commit on the laser's hit, and no tap.
        app.world_mut().resource_mut::<Laser>().tracked = true;
        app.world_mut().resource_mut::<PadState>().right = Vec2::ZERO;
        hold(&mut app, &["PADRTRIGGER"]);
        app.update();
        assert_eq!(clicks(&app), [(MouseButton::Left, ButtonState::Pressed)]);
        hold(&mut app, &[]);
        app.update();
        assert_eq!(clicks(&app), [(MouseButton::Left, ButtonState::Released)]);
        assert!(soft_inputs(&app).is_empty(), "no tap in cursor mode");
    }

    /// A tracked laser in gameplay mode: a trigger tapped locks or interacts with what the laser
    /// hovers, and a trigger held for a layer does not.
    #[test]
    fn a_trigger_tapped_under_a_laser_acts_on_the_hover_and_a_layer_does_not() {
        let mut app = app();
        app.world_mut().resource_mut::<Laser>().tracked = true;
        hold(&mut app, &["PADRTRIGGER"]);
        app.update();
        assert!(
            soft_inputs(&app).is_empty(),
            "the press waits for the release"
        );
        hold(&mut app, &[]);
        app.update();
        assert_eq!(soft_inputs(&app), [SoftTargetInput::LockHovered]);

        hold(&mut app, &["PADLTRIGGER"]);
        app.update();
        hold(&mut app, &[]);
        app.update();
        assert_eq!(soft_inputs(&app), [SoftTargetInput::InteractHovered]);

        // The layer: a bar press under the trigger, then the trigger lets go.
        hold(&mut app, &["PADRTRIGGER"]);
        app.update();
        hold(&mut app, &["PADRTRIGGER", "PAD1"]);
        app.update();
        hold(&mut app, &["PAD1"]);
        app.update();
        hold(&mut app, &[]);
        app.update();
        assert!(soft_inputs(&app).is_empty());

        // No laser, no tap: the Deck's triggers are layers alone.
        app.world_mut().resource_mut::<Laser>().tracked = false;
        hold(&mut app, &["PADRTRIGGER"]);
        app.update();
        hold(&mut app, &[]);
        app.update();
        assert!(soft_inputs(&app).is_empty());
    }

    #[test]
    fn a_routed_token_is_reported_with_where_it_went_and_the_mode_is_kept() {
        let mut app = app();
        app.world_mut().resource_mut::<RoutedTrace>().0 = true;
        let routed = |app: &App| -> Vec<(&'static str, bool, String)> {
            app.world()
                .resource::<Messages<PadRouted>>()
                .iter_current_update_messages()
                .map(|r| (r.token, r.pressed, r.what.clone()))
                .collect()
        };
        hold(&mut app, &["PAD1"]);
        app.update();
        // No UI in this harness, so the binding table resolves nothing.
        assert_eq!(routed(&app), [("PAD1", true, "-".to_owned())]);
        assert_eq!(app.world().resource::<PadMode>().0, Mode::Gameplay);

        app.world_mut().resource_mut::<Laser>().on_panel = true;
        hold(&mut app, &["PAD1", "PADRTRIGGER"]);
        app.update();
        assert_eq!(app.world().resource::<PadMode>().0, Mode::Cursor);
        assert_eq!(
            routed(&app),
            [("PADRTRIGGER", true, "cursor:Left".to_owned())]
        );
        hold(&mut app, &[]);
        app.update();
        assert_eq!(
            routed(&app),
            [
                ("PAD1", false, "binding".to_owned()),
                ("PADRTRIGGER", false, "cursor".to_owned())
            ]
        );

        // The left menu in gameplay mode goes to the mode and the keyboard, and is still reported.
        app.world_mut().resource_mut::<Laser>().on_panel = false;
        hold(&mut app, &["PADBACK"]);
        app.update();
        assert_eq!(routed(&app), [("PADBACK", true, "mode".to_owned())]);
        hold(&mut app, &[]);
        app.update();
        assert_eq!(routed(&app), [("PADBACK", false, "mode".to_owned())]);
    }

    #[test]
    fn a_trackpad_hover_does_not_change_the_presentation_and_a_pad_button_does() {
        let mut app = app();
        let look = |app: &App| {
            let rt = app.world().resource::<Runtime>();
            rt.auto.resolve(Presentation::Auto, false)
        };
        assert_eq!(look(&app), presentation::Look::Kbm);
        app.world_mut().write_message(MouseMotion {
            delta: Vec2::splat(3.0),
        });
        app.update();
        assert_eq!(look(&app), presentation::Look::Kbm);
        hold(&mut app, &["PAD3"]);
        app.update();
        assert_eq!(look(&app), presentation::Look::Controller);
        // A real click is the keyboard and mouse again.
        app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Left,
            state: ButtonState::Pressed,
            window: Entity::PLACEHOLDER,
        });
        app.update();
        assert_eq!(look(&app), presentation::Look::Kbm);
    }

    fn soft_inputs(app: &App) -> Vec<SoftTargetInput> {
        let m = app.world().resource::<Messages<SoftTargetInput>>();
        m.iter_current_update_messages().copied().collect()
    }

    #[test]
    fn the_bumpers_lock_and_interact_in_gameplay_mode_and_scroll_in_cursor_mode() {
        use benilla_app::SoftSide;
        let mut app = app();
        hold(&mut app, &["PADLSHOULDER"]);
        app.update();
        assert_eq!(
            soft_inputs(&app),
            [SoftTargetInput::Cycle(SoftSide::Friend)]
        );
        assert!(bindings(&app).is_empty(), "no bound command any more");
        hold(&mut app, &[]);
        app.update();
        assert!(soft_inputs(&app).is_empty() && bindings(&app).is_empty());

        hold(&mut app, &["PADRSHOULDER"]);
        app.update();
        assert_eq!(soft_inputs(&app), [SoftTargetInput::Cycle(SoftSide::Enemy)]);
        hold(&mut app, &[]);
        app.update();

        // The left trigger is a layer, and under it the right bumper is the bound
        // `INTERACTTARGET`: a binding, not a lock.
        hold(&mut app, &["PADLTRIGGER", "PADRSHOULDER"]);
        app.update();
        assert!(soft_inputs(&app).is_empty(), "no lock under a layer");
        assert!(bindings(&app).contains(&("PADRSHOULDER", true)));

        // With a window open the bumpers are the cursor's wheel and target nothing.
        hold(&mut app, &[]);
        app.update();
        open(&mut app, &["ContainerFrame1"]);
        app.update();
        hold(&mut app, &["PADRSHOULDER"]);
        app.update();
        assert!(soft_inputs(&app).is_empty());
    }

    #[test]
    fn the_d_pad_under_the_left_bumper_and_trigger_targets_a_party_member() {
        let mut app = app();
        let script = UiScript::new().expect("construct UiScript");
        script
            .run("UNITS = '' function TargetUnit(u) UNITS = UNITS .. u .. ',' end")
            .unwrap();
        app.insert_non_send_resource(script);
        let units = |app: &App| {
            app.world()
                .non_send_resource::<UiScript>()
                .eval::<String>("return UNITS")
                .unwrap()
        };
        hold(&mut app, &["PADLTRIGGER", "PADLSHOULDER"]);
        app.update();
        assert!(
            soft_inputs(&app).is_empty(),
            "opening the layer locks nothing"
        );
        assert_eq!(units(&app), "");
        hold(&mut app, &["PADLTRIGGER", "PADLSHOULDER", "PADDRIGHT"]);
        app.update();
        assert_eq!(units(&app), "party2,");
        assert!(
            !bindings(&app).iter().any(|(t, _)| *t == "PADDRIGHT"),
            "not a bar slot"
        );
        hold(&mut app, &["PADLTRIGGER", "PADLSHOULDER"]);
        app.update();
        assert_eq!(units(&app), "party2,", "the release does nothing");

        // Out of the layer the same button is its bar slot.
        hold(&mut app, &["PADDRIGHT"]);
        app.update();
        assert!(bindings(&app).contains(&("PADDRIGHT", true)));
        assert_eq!(units(&app), "party2,");
    }

    #[test]
    fn the_soft_target_runs_with_gameplay_mode_and_only_then() {
        use bevy::input::gamepad::{GamepadConnection, GamepadConnectionEvent};
        let mut app = app();
        // A UI to play in: without a script the pad has no world.
        app.insert_non_send_resource(UiScript::new().expect("construct UiScript"));
        let mut device = PadDevice::new(PadConfig::default());
        device.handle(
            &RawGamepadEvent::Connection(GamepadConnectionEvent::new(
                Entity::from_raw_u32(7).expect("a valid index"),
                GamepadConnection::Connected {
                    name: "pad".into(),
                    vendor_id: None,
                    product_id: None,
                },
            )),
            0.0,
        );
        app.insert_resource(device)
            .add_systems(Update, drive_soft.after(step_mode));
        app.world_mut().resource_mut::<Runtime>().world = true;
        app.update();
        assert_eq!(soft_inputs(&app), [SoftTargetInput::Active(true)]);
        app.update();
        assert!(soft_inputs(&app).is_empty(), "told once, not every frame");
        open(&mut app, &["CharacterFrame"]);
        app.update();
        assert_eq!(soft_inputs(&app), [SoftTargetInput::Active(false)]);
        open(&mut app, &[]);
        app.update();
        assert_eq!(soft_inputs(&app), [SoftTargetInput::Active(true)]);
    }

    /// The carried review note of the cursor phase: with the left stick as the cursor stick, a left
    /// deflection moves the cursor and must not end cursor mode, which with the right stick as the
    /// cursor it does (a walk).
    #[test]
    fn step_mode_keeps_cursor_mode_on_a_left_deflection_when_the_left_stick_is_the_cursor() {
        let deflect = |app: &mut App| {
            app.world_mut().resource_mut::<PadState>().left = Vec2::new(1.0, 0.0);
            app.update();
        };
        let mode = |app: &App| app.world().resource::<Runtime>().machine.mode();

        let mut left = app();
        left.world_mut().resource_mut::<PadConfig>().cursor_stick = CursorStick::Left;
        open(&mut left, &["ContainerFrame1"]);
        left.update();
        assert_eq!(mode(&left), Mode::Cursor);
        deflect(&mut left);
        assert_eq!(mode(&left), Mode::Cursor, "the left stick is the cursor");
        for _ in 0..5 {
            left.update();
            assert_eq!(mode(&left), Mode::Cursor);
        }

        // The same deflection with the right stick as the cursor is a walk.
        let mut right = app();
        open(&mut right, &["ContainerFrame1"]);
        right.update();
        assert_eq!(mode(&right), Mode::Cursor);
        deflect(&mut right);
        assert_eq!(mode(&right), Mode::Gameplay);
    }

    #[test]
    fn edges_are_the_symmetric_difference_in_token_order() {
        let a: BTreeSet<_> = ["PAD1", "PAD2"].into_iter().collect();
        let b: BTreeSet<_> = ["PAD2", "PAD3", "PAD4"].into_iter().collect();
        let (up, down) = edges(&a, &b);
        assert_eq!(up, ["PAD1"]);
        assert_eq!(down, ["PAD3", "PAD4"]);
    }
    /// The movement tokens the bindings saw, per frame, with the ring's token left out.
    fn movement(app: &App) -> Vec<(&'static str, bool)> {
        bindings(app)
            .into_iter()
            .filter(|(t, _)| *t != "PADPADDLE1" && *t != "PADRSTICK")
            .collect()
    }

    /// The ring is a held binding like any other: opening and closing it neither presses nor
    /// releases a movement key, which comes from the pad's state every frame.
    #[test]
    fn a_movement_key_stays_held_through_an_open_and_closed_ring() {
        let mut app = app();
        hold(&mut app, &["PADLSTICKUP"]);
        app.update();
        assert_eq!(bindings(&app), [("PADLSTICKUP", true)]);
        // The ring opens on a back button, with the stick still forward.
        hold(&mut app, &["PADLSTICKUP", "PADPADDLE1"]);
        app.update();
        assert_eq!(bindings(&app), [("PADPADDLE1", true)]);
        // Held open for frames, the stick drifting to a diagonal: only the new direction changes.
        hold(&mut app, &["PADLSTICKUP", "PADLSTICKRIGHT", "PADPADDLE1"]);
        app.update();
        assert_eq!(movement(&app), [("PADLSTICKRIGHT", true)]);
        hold(&mut app, &["PADLSTICKUP", "PADPADDLE1"]);
        app.update();
        assert_eq!(movement(&app), [("PADLSTICKRIGHT", false)]);
        // Released: the ring's up, and the forward key is not touched.
        hold(&mut app, &["PADLSTICKUP"]);
        app.update();
        assert_eq!(bindings(&app), [("PADPADDLE1", false)]);
        hold(&mut app, &["PADLSTICKUP"]);
        app.update();
        assert!(bindings(&app).is_empty(), "still held, nothing sent");
        hold(&mut app, &[]);
        app.update();
        assert_eq!(bindings(&app), [("PADLSTICKUP", false)]);
    }

    /// The other default: under the left trigger the right stick's click opens the ring, as a
    /// shifted chord whose release goes to the same binding.
    #[test]
    fn the_ring_opens_from_the_shifted_stick_click_and_its_release_follows_it() {
        let mut app = app();
        hold(&mut app, &["PADLTRIGGER", "PADRSTICK"]);
        app.update();
        let m = app.world().resource::<Messages<BindingKeyInput>>();
        let sent: Vec<_> = m
            .iter_current_update_messages()
            .map(|b| (b.token, b.pressed, b.shift))
            .collect();
        assert!(sent.contains(&("PADRSTICK", true, true)), "{sent:?}");
        // The trigger lets go first, the click after it: the release is still the click's own.
        hold(&mut app, &["PADRSTICK"]);
        app.update();
        assert!(!bindings(&app).contains(&("PADRSTICK", false)));
        hold(&mut app, &[]);
        app.update();
        assert!(bindings(&app).contains(&("PADRSTICK", false)));
    }

    #[test]
    fn a_padback_tap_switches_the_mode_on_release_and_a_long_press_toggles_the_keyboard() {
        let mut app = app();
        app.init_resource::<Host>()
            .add_systems(Update, drive_text.after(step_mode));
        let script = UiScript::new().unwrap();
        script
            .run("Toggled = 0 function BenillaPadToggleKeyboard() Toggled = Toggled + 1 end")
            .unwrap();
        app.world_mut().insert_non_send_resource(script);
        let mode = |app: &App| app.world().resource::<Runtime>().machine.mode();
        let toggled = |app: &App| -> i32 {
            let s = app.world().non_send_resource::<UiScript>();
            s.eval("return Toggled").unwrap()
        };
        let step = |app: &mut App, secs: f32| {
            app.world_mut()
                .resource_mut::<Time<Real>>()
                .advance_by(std::time::Duration::from_secs_f32(secs));
        };

        // A tap: nothing while it is down, the switch on release.
        hold(&mut app, &["PADBACK"]);
        step(&mut app, 0.05);
        app.update();
        assert_eq!(mode(&app), Mode::Gameplay);
        hold(&mut app, &[]);
        app.update();
        assert_eq!(mode(&app), Mode::Cursor);
        assert_eq!(toggled(&app), 0);

        // A long press: the keyboard, once, and neither the hold nor the release switches the mode.
        hold(&mut app, &["PADBACK"]);
        app.update();
        step(&mut app, 0.4);
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(toggled(&app), 1, "once, however long it is held");
        hold(&mut app, &[]);
        app.update();
        assert_eq!(
            mode(&app),
            Mode::Cursor,
            "the release of a long press is not a tap"
        );
        assert_eq!(toggled(&app), 1);
    }

    #[test]
    fn what_the_keyboard_types_is_written_as_keyboard_input_and_is_not_the_players() {
        let mut app = app();
        app.init_resource::<Host>()
            .add_systems(Update, drive_text.after(step_mode));
        let script = UiScript::new().unwrap();
        app.world()
            .resource::<Host>()
            .install(script.lua())
            .unwrap();
        script
            .run("BenillaPad_Type(\"h\") BenillaPad_Key(\"ENTER\")")
            .unwrap();
        app.world_mut().insert_non_send_resource(script);
        hold(&mut app, &["PAD3"]);
        app.update();
        let keys: Vec<_> = app
            .world()
            .resource::<Messages<KeyboardInput>>()
            .iter_current_update_messages()
            .map(|k| (k.state, k.text.as_deref().map(str::to_owned)))
            .collect();
        assert_eq!(
            keys,
            [
                (ButtonState::Pressed, Some("h".to_owned())),
                (ButtonState::Released, None),
                (ButtonState::Pressed, None),
                (ButtonState::Released, None),
            ]
        );
        assert_eq!(app.world().resource::<Runtime>().own.keys, 2);
        // They come back through the reader next frame and do not flip the presentation.
        app.update();
        let look = app
            .world()
            .resource::<Runtime>()
            .auto
            .resolve(Presentation::Auto, true);
        assert_eq!(look, presentation::Look::Controller);
        assert!(app.world().resource::<Host>().take_typed().is_empty());
    }
}
