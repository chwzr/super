use ratatui::style::Color;

// Exact 256-color ANSI indexes captured from Claude Code via tmux capture-pane.
pub const CC_BLUE: Color     = Color::Indexed(153); // separator, cursor ❯, title, selected text
pub const CC_GREEN: Color    = Color::Indexed(114); // connected / active / ✔ checkmark
pub const CC_YELLOW: Color   = Color::Indexed(220); // warning / degraded
pub const CC_DIM: Color      = Color::Indexed(246); // secondary / placeholder text
pub const CC_ROSE: Color     = Color::Indexed(174); // logo / cost accent
pub const CC_TAB_BG: Color   = Color::Indexed(153); // active tab background
pub const CC_TAB_FG: Color   = Color::Indexed(16);  // active tab foreground (near-black)
pub const CC_BAR_BG: Color   = Color::Indexed(102); // usage progress bar track
pub const CC_INPUT_BG: Color = Color::Indexed(237); // input bar / modal background
pub const CC_RED: Color      = Color::Indexed(196); // error / disconnected
