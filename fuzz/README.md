# Fuzz targets of `line_editor`

The targets use cargo-fuzz and need a nightly toolchain. From the repository root:

```sh
cargo install --locked cargo-fuzz
cargo +nightly fuzz list
cargo +nightly fuzz build
```

The weekly workflow runs every target for five minutes. To run one by hand, give libFuzzer a scratch directory first, so that what it finds does not land among the committed inputs:

```sh
mkdir -p "$TMPDIR/work"
cargo +nightly fuzz run <target> "$TMPDIR/work" corpus/<target>
```

Each `corpus/<target>` directory holds the inputs a run starts from. A crash input is not committed: it becomes a regression test in the crate, beside the code it broke.

## `line_editor_feed` and `line_editor_handoff`

The line editor reads a serial console byte by byte, and what arrives there is escape sequences cut
anywhere, mouse reports and bytes above ASCII. `line_editor_feed` reads its input as a stream of
keystrokes with five calls mixed in (`start_line`, `resume_line`, `repaint`, `abandon` and
`insert_text`, each introduced by a byte from `0xf0` to `0xf4`). After every byte the cursor must lie
inside the line and the line inside `MAXIMUM_LINE_BYTES`, and no echo write is empty or longer than a
prompt and a line. Two Kani harnesses cover the escape parser and the range delete for every input;
this target covers the whole state machine, history and yank included.

`line_editor_handoff` hands arbitrary bytes to the replacement editor's reader. The reader refuses a
blob or restores a state whose cursor is inside its line, and that state saves and restores again
to the same line. A restored raw queue never drains more than it holds. The target has no seeds yet: the
two layouts are short, and the header check passes within a few thousand inputs.
