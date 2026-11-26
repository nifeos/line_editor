//! **The editing engine**: [`LineEditor`], the limits it holds lines to, the [`Event`] each byte
//! amounts to, and the cursor-move sequences its echo writes. The crate docs say what it does and
//! does not do.
//!
//! The fields `handoff` saves and restores are `pub(super)` so that it can reach them.

use crate::output::Sink;

/// The longest line the discipline will assemble. Longer input is refused with a bell.
#[cfg(not(kani))]
pub const MAXIMUM_LINE_BYTES: usize = 256;
/// The proofs run over a line of 16 bytes: the solver cannot finish a symbolic move inside 256.
#[cfg(kani)]
pub const MAXIMUM_LINE_BYTES: usize = 16;
/// How many history entries the ring holds.
pub const HISTORY_LINES: usize = 8;
/// The longest prompt the discipline will repaint. A longer prompt is cut at the last character
/// boundary within it.
pub const MAXIMUM_PROMPT_BYTES: usize = 64;

const BS: u8 = 0x08;
const BEL: u8 = 0x07;
const ESC: u8 = 0x1b;

/// What one input byte amounted to, when it amounted to something the caller acts on.
/// [`LineEditor::feed`] returns `None` for every other byte, whose echo, if any, has already gone
/// to the sink.
///
/// More events may follow in a minor release, so a `match` on one ends in a wildcard arm.
///
/// # Examples
///
/// `^D` ends input on an empty line and deletes on a non-empty one; `^C` discards the line.
///
/// ```
/// # use line_editor::{Event, LineEditor, Sink};
/// # struct Echo(Vec<u8>);
/// # impl Sink for Echo {
/// #     fn put(&mut self, bytes: &[u8]) { self.0.extend_from_slice(bytes); }
/// # }
/// let mut out = Echo(Vec::new());
/// let mut ld = LineEditor::new();
/// ld.start_line(b"$ ", &mut out);
///
/// // ^D on an empty line: end of input.
/// assert_eq!(ld.feed(0x04, &mut out), Some(Event::Eof));
///
/// // ^C discards whatever was in progress.
/// ld.start_line(b"$ ", &mut out);
/// for b in b"rm -rf" {
///     ld.feed(*b, &mut out);
/// }
/// assert_eq!(ld.feed(0x03, &mut out), Some(Event::Interrupt));
///
/// // On a non-empty line, ^D deletes at the cursor instead. ^A goes to the start first.
/// ld.start_line(b"$ ", &mut out);
/// for b in b"xls" {
///     ld.feed(*b, &mut out);
/// }
/// ld.feed(0x01, &mut out); // ^A: home
/// assert_eq!(ld.feed(0x04, &mut out), None); // deletes the 'x', does NOT end input
/// assert_eq!(ld.feed(b'\r', &mut out), Some(Event::Line));
/// assert_eq!(ld.line(), b"ls");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Event {
    /// A line is complete; read it with [`LineEditor::line`] before feeding the next byte.
    Line,
    /// ^D on an empty line: end of input.
    Eof,
    /// ^C: the line in progress was discarded.
    Interrupt,
    /// ^Z: the line in progress was discarded, and whoever runs a foreground job is asked to
    /// suspend it (job control). At a prompt it is a fresh line, as `^C` is.
    Suspend,
    /// Tab: nothing was echoed and the line is unchanged. Whoever runs the engine decides what a
    /// Tab means, because completion needs a namespace the engine does not have.
    Tab,
}

/// The ANSI parser's state. `Esc` has seen 0x1b; `Csi` is inside `ESC [`, a control sequence
/// introducer (CSI), accumulating numeric parameters; `Ss3` is inside `ESC O`, a single shift
/// (SS3), which application-mode cursor keys send instead of a CSI.
enum EscapeState {
    Idle,
    Esc,
    Csi {
        parameters: [u16; 2],
        n: usize,
    },
    Ss3,
    /// Inside an X10 mouse report, `ESC [ M`, which is followed by three payload bytes (button,
    /// column, row) that are not keys. `remaining` counts the payload bytes still to drop.
    Mouse {
        remaining: u8,
    },
}

