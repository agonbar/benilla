//! The pad on the glue screens, end to end over synthetic frames named as the stock GlueXML names
//! its own: the real systems (mode, routing, cursor, keyboard) over a glue VM, with a stand-in for
//! the host's UI feed that does what `feed_ui_input` does with the cursor, clicks and keys.

use std::sync::{Mutex, MutexGuard, PoisonError};

use benilla_ui::script::UiScript;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::input::mouse::MouseButtonInput;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::tests::{app, hold};
use crate::*;

static SEAM: Mutex<()> = Mutex::new(());

/// The pointer seam is process-wide and the tests run in parallel: one test at a time owns it.
pub(crate) fn pointer_seam() -> MutexGuard<'static, ()> {
    SEAM.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the host's `feed_ui_input` does with the pad's synthetic input: the pointer to the VM, a
/// click as a mouse button, typed characters and the named keys as key events.
fn feed_ui(
    mut script: NonSendMut<UiScript>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut buttons: MessageReader<MouseButtonInput>,
    mut keys: MessageReader<KeyboardInput>,
) {
    let window = window.single().unwrap();
    script.resolve();
    if let Some(cursor) = benilla_app::pointer::ui_cursor(window) {
        let at = benilla_app::pointer::window_to_ui(window.height(), cursor);
        script.mouse_move(at.x, at.y);
        for b in buttons.read() {
            let name = match b.button {
                MouseButton::Left => "LeftButton",
                _ => "RightButton",
            };
            script.mouse_button(at.x, at.y, name, b.state == ButtonState::Pressed);
        }
    }
    for k in keys.read().filter(|k| k.state == ButtonState::Pressed) {
        match (k.key_code, &k.text) {
            (KeyCode::Escape, _) => {
                script.key_input("ESCAPE");
            }
            (KeyCode::Enter, _) => {
                script.key_input("ENTER");
            }
            (KeyCode::Backspace, _) => {
                script.key_input("BACKSPACE");
            }
            (_, Some(text)) => {
                script.char_input(text);
            }
            _ => {}
        }
    }
    script.resolve();
}

/// The pad's systems over a glue VM with `lua` as its screens, in a 1024 x 768 window.
fn glue_app(lua: &str) -> App {
    let mut app = app();
    app.insert_resource(PadDevice::new(PadConfig::default()))
        .init_resource::<Host>()
        .add_systems(
            Update,
            (
                drive_cursor,
                drive_text,
                drive_glue_keyboard,
                push_state,
                feed_ui,
            )
                .chain()
                .after(route_keys),
        );
    let mut windows = app.world_mut().query::<&mut Window>();
    for mut w in windows.iter_mut(app.world_mut()) {
        w.resolution.set(1024.0, 768.0);
    }
    let mut script = UiScript::new_glue().expect("construct the glue VM");
    script.set_screen_size(1024.0, 768.0);
    script.run(GLUE_PARENT).unwrap();
    script.run(lua).unwrap();
    script.resolve();
    app.world_mut().insert_non_send_resource(script);
    app
}

/// `GlueParent`, which the keyboard hangs from where the world has `UIParent`.
const GLUE_PARENT: &str = r#"
    local p = CreateFrame("Frame", "GlueParent")
    p:SetAllPoints()
    Clicks = {}
    function Button(name, parent, x, y, w, h)
        local b = CreateFrame("Button", name, parent or GlueParent)
        b:SetPoint("CENTER", GlueParent, "BOTTOMLEFT", x, y)
        b:SetWidth(w); b:SetHeight(h)
        b:EnableMouse(true)
        b:RegisterForClicks("LeftButtonUp")
        b:SetScript("OnClick", function() table.insert(Clicks, name) end)
        return b
    end
    function Edit(name, parent, x, y)
        local e = CreateFrame("EditBox", name, parent or GlueParent)
        e:SetPoint("CENTER", GlueParent, "BOTTOMLEFT", x, y)
        e:SetWidth(200); e:SetHeight(30)
        e:SetFont("Fonts\\FRIZQT__.TTF", 14)
        e:SetAutoFocus(false)
        e:EnableMouse(true)
        return e
    end
