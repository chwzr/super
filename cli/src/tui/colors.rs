use ratatui::style::Color;

// Exact 256-color ANSI indexes captured from Claude Code via tmux capture-pane.
pub const CC_BLUE: Color = Color::Indexed(153); // separator, cursor ❯, title, selected text
pub const CC_GREEN: Color = Color::Indexed(114); // connected / active / ✔ checkmark
pub const CC_YELLOW: Color = Color::Indexed(220); // warning / degraded
pub const CC_DIM: Color = Color::Indexed(246); // secondary / placeholder text
pub const CC_ROSE: Color = Color::Indexed(174); // logo / cost accent
pub const CC_TAB_BG: Color = Color::Indexed(153); // active tab background
pub const CC_TAB_FG: Color = Color::Indexed(16); // active tab foreground (near-black)
pub const CC_BAR_BG: Color = Color::Indexed(102); // usage progress bar track
pub const CC_INPUT_BG: Color = Color::Indexed(237); // input bar / modal background
pub const CC_RED: Color = Color::Indexed(196); // error / disconnected

// Captured from Claude Code v2.1.143 (see cli/docs/tool-call-render-spec.md).
pub const CC_ORANGE: Color = Color::Indexed(211); // error tool prefix, bash error text
pub const CC_DIFF_DEL_FG: Color = Color::Indexed(167); // removed line fg
pub const CC_DIFF_DEL_BG: Color = Color::Indexed(52); // removed line bg
pub const CC_DIFF_ADD_FG: Color = Color::Indexed(77); // added line fg
pub const CC_DIFF_ADD_BG: Color = Color::Indexed(22); // added line bg

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn diff_palette_uses_expected_ansi_indexes() {
        assert_eq!(CC_ORANGE, Color::Indexed(211));
        assert_eq!(CC_DIFF_DEL_FG, Color::Indexed(167));
        assert_eq!(CC_DIFF_DEL_BG, Color::Indexed(52));
        assert_eq!(CC_DIFF_ADD_FG, Color::Indexed(77));
        assert_eq!(CC_DIFF_ADD_BG, Color::Indexed(22));
    }
}
