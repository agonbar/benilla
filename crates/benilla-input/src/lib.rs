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
//! mouse input ([`actions`]) and the pointer seam of `benilla_app::pointer`. The bars show pad
//! glyphs ([`glyphs`], the addon of [`addon`]) by the presentation setting ([`presentation`]). The
//! pad's own settings are `pad.toml` ([`config`]).
//!
//! Gamepad input is off in stock benilla (`benilla-world`'s `boot.rs` disables `GilrsPlugin`);
//! [`run`] turns it back on from the plugin group's hook, so stock benilla stays as it is.

mod actions;
mod addon;
mod config;
mod cursor;
mod device;
mod glyphs;
mod layers;
mod look;
mod mode;
mod panels;
mod presentation;
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

use benilla_app::{AppExit, BindingKeyInput, BuildId, CoverUp};
use benilla_ui::script::UiScript;

use actions::Act;
use config::CursorStick;
use mode::Mode;

pub use config::{Device, PadConfig, Presentation, Source};
pub use device::{PadDevice, PadInfo, PadState};
pub use layers::Modifiers;

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
            .insert_resource(cfg)
            .init_resource::<PadState>()
            .init_resource::<Runtime>()
            .insert_resource(Trace(
                std::env::var_os("WOW_PAD_TRACE").is_some_and(|v| v != "0"),
            ))
            .add_message::<BindingKeyInput>()
            // Before `InputSystems`: the motion, buttons and keys the pad writes are accumulated
            // there, and the `BindingKeyInput`s are read by the dispatch in `Update`.
            .add_systems(
                PreUpdate,
                (
                    read_pad,
                    read_world,
                    step_mode,
                    route_keys,
                    drive_cursor,
                    drive_look,
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
    back_was: bool,
    left_was: bool,
    prev_down: BTreeSet<&'static str>,
    /// How each pressed token was routed, for its release.
    routed: HashMap<&'static str, Routed>,
    acts: Vec<Act>,
    /// The OS cursor is hidden because the pad's is drawn.
    hid: bool,
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
    trace: Res<Trace>,
    mut rt: ResMut<Runtime>,
) {
    let back = state.is_down("PADBACK");
    let left = state.left != Vec2::ZERO;
    let signals = mode::Signals {
        panels: &rt.reading.panels.clone(),
        targeting: rt.reading.targeting,
        back: back && !rt.back_was,
        // Pushing the stick again is a walk; a stick already held when a window opened is not.
        walk: left && !rt.left_was && cfg.cursor_stick == CursorStick::Right,
    };
    let before = rt.machine.mode();
    let after = rt.machine.step(signals);
    rt.back_was = back;
    rt.left_was = left;
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
    trace: Res<Trace>,
    script: Option<NonSend<UiScript>>,
    mut rt: ResMut<Runtime>,
    mut out: MessageWriter<BindingKeyInput>,
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
            // Pressed while the pad was not looking (a stick direction the dead zone made up).
            None => continue,
        };
        if trace.0 {
            info!("pad: {token} up -> {command}");
        }
    }
    for token in down {
        if cursor_mode && actions::consumed(token, left_is_cursor) {
            let act = actions::press(token);
            rt.acts.extend(act);
            rt.routed.insert(token, Routed::Cursor(act));
            if trace.0 {
                info!("pad: {token} down -> cursor:{}", act_name(act));
            }
            continue;
        }
        if token == "PADBACK" {
            continue;
        }
        out.write(BindingKeyInput {
            token,
            pressed: true,
            alt: mods.alt,
            ctrl: mods.ctrl,
            shift: mods.shift,
        });
        rt.routed.insert(token, Routed::Binding);
        if trace.0 {
            let command = script
                .as_ref()
                .and_then(|s| trace::resolve(&s.binding_keys(), mods, token))
                .unwrap_or_else(|| "-".into());
            info!("pad: {} down -> {command}", trace::chord_text(mods, token));
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
    let acts = std::mem::take(&mut rt.acts);
    let Ok((entity, mut window, mut opts)) = window.single_mut() else {
        return;
    };
    if rt.machine.mode() == Mode::Cursor {
        let size = Vec2::new(window.width(), window.height());
        let mut input = cursor::Input {
            size,
            os: window.cursor_position(),
            stick: match cfg.cursor_stick {
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
                Act::Button { down: true, .. } => input.pad_action = true,
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
    mouse: Res<ButtonInput<MouseButton>>,
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
    if session.open && want && !covered {
        let delta = look::motion(state.right, &cfg, dt);
        if delta != Vec2::ZERO {
            motion.write(MouseMotion { delta });
            rt.own.motion += 1;
        }
    }
}

/// The state the addon reads, the glyph tables with it, and the UI scale the player asked for.
fn push_state(
    cfg: Res<PadConfig>,
    device: Res<PadDevice>,
    trace: Res<Trace>,
    script: Option<NonSend<UiScript>>,
    mut rt: ResMut<Runtime>,
) {
    let Some(script) = script else {
        return;
    };
    let rt = &mut *rt;
    if rt.pushed.session != Some(script.session()) {
        match script.run(&glyphs::lua_tables(&cfg)) {
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
            .init_resource::<PadState>()
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

    #[test]
    fn edges_are_the_symmetric_difference_in_token_order() {
        let a: BTreeSet<_> = ["PAD1", "PAD2"].into_iter().collect();
        let b: BTreeSet<_> = ["PAD2", "PAD3", "PAD4"].into_iter().collect();
        let (up, down) = edges(&a, &b);
        assert_eq!(up, ["PAD1"]);
        assert_eq!(down, ["PAD3", "PAD4"]);
    }
}