/// The editing engine. One per terminal; a few KiB of fixed buffers, no allocation.
///
/// # Examples
///
/// Typing a line, editing it, and getting it back. The [`Sink`] is where echo goes; here a `Vec`,
/// and on a device the serial port's transmitter.
///
/// ```
/// use line_editor::{Event, LineEditor, Sink};
///
/// struct Echo(Vec<u8>);
/// impl Sink for Echo {
///     fn put(&mut self, bytes: &[u8]) {
///         self.0.extend_from_slice(bytes);
///     }
/// }
///
/// let mut out = Echo(Vec::new());
/// let mut ld = LineEditor::new();
/// ld.start_line(b"$ ", &mut out);
///
/// // Type "13", then fix it: ^B moves the cursor left one, onto the 3, and typing 2 **inserts**
/// // before it rather than overwriting it.
/// for b in b"13" {
///     assert_eq!(ld.feed(*b, &mut out), None);
/// }
/// ld.feed(0x02, &mut out); // ^B
/// ld.feed(b'2', &mut out);
///
/// // Carriage return ends the line. The caller reads it before feeding the next byte.
/// assert_eq!(ld.feed(b'\r', &mut out), Some(Event::Line));
/// assert_eq!(ld.line(), b"123");
///
/// // The prompt was echoed, and so was everything typed.
/// assert!(out.0.starts_with(b"$ "));
/// ```
pub struct LineEditor {
    pub(super) buffer: [u8; MAXIMUM_LINE_BYTES],
    pub(super) length_bytes: usize,
    pub(super) cursor: usize,
    /// The completed line, snapshotted when [`Event::Line`] is returned.
    done: [u8; MAXIMUM_LINE_BYTES],
    done_length_bytes: usize,
    pub(super) kill: [u8; MAXIMUM_LINE_BYTES],
    pub(super) kill_length_bytes: usize,
    pub(super) history: [([u8; MAXIMUM_LINE_BYTES], usize); HISTORY_LINES],
    pub(super) history_count: usize,
    pub(super) history_next: usize,
    /// `Some(k)`: showing the k-th most recent history entry (k in `1..=history_count`).
    pub(super) browse: Option<usize>,
    stash: ([u8; MAXIMUM_LINE_BYTES], usize, usize),
    pub(super) prompt: [u8; MAXIMUM_PROMPT_BYTES],
    pub(super) prompt_length_bytes: usize,
    escape: EscapeState,
    last_was_cr: bool,
    /// The bytes of a UTF-8 character received so far, held until the character is whole so that
    /// echo never carries half of one.
    partial: [u8; 4],
    partial_length_bytes: usize,
    /// How many bytes the character in `partial` takes, from its leading byte.
    partial_expected_bytes: usize,
}

/// Prints lengths and positions, never the bytes of a line, so a log of an editor does not carry
/// what was typed into it.
impl core::fmt::Debug for LineEditor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LineEditor")
            .field("length_bytes", &self.length_bytes)
            .field("cursor", &self.cursor)
            .field("kill_length_bytes", &self.kill_length_bytes)
            .field("prompt_length_bytes", &self.prompt_length_bytes)
            .field("history_count", &self.history_count)
            .field("browse", &self.browse)
            .finish_non_exhaustive()
    }
}

impl Default for LineEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl LineEditor {
    /// An empty discipline: no line in progress, no history, prompt unset. `const` on purpose: the
    /// struct is a few KiB before the temporary a stack-local move would make, so a program with a
    /// small stack (one 4 KiB page, say) holds it in a `static` rather than in its entry point's
    /// own stack frame.
    pub const fn new() -> Self {
        LineEditor {
            buffer: [0; MAXIMUM_LINE_BYTES],
            length_bytes: 0,
            cursor: 0,
            done: [0; MAXIMUM_LINE_BYTES],
            done_length_bytes: 0,
            kill: [0; MAXIMUM_LINE_BYTES],
            kill_length_bytes: 0,
            history: [([0; MAXIMUM_LINE_BYTES], 0); HISTORY_LINES],
            history_count: 0,
            history_next: 0,
            browse: None,
            stash: ([0; MAXIMUM_LINE_BYTES], 0, 0),
            prompt: [0; MAXIMUM_PROMPT_BYTES],
            prompt_length_bytes: 0,
            escape: EscapeState::Idle,
            last_was_cr: false,
            partial: [0; 4],
            partial_length_bytes: 0,
            partial_expected_bytes: 0,
        }
    }

