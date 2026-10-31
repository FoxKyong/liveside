//! Keeps rows straight after emoji such as ❤️ (with VS16, U+FE0F).
//!
//! ratatui 0.30.2 sends the column an emoji covers right after the emoji,
//! for terminals that draw it one column wide. The crossterm backend then
//! leaves out the cursor move, because the cell is "next" to the last one,
//! but a terminal that draws the emoji two columns wide has its cursor one
//! column further on. Everything after it on the row lands one column to
//! the right, and a dark gap is left behind (ratatui issue #2651).
//!
//! The fix is merged upstream (ratatui PRs #2721 and #2686) but not yet
//! released. This does the same from outside: the covered columns go
//! first and the emoji after them, and nothing is handed over as
//! "the next cell" after a cell that is not one column wide, so the
//! backend moves the cursor. Once a ratatui with the fix is released, this
//! file can go.

use std::io;

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::{Cell, CellWidth};
use ratatui::layout::{Position, Size};

/// A backend passing everything to the one inside, except that drawing is
/// put in an order the terminal cannot misplace.
pub struct WideSafe<B> {
    inner: B,
}

impl<B> WideSafe<B> {
    pub fn new(inner: B) -> Self {
        Self { inner }
    }
}

/// The cells in an order that survives a terminal of either opinion on
/// the width of an emoji: what a wide cell covers comes before it, so
/// drawing it two columns wide paints over those, and drawing it one
/// column wide leaves them cleared.
fn reorder<'a>(cells: &[(u16, u16, &'a Cell)]) -> Vec<(u16, u16, &'a Cell)> {
    let mut ordered = Vec::with_capacity(cells.len());
    let mut i = 0;
    while i < cells.len() {
        let (x, y, cell) = cells[i];
        let reach = u32::from(x) + u32::from(cell.cell_width());
        let mut end = i + 1;
        while let Some(&(cx, cy, _)) = cells.get(end) {
            if cy != y || cx <= x || u32::from(cx) >= reach {
                break;
            }
            end += 1;
        }
        ordered.extend_from_slice(&cells[i + 1..end]);
        ordered.push(cells[i]);
        i = end;
    }
    ordered
}

impl<B: Backend> Backend for WideSafe<B> {
    type Error = B::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let cells: Vec<_> = content.collect();
        let ordered = reorder(&cells);
        // The backend skips the cursor move only for the cell right after
        // the previous one, as if every cell were one column wide. Handing
        // over a fresh run after any other cell makes it move.
        let mut start = 0;
        for end in 1..=ordered.len() {
            if end == ordered.len() || ordered[end - 1].2.cell_width() != 1 {
                self.inner.draw(ordered[start..end].iter().copied())?;
                start = end;
            }
        }
        Ok(())
    }

    fn append_lines(&mut self, n: u16) -> Result<(), Self::Error> {
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }
}