"#;

/// A tap of one pad token: down for a frame, up for the next, and a frame for the UI to settle.
fn tap(app: &mut App, token: &'static str) {
    hold(app, &[token]);
    app.update();
    hold(app, &[]);
    app.update();
    app.update();
}

fn script(app: &App) -> &UiScript {
    app.world().non_send_resource::<UiScript>()
}

fn clicks(app: &App) -> Vec<String> {
    let s = script(app);
    let n: usize = s.eval("return table.getn(Clicks)").unwrap();
    (1..=n)
        .map(|i| s.eval::<String>(&format!("return Clicks[{i}]")).unwrap())
        .collect()
}

fn mode(app: &App) -> Mode {
    app.world().resource::<Runtime>().machine.mode()
}

/// The name of the widget the pad's cursor is over.
fn under_cursor(app: &App) -> Option<String> {
    let rt = app.world().resource::<Runtime>();
    let at = benilla_app::pointer::window_to_ui(768.0, rt.cursor.pos);
    let s = script(app);
    s.hit_test_frame(at.x, at.y).and_then(|h| s.frame_name(h))
}

/// Presses the d-pad toward `target` until the cursor is over it: a player's way of getting there.
fn walk_to(app: &mut App, target: &str) {
    for _ in 0..40 {
        if under_cursor(app).as_deref() == Some(target) {
            return;
        }
        let (cx, cy): (f32, f32) = script(app)
            .eval(&format!("return {target}:GetCenter()"))
            .unwrap();
        let rt = app.world().resource::<Runtime>();
        let at = benilla_app::pointer::window_to_ui(768.0, rt.cursor.pos);
        let (dx, dy) = (cx - at.x, cy - at.y);
        let horizontal = dx.abs() > dy.abs();
        let token = match (horizontal, dx > 0.0, dy > 0.0) {
            (true, true, _) => "PADDRIGHT",
            (true, false, _) => "PADDLEFT",
            (false, _, true) => "PADDUP",
            (false, _, false) => "PADDDOWN",
        };
        tap(app, token);
    }
    panic!(
        "the d-pad did not reach {target}; the cursor is over {:?}",
        under_cursor(app)
    );
}

fn text(app: &App, frame: &str) -> String {
    script(app)
        .eval::<String>(&format!("return {frame}:GetText() or ''"))
        .unwrap()
}

fn shown(app: &App, frame: &str) -> bool {
    script(app)
        .eval::<bool>(&format!(
            "return {frame} and {frame}:IsShown() and true or false"
        ))
        .unwrap()
}

/// Character select and create as the stock screens lay them out: the list on the right, Create
/// below it, Enter World at the bottom, and a name box on the create screen that takes focus when
/// the screen shows. Escape on the create screen goes back.
const CHARACTER_SCREENS: &str = r#"
    Selected = 0
    local select = CreateFrame("Frame", "CharacterSelect", GlueParent)
    select:SetAllPoints()
    for i = 1, 3 do
        local b = Button("CharSelectCharacterButton" .. i, select, 850, 680 - i * 60, 200, 50)
        b:SetScript("OnClick", function()
            Selected = i
            table.insert(Clicks, "CharSelectCharacterButton" .. i)
        end)
    end
    Button("CharSelectCreateCharacterButton", select, 850, 150, 200, 40):SetScript("OnClick", function()
        table.insert(Clicks, "CharSelectCreateCharacterButton")
        select:Hide()
        CharacterCreate:Show()
    end)
    Button("CharSelectEnterWorldButton", select, 512, 60, 200, 40):SetScript("OnClick", function()
        table.insert(Clicks, "CharSelectEnterWorldButton")
        EnteredWorld = Selected
    end)

    local create = CreateFrame("Frame", "CharacterCreate", GlueParent)
    create:SetAllPoints()
    create:EnableKeyboard(true)
    create:Hide()
    local name = Edit("CharacterCreateNameEdit", create, 512, 600)
    Button("CharCreateOkayButton", create, 700, 100, 150, 40)
    Button("CharCreateBackButton", create, 300, 100, 150, 40)
    create:SetScript("OnShow", function() name:SetFocus() end)
    create:SetScript("OnKeyDown", function()
        if arg1 == "ESCAPE" then
            create:Hide()
            select:Show()
        end
    end)