    /// The completed line, valid after [`Event::Line`] until the next one.
    pub fn line(&self) -> &[u8] {
        &self.done[..self.done_length_bytes]
    }

    /// Discard whatever line is in progress: the edit buffer, the cursor, and any history
    /// browsing. History and the kill buffer are untouched, because neither belongs to the line
    /// that was in progress. No echo; the caller prints whatever tells the human why, or nothing
    /// (`^C`'s own `"^C\r\n"` is the caller side of that choice, and a switch to raw mode
    /// prints nothing at all).
    pub fn abandon(&mut self) {
        self.length_bytes = 0;
        self.cursor = 0;
        self.browse = None;
        self.partial_length_bytes = 0;
    }

    /// **Resume a read that was interrupted and asked again**: remember `prompt` as
    /// [`start_line`](LineEditor::start_line) does, but paint nothing, because the screen already
    /// shows the prompt and the half-typed line. Painting again would print them twice.
    pub fn resume_line(&mut self, prompt: &[u8]) {
        let n = character_floor(prompt, MAXIMUM_PROMPT_BYTES);
        self.prompt[..n].copy_from_slice(&prompt[..n]);
        self.prompt_length_bytes = n;
    }

    /// The line being edited and the cursor's position in it, for a caller answering
    /// [`Event::Tab`]. Unlike [`LineEditor::line`], this is the line in progress, not a finished one.
    pub fn pending(&self) -> (&[u8], usize) {
        (&self.buffer[..self.length_bytes], self.cursor)
    }

    /// Insert `text` at the cursor as if it had been typed, echoing it. Printable characters only,
    /// ASCII or UTF-8; a control character and a malformed UTF-8 sequence are skipped rather
    /// than inserted, so a completion cannot smuggle a control sequence into the line. A
    /// character that does not fit in [`MAXIMUM_LINE_BYTES`] is refused with a bell.
    pub fn insert_text(&mut self, text: &[u8], out: &mut (impl Sink + ?Sized)) {
        for chunk in text.utf8_chunks() {
            for character in chunk.valid().chars() {
                if !character.is_control() {
                    self.insert(character.encode_utf8(&mut [0; 4]).as_bytes(), out);
                }
            }
        }
    }

    /// Paint the prompt and the line again on a fresh row, cursor where it was. For a caller that
    /// printed something between two keystrokes, as a completion listing does.
    pub fn repaint(&mut self, out: &mut (impl Sink + ?Sized)) {
        if self.prompt_length_bytes > 0 {
            out.put(&self.prompt[..self.prompt_length_bytes]);
        }
        if self.length_bytes > 0 {
            out.put(&self.buffer[..self.length_bytes]);
        }
        csi_left(out, self.columns(self.cursor, self.length_bytes));
    }

    /// Begin a read: remember `prompt` (for repaints) and paint it, followed by whatever the
    /// user has already typed ahead. The caller calls it when its reader asks for a line.
    pub fn start_line(&mut self, prompt: &[u8], out: &mut (impl Sink + ?Sized)) {
        let n = character_floor(prompt, MAXIMUM_PROMPT_BYTES);
        self.prompt[..n].copy_from_slice(&prompt[..n]);
        self.prompt_length_bytes = n;
        self.repaint(out);
    }

