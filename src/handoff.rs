// This module's docs are on its `pub mod` line in `lib.rs`.
//
// The encoding is this crate's alone, so it is described here and not in the public docs. Every
// word is a little-endian `u32`, and a field is a length word followed by that many bytes. A blob
// opens with `MAGIC` and `LAYOUT`. The editor's part is, in order: the line (a field), the cursor
// (a word), the kill buffer (a field), the prompt (a field), the number of history entries held
// and the next history slot (two words), then the eight history slots in slot order, each a field.
// A raw queue's part is one field of its unread bytes, oldest first. Any change to this order, to
// a field's meaning or to a width bumps `LAYOUT`; a reader refuses every layout but its own and
// reports which one it met.

use core::fmt;

use super::{HISTORY_LINES, LineEditor, MAXIMUM_PROMPT_BYTES, MAXIMUM_RAW_QUEUE_BYTES, RawQueue};

/// The bytes `LEDT`, then the layout, in the blob's first eight bytes.
const MAGIC: u32 = 0x5444_454c;
/// The one layout this build writes and reads.
const LAYOUT: u32 = 1;

/// A blob being written into the caller's buffer. [`Writer::new`] opens it,
/// [`LineEditor::save`] and [`RawQueue::save`] add to it, and [`Writer::finish`] says how long
/// it came out.
///
/// A writer refuses to run past the end of its buffer rather than truncate, because a truncated
/// blob that parsed would be state silently lost.
///
/// # Examples
///
/// Save an editor and a raw queue into one buffer, then restore both from it.
///
/// ```
/// use line_editor::handoff::{Reader, Writer};
/// use line_editor::{LineEditor, RawQueue};
///
/// let (editor, mut queue) = (LineEditor::new(), RawQueue::new());
/// queue.push(b"typed ahead");
///
/// let mut buffer = [0u8; 4096];
/// let mut writer = Writer::new(&mut buffer);
/// editor.save(&mut writer);
/// queue.save(&mut writer);
/// let length = writer.finish()?;
///
/// let mut reader = Reader::new(&buffer[..length])?;
/// let editor = LineEditor::restore(&mut reader)?;
/// let mut queue = RawQueue::restore(&mut reader)?;
/// assert_eq!(reader.finish(), length);
///
/// let mut bytes = [0u8; 16];
/// let n = queue.read(&mut bytes);
/// assert_eq!(&bytes[..n], b"typed ahead");
/// assert_eq!(editor.pending(), (&b""[..], 0));
/// # Ok::<(), line_editor::handoff::Refusal>(())
/// ```
pub struct Writer<'a> {
    buffer: &'a mut [u8],
    at: usize,
    full: bool,
}

impl<'a> Writer<'a> {
    /// Open a blob at the start of `buffer`. The blob's header goes in first.
    pub fn new(buffer: &'a mut [u8]) -> Self {
        let mut w = Writer {
            buffer,
            at: 0,
            full: false,
        };
        w.u32(MAGIC);
        w.u32(LAYOUT);
        w
    }

    /// Append bytes.
    fn bytes(&mut self, b: &[u8]) {
        if self.at + b.len() > self.buffer.len() {
            self.full = true;
            return;
        }
        self.buffer[self.at..self.at + b.len()].copy_from_slice(b);
        self.at += b.len();
    }

    /// Append a little-endian word.
    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }

    /// Append a length, then that many bytes.
    fn field(&mut self, b: &[u8]) {
        match u32::try_from(b.len()) {
            Ok(length) => {
                self.u32(length);
                self.bytes(b);
            }
            // A field this long is not one any caller writes; refuse the blob rather than wrap.
            Err(_) => self.full = true,
        }
    }

    /// Close the blob and return how many bytes of the buffer it fills.
    ///
    /// # Errors
    ///
    /// [`Refusal::BufferTooSmall`] when the buffer ran out before everything saved into it was
    /// written. Nothing in the buffer is then a blob to restore.
    pub fn finish(self) -> Result<usize, Refusal> {
        if self.full {
            Err(Refusal::BufferTooSmall)
        } else {
            Ok(self.at)
        }
    }
}

