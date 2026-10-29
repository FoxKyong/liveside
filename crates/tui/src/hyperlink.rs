//! Making links clickable in the terminal (OSC 8), after ratatui has drawn.
//!
//! ratatui measures what it writes into a cell, and an escape sequence
//! written there would count as text and push everything after it aside.
//! So the screen is drawn as usual, and then each piece of a link is
//! written once more over itself, the same characters in the same style,
//! wrapped in `ESC ] 8 ; id=… ; URL ST` and `ESC ] 8 ; ; ST`. The pieces of
//! one wrapped link share an id, so a terminal lights them up together.
//!
//! Only with `security.links = "open"`: a terminal cannot ask first, so
//! anything else leaves the text as text. Whatever this says, a terminal
//! that finds addresses in plain text by itself still does so.

use std::io::Write;

use ratatui::buffer::Buffer;
use ratatui::crossterm::{cursor, queue, style};
use ratatui::prelude::IntoCrossterm;
use unicode_width::UnicodeWidthStr;

use crate::render::LinkArea;

/// One piece of a link with what is on screen there: each cell's symbol
/// and style.
pub struct Piece {
    pub area: LinkArea,
    pub cells: Vec<(String, ratatui::style::Style)>,
}

/// Reads what ratatui put in the cells the links occupy.
pub fn pieces(buffer: &Buffer, links: Vec<LinkArea>) -> Vec<Piece> {
    links
        .into_iter()
        // An address that could end the escape sequence early is not
        // linked; the url crate never produces one, but this is the place
        // where it would matter.
        .filter(|area| !area.url.chars().any(char::is_control))
        .map(|area| {
            let cells = (area.x..area.x + area.width)
                .filter_map(|x| buffer.cell((x, area.y)))
                .map(|cell| (cell.symbol().to_owned(), cell.style()))
                .collect();
            Piece { area, cells }
        })
        .collect()
}

/// Writes the pieces over themselves as links, and leaves the cursor and
/// the attributes as they were.
pub fn paint(out: &mut impl Write, pieces: &[Piece]) -> std::io::Result<()> {
    if pieces.is_empty() {
        return Ok(());
    }
    queue!(out, cursor::SavePosition)?;
    for piece in pieces {
        let area = &piece.area;
        queue!(
            out,
            cursor::MoveTo(area.x, area.y),
            style::Print(format!("\x1b]8;id={};{}\x1b\\", area.id, area.url))
        )?;
        let mut cells = piece.cells.iter();
        while let Some((symbol, cell_style)) = cells.next() {
            let content_style: style::ContentStyle = (*cell_style).into_crossterm();
            queue!(
                out,
                style::PrintStyledContent(style::StyledContent::new(
                    content_style,
                    symbol.as_str()
                ))
            )?;
            // A wide character covers the next cell too, which holds
            // nothing of its own.
            if symbol.width() > 1 {
                cells.next();
            }
        }
        queue!(out, style::Print("\x1b]8;;\x1b\\"))?;
    }
    queue!(
        out,
        style::SetAttribute(style::Attribute::Reset),
        style::ResetColor,
        cursor::RestorePosition
    )?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    #[test]
    fn a_piece_is_written_over_itself_inside_the_link() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 20, 2));
        buffer.set_string(3, 1, "a.cz/x", Style::new().underlined());
        let pieces = pieces(
            &buffer,
            vec![LinkArea {
                x: 3,
                y: 1,
                width: 6,
                url: "https://a.cz/x".into(),
                id: "0.0.4".into(),
            }],
        );
        assert_eq!(pieces[0].cells.len(), 6);
        let mut out = Vec::new();
        paint(&mut out, &pieces).unwrap();
        let out = String::from_utf8(out).unwrap();
        let open = out
            .find("\x1b]8;id=0.0.4;https://a.cz/x\x1b\\")
            .expect("opens");
        let close = out.find("\x1b]8;;\x1b\\").expect("closes");
        assert!(open < close);
        let inside = &out[open..close];
        for c in ["a", ".", "c", "z", "/", "x"] {
            assert!(inside.contains(c), "{inside:?}");
        }
        // Moved to the piece, and the cursor put back afterwards.
        assert!(out.contains("\x1b[2;4H"), "{out:?}");
        assert!(
            out.starts_with("\x1b7") && out.ends_with("\x1b8"),
            "{out:?}"
        );
    }

    #[test]
    fn nothing_to_link_writes_nothing() {
        let mut out = Vec::new();
        paint(&mut out, &[]).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn an_address_with_control_characters_is_not_linked() {
        let buffer = Buffer::empty(Rect::new(0, 0, 10, 1));
        let found = pieces(
            &buffer,
            vec![LinkArea {
                x: 0,
                y: 0,
                width: 3,
                url: "https://a.cz/\x1b]8;;evil".into(),
                id: "x".into(),
            }],
        );
        assert!(found.is_empty());
    }
}