    /// Feed one input byte; echo goes to `out`. Returns the [`Event`] the byte amounted to, or
    /// `None` when there is nothing for the caller to act on.
    ///
    /// # Examples
    ///
    /// The LF of a CRLF pair is not a second, empty line.
    ///
    /// ```
    /// # use line_editor::{Event, LineEditor, Sink};
    /// # struct Echo(Vec<u8>);
    /// # impl Sink for Echo {
    /// #     fn put(&mut self, bytes: &[u8]) { self.0.extend_from_slice(bytes); }
    /// # }
    /// let mut out = Echo(Vec::new());
    /// let mut ld = LineEditor::new();
    /// ld.start_line(b"", &mut out);
    ///
    /// ld.feed(b'a', &mut out);
    /// assert_eq!(ld.feed(b'\r', &mut out), Some(Event::Line));
    /// assert_eq!(ld.line(), b"a");
    ///
    /// // The LF of the CRLF pair is not a second, empty line.
    /// assert_eq!(ld.feed(b'\n', &mut out), None);
    /// ```
    pub fn feed(&mut self, b: u8, out: &mut (impl Sink + ?Sized)) -> Option<Event> {
        // A bare LF right after a CR is the second half of a CRLF, not a second Enter.
        let was_cr = core::mem::replace(&mut self.last_was_cr, false);
        if b == b'\n' && was_cr {
            return None;
        }

        // Any byte that cannot continue a UTF-8 character ends one left half received.
        if b < 0x80 {
            self.partial_length_bytes = 0;
        }

        let state = core::mem::replace(&mut self.escape, EscapeState::Idle);
        if self.continue_escape(state, b, out) {
            return None;
        }

        match b {
            ESC => {
                self.escape = EscapeState::Esc;
                None
            }
            b'\r' | b'\n' => {
                self.last_was_cr = b == b'\r';
                Some(self.finish_line(out))
            }
            0x03 => {
                // ^C: discard the line in progress. The caller routes the interrupt (or, today,
                // merely fails a pending read).
                out.put(b"^C\r\n");
                self.abandon();
                Some(Event::Interrupt)
            }
            0x1a => {
                // ^Z: the same discard as ^C, and the caller decides what a suspend means.
                out.put(b"^Z\r\n");
                self.abandon();
                Some(Event::Suspend)
            }
            b'\t' => Some(Event::Tab),
            0x04 => self.eof_or_delete(out),
            0x01 => {
                self.home(out);
                None
            }
            0x05 => {
                self.end(out);
                None
            }
            0x02 => {
                self.left(out);
                None
            }
            0x06 => {
                self.right(out);
                None
            }
            0x0b => {
                // ^K: kill to end of line.
                self.kill_to_end(out);
                None
            }
            0x15 => {
                // ^U: kill to start of line.
                self.kill_to_start(out);
                None
            }
            0x17 => {
                // ^W: kill the word before the cursor (blanks, then non-blanks, like bash).
                self.kill_word_before(out);
                None
            }
            0x19 => {
                self.yank(out);
                None
            }
            0x0c => {
                // ^L: clear the screen, repaint prompt and line. The one operation that needs the
                // prompt, which is why `start_line` takes it.
                self.clear_and_repaint(out);
                None
            }
            BS | 0x7f => {
                self.backspace(out);
                None
            }
            0x20..=0x7e => {
                self.insert(&[b], out);
                None
            }
            0x80..=0xff => {
                self.feed_utf8(b, out);
                None
            }
            _ => None, // other control bytes: ignored, deliberately unecho'd
        }
    }

    /// A byte of a UTF-8 character. The character is inserted, and echoed, once it is whole; a
    /// malformed sequence and a control character (U+0080 to U+009F) are dropped without echo.
    fn feed_utf8(&mut self, b: u8, out: &mut (impl Sink + ?Sized)) {
        if !is_continuation(b) {
            // A leading byte starts a character, abandoning any half received.
            self.partial_length_bytes = 0;
            self.partial_expected_bytes = match b {
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => return, // never a leading byte in UTF-8
            };
            self.partial[0] = b;
            self.partial_length_bytes = 1;
            return;
        }
        if self.partial_length_bytes == 0 {
            return; // a continuation byte with no character to continue
        }
        self.partial[self.partial_length_bytes] = b;
        self.partial_length_bytes += 1;
        if self.partial_length_bytes < self.partial_expected_bytes {
            return;
        }
        let (partial, n) = (self.partial, self.partial_length_bytes);
        self.partial_length_bytes = 0;
        // The leading byte gave the length; the check here refuses an overlong form, a surrogate
        // and a value past U+10FFFF.
        if let Ok(text) = core::str::from_utf8(&partial[..n])
            && !text.chars().any(char::is_control)
        {
            self.insert(&partial[..n], out);
        }
    }