"#;

/// The acceptance flow: character select driven by pad tokens alone. Moves the selection, opens
/// character create, types a name on the on-screen keyboard, backs out with B and enters the world.
#[test]
fn character_select_plays_with_the_pad_alone() {
    let _seam = pointer_seam();
    let mut app = glue_app(CHARACTER_SCREENS);
    app.update();
    assert_eq!(mode(&app), Mode::Cursor, "a glue screen is cursor mode");

    // A d-pad press with nothing under the cursor lands on the nearest widget, then they step.
    tap(&mut app, "PADDRIGHT");
    assert_eq!(
        under_cursor(&app).as_deref(),
        Some("CharSelectCharacterButton3")
    );
    tap(&mut app, "PADDUP");
    assert_eq!(
        under_cursor(&app).as_deref(),
        Some("CharSelectCharacterButton2")
    );
    // A clicks, and as a left click: the stock buttons register for nothing else.
    tap(&mut app, "PAD1");
    assert_eq!(clicks(&app), ["CharSelectCharacterButton2"]);
    assert_eq!(script(&app).eval::<i32>("return Selected").unwrap(), 2);
    // PADBACK is no way out of cursor mode here.
    tap(&mut app, "PADBACK");
    assert_eq!(mode(&app), Mode::Cursor);

    // Create Character: the name box takes focus and the keyboard comes up.
    walk_to(&mut app, "CharSelectCreateCharacterButton");
    tap(&mut app, "PAD1");
    assert!(shown(&app, "CharacterCreate"));
    app.update();
    assert!(
        shown(&app, "BenillaPadKeyboard"),
        "the name box opens the keyboard"
    );

    // "bo", typed on the keyboard by the pad.
    walk_to(&mut app, "BenillaPadKey4_5");
    tap(&mut app, "PAD1");
    walk_to(&mut app, "BenillaPadKey2_9");
    tap(&mut app, "PAD1");
    assert_eq!(text(&app, "CharacterCreateNameEdit"), "bo");

    // B puts the keyboard away, B again is the screen's Escape: back to the list.
    tap(&mut app, "PAD2");
    assert!(!shown(&app, "BenillaPadKeyboard"));
    assert!(shown(&app, "CharacterCreate"));
    tap(&mut app, "PAD2");
    assert!(!shown(&app, "CharacterCreate"));
    assert!(shown(&app, "CharacterSelect"));
    assert_eq!(mode(&app), Mode::Cursor);

    // Enter World.
    walk_to(&mut app, "CharSelectEnterWorldButton");
    tap(&mut app, "PAD1");
    assert_eq!(script(&app).eval::<i32>("return EnteredWorld").unwrap(), 2);
    assert_eq!(
        clicks(&app),
        [
            "CharSelectCharacterButton2",
            "CharSelectCreateCharacterButton",
            "CharSelectEnterWorldButton"
        ],
        "the keyboard's keys type and are not clicks of the screen's"
    );
}

/// The login screen: two boxes, Login and Exit, one under the other.
const LOGIN_SCREEN: &str = r#"
    local ui = CreateFrame("Frame", "AccountLoginUI", GlueParent)
    ui:SetAllPoints()
    Edit("AccountLoginAccountEdit", ui, 512, 420)
    Edit("AccountLoginPasswordEdit", ui, 512, 360)
    Button("AccountLoginLoginButton", ui, 512, 280, 200, 40)
    Button("AccountLoginExitButton", ui, 512, 100, 200, 40)
"#;

