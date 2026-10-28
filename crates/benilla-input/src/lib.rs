//! Gamepad input for benilla: a plugin on top of the client ([`benilla_app::run_with_group`]);
//! 1.12.1 has no gamepad support, so none of it lives in benilla.
//!
//! gilrs' events become one [`PadState`] ([`device`]): sticks with dead zones, triggers, and every
//! input as a `PAD*` token ([`tokens`]), digital stick directions included. The tokens enter the
//! binding store through `benilla_app::register_key_tokens` and reach the dispatch as
//! [`benilla_app::BindingKeyInput`] messages carrying the layer's modifiers ([`layers`]), so a
//! pad button is bound, rebound and saved like a key. The right stick feeds the camera as a held
//! mouselook ([`look`]). The pad's own settings are `pad.toml` ([`config`]).
//!
//! Gamepad input is off in stock benilla (`benilla-world`'s `boot.rs` disables `GilrsPlugin`);
//! [`run`] turns it back on from the plugin group's hook, so stock benilla stays as it is.

mod config;
mod device;
mod layers;
mod look;
mod tokens;
mod trace;

use std::collections::{BTreeSet, HashMap};

use bevy::app::PluginGroupBuilder;
use bevy::input::gamepad::RawGamepadEvent;
use bevy::input::mouse::MouseMotion;
use bevy::input::InputSystems;
use bevy::prelude::*;

use benilla_app::{AppExit, BindingKeyInput, BuildId};
use benilla_ui::script::UiScript;

pub use config::{Device, PadConfig, Source};
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

/// Pad input: the device layer, the tokens and default bindings, and the camera stick.
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

        let cfg = PadConfig::load();
        app.insert_resource(PadDevice::new(cfg.clone()))
            .insert_resource(cfg)
            .init_resource::<PadState>()
            .insert_resource(Trace(
                std::env::var_os("WOW_PAD_TRACE").is_some_and(|v| v != "0"),
            ))
            .add_message::<BindingKeyInput>()
            // Before `InputSystems`: the motion the camera stick writes is accumulated there, and
            // the `BindingKeyInput`s are read by the dispatch in `Update`.
            .add_systems(
                PreUpdate,
                (read_pad, emit_keys, drive_look)
                    .chain()
                    .before(InputSystems),
            );
    }
}

/// `WOW_PAD_TRACE=1`.
#[derive(Resource)]
struct Trace(bool);

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

/// One [`BindingKeyInput`] per input that changed, releases first, a press carrying the layer.
fn emit_keys(
    state: Res<PadState>,
    cfg: Res<PadConfig>,
    trace: Res<Trace>,
    script: Option<NonSend<UiScript>>,
    mut prev: Local<BTreeSet<&'static str>>,
    mut pressed: Local<HashMap<&'static str, String>>,
    mut out: MessageWriter<BindingKeyInput>,
) {
    if *prev == *state.down() {
        return;
    }
    let (up, down) = edges(&prev, state.down());
    let mods = layers::modifiers(&cfg, &state);
    for token in up {
        out.write(BindingKeyInput {
            token,
            pressed: false,
            alt: mods.alt,
            ctrl: mods.ctrl,
            shift: mods.shift,
        });
        if trace.0 {
            let command = pressed.remove(token).unwrap_or_else(|| "-".into());
            info!("pad: {token} up -> {command}");
        }
    }
    for token in down {
        out.write(BindingKeyInput {
            token,
            pressed: true,
            alt: mods.alt,
            ctrl: mods.ctrl,
            shift: mods.shift,
        });
        if trace.0 {
            let command = script
                .as_ref()
                .and_then(|s| trace::resolve(&s.binding_keys(), mods, token))
                .unwrap_or_else(|| "-".into());
            info!("pad: {} down -> {command}", trace::chord_text(mods, token));
            pressed.insert(token, command);
        }
    }
    *prev = state.down().clone();
}

/// The right stick as a held mouselook session ([`look`]).
fn drive_look(
    state: Res<PadState>,
    cfg: Res<PadConfig>,
    trace: Res<Trace>,
    time: Res<Time<Real>>,
    mut session: Local<look::Session>,
    mut script: Option<NonSendMut<UiScript>>,
    mut motion: MessageWriter<MouseMotion>,
) {
    let dt = time.delta_secs();
    if let Some(call) = session.step(state.right != Vec2::ZERO, dt) {
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
    if session.open {
        let delta = look::motion(state.right, &cfg, dt);
        if delta != Vec2::ZERO {
            motion.write(MouseMotion { delta });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_are_the_symmetric_difference_in_token_order() {
        let a: BTreeSet<_> = ["PAD1", "PAD2"].into_iter().collect();
        let b: BTreeSet<_> = ["PAD2", "PAD3", "PAD4"].into_iter().collect();
        let (up, down) = edges(&a, &b);
        assert_eq!(up, ["PAD1"]);
        assert_eq!(down, ["PAD3", "PAD4"]);
    }
}