    /// The byte after an escape introducer, or inside a CSI sequence, in escape state `state`.
    /// False when no escape was in progress and the byte is an ordinary key. No escape sequence
    /// amounts to an event, so a byte it consumes is only ever echo.
    fn continue_escape(
        &mut self,
        state: EscapeState,
        b: u8,
        out: &mut (impl Sink + ?Sized),
    ) -> bool {
        match state {
            EscapeState::Idle => false,
            EscapeState::Esc => {
                match b {
                    b'[' => {
                        self.escape = EscapeState::Csi {
                            parameters: [0; 2],
                            n: 0,
                        }
                    }
                    b'O' => self.escape = EscapeState::Ss3,
                    // A second escape (a Meta-prefixed key, or a double press) is one introducer.
                    ESC => self.escape = EscapeState::Esc,
                    _ => {} // an escape we do not speak; drop it and the introducer
                }
                true
            }
            EscapeState::Csi {
                mut parameters,
                mut n,
            } => {
                match b {
                    b'0'..=b'9' => {
                        let i = n.min(1);
                        parameters[i] = parameters[i]
                            .saturating_mul(10)
                            .saturating_add((b - b'0') as u16);
                        self.escape = EscapeState::Csi { parameters, n };
                    }
                    b';' => {
                        n = n.saturating_add(1);
                        self.escape = EscapeState::Csi { parameters, n };
                    }
                    // `ESC [ M` with no parameters opens an X10 mouse report: three payload bytes
                    // follow, and none of them is a key.
                    b'M' if n == 0 && parameters[0] == 0 => {
                        self.escape = EscapeState::Mouse { remaining: 3 };
                    }
                    // A final byte: dispatch. Unknown finals are dropped whole, which is what
                    // makes unrecognized keys (F-keys, mouse noise) harmless.
                    0x40..=0x7e => self.csi_final(b, parameters[0], out),
                    _ => {} // malformed; abandon
                }
                true
            }
            EscapeState::Ss3 => {
                self.csi_final(b, 0, out);
                true
            }
            EscapeState::Mouse { remaining } => {
                if remaining > 1 {
                    self.escape = EscapeState::Mouse {
                        remaining: remaining - 1,
                    };
                }
                true
            }
        }
    }

    /// ^D: end of input on an empty line, otherwise delete the byte under the cursor.
    fn eof_or_delete(&mut self, out: &mut (impl Sink + ?Sized)) -> Option<Event> {
        if self.length_bytes == 0 {
            Some(Event::Eof)
        } else {
            self.delete_at(out);
            None
        }
    }

    /// ^K: move everything from the cursor to the end of the line into the kill buffer.
    fn kill_to_end(&mut self, out: &mut (impl Sink + ?Sized)) {
        self.kill[..self.length_bytes - self.cursor]
            .copy_from_slice(&self.buffer[self.cursor..self.length_bytes]);
        self.kill_length_bytes = self.length_bytes - self.cursor;
        self.length_bytes = self.cursor;
        out.put(b"\x1b[K");
    }

    /// ^U: move everything before the cursor into the kill buffer.
    fn kill_to_start(&mut self, out: &mut (impl Sink + ?Sized)) {
        self.kill[..self.cursor].copy_from_slice(&self.buffer[..self.cursor]);
        self.kill_length_bytes = self.cursor;
        self.delete_range(0, self.cursor, out);
    }

    /// ^W: move the word before the cursor (blanks, then non-blanks) into the kill buffer.
    fn kill_word_before(&mut self, out: &mut (impl Sink + ?Sized)) {
        let mut a = self.cursor;
        while a > 0 && self.buffer[a - 1] == b' ' {
            a -= 1;
        }
        while a > 0 && self.buffer[a - 1] != b' ' {
            a -= 1;
        }
        self.kill[..self.cursor - a].copy_from_slice(&self.buffer[a..self.cursor]);
        self.kill_length_bytes = self.cursor - a;
        self.delete_range(a, self.cursor, out);
    }

