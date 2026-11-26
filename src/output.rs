//! **The output side**: [`Sink`], where echo bytes go, and [`expand_output`], the newline
//! translation applied to what an application prints.

/// Where echo bytes go. A caller implements it over its serial port, its terminal or a buffer;
/// the tests implement it over a terminal model.
///
/// # Examples
///
/// A sink over a fixed buffer that counts what did not fit, since echo has no one to report a
/// failure to between keystrokes.
///
/// ```
/// use line_editor::{LineEditor, Sink};
///
/// struct Fixed {
///     bytes: [u8; 16],
///     length: usize,
///     dropped: usize,
/// }
///
/// impl Sink for Fixed {
///     fn put(&mut self, bytes: &[u8]) {
///         let room = self.bytes.len() - self.length;
///         let n = bytes.len().min(room);
///         self.bytes[self.length..self.length + n].copy_from_slice(&bytes[..n]);
///         self.length += n;
///         self.dropped += bytes.len() - n;
///     }
/// }
///
/// let mut out = Fixed { bytes: [0; 16], length: 0, dropped: 0 };
/// let mut editor = LineEditor::new();
/// editor.start_line(b"> ", &mut out);
/// editor.feed(b'a', &mut out);
/// assert_eq!(&out.bytes[..out.length], b"> a");
/// assert_eq!(out.dropped, 0);
/// ```
pub trait Sink {
    /// Write echo bytes out. Never called with an empty slice, so an implementation may treat a
    /// call as a message to send. Called synchronously from the byte-feeding methods below, so a
    /// `Sink` that blocks blocks the whole discipline.
    fn put(&mut self, bytes: &[u8]);
}

/// Output-side processing: translate `\n` to `\r\n` so applications can speak Unix newlines to
/// a serial terminal that wants a carriage return. Everything else, ANSI included, passes
/// through untouched: the terminal belongs to the application when it is printing.
///
/// # Examples
///
/// ```
/// use line_editor::{Sink, expand_output};
///
/// struct Echo(Vec<u8>);
/// impl Sink for Echo {
///     fn put(&mut self, bytes: &[u8]) {
///         self.0.extend_from_slice(bytes);
///     }
/// }
///
/// let mut out = Echo(Vec::new());
/// expand_output(b"one\ntwo\n\x1b[1mbold\x1b[0m", &mut out);
/// assert_eq!(out.0, b"one\r\ntwo\r\n\x1b[1mbold\x1b[0m");
/// ```
pub fn expand_output(source: &[u8], out: &mut (impl Sink + ?Sized)) {
    let mut from = 0;
    for (i, &b) in source.iter().enumerate() {
        if b == b'\n' {
            if from < i {
                out.put(&source[from..i]);
            }
            out.put(b"\r\n");
            from = i + 1;
        }
    }
    if from < source.len() {
        out.put(&source[from..]);
    }
}

#[cfg(test)]
mod tests;
