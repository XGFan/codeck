//! Screen: the Pane's terminal as text, kept by a VT (alacritty_terminal, the same VT the Holder
//! uses) fed with the Holder's Snapshots and Output. Used only to recognize UI and option positions.

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, test::TermSize};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

#[derive(Clone, Copy, Default)]
struct Sink;

impl EventListener for Sink {
    fn send_event(&self, _: Event) {}
}

/// Current screen as text: one string per row with trailing whitespace removed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Screen {
    pub lines: Vec<String>,
    /// 0-based (row, col).
    pub cursor: (u16, u16),
}

/// A terminal kept up to date from a byte stream.
pub struct Vt {
    term: Term<Sink>,
    parser: Processor<StdSyncHandler>,
    cols: u16,
    rows: u16,
}

fn term_size(cols: u16, rows: u16) -> TermSize {
    TermSize::new(cols.max(1) as usize, rows.max(1) as usize)
}

impl Vt {
    pub fn new(cols: u16, rows: u16) -> Vt {
        Vt {
            term: Term::new(Config::default(), &term_size(cols, rows), Sink),
            parser: Processor::new(),
            cols,
            rows,
        }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// Starts over from a Snapshot (it clears the screen and redraws it, cursor and SGR included).
    pub fn reset_to(&mut self, snapshot: &[u8]) {
        *self = Vt::new(self.cols, self.rows);
        self.feed(snapshot);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        if (cols, rows) != (self.cols, self.rows) {
            self.cols = cols;
            self.rows = rows;
            self.term.resize(term_size(cols, rows));
        }
    }

    pub fn screen(&self) -> Screen {
        let grid = self.term.grid();
        let lines = (0..grid.screen_lines())
            .map(|l| {
                let row = &grid[Line(l as i32)];
                let mut s = String::new();
                for c in 0..grid.columns() {
                    let cell = &row[Column(c)];
                    if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                        continue;
                    }
                    s.push(cell.c);
                    if let Some(zw) = cell.zerowidth() {
                        s.extend(zw.iter());
                    }
                }
                s.trim_end().to_string()
            })
            .collect();
        let p = grid.cursor.point;
        Screen {
            lines,
            cursor: (p.line.0.max(0) as u16, p.column.0 as u16),
        }
    }
}

impl Screen {
    /// Renders `bytes` (a Snapshot, optionally followed by Output) on a `cols` × `rows` grid.
    pub fn render(bytes: &[u8], cols: u16, rows: u16) -> Screen {
        let mut vt = Vt::new(cols, rows);
        vt.feed(bytes);
        vt.screen()
    }

    /// Non-empty lines joined with `\n` (for logs and probes).
    pub fn text(&self) -> String {
        self.lines.join("\n").trim_end().to_string()
    }

    /// Index of the first line containing `needle`.
    pub fn find(&self, needle: &str) -> Option<usize> {
        self.lines.iter().position(|l| l.contains(needle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_text_and_cursor() {
        let s = Screen::render(b"\x1b[2J\x1b[Hhello\r\n  world\x1b[3;5H", 20, 4);
        assert_eq!(s.lines, vec!["hello", "  world", "", ""]);
        assert_eq!(s.cursor, (2, 4));
    }

    #[test]
    fn output_after_a_snapshot_updates_the_screen() {
        let mut vt = Vt::new(20, 3);
        vt.feed(b"\x1b[2J\x1b[Hold");
        vt.reset_to(b"\x1b[2J\x1b[Hsnap");
        assert_eq!(vt.screen().lines[0], "snap");
        vt.feed(b"\r\nmore");
        assert_eq!(vt.screen().lines[..2], ["snap", "more"]);
    }

    #[test]
    fn wide_chars_are_not_doubled() {
        let s = Screen::render("中文 ok".as_bytes(), 20, 1);
        assert_eq!(s.lines[0], "中文 ok");
    }
}
