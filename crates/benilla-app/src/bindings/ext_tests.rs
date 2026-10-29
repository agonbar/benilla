//! The extension inputs ([`super::BindingKeyInput`]): a crate on top's own base inputs, a gamepad's
//! `PAD*` buttons, through the same walk and dispatch a key takes.

use benilla_ui::script::UiScript;

use super::chord::{register_key_tokens, BindKey, Chord};
use super::*;

const BINDINGS: &str = r#"<Bindings>
    <Binding name="HOLD" runOnUp="true">HOLD_LOG = (HOLD_LOG or "") .. keystate .. ";"</Binding>
    <Binding name="SHIFTED" runOnUp="true">SHIFTED_LOG = (SHIFTED_LOG or "") .. keystate .. ";"</Binding>
    <Binding name="TAP">TAPS = (TAPS or 0) + 1</Binding>
</Bindings>"#;

/// The tokens these tests use; the registry is process-wide and idempotent.
fn register() {
    register_key_tokens(&["PAD1", "PAD2", "PADLTRIGGER"]);
}

fn input(token: &'static str, pressed: bool) -> BindingKeyInput {
    BindingKeyInput {
        token,
        pressed,
        alt: false,
        ctrl: false,
        shift: false,
    }
}

/// A VM with [`BINDINGS`] declared, and a dispatch map holding `binds` (chord, command) directly:
/// these tests are of the dispatch loop, and the store's side is `store_tests`.
fn harness(binds: &[(&str, &str)]) -> App {
    register();
    let script = UiScript::new().expect("VM");
    script.register_bindings(&benilla_ui::bindings_xml::parse(BINDINGS).expect("well-formed"));
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, bevy::input::InputPlugin))
        .init_resource::<UiKeyboardCapture>()
        .init_resource::<PlayerUiHover>()
        .init_resource::<WorldWheel>()
        .init_resource::<BindingsState>()
        .init_resource::<BindingDispatch>()
        .init_resource::<LayoutNames>()
        .add_message::<BindingKeyInput>()
        .add_systems(Update, latch_and_dispatch);
    app.insert_non_send_resource(script);
    let mut dispatch = app.world_mut().resource_mut::<BindingDispatch>();
    for (chord, command) in binds {
        let chord = Chord::parse(chord).unwrap_or_else(|| panic!("{chord} parses"));
        dispatch.map.insert(chord, command.to_string());
    }
    app
}

fn send(app: &mut App, ev: BindingKeyInput) {
    app.world_mut().write_message(ev);
    app.update();
}