#[test]
fn the_d_pad_walks_the_login_boxes_and_buttons_and_a_clicks_them() {
    let _seam = pointer_seam();
    let mut app = glue_app(LOGIN_SCREEN);
    app.update();
    assert_eq!(mode(&app), Mode::Cursor);

    tap(&mut app, "PADDDOWN");
    assert_eq!(
        under_cursor(&app).as_deref(),
        Some("AccountLoginPasswordEdit"),
        "the first press lands on the nearest widget that way"
    );
    tap(&mut app, "PADDUP");
    assert_eq!(
        under_cursor(&app).as_deref(),
        Some("AccountLoginAccountEdit")
    );
    tap(&mut app, "PADDDOWN");
    tap(&mut app, "PADDDOWN");
    assert_eq!(
        under_cursor(&app).as_deref(),
        Some("AccountLoginLoginButton")
    );
    tap(&mut app, "PAD1");
    assert_eq!(clicks(&app), ["AccountLoginLoginButton"]);
    tap(&mut app, "PADDDOWN");
    assert_eq!(
        under_cursor(&app).as_deref(),
        Some("AccountLoginExitButton")
    );
    tap(&mut app, "PADDDOWN");
    assert_eq!(
        under_cursor(&app).as_deref(),
        Some("AccountLoginExitButton"),
        "nothing wraps"
    );

    // A on a box focuses it and brings up the keyboard, and the keys type into it.
    walk_to(&mut app, "AccountLoginPasswordEdit");
    tap(&mut app, "PAD1");
    assert!(shown(&app, "BenillaPadKeyboard"));
    walk_to(&mut app, "BenillaPadKey3_4");
    tap(&mut app, "PAD1");
    assert_eq!(text(&app, "AccountLoginPasswordEdit"), "f");
    assert_eq!(text(&app, "AccountLoginAccountEdit"), "");
}

/// The realm list: rows, then OK and Cancel below them.
const REALM_LIST: &str = r#"
    local ui = CreateFrame("Frame", "RealmListUI", GlueParent)
    ui:SetAllPoints()
    for i = 1, 3 do
        Button("RealmListRealmButton" .. i, ui, 512, 550 - i * 50, 300, 40)
    end
    Button("RealmListOkButton", ui, 420, 200, 150, 40)
    Button("RealmListCancelButton", ui, 624, 200, 150, 40)
"#;

#[test]
fn the_d_pad_walks_the_realm_list_and_a_clicks_a_realm_or_cancel() {
    let _seam = pointer_seam();
    let mut app = glue_app(REALM_LIST);
    app.update();
    assert_eq!(mode(&app), Mode::Cursor);

    tap(&mut app, "PADDDOWN");
    assert_eq!(under_cursor(&app).as_deref(), Some("RealmListOkButton"));
    tap(&mut app, "PADDRIGHT");
    assert_eq!(under_cursor(&app).as_deref(), Some("RealmListCancelButton"));
    tap(&mut app, "PAD1");
    assert_eq!(clicks(&app), ["RealmListCancelButton"]);
    tap(&mut app, "PADDLEFT");
    tap(&mut app, "PADDUP");
    assert_eq!(under_cursor(&app).as_deref(), Some("RealmListRealmButton3"));
    tap(&mut app, "PADDUP");
    assert_eq!(under_cursor(&app).as_deref(), Some("RealmListRealmButton2"));
    tap(&mut app, "PAD1");
    assert_eq!(
        clicks(&app),
        ["RealmListCancelButton", "RealmListRealmButton2"]
    );
    // B is Escape, which this fixture's screen has no use for: nothing happens and nothing breaks.
    tap(&mut app, "PAD2");
    assert_eq!(mode(&app), Mode::Cursor);
    assert_eq!(clicks(&app).len(), 2);
}

/// The world's VM replaces the glue one: cursor mode goes with it and the pad plays again.
#[test]
fn leaving_the_glue_screens_gives_the_pad_back_to_gameplay() {
    let _seam = pointer_seam();
    let mut app = glue_app(REALM_LIST);
    tap(&mut app, "PADDDOWN");
    assert_eq!(mode(&app), Mode::Cursor);
    app.world_mut()
        .insert_non_send_resource(UiScript::new().unwrap());
    app.update();
    assert_eq!(mode(&app), Mode::Gameplay);
}