/// Written straight through, for what is put on screen besides the cells
/// (links, the bell).
impl<B: io::Write> io::Write for WideSafe<B> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    /// A terminal as the crossterm backend drives it: the cursor moves
    /// unless a cell comes right after the previous one in the same draw,
    /// and each symbol advances the cursor by as many columns as the
    /// terminal draws it. A wide symbol leaves "" in the columns it covers,
    /// and writing into one of those wipes the whole symbol, as Ghostty and
    /// kitty do.
    struct Model {
        width: u16,
        rows: Vec<Vec<String>>,
        /// How wide this terminal draws an emoji with VS16.
        vs16_width: u16,
    }

    impl Model {
        fn new(width: u16, height: u16, vs16_width: u16) -> Self {
            Self {
                width,
                rows: vec![vec![" ".to_owned(); width.into()]; height.into()],
                vs16_width,
            }
        }

        fn drawn_width(&self, symbol: &str) -> u16 {
            if symbol.contains('\u{FE0F}') {
                self.vs16_width
            } else {
                symbol.cell_width()
            }
        }

        fn put(&mut self, x: u16, y: u16, symbol: &str) -> u16 {
            let width = self.drawn_width(symbol);
            let row = &mut self.rows[usize::from(y)];
            let x = usize::from(x);
            // Writing over part of a wide symbol wipes all of it.
            if row[x].is_empty() {
                let mut head = x;
                while head > 0 && row[head].is_empty() {
                    head -= 1;
                }
                row[head] = " ".to_owned();
                for cell in &mut row[head + 1..=x] {
                    *cell = " ".to_owned();
                }
            }
            let mut after = x + 1;
            while after < row.len() && row[after].is_empty() {
                row[after] = " ".to_owned();
                after += 1;
            }
            row[x] = symbol.to_owned();
            for k in 1..usize::from(width) {
                if let Some(cell) = row.get_mut(x + k)
                    && !cell.is_empty()
                {
                    *cell = String::new();
                }
            }
            width
        }

        fn row(&self, y: usize) -> String {
            self.rows[y].concat()
        }
    }

    impl Backend for Model {
        type Error = io::Error;

        fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
        where
            I: Iterator<Item = (u16, u16, &'a Cell)>,
        {
            let mut last: Option<(u16, u16)> = None;
            let mut cursor = (0, 0);
            for (x, y, cell) in content {
                if !matches!(last, Some((lx, ly)) if x == lx + 1 && y == ly) {
                    cursor = (x, y);
                }
                last = Some((x, y));
                if cursor.0 < self.width {
                    let advance = self.put(cursor.0, cursor.1, cell.symbol());
                    cursor.0 += advance;
                }
            }
            Ok(())
        }

        fn hide_cursor(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn show_cursor(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn get_cursor_position(&mut self) -> io::Result<Position> {
            Ok(Position::ORIGIN)
        }
        fn set_cursor_position<P: Into<Position>>(&mut self, _: P) -> io::Result<()> {
            Ok(())
        }
        fn clear(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn clear_region(&mut self, _: ClearType) -> io::Result<()> {
            Ok(())
        }
        fn size(&self) -> io::Result<Size> {
            Ok(Size::new(self.width, self.rows.len() as u16))
        }
        fn window_size(&mut self) -> io::Result<WindowSize> {
            Err(io::Error::other("not a real terminal"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn line(text: &str, width: u16) -> Buffer {
        let mut buffer = Buffer::empty(Rect::new(0, 0, width, 1));
        buffer.set_string(0, 0, text, ratatui::style::Style::default());
        buffer
    }

    const WIDTH: u16 = 16;

    /// Draws `before`, then the change to `after`, as ratatui would, and
    /// gives back what the terminal shows.
    fn screen(before: &str, after: &str, vs16_width: u16, safe: bool) -> String {
        let (old, new) = (line(before, WIDTH), line(after, WIDTH));
        let mut model = Model::new(WIDTH, 1, vs16_width);
        let blank = Buffer::empty(old.area);
        if safe {
            let mut backend = WideSafe::new(model);
            backend.draw(blank.diff(&old).into_iter()).unwrap();
            backend.draw(old.diff(&new).into_iter()).unwrap();
            model = backend.inner;
        } else {
            model.draw(blank.diff(&old).into_iter()).unwrap();
            model.draw(old.diff(&new).into_iter()).unwrap();
        }
        model.row(0)
    }

    /// What the terminal shows when `text` is drawn on an empty screen,
    /// which comes out right with or without the wrapper.
    fn fresh(text: &str, vs16_width: u16) -> String {
        let mut model = Model::new(WIDTH, 1, vs16_width);
        let new = line(text, WIDTH);
        model
            .draw(Buffer::empty(new.area).diff(&new).into_iter())
            .unwrap();
        model.row(0)
    }

    #[test]
    fn the_bug_this_works_around_is_still_there() {
        // When this fails, ratatui has fixed it and the file can go.
        assert_ne!(screen("aaaaaaaaaa", "❤️ b c", 2, false), fresh("❤️ b c", 2));
    }

    #[test]
    fn a_row_with_an_emoji_stays_in_place() {
        for vs16_width in [2, 1] {
            for (before, after) in [
                ("aaaaaaaaaa", "❤️ b c"),
                ("xx srdce", "ok ❤️ 🏴‍☠️ 👍 z"),
                ("ok ❤️ 🏴‍☠️ 👍 z", "xx srdce"),
                ("a ❤️ b", "❤️ a ❤️ b"),
            ] {
                assert_eq!(
                    screen(before, after, vs16_width, true),
                    fresh(after, vs16_width),
                    "{before:?} -> {after:?}, VS16 drawn {vs16_width} wide"
                );
            }
        }
        assert_eq!(fresh("❤️ b", 2), format!("❤️ b{}", " ".repeat(12)));
    }

    #[test]
    fn plain_text_and_other_wide_letters_are_unchanged() {
        for (before, after) in [("abcdef", "abXdef"), ("aaaaaa", "コbc"), ("コbc", "abcd")] {
            assert_eq!(screen(before, after, 2, true), fresh(after, 2));
        }
    }
}
