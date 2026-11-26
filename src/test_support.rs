//! **The terminal model the tests share**: `Screen`, `feed_all` and `drain`, kept outside any one
//! test module because the editor's tests and `handoff`'s both drive a [`LineEditor`] on it, and
//! the raw queue's tests and `handoff`'s both drain a [`RawQueue`].

use crate::{Event, LineEditor, MAXIMUM_RAW_QUEUE_BYTES, RawQueue, Sink};
extern crate std;
use std::string::String;
use std::vec::Vec;

/// A one-row terminal model: interprets the echo stream the way a video terminal (VT) does, so the tests
/// assert **what the user sees**, not which escape sequences we happened to emit. This is
/// what lets the engine change its redraw strategy without rewriting every test. A column holds one
/// character, so a UTF-8 character of several bytes takes one column, as on a terminal.
pub(super) struct Screen {
    row: Vec<char>,
    /// The bytes of a UTF-8 character the echo has begun and not yet finished.
    partial: Vec<u8>,
    pub(super) column: usize,
    pub(super) bells: usize,
    pub(super) cleared: bool,
}

impl Screen {
    pub(super) fn new() -> Self {
        Screen {
            row: Vec::new(),
            partial: Vec::new(),
            column: 0,
            bells: 0,
            cleared: false,
        }
    }

    /// The visible row as a string (trailing blanks trimmed).
    pub(super) fn text(&self) -> String {
        let end = self
            .row
            .iter()
            .rposition(|&c| c != ' ')
            .map_or(0, |p| p + 1);
        self.row[..end].iter().collect()
    }

    /// The column the cursor is in.
    pub(super) fn cursor(&self) -> usize {
        self.column
    }

    fn interpret(&mut self, bytes: &[u8]) {
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if b >= 0x80 || !self.partial.is_empty() {
                // Echo carries only whole characters, so a byte here belongs to one.
                assert!(b >= 0x80, "echo cut a UTF-8 character short");
                self.partial.push(b);
                match core::str::from_utf8(&self.partial) {
                    Ok(text) => {
                        let c = text.chars().next().unwrap();
                        self.partial.clear();
                        self.print(c);
                    }
                    Err(e) => assert!(e.error_len().is_none(), "echo sent malformed UTF-8"),
                }
                i += 1;
                continue;
            }
            match b {
                0x07 => self.bells += 1,
                0x08 => self.column = self.column.saturating_sub(1),
                b'\r' => self.column = 0,
                b'\n' => { /* row model: a fresh line, content untouched */ }
                0x1b => {
                    // CSI only; that is all the engine emits.
                    assert_eq!(bytes.get(i + 1), Some(&b'['), "non-CSI escape in echo");
                    let mut j = i + 2;
                    let mut n: usize = 0;
                    let mut has_n = false;
                    while j < bytes.len() && bytes[j].is_ascii_digit() {
                        n = n * 10 + (bytes[j] - b'0') as usize;
                        has_n = true;
                        j += 1;
                    }
                    let count = if has_n { n } else { 1 };
                    match bytes[j] {
                        b'D' => self.column = self.column.saturating_sub(count),
                        b'C' => self.column += count,
                        b'K' => self.row.truncate(self.column),
                        b'J' => {
                            self.row.clear();
                            self.cleared = true;
                        }
                        b'H' => self.column = 0,
                        other => panic!("echo used unmodeled CSI final {other:?}"),
                    }
                    i = j;
                }
                _ => self.print(char::from(b)),
            }
            i += 1;
        }
    }

    /// Put `c` in the cursor's column and step past it.
    fn print(&mut self, c: char) {
        // A cursor moved past the row's end leaves blanks behind it.
        if self.column > self.row.len() {
            self.row.resize(self.column, ' ');
        }
        if self.column == self.row.len() {
            self.row.push(c);
        } else {
            self.row[self.column] = c;
        }
        self.column += 1;
    }
}

impl Sink for Screen {
    fn put(&mut self, bytes: &[u8]) {
        self.interpret(bytes);
    }
}

pub(super) fn feed_all(d: &mut LineEditor, s: &mut Screen, bytes: &[u8]) -> Vec<Event> {
    bytes.iter().filter_map(|&b| d.feed(b, s)).collect()
}

/// Every byte the queue holds, oldest first, read `chunk` bytes at a time. Bounded at one read per
/// byte the ring can hold, so a `read` that never empties the queue fails instead of hanging.
pub(super) fn drain(q: &mut RawQueue, chunk: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; MAXIMUM_RAW_QUEUE_BYTES];
    for _ in 0..=MAXIMUM_RAW_QUEUE_BYTES {
        let n = q.read(&mut buffer[..chunk]);
        if n == 0 {
            return bytes;
        }
        assert!(
            n <= chunk,
            "a read moved {n} bytes into a buffer of {chunk}"
        );
        bytes.extend_from_slice(&buffer[..n]);
    }
    panic!("the queue never emptied");
}
