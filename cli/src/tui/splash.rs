//! Splash banner — diamond mascot + version/model/cwd metadata, rendered once
//! at startup into the terminal's scroll buffer (not into the live viewport).
//!
//! Per CLAUDE.md: "Splash: ASCII-art diamond rendered inside the scroll area
//! on launch." The "scroll area" here is the terminal's native scrollback.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// Six-line brilliant-cut diamond. Each line is padded to 17 columns so the
/// header metadata to its right stays vertically aligned regardless of the
/// glyph distribution on a given row.
pub const MASCOT: [&str; 6] = [
    "        _______  ",
    "      .'_/_|_\\_'.",
    "      \\`\\  |  /`/",
    "       `\\\\ | //' ",
    "         `\\|/`   ",
    "           `     ",
];

/// Build the splash banner (mascot + version/model/cwd metadata) as ratatui
/// `Line`s. Returned once at startup so the host can write it to scrollback
/// via `Terminal::insert_before`.
pub fn banner_lines(
    version: &str,
    model_label: &str,
    provider_label: &str,
    user_handle: &str,
    cwd: &str,
) -> Vec<Line<'static>> {
    let mascot_style = Style::default().fg(Color::Cyan);
    let title_style = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);
    let meta_style = Style::default().fg(Color::Gray);
    let gap = "   ";

    vec![
        Line::from(Span::styled(MASCOT[0], mascot_style)),
        Line::from(vec![
            Span::styled(MASCOT[1], mascot_style),
            Span::raw(gap),
            Span::styled(format!("Super CLI v{version}"), title_style),
        ]),
        Line::from(vec![
            Span::styled(MASCOT[2], mascot_style),
            Span::raw(gap),
            Span::styled(format!("{model_label} · {provider_label}"), meta_style),
        ]),
        Line::from(vec![
            Span::styled(MASCOT[3], mascot_style),
            Span::raw(gap),
            Span::styled(format!("@{user_handle} · {cwd}"), meta_style),
        ]),
        Line::from(Span::styled(MASCOT[4], mascot_style)),
        Line::from(Span::styled(MASCOT[5], mascot_style)),
        Line::from(""),
    ]
}
