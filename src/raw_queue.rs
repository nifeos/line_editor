//! **Raw-mode byte delivery**: [`RawQueue`] and its bound.
//!
//! Its fields are `pub(super)` because `handoff` saves and restores them.

/// How many raw bytes [`RawQueue`] holds while its reader is not reading, when bytes arrive faster
/// than the reader drains them. Sized like [`MAXIMUM_LINE_BYTES`](crate::MAXIMUM_LINE_BYTES)'s
/// bell-on-overflow precedent: a burst past this is dropped, audibly, rather than grown without
/// bound. 64 is generous for a reader that takes a keystroke at a time (a full-screen editor's
/// event loop reads one key, redraws, reads the next) and still covers a short paste.
pub const MAXIMUM_RAW_QUEUE_BYTES: usize = 64;

/// **Raw-mode byte delivery**: a plain first-in, first-out (FIFO) queue of input bytes, for a
/// reader that wants every byte as it was typed, with no interpretation of any of them. Kept separate from [`LineEditor`](crate::LineEditor) rather than a mode bit on it,
/// because raw mode's whole point is that nothing here ever looks at what a byte means; a
/// `LineEditor` exists to do exactly that.
///
/// # Examples
///
/// Bytes come out in the order they went in, and a burst past the bound is reported, not grown.
///
/// ```
/// use line_editor::{MAXIMUM_RAW_QUEUE_BYTES, RawQueue};
///
/// let mut queue = RawQueue::new();
/// assert_eq!(queue.push(b"\x1b[A"), 3); // an Up arrow, uninterpreted
/// let mut bytes = [0u8; 8];
/// let n = queue.read(&mut bytes);
/// assert_eq!(&bytes[..n], b"\x1b[A");
/// assert!(queue.is_empty());
///
/// let burst = [b'x'; MAXIMUM_RAW_QUEUE_BYTES + 10];
/// assert_eq!(queue.push(&burst), MAXIMUM_RAW_QUEUE_BYTES); // the caller rings the bell for 10
/// ```
pub struct RawQueue {
    pub(super) buffer: [u8; MAXIMUM_RAW_QUEUE_BYTES],
    pub(super) head: usize,
    pub(super) length_bytes: usize,
}

/// Prints how many bytes are queued, never the bytes themselves.
impl core::fmt::Debug for RawQueue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RawQueue")
            .field("length_bytes", &self.length_bytes)
            .finish_non_exhaustive()
    }
}

impl Default for RawQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl RawQueue {
    /// An empty queue. `const` for the same reason [`LineEditor::new`](crate::LineEditor::new) is: so a holder can be a
    /// `static` instead of a stack local.
    pub const fn new() -> Self {
        RawQueue {
            buffer: [0; MAXIMUM_RAW_QUEUE_BYTES],
            head: 0,
            length_bytes: 0,
        }
    }

    /// True once nothing is queued.
    pub fn is_empty(&self) -> bool {
        self.length_bytes == 0
    }

    /// Push bytes onto the queue. Returns the number actually pushed; once the queue is full the
    /// rest are dropped, and the caller (which holds the [`Sink`](crate::Sink) this crate deliberately does
    /// not) is the one that rings the bell, the same split [`LineEditor::feed`](crate::LineEditor::feed) keeps between
    /// interpretation and echo.
    pub fn push(&mut self, bytes: &[u8]) -> usize {
        let room = MAXIMUM_RAW_QUEUE_BYTES - self.length_bytes;
        let n = bytes.len().min(room);
        for &b in &bytes[..n] {
            self.buffer[(self.head + self.length_bytes) % MAXIMUM_RAW_QUEUE_BYTES] = b;
            self.length_bytes += 1;
        }
        n
    }

    /// Move the oldest queued bytes into `buffer`, as many as it holds, and return how many
    /// moved. Returns 0 when the queue is empty or `buffer` is.
    ///
    /// # Examples
    ///
    /// ```
    /// use line_editor::RawQueue;
    ///
    /// let mut queue = RawQueue::new();
    /// assert_eq!(queue.push(b"abcde"), 5);
    /// let mut buffer = [0u8; 3];
    /// assert_eq!(queue.read(&mut buffer), 3);
    /// assert_eq!(&buffer, b"abc");
    /// assert_eq!(queue.read(&mut buffer), 2);
    /// assert_eq!(&buffer[..2], b"de");
    /// assert_eq!(queue.read(&mut buffer), 0);
    /// ```
    pub fn read(&mut self, buffer: &mut [u8]) -> usize {
        let n = self.length_bytes.min(buffer.len());
        for (i, slot) in buffer[..n].iter_mut().enumerate() {
            *slot = self.buffer[(self.head + i) % MAXIMUM_RAW_QUEUE_BYTES];
        }
        self.head = (self.head + n) % MAXIMUM_RAW_QUEUE_BYTES;
        self.length_bytes -= n;
        n
    }
}

#[cfg(test)]
mod tests;
