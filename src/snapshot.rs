//! Export the exact Ratatui cell buffer, useful for reproducible visual reviews.
use anyhow::Result;
use ratatui::{
    buffer::Buffer,
    style::{Color, Modifier},
};
use std::fmt::Write;
use std::path::Path;
pub fn save(buffer: &Buffer, path: &Path) -> Result<()> {
    if path.extension().is_some_and(|e| e == "svg") {
        std::fs::write(path, svg(buffer))?;
    } else {
        let mut output = String::new();
        for y in buffer.area.y..buffer.area.bottom() {
            for x in buffer.area.x..buffer.area.right() {
                output.push_str(buffer[(x, y)].symbol());
            }
            output.push('\n');
        }
        std::fs::write(path, output)?;
    }
    Ok(())
}
pub fn svg(buffer: &Buffer) -> String {
    let cw = 9;
    let ch = 19;
    let width = u32::from(buffer.area.width) * cw;
    let height = u32::from(buffer.area.height) * ch;
    let mut out = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><rect width="100%" height="100%" fill="#1a1b26"/><g font-family="DejaVu Sans Mono, monospace" font-size="14">"##
    );
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            let px = u32::from(x) * cw;
            let py = u32::from(y) * ch;
            let _ = write!(
                out,
                r#"<rect x="{px}" y="{py}" width="{cw}" height="{ch}" fill="{}"/>"#,
                color(cell.bg, false)
            );
        }
    }
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            if cell.symbol().trim().is_empty() {
                continue;
            }
            let px = u32::from(x) * cw;
            let py = u32::from(y) * ch + 14;
            let symbol = cell
                .symbol()
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            let weight = if cell.modifier.contains(Modifier::BOLD) {
                "bold"
            } else {
                "normal"
            };
            let _ = write!(
                out,
                r#"<text x="{px}" y="{py}" fill="{}" font-weight="{weight}">{symbol}</text>"#,
                color(cell.fg, true)
            );
        }
    }
    out.push_str("</g></svg>");
    out
}
fn color(c: Color, fg: bool) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Reset => if fg { "#c0caf5" } else { "#1a1b26" }.into(),
        Color::Indexed(i) => {
            if i < 16 {
                const BASIC: [&str; 16] = [
                    "#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff",
                    "#a9b1d6", "#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7",
                    "#7dcfff", "#c0caf5",
                ];
                BASIC[i as usize].into()
            } else if i >= 232 {
                let n = 8 + 10 * (i - 232);
                format!("#{n:02x}{n:02x}{n:02x}")
            } else {
                let n = i - 16;
                let channel = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
                format!(
                    "#{:02x}{:02x}{:02x}",
                    channel(n / 36),
                    channel(n / 6 % 6),
                    channel(n % 6)
                )
            }
        }
        Color::Black => "#000000".into(),
        Color::White => "#ffffff".into(),
        Color::Red | Color::LightRed => "#ff5c7c".into(),
        Color::Green | Color::LightGreen => "#9ece6a".into(),
        Color::Yellow | Color::LightYellow => "#e0af68".into(),
        Color::Blue | Color::LightBlue => "#7aa2f7".into(),
        Color::Magenta | Color::LightMagenta => "#bb9af7".into(),
        Color::Cyan | Color::LightCyan => "#7dcfff".into(),
        Color::Gray => "#c0caf5".into(),
        Color::DarkGray => "#565f89".into(),
    }
}