impl fmt::Debug for Writer<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Writer")
            .field("written_bytes", &self.at)
            .field("buffer_bytes", &self.buffer.len())
            .field("full", &self.full)
            .finish()
    }
}

/// A blob being read from the caller's buffer. [`Reader::new`] opens it,
/// [`LineEditor::restore`] and [`RawQueue::restore`] read from it in the order the blob was
/// saved, and [`Reader::finish`] says how far they read.
///
/// The [`Writer`] example saves and restores a blob.
///
/// # Examples
///
/// A buffer that holds no blob, such as one never written, is refused with its reason.
///
/// ```
/// use line_editor::handoff::{Reader, Refusal};
///
/// let never_written = [0u8; 64];
/// assert_eq!(Reader::new(&never_written).err(), Some(Refusal::NoBlob));
/// ```
pub struct Reader<'a> {
    buffer: &'a [u8],
    at: usize,
}

/// Why a blob was not saved or restored.
///
/// More reasons may follow in a minor release, so a `match` on one ends in a wildcard arm.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
#[non_exhaustive]
pub enum Refusal {
    /// The buffer does not start with a blob's header: there is no blob at all.
    NoBlob,
    /// The blob is in a layout this build does not read; carries that layout.
    Layout(u32),
    /// The blob ends before the state being restored does.
    Truncated,
    /// A field of the blob is longer than the buffer it restores into.
    FieldTooLong,
    /// A field holds a value the state cannot take, such as a cursor past the end of its line.
    Malformed,
    /// The buffer being saved into is too short for the blob.
    BufferTooSmall,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::NoBlob => f.write_str("no line editor handoff blob in the buffer"),
            Refusal::Layout(layout) => write!(
                f,
                "the handoff blob is in layout {layout}, and this build reads only layout {LAYOUT}"
            ),
            Refusal::Truncated => f.write_str("the handoff blob ends early"),
            Refusal::FieldTooLong => {
                f.write_str("a handoff blob field is longer than the buffer it restores into")
            }
            Refusal::Malformed => f.write_str("a handoff blob field holds a value out of range"),
            Refusal::BufferTooSmall => f.write_str("the buffer is too short for the handoff blob"),
        }
    }
}

impl core::error::Error for Refusal {}

impl<'a> Reader<'a> {
    /// Open the blob at the start of `buffer`, checking its header.
    ///
    /// # Errors
    ///
    /// - [`Refusal::NoBlob`] when the buffer does not start with a blob's header.
    /// - [`Refusal::Layout`] when the blob is in a layout this build does not read.
    /// - [`Refusal::Truncated`] when the buffer ends inside the header.
    pub fn new(buffer: &'a [u8]) -> Result<Self, Refusal> {
        let mut r = Reader { buffer, at: 0 };
        if r.u32().map_err(|_| Refusal::NoBlob)? != MAGIC {
            return Err(Refusal::NoBlob);
        }
        let layout = r.u32()?;
        if layout != LAYOUT {
            return Err(Refusal::Layout(layout));
        }
        Ok(r)
    }

    /// Close the blob and return how many bytes of the buffer were read, header included. A
    /// caller that keeps its own state after the blob finds it there.
    pub fn finish(self) -> usize {
        self.at
    }

    /// Read `n` bytes.
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Refusal> {
        let end = self.at.checked_add(n).ok_or(Refusal::Truncated)?;
        let out = self.buffer.get(self.at..end).ok_or(Refusal::Truncated)?;
        self.at = end;
        Ok(out)
    }

    /// Read a little-endian word.
    fn u32(&mut self) -> Result<u32, Refusal> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Read a length-prefixed field into `out`, returning its length. A field longer than `out`
    /// is refused rather than truncated.
    fn field(&mut self, out: &mut [u8]) -> Result<usize, Refusal> {
        let n = self.u32()? as usize;
        if n > out.len() {
            return Err(Refusal::FieldTooLong);
        }
        out[..n].copy_from_slice(self.bytes(n)?);
        Ok(n)
    }
}

