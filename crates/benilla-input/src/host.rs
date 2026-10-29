//! The functions the shipped addon calls into the host, `BenillaPad_*`, and the state they read and
//! write. The ring and the keyboard are frames of the 2D UI, so they see the pad only through
//! these: the stick vector, the mode, a glyph, the ring's pick, and the text to type. They are
//! registered on the UI's Lua state ([`benilla_ui::script::UiScript::lua`]) once per session, over
//! a [`Host`] the pad's systems share.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use benilla_ui::mlua::{self, Lua};
use bevy::math::Vec2;
use bevy::prelude::Resource;

use crate::keyboard::{self, Typed};
use crate::{glyphs, ring};

#[derive(Default)]
struct Inner {
    left: Vec2,
    right: Vec2,
    cursor_mode: bool,
    ring_open: bool,
    typed: Vec<Typed>,
}

/// What the pad's systems and the addon's functions share.
#[derive(Resource, Clone, Default)]
pub struct Host(Arc<Mutex<Inner>>);

impl Host {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The pad's sticks and mode this frame.
    pub fn set(&self, left: Vec2, right: Vec2, cursor_mode: bool) {
        let mut inner = self.lock();
        inner.left = left;
        inner.right = right;
        inner.cursor_mode = cursor_mode;
    }

    /// The ring is open: the camera stick picks an entry and does not turn the camera.
    pub fn ring_open(&self) -> bool {
        self.lock().ring_open
    }

    /// What the keyboard frame asked to type since the last call, in order.
    pub fn take_typed(&self) -> Vec<Typed> {
        std::mem::take(&mut self.lock().typed)
    }

    /// Register the functions on `lua`, replacing any an earlier session registered.
    pub fn install(&self, lua: &Lua) -> mlua::Result<()> {
        let g = lua.globals();
        let host = self.clone();
        g.set(
            "BenillaPad_GetStick",
            lua.create_function(move |_, ()| {
                let inner = host.lock();
                Ok((inner.left.x, inner.left.y, inner.right.x, inner.right.y))
            })?,
        )?;
        let host = self.clone();
        g.set(
            "BenillaPad_GetMode",
            lua.create_function(move |_, ()| {
                Ok(if host.lock().cursor_mode {
                    "cursor"
                } else {
                    "gameplay"
                })
            })?,
        )?;
        g.set(
            "BenillaPad_GetGlyph",
            lua.create_function(|_, token: String| Ok(glyphs::glyph(&token)))?,
        )?;
        g.set(
            "BenillaPad_RingSlot",
            lua.create_function(|_, (x, y, n): (f32, f32, usize)| {
                Ok(ring::slot(x, y, n).map(|s| s + 1))
            })?,
        )?;
        let host = self.clone();
        g.set(
            "BenillaPad_RingOpen",
            lua.create_function(move |_, open: Option<bool>| {
                host.lock().ring_open = open.unwrap_or(false);
                Ok(())
            })?,
        )?;
        let host = self.clone();
        g.set(
            "BenillaPad_Type",
            lua.create_function(move |_, text: String| {
                if !text.is_empty() {
                    host.lock().typed.push(Typed::Text(text));
                }
                Ok(())
            })?,
        )?;
        let host = self.clone();
        g.set(
            "BenillaPad_Key",
            lua.create_function(move |_, name: String| {
                if let Some(code) = keyboard::named_key(&name) {
                    host.lock().typed.push(Typed::Key(code));
                }
                Ok(())
            })?,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use benilla_ui::script::UiScript;
    use bevy::input::keyboard::KeyCode;

    use super::*;

    fn installed() -> (UiScript, Host) {
        let s = UiScript::new().expect("construct UiScript");
        let host = Host::default();
        host.install(s.lua()).expect("the functions register");
        (s, host)
    }

    /// The spike: a function table registered through `UiScript::lua()` is callable from the UI's
    /// Lua and reads the host's state live.
    #[test]
    fn the_functions_register_on_the_ui_state_and_read_the_hosts_state() {
        let (s, host) = installed();
        host.set(Vec2::new(0.25, -0.5), Vec2::new(-1.0, 0.75), false);
        let (lx, ly, rx, ry): (f32, f32, f32, f32) =
            s.eval("return BenillaPad_GetStick()").unwrap();
        assert_eq!((lx, ly, rx, ry), (0.25, -0.5, -1.0, 0.75));
        assert_eq!(
            s.eval::<String>("return BenillaPad_GetMode()").unwrap(),
            "gameplay"
        );
        host.set(Vec2::ZERO, Vec2::ZERO, true);
        assert_eq!(
            s.eval::<String>("return BenillaPad_GetMode()").unwrap(),
            "cursor"
        );
    }

    #[test]
    fn the_glyph_and_the_ring_pick_answer_from_the_pure_parts() {
        let (s, _) = installed();
        assert_eq!(
            s.eval::<Option<String>>("return BenillaPad_GetGlyph(\"PAD1\")")
                .unwrap()
                .as_deref(),
            Some("A")
        );
        assert_eq!(
            s.eval::<Option<String>>("return BenillaPad_GetGlyph(\"NOPE\")")
                .unwrap(),
            None
        );
        let slot = |x: &str, y: &str| -> Option<i32> {
            s.eval(&format!("return BenillaPad_RingSlot({x}, {y}, 8)"))
                .unwrap()
        };
        assert_eq!(slot("0", "1"), Some(1), "Lua counts from 1");
        assert_eq!(slot("1", "0"), Some(3));
        assert_eq!(slot("0", "0.2"), None);
    }

    #[test]
    fn typed_text_and_keys_queue_in_order_and_an_unknown_key_is_dropped() {
        let (s, host) = installed();
        s.run(
            "BenillaPad_Type(\"h\") BenillaPad_Type(\"\") BenillaPad_Key(\"BACKSPACE\") \
             BenillaPad_Key(\"F5\") BenillaPad_Key(\"ENTER\")",
        )
        .unwrap();
        assert_eq!(
            host.take_typed(),
            [
                Typed::Text("h".into()),
                Typed::Key(KeyCode::Backspace),
                Typed::Key(KeyCode::Enter)
            ]
        );
        assert!(host.take_typed().is_empty());
    }

    #[test]
    fn the_ring_flag_follows_the_addon() {
        let (s, host) = installed();
        assert!(!host.ring_open());
        s.run("BenillaPad_RingOpen(true)").unwrap();
        assert!(host.ring_open());
        s.run("BenillaPad_RingOpen(false)").unwrap();
        assert!(!host.ring_open());
        s.run("BenillaPad_RingOpen(true) BenillaPad_RingOpen()")
            .unwrap();
        assert!(!host.ring_open(), "no argument is closed");
    }
}
