use super::*;
use crate::test_support::drain;

/// [`RawQueue`] delivers bytes in order, fills the caller's buffer, and drains to empty.
#[test]
fn raw_queue_reads_in_order_into_the_callers_buffer() {
    let mut q = RawQueue::new();
    assert!(q.is_empty());
    assert_eq!(q.push(b"hello, kilo!"), 12); // 12 bytes, under the cap
    assert!(!q.is_empty());

    let mut buffer = [0u8; 8];
    assert_eq!(q.read(&mut buffer), 8);
    assert_eq!(&buffer, b"hello, k");

    assert_eq!(q.read(&mut buffer), 4);
    assert_eq!(&buffer[..4], b"ilo!");
    assert!(q.is_empty());
    assert_eq!(q.read(&mut buffer), 0);
}

/// An empty buffer takes nothing and leaves the queue as it was.
#[test]
fn raw_queue_read_into_an_empty_buffer_moves_nothing() {
    let mut q = RawQueue::new();
    q.push(b"ab");
    assert_eq!(q.read(&mut []), 0);
    assert_eq!(drain(&mut q, 1), b"ab");
}

/// A burst past [`MAXIMUM_RAW_QUEUE_BYTES`] is truncated: `push` reports how many actually landed, so
/// the caller (which owns the [`Sink`](crate::Sink)) knows to ring the bell for the rest, exactly the
/// `LineEditor` overflow contract one level over.
#[test]
fn raw_queue_overflow_is_reported_not_grown() {
    let mut q = RawQueue::new();
    assert_eq!(
        q.push(&[b'a'; MAXIMUM_RAW_QUEUE_BYTES]),
        MAXIMUM_RAW_QUEUE_BYTES
    );
    assert_eq!(q.push(b"more"), 0, "a full queue accepts nothing further");
    // Draining still returns exactly what was pushed, byte for byte.
    assert_eq!(drain(&mut q, 8), [b'a'; MAXIMUM_RAW_QUEUE_BYTES]);
    assert!(q.is_empty());
}

/// The queue wraps its ring correctly: push past the wrap point, read some, push more, and the
/// bytes must still come out in the order they went in, whatever the caller's buffer size.
#[test]
fn raw_queue_wraps_the_ring_in_order() {
    for chunk in [1, 3, 8, MAXIMUM_RAW_QUEUE_BYTES] {
        let mut q = RawQueue::new();
        q.push(&[0; MAXIMUM_RAW_QUEUE_BYTES - 2]);
        drain(&mut q, chunk); // head now two short of the ring's end
        q.push(&[1, 2, 3, 4, 5]); // wraps past MAXIMUM_RAW_QUEUE_BYTES
        assert_eq!(drain(&mut q, chunk), [1, 2, 3, 4, 5], "chunk {chunk}");
    }
}

/// **`head` is a ring index after every read**, not a count that merely reads the same through
/// `% MAXIMUM_RAW_QUEUE_BYTES` later. `save` and `restore` walk the ring from `head`, and a `head` that
/// grew by a whole ring each read would still deliver the same bytes while no longer naming a
/// slot, so the bytes alone cannot catch it. Mutation testing found the gap.
#[test]
fn raw_queue_head_stays_inside_the_ring() {
    let mut q = RawQueue::new();
    let mut buffer = [0u8; 8];
    for round in 0..20u8 {
        q.push(&[round; 5]);
        assert_eq!(q.read(&mut buffer), 5);
        assert!(
            q.head < MAXIMUM_RAW_QUEUE_BYTES,
            "round {round}: head {}",
            q.head
        );
    }
}
