# Changelog

Dates are UTC. One line per release, what changed and why, so a reader of the diff has the
sentence beside it.

## Unreleased

- First version.
- `LineEditor::feed` returns `Option<Event>`, and `Event::None` is gone, so a caller matches only
  real events.
- Every function that takes a sink accepts a `dyn Sink` as well as a concrete one.
- `RawQueue::read` moves queued bytes into a buffer of the caller's size, in place of
  `pop_up_to_eight`.
- The handoff encoding is private: `LineEditor::restore` and `RawQueue::restore` build a value
  from a `Reader`, `Writer::finish` returns a `Result`, and `Refusal` says why a blob was refused
  and implements `Error`.
- `LineEditor`, `RawQueue`, `Writer` and `Reader` implement `Debug`, and `Event` and `Refusal` are
  non-exhaustive and implement `Hash`.
- The editor accepts UTF-8 text and edits it one character at a time.
- The documentation describes the crate in its own terms, every fallible function has an Errors
  section, every public type has an example, and the manifest names the documentation.

## BUGS

- The template's lag check guards the CI pin, not this file; a change that matters to a user of
  the crate belongs in a release line here.