    /// ^L: clear the screen and paint the prompt and the line, cursor where it was.
    fn clear_and_repaint(&mut self, out: &mut (impl Sink + ?Sized)) {
        out.put(b"\x1b[2J\x1b[H");
        self.repaint(out);
    }

    /// A CSI (or SS3) final byte, with its first numeric parameter.
    fn csi_final(&mut self, b: u8, p: u16, out: &mut (impl Sink + ?Sized)) {
        match (b, p) {
            (b'D', _) => self.left(out),
            (b'C', _) => self.right(out),
            (b'A', _) => self.history_previous(out),
            (b'B', _) => self.history_next_entry(out),
            (b'H', _) | (b'~', 1) | (b'~', 7) => self.home(out),
            (b'F', _) | (b'~', 4) | (b'~', 8) => self.end(out),
            (b'~', 3) => self.delete_at(out),
            _ => {} // unknown key; ignore
        }
    }

    fn finish_line(&mut self, out: &mut (impl Sink + ?Sized)) -> Event {
        out.put(b"\r\n");
        self.done[..self.length_bytes].copy_from_slice(&self.buffer[..self.length_bytes]);
        self.done_length_bytes = self.length_bytes;
        // Record non-empty lines, skipping an exact repeat of the newest entry.
        if self.length_bytes > 0 && !self.repeats_newest() {
            self.history[self.history_next].0[..self.length_bytes]
                .copy_from_slice(&self.buffer[..self.length_bytes]);
            self.history[self.history_next].1 = self.length_bytes;
            self.history_next = (self.history_next + 1) % HISTORY_LINES;
            self.history_count = (self.history_count + 1).min(HISTORY_LINES);
        }
        self.length_bytes = 0;
        self.cursor = 0;
        self.browse = None;
        // The prompt belonged to the read that just completed; the next read brings its own.
        self.prompt_length_bytes = 0;
        Event::Line
    }

    fn repeats_newest(&self) -> bool {
        if self.history_count == 0 {
            return false;
        }
        let (entry, entry_length) =
            &self.history[(self.history_next + HISTORY_LINES - 1) % HISTORY_LINES];
        *entry_length == self.length_bytes
            && entry[..*entry_length] == self.buffer[..self.length_bytes]
    }

    /// Insert one whole character, `character` being its one to four bytes, at the cursor.
    fn insert(&mut self, character: &[u8], out: &mut (impl Sink + ?Sized)) {
        let n = character.len();
        if self.length_bytes + n > MAXIMUM_LINE_BYTES {
            out.put(&[BEL]);
            return;
        }
        self.buffer
            .copy_within(self.cursor..self.length_bytes, self.cursor + n);
        self.buffer[self.cursor..self.cursor + n].copy_from_slice(character);
        self.length_bytes += n;
        self.cursor += n;
        if self.cursor == self.length_bytes {
            // The common case, typing at the end: echo the character and nothing else.
            out.put(character);
        } else {
            // Mid-line: echo the character and the shifted tail, then walk back over the tail.
            out.put(&self.buffer[self.cursor - n..self.length_bytes]);
            csi_left(out, self.columns(self.cursor, self.length_bytes));
        }
    }

    fn backspace(&mut self, out: &mut (impl Sink + ?Sized)) {
        if self.cursor == 0 {
            return;
        }
        self.delete_range(self.previous_boundary(self.cursor), self.cursor, out);
    }

    fn delete_at(&mut self, out: &mut (impl Sink + ?Sized)) {
        if self.cursor == self.length_bytes {
            return;
        }
        self.delete_range(self.cursor, self.next_boundary(self.cursor), out);
    }