impl fmt::Debug for Reader<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reader")
            .field("read_bytes", &self.at)
            .field("buffer_bytes", &self.buffer.len())
            .finish()
    }
}

impl LineEditor {
    /// Add this editor's carried state (see the [`handoff`](crate::handoff) module) to the blob
    /// `writer` is writing. [`Writer::finish`] reports a buffer that was too short.
    pub fn save(&self, writer: &mut Writer<'_>) {
        writer.field(&self.buffer[..self.length_bytes]);
        writer.u32(self.cursor as u32);
        writer.field(&self.kill[..self.kill_length_bytes]);
        writer.field(&self.prompt[..self.prompt_length_bytes]);
        writer.u32(self.history_count as u32);
        writer.u32(self.history_next as u32);
        for (line, n) in &self.history {
            writer.field(&line[..*n]);
        }
    }

    /// An editor built from the state [`LineEditor::save`] wrote, read from `reader`. It carries
    /// on where the saved one stopped, and no history entry is being browsed.
    ///
    /// The [`Writer`] example saves and restores an editor.
    ///
    /// # Errors
    ///
    /// - [`Refusal::Truncated`] when the blob ends before the editor's state does.
    /// - [`Refusal::FieldTooLong`] when a line, the kill buffer or the prompt is longer than this
    ///   build holds.
    /// - [`Refusal::Malformed`] when the cursor lies past the end of its line.
    ///
    /// On any of them nothing is returned, so no half-restored editor reaches the caller.
    pub fn restore(reader: &mut Reader<'_>) -> Result<LineEditor, Refusal> {
        let mut editor = LineEditor::new();
        editor.length_bytes = reader.field(&mut editor.buffer)?;
        let cursor = reader.u32()? as usize;
        if cursor > editor.length_bytes {
            return Err(Refusal::Malformed);
        }
        editor.cursor = cursor;
        editor.kill_length_bytes = reader.field(&mut editor.kill)?;
        editor.prompt_length_bytes = reader.field(&mut editor.prompt[..MAXIMUM_PROMPT_BYTES])?;
        editor.history_count = (reader.u32()? as usize).min(HISTORY_LINES);
        editor.history_next = (reader.u32()? as usize) % HISTORY_LINES;
        for entry in &mut editor.history {
            entry.1 = reader.field(&mut entry.0)?;
        }
        Ok(editor)
    }
}

impl RawQueue {
    /// Add the unread bytes, oldest first, to the blob `writer` is writing.
    /// [`Writer::finish`] reports a buffer that was too short.
    pub fn save(&self, writer: &mut Writer<'_>) {
        // A length word, then the ring read out in order: the run up to the end of the buffer,
        // then the wrapped run from its start.
        writer.u32(self.length_bytes as u32);
        let first = self.length_bytes.min(MAXIMUM_RAW_QUEUE_BYTES - self.head);
        writer.bytes(&self.buffer[self.head..self.head + first]);
        writer.bytes(&self.buffer[..self.length_bytes - first]);
    }

    /// A queue built from the bytes [`RawQueue::save`] wrote, read from `reader`.
    ///
    /// The [`Writer`] example saves and restores a raw queue.
    ///
    /// # Errors
    ///
    /// - [`Refusal::Truncated`] when the blob ends before the queue's bytes do.
    /// - [`Refusal::FieldTooLong`] when the blob holds more bytes than
    ///   [`MAXIMUM_RAW_QUEUE_BYTES`].
    pub fn restore(reader: &mut Reader<'_>) -> Result<RawQueue, Refusal> {
        let mut buffer = [0u8; MAXIMUM_RAW_QUEUE_BYTES];
        let n = reader.field(&mut buffer)?;
        let mut queue = RawQueue::new();
        queue.push(&buffer[..n]);
        Ok(queue)
    }
}

#[cfg(test)]
mod tests;