fn lua_str(app: &App, global: &str) -> String {
    app.world()
        .non_send_resource::<UiScript>()
        .eval::<String>(&format!(r#"return {global} or """#))
        .expect("eval")
}

fn latched(app: &App) -> usize {
    app.world().resource::<BindingsState>().latched.len()
}

#[test]
fn a_registered_token_parses_as_a_chord_base_and_an_unregistered_one_does_not() {
    register();
    assert_eq!(
        Chord::parse("SHIFT-PAD1"),
        Some(Chord {
            alt: false,
            ctrl: false,
            shift: true,
            key: BindKey::Ext("PAD1")
        })
    );
    assert_eq!(Chord::parse("PADNOPE1"), None);
}

#[test]
fn a_layer_miss_falls_back_to_the_plain_button_and_never_to_a_neighbour_layer() {
    register();
    let both = Chord::parse("ALT-PAD1").unwrap();
    assert_eq!(both.fallback(), Chord::parse("PAD1"));
    // The plain layer has no modifier left to drop.
    assert_eq!(Chord::parse("PAD1").unwrap().fallback(), None);
}

#[test]
fn a_press_runs_the_command_and_a_release_runs_its_up_half() {
    let mut app = harness(&[("PAD1", "HOLD")]);
    send(&mut app, input("PAD1", true));
    assert_eq!(lua_str(&app, "HOLD_LOG"), "down;");
    assert_eq!(latched(&app), 1);
    send(&mut app, input("PAD1", false));
    assert_eq!(lua_str(&app, "HOLD_LOG"), "down;up;");
    assert_eq!(latched(&app), 0);
}

#[test]
fn the_modifiers_travel_with_the_event_and_no_key_state_changes() {
    let mut app = harness(&[("PAD1", "HOLD"), ("SHIFT-PAD1", "SHIFTED")]);
    send(
        &mut app,
        BindingKeyInput {
            shift: true,
            ..input("PAD1", true)
        },
    );
    assert_eq!(lua_str(&app, "SHIFTED_LOG"), "down;");
    assert_eq!(lua_str(&app, "HOLD_LOG"), "");
    let keys = app.world().resource::<ButtonInput<KeyCode>>();
    assert!(!keys.pressed(KeyCode::ShiftLeft), "no emulated Shift leaks");
    // The release runs the up-half of the command the press resolved, whatever the layer is now.
    send(&mut app, input("PAD1", false));
    assert_eq!(lua_str(&app, "SHIFTED_LOG"), "down;up;");
}

#[test]
fn a_chord_with_no_binding_falls_back_one_modifier() {
    let mut app = harness(&[("PAD1", "HOLD")]);
    send(
        &mut app,
        BindingKeyInput {
            alt: true,
            ..input("PAD1", true)
        },
    );
    assert_eq!(lua_str(&app, "HOLD_LOG"), "down;");
}

#[test]
fn a_focused_box_suppresses_the_press() {
    let mut app = harness(&[("PAD1", "TAP")]);
    app.world_mut().resource_mut::<UiKeyboardCapture>().typing = true;
    send(&mut app, input("PAD1", true));
    send(&mut app, input("PAD1", false));
    let taps: i64 = app
        .world()
        .non_send_resource::<UiScript>()
        .eval("return TAPS or 0")
        .unwrap();
    assert_eq!(taps, 0);
    assert_eq!(latched(&app), 0);
}

#[test]
fn a_keyboard_frame_sees_the_press_first_and_keeps_it_from_the_binding() {
    let mut app = harness(&[("PAD1", "TAP")]);
    app.world()
        .non_send_resource::<UiScript>()
        .run(
            "local f = CreateFrame('Frame') f:EnableKeyboard(true) \
             f:SetScript('OnKeyDown', function() SEEN = arg1 end)",
        )
        .unwrap();
    send(&mut app, input("PAD1", true));
    assert_eq!(
        lua_str(&app, "SEEN"),
        "PAD1",
        "the frame's walk saw the pad token"
    );
    let taps: i64 = app
        .world()
        .non_send_resource::<UiScript>()
        .eval("return TAPS or 0")
        .unwrap();
    assert_eq!(taps, 0, "the frame took it, so no binding ran");
}

#[test]
fn a_held_pad_press_is_not_swept_as_a_stuck_latch() {
    let mut app = harness(&[("PAD1", "HOLD")]);
    send(&mut app, input("PAD1", true));
    for _ in 0..5 {
        app.update();
    }
    assert_eq!(
        latched(&app),
        1,
        "no key plane says it is up, and none should"
    );
    assert_eq!(lua_str(&app, "HOLD_LOG"), "down;");
}

#[test]
fn a_second_press_without_a_release_does_not_latch_twice() {
    let mut app = harness(&[("PAD1", "HOLD")]);
    send(&mut app, input("PAD1", true));
    send(&mut app, input("PAD1", true));
    assert_eq!(lua_str(&app, "HOLD_LOG"), "down;");
    assert_eq!(latched(&app), 1);
}

#[test]
fn registered_defaults_join_the_players_and_stay_out_of_the_saved_diff() {
    // The list is process-wide and later seeds in this binary read it, so the test puts back
    // what it found, even when it fails.
    struct Restore(Vec<(String, String)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            *EXT_DEFAULTS.lock().unwrap_or_else(|e| e.into_inner()) = std::mem::take(&mut self.0);
        }
    }
    let _restore = Restore(
        EXT_DEFAULTS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
    );
    register_default_bindings(&[("PAD2", "TAP")]);
    let defaults = with_ext_defaults(vec![("W".into(), "MOVEFORWARD".into())]);
    assert!(defaults.contains(&("W".into(), "MOVEFORWARD".into())));
    assert!(defaults.contains(&("PAD2".into(), "TAP".into())));
    // A live table equal to the defaults saves nothing: the pad's defaults are not the account's.
    let snapshot = vec![
        ("MOVEFORWARD".to_string(), vec!["W".to_string()]),
        ("TAP".to_string(), vec!["PAD2".to_string()]),
    ];
    let diff = store::to_diff(&snapshot, &defaults);
    assert_eq!(diff.lines().count(), 1, "header only: {diff}");
}

#[test]
fn a_pad_token_round_trips_through_the_diff_and_steals_its_key() {
    register();
    let defaults: Vec<KeyBinding> = vec![("PAD1".into(), "TAP".into())];
    let diff = store::from_diff("bind HOLD PAD1\nbind GONE PADUNREGISTERED\n");
    // An unregistered token is kept in the file, as an addon's unknown command is.
    assert_eq!(
        diff,
        vec![
            ("HOLD".to_string(), vec!["PAD1".to_string()]),
            ("GONE".to_string(), vec!["PADUNREGISTERED".to_string()]),
        ]
    );
    let table = store::resolve(&diff, &defaults);
    assert!(table.contains(&("PAD1".into(), "HOLD".into())));
    assert!(
        !table.contains(&("PAD1".into(), "TAP".into())),
        "PAD1 was stolen"
    );
    assert!(table.contains(&("PADUNREGISTERED".into(), "GONE".into())));
}

#[test]
fn the_validator_passes_a_registered_token_and_still_refuses_any_other() {
    use benilla_ui::script::keybind::normalize_binding_key;
    register();
    assert_eq!(normalize_binding_key("PAD1").as_deref(), Some("PAD1"));
    assert_eq!(
        normalize_binding_key("ALT-SHIFT-PADLTRIGGER").as_deref(),
        Some("ALT-SHIFT-PADLTRIGGER")
    );
    assert_eq!(normalize_binding_key("PADNOPE"), None);
    // The reference's own refusals are unchanged.
    assert_eq!(normalize_binding_key("scrolllock"), None);
    assert_eq!(normalize_binding_key("SHIFT-W").as_deref(), Some("SHIFT-W"));
}

#[test]
fn the_key_bindings_window_can_bind_a_pad_button() {
    register();
    let script = UiScript::new().expect("VM");
    script.register_bindings(&benilla_ui::bindings_xml::parse(BINDINGS).expect("well-formed"));
    script
        .run(r#"SetBinding("PAD2", "TAP")"#)
        .expect("SetBinding");
    assert_eq!(
        script
            .eval::<String>(r#"return GetBindingAction("PAD2")"#)
            .unwrap(),
        "TAP"
    );
}

#[test]
fn a_press_under_the_loading_cover_is_dropped_and_a_release_still_runs() {
    let mut app = harness(&[("PAD1", "HOLD")]);
    app.insert_resource(crate::loading_screen::LoadingScreen::test_covering());
    send(&mut app, input("PAD1", true));
    assert_eq!(lua_str(&app, "HOLD_LOG"), "");
    assert_eq!(latched(&app), 0);
    // A press from before the cover is released by its own up edge.
    app.world_mut()
        .remove_resource::<crate::loading_screen::LoadingScreen>();
    send(&mut app, input("PAD1", true));
    app.insert_resource(crate::loading_screen::LoadingScreen::test_covering());
    send(&mut app, input("PAD1", false));
    assert_eq!(lua_str(&app, "HOLD_LOG"), "down;up;");
    assert_eq!(latched(&app), 0);
}