    /// Remove `buffer[a..b]` and repaint what changed: step back to column `a`, print the shifted
    /// tail, erase the leftover columns, step back to the cursor. CSI K does the erasing, so no
    /// space-padding arithmetic. Works for both directions: a backward kill has the cursor at
    /// `b` (it moves left with the deletion), a forward delete has it at `a` (it stays put).
    fn delete_range(&mut self, a: usize, b: usize, out: &mut (impl Sink + ?Sized)) {
        debug_assert!(a <= b && b <= self.length_bytes && a <= self.cursor);
        let removed = b - a;
        if removed == 0 {
            return;
        }
        let old_cursor = self.cursor;
        // The columns from `a` to the old cursor, counted before the move overwrites them.
        let removed_columns = self.columns(a, old_cursor);
        self.buffer.copy_within(b..self.length_bytes, a);
        self.length_bytes -= removed;
        self.cursor = if old_cursor >= b {
            old_cursor - removed
        } else {
            old_cursor.min(a)
        };
        csi_left(out, removed_columns);
        if a < self.length_bytes {
            out.put(&self.buffer[a..self.length_bytes]);
        }
        out.put(b"\x1b[K");
        csi_left(out, self.columns(self.cursor, self.length_bytes));
    }

    fn yank(&mut self, out: &mut (impl Sink + ?Sized)) {
        // What fits, ending on a character boundary so that no half character enters the line.
        let n = character_floor(
            &self.kill[..self.kill_length_bytes],
            MAXIMUM_LINE_BYTES - self.length_bytes,
        );
        if n == 0 {
            if self.kill_length_bytes > 0 {
                out.put(&[BEL]); // there was something to yank and no room for it
            }
            return;
        }
        self.buffer
            .copy_within(self.cursor..self.length_bytes, self.cursor + n);
        // A yank of the kill buffer into the line it was cut from cannot overlap: kill is a
        // separate field, copied out at kill time, so the two borrows are disjoint.
        self.buffer[self.cursor..self.cursor + n].copy_from_slice(&self.kill[..n]);
        self.length_bytes += n;
        self.cursor += n;
        out.put(&self.buffer[self.cursor - n..self.length_bytes]);
        csi_left(out, self.columns(self.cursor, self.length_bytes));
    }

    fn left(&mut self, out: &mut (impl Sink + ?Sized)) {
        if self.cursor > 0 {
            self.cursor = self.previous_boundary(self.cursor);
            out.put(&[BS]);
        }
    }

    fn right(&mut self, out: &mut (impl Sink + ?Sized)) {
        if self.cursor < self.length_bytes {
            self.cursor = self.next_boundary(self.cursor);
            out.put(b"\x1b[C");
        }
    }

    fn home(&mut self, out: &mut (impl Sink + ?Sized)) {
        csi_left(out, self.columns(0, self.cursor));
        self.cursor = 0;
    }

    fn end(&mut self, out: &mut (impl Sink + ?Sized)) {
        csi_right(out, self.columns(self.cursor, self.length_bytes));
        self.cursor = self.length_bytes;
    }

    /// The columns `buffer[a..b]` takes on the screen: one for each character, so one for each
    /// byte that does not continue a UTF-8 character.
    fn columns(&self, a: usize, b: usize) -> usize {
        self.buffer[a..b]
            .iter()
            .filter(|&&byte| !is_continuation(byte))
            .count()
    }

    /// True when `at` falls between two bytes of one character in the line.
    pub(crate) fn splits_a_character(&self, at: usize) -> bool {
        at < self.length_bytes && is_continuation(self.buffer[at])
    }

    /// The start of the character before `at`, which is above zero.
    fn previous_boundary(&self, at: usize) -> usize {
        let mut i = at - 1;
        while i > 0 && is_continuation(self.buffer[i]) {
            i -= 1;
        }
        i
    }

    /// The end of the character that starts at `at`, which is below the line's length.
    fn next_boundary(&self, at: usize) -> usize {
        let mut i = at + 1;
        while i < self.length_bytes && is_continuation(self.buffer[i]) {
            i += 1;
        }
        i
    }

