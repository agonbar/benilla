//! What the bars and prompts show: keyboard-and-mouse or controller. `auto` follows the last input
//! that is not bare pointer motion, so a trackpad hover on a Deck never flips it; a pad button or
//! stick does, and a key or a mouse click flips it back. `kbm` and `controller` pin it.

use crate::config::Presentation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Kbm,
    Controller,
}

impl Look {
    pub fn name(self) -> &'static str {
        match self {
            Look::Kbm => "kbm",
            Look::Controller => "controller",
        }
    }
}

/// What `auto` last saw.
#[derive(Debug, Default)]
pub struct Auto {
    last: Option<Look>,
}

impl Auto {
    /// A pad button went down or a stick is out of its dead zone.
    pub fn pad_input(&mut self) {
        self.last = Some(Look::Controller);
    }

    /// A key went down or a mouse button was clicked. Pointer motion is not reported here.
    pub fn kbm_input(&mut self) {
        self.last = Some(Look::Kbm);
    }

    /// The look in force. Before any input, `auto` is the controller if a pad is connected.
    pub fn resolve(&self, setting: Presentation, pad_connected: bool) -> Look {
        match setting {
            Presentation::Kbm => Look::Kbm,
            Presentation::Controller => Look::Controller,
            Presentation::Auto => self.last.unwrap_or(if pad_connected {
                Look::Controller
            } else {
                Look::Kbm
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_follows_pad_and_keyboard_input_and_nothing_else() {
        let mut a = Auto::default();
        assert_eq!(a.resolve(Presentation::Auto, false), Look::Kbm);
        assert_eq!(a.resolve(Presentation::Auto, true), Look::Controller);
        // A trackpad hover reports nothing, so nothing changes.
        assert_eq!(a.resolve(Presentation::Auto, false), Look::Kbm);
        a.pad_input();
        assert_eq!(a.resolve(Presentation::Auto, false), Look::Controller);
        a.kbm_input();
        assert_eq!(
            a.resolve(Presentation::Auto, true),
            Look::Kbm,
            "a key beats a pad that is merely plugged in"
        );
    }

    #[test]
    fn a_pinned_presentation_ignores_input() {
        let mut a = Auto::default();
        a.kbm_input();
        assert_eq!(a.resolve(Presentation::Controller, false), Look::Controller);
        a.pad_input();
        assert_eq!(a.resolve(Presentation::Kbm, true), Look::Kbm);
    }
}
