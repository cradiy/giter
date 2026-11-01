use ratatui::style::Color;

pub const BG: Color = Color::Rgb(24, 24, 37);
pub const PANEL: Color = Color::Rgb(30, 30, 46);
pub const FG: Color = Color::Rgb(205, 214, 244);
pub const MUTED: Color = Color::Rgb(127, 132, 156);
pub const BORDER: Color = Color::Rgb(69, 71, 90);
pub const BLUE: Color = Color::Rgb(137, 180, 250);
pub const GREEN: Color = Color::Rgb(166, 227, 161);
pub const RED: Color = Color::Rgb(243, 139, 168);
pub const AMBER: Color = Color::Rgb(249, 226, 175);
pub const PURPLE: Color = Color::Rgb(203, 166, 247);
pub const CYAN: Color = Color::Rgb(137, 220, 235);
pub const PEACH: Color = Color::Rgb(250, 179, 135);
pub const TEAL: Color = Color::Rgb(148, 226, 213);
pub const ADD_BG: Color = Color::Rgb(31, 47, 43);
pub const DELETE_BG: Color = Color::Rgb(51, 32, 45);
pub const INCOMING_BG: Color = Color::Rgb(29, 39, 61);

pub fn safe(text: &str) -> String {
    text.chars()
        .flat_map(|c| match c {
            '\t' => "    ".chars().collect::<Vec<_>>(),
            '\r' => Vec::new(),
            c if c.is_control() => vec!['�'],
            c => vec![c],
        })
        .collect()
}