    /// Up arrow: show the previous (older) history entry. The first press stashes the line in
    /// progress so browsing is non-destructive.
    fn history_previous(&mut self, out: &mut (impl Sink + ?Sized)) {
        let k = match self.browse {
            None => {
                if self.history_count == 0 {
                    out.put(&[BEL]);
                    return;
                }
                self.stash.0[..self.length_bytes]
                    .copy_from_slice(&self.buffer[..self.length_bytes]);
                self.stash.1 = self.length_bytes;
                self.stash.2 = self.cursor;
                1
            }
            Some(k) if k < self.history_count => k + 1,
            Some(_) => {
                out.put(&[BEL]); // already at the oldest
                return;
            }
        };
        self.browse = Some(k);
        let (entry, entry_length) =
            self.history[(self.history_next + HISTORY_LINES - k) % HISTORY_LINES];
        self.replace_line(&entry[..entry_length], out);
    }

    /// Down arrow: toward newer entries; past the newest, restore the stashed line.
    fn history_next_entry(&mut self, out: &mut (impl Sink + ?Sized)) {
        match self.browse {
            None => out.put(&[BEL]),
            Some(1) => {
                self.browse = None;
                let (stash, stash_length, stash_cursor) = self.stash;
                self.replace_line(&stash[..stash_length], out);
                // replace_line leaves the cursor at the end; restore the stashed column.
                csi_left(out, self.columns(stash_cursor, stash_length));
                self.cursor = stash_cursor;
            }
            Some(k) => {
                self.browse = Some(k - 1);
                let (entry, entry_length) =
                    self.history[(self.history_next + HISTORY_LINES - (k - 1)) % HISTORY_LINES];
                self.replace_line(&entry[..entry_length], out);
            }
        }
    }

    /// Replace the whole edit line with `text` and repaint the input region, cursor at the end.
    fn replace_line(&mut self, text: &[u8], out: &mut (impl Sink + ?Sized)) {
        csi_left(out, self.columns(0, self.cursor));
        let n = text.len().min(MAXIMUM_LINE_BYTES);
        self.buffer[..n].copy_from_slice(&text[..n]);
        self.length_bytes = n;
        self.cursor = n;
        if n > 0 {
            out.put(&self.buffer[..n]);
        }
        out.put(b"\x1b[K");
    }
}

/// True for a byte that continues a UTF-8 character rather than starting one.
fn is_continuation(byte: u8) -> bool {
    byte & 0xc0 == 0x80
}

/// The longest prefix of `text` no longer than `most` bytes that ends on a character boundary.
fn character_floor(text: &[u8], most: usize) -> usize {
    let mut n = text.len().min(most);
    while n > 0 && n < text.len() && is_continuation(text[n]) {
        n -= 1;
    }
    n
}

/// Emit `CSI <n> D` (cursor left `n`), eliding the count when it is 1 and the whole sequence
/// when it is 0.
fn csi_left(out: &mut (impl Sink + ?Sized), n: usize) {
    csi_move(out, n, b'D');
}

/// Emit `CSI <n> C` (cursor right `n`), same elisions.
fn csi_right(out: &mut (impl Sink + ?Sized), n: usize) {
    csi_move(out, n, b'C');
}

fn csi_move(out: &mut (impl Sink + ?Sized), n: usize, direction: u8) {
    // The distance is a position inside a line, so it fits sixteen bits, and five decimal digits
    // hold any sixteen-bit number. The sequence buffer is sized from those two facts.
    const { assert!(MAXIMUM_LINE_BYTES <= u16::MAX as usize) };
    const DIGITS: usize = 5;
    debug_assert!(n <= MAXIMUM_LINE_BYTES);
    match n {
        0 => {}
        1 => out.put(&[ESC, b'[', direction]),
        _ => {
            // One contiguous put per sequence: a sink must never see half an escape.
            let mut sequence = [0u8; 3 + DIGITS];
            sequence[0] = ESC;
            sequence[1] = b'[';
            let mut start = 2 + DIGITS;
            let mut rest = n as u16;
            while rest > 0 {
                start -= 1;
                sequence[start] = b'0' + (rest % 10) as u8;
                rest /= 10;
            }
            sequence.copy_within(start..2 + DIGITS, 2);
            let digits = 2 + DIGITS - start;
            sequence[2 + digits] = direction;
            out.put(&sequence[..3 + digits]);
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(kani)]
mod verification;
