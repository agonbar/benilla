//! D-pad snapping for a pointer with no mouse under it (a gamepad's cursor, the VR laser): from the
//! cursor, the nearest clickable widget in a direction. The widgets are the arena's own, so no Lua
//! walk is needed; 1.12.1 has no such verb, so the host app moves the cursor with the answer.
//!
//! A candidate is a shown, mouse-enabled button, check button, slider or edit box whose hit-rect
//! centre the hover walk would give to that frame ([`UiScript::hit_test_frame`]), so a widget
//! covered or clipped at its centre is not offered.

use super::pointer::frame_contains;
use super::UiScript;
use crate::order;
use crate::widget::FrameKind;

/// A direction on the screen, as the d-pad names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapDir {
    Up,
    Down,
    Left,
    Right,
}

/// The index of the centre in `centres` a snap from `from` in `dir` lands on, all in UI units
/// (y up). A centre is in play if it lies inside the 45 degree cone about the direction (its
/// sideways offset no larger than its forward distance). The pick is the least `forward + 2 *
/// sideways`, so a widget in line beats a nearer one off to the side. Nothing wraps around.
pub fn nearest(from: (f32, f32), dir: SnapDir, centres: &[(f32, f32)]) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, &(x, y)) in centres.iter().enumerate() {
        let (dx, dy) = (x - from.0, y - from.1);
        let (forward, side) = match dir {
            SnapDir::Up => (dy, dx.abs()),
            SnapDir::Down => (-dy, dx.abs()),
            SnapDir::Right => (dx, dy.abs()),
            SnapDir::Left => (-dx, dy.abs()),
        };
        // A centre on the cursor is where the cursor already is; under a pixel is not a move.
        if forward < 1.0 || side > forward {
            continue;
        }
        let score = forward + 2.0 * side;
        if best.is_none_or(|(_, s)| score < s) {
            best = Some((i, score));
        }
    }
    best.map(|(i, _)| i)
}

impl UiScript {
    /// Where the cursor goes for a snap from `from` (UI units, y up) in `dir`: the hit-rect centre
    /// of the nearest clickable widget there, or `None` when there is none that way. Needs
    /// [`Self::resolve`] first, as the hit test does.
    pub fn snap_target(&self, from: (f32, f32), dir: SnapDir) -> Option<(f32, f32)> {
        let centres = self.snap_centres();
        nearest(from, dir, &centres).map(|i| centres[i])
    }

    /// The hit-rect centre of every clickable widget the hover walk would give to that widget.
    fn snap_centres(&self) -> Vec<(f32, f32)> {
        let model = self.model_ref();
        let mut out = Vec::new();
        for &(fh, _) in order::pick_order(&model.arena).iter() {
            let Some(frame) = model.arena.frame(fh) else {
                continue;
            };
            if !matches!(
                frame.kind,
                FrameKind::Button
                    | FrameKind::LootButton
                    | FrameKind::CheckButton
                    | FrameKind::Slider
                    | FrameKind::EditBox
            ) || !model.arena.is_mouse_enabled(fh)
            {
                continue;
            }
            let Some(r) = model.resolved.get(&fh) else {
                continue;
            };
            let r = super::clip::hit_rect(&model, fh, *r);
            let (cx, cy) = ((r.left + r.right) / 2.0, (r.bottom + r.top) / 2.0);
            if r.right <= r.left || r.top <= r.bottom || !frame_contains(&model, fh, cx, cy) {
                continue;
            }
            out.push((fh, (cx, cy)));
        }
        drop(model);
        out.into_iter()
            .filter(|&(fh, (cx, cy))| self.hit_test_frame(cx, cy) == Some(fh))
            .map(|(_, c)| c)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script() -> UiScript {
        UiScript::new().expect("construct UiScript")
    }

    #[test]
    fn the_nearest_in_the_cone_wins_and_a_widget_in_line_beats_one_off_to_the_side() {
        let c = [(100.0, 0.0), (60.0, 50.0), (0.0, 100.0), (-100.0, 0.0)];
        assert_eq!(nearest((0.0, 0.0), SnapDir::Right, &c), Some(0));
        assert_eq!(nearest((0.0, 0.0), SnapDir::Up, &c), Some(2));
        assert_eq!(nearest((0.0, 0.0), SnapDir::Left, &c), Some(3));
        assert_eq!(
            nearest((0.0, 0.0), SnapDir::Down, &c),
            None,
            "nothing wraps"
        );
    }

    #[test]
    fn a_centre_outside_the_45_degree_cone_is_not_a_candidate() {
        // 100 right, 150 up is steeper than 45 degrees: up only.
        let c = [(100.0, 150.0)];
        assert_eq!(nearest((0.0, 0.0), SnapDir::Right, &c), None);
        assert_eq!(nearest((0.0, 0.0), SnapDir::Up, &c), Some(0));
        // Exactly 45 degrees is in.
        assert_eq!(
            nearest((0.0, 0.0), SnapDir::Right, &[(100.0, 100.0)]),
            Some(0)
        );
    }

    #[test]
    fn the_widget_under_the_cursor_is_not_a_move() {
        assert_eq!(nearest((10.0, 10.0), SnapDir::Right, &[(10.0, 10.0)]), None);
    }

    #[test]
    fn a_grid_steps_one_slot_at_a_time() {
        let mut s = script();
        s.set_screen_size(800.0, 600.0);
        s.run(
            r#"
            for row = 0, 1 do
              for col = 0, 2 do
                local b = CreateFrame("Button", "Slot"..row..col)
                b:SetPoint("BOTTOMLEFT", 100 + col * 50, 100 + row * 50)
                b:SetWidth(40); b:SetHeight(40)
                b:EnableMouse(true)
              end
            end
        "#,
        )
        .unwrap();
        s.resolve();
        // Slot00 spans 100..140; its centre is (120, 120).
        let from = (120.0, 120.0);
        assert_eq!(s.snap_target(from, SnapDir::Right), Some((170.0, 120.0)));
        assert_eq!(s.snap_target(from, SnapDir::Up), Some((120.0, 170.0)));
        assert_eq!(s.snap_target(from, SnapDir::Left), None);
        assert_eq!(s.snap_target(from, SnapDir::Down), None);
        assert_eq!(
            s.snap_target((220.0, 170.0), SnapDir::Left),
            Some((170.0, 170.0))
        );
    }

    #[test]
    fn a_covered_hidden_or_mouse_less_widget_is_not_offered() {
        let mut s = script();
        s.set_screen_size(800.0, 600.0);
        s.run(
            r#"
            local function btn(name, x, mouse)
              local b = CreateFrame("Button", name)
              b:SetPoint("BOTTOMLEFT", x, 100); b:SetWidth(40); b:SetHeight(40)
              b:EnableMouse(mouse)
              return b
            end
            btn("Here", 100, true)
            btn("NoMouse", 200, false)
            btn("Hidden", 300, true):Hide()
            btn("Under", 400, true)
            local over = CreateFrame("Frame", "Over")
            over:SetFrameStrata("DIALOG")
            over:SetPoint("BOTTOMLEFT", 390, 90); over:SetWidth(60); over:SetHeight(60)
            over:EnableMouse(true)
            btn("Clear", 500, true)
        "#,
        )
        .unwrap();
        s.resolve();
        assert_eq!(
            s.snap_target((120.0, 120.0), SnapDir::Right),
            Some((520.0, 120.0))
        );
    }
}
