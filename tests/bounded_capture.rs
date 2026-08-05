use crabot::BoundedCapture;

fn expected(i: usize) -> u8 {
    (i % 251) as u8
}

/// Feed a synthetic stream of `total` bytes in chunks of at most `chunk`.
fn capture(total: usize, keep: usize, chunk: usize) -> BoundedCapture {
    let mut c = BoundedCapture::new(keep);
    let mut i = 0;
    while i < total {
        let n = chunk.min(total - i);
        let bytes: Vec<u8> = (i..i + n).map(expected).collect();
        c.push(&bytes);
        i += n;
    }
    c
}

fn assert_roundtrip(total: usize, keep: usize, chunk: usize) {
    let c = capture(total, keep, chunk);
    assert_eq!(c.total(), total);
    let bytes = c.into_bytes();
    assert_eq!(bytes.len(), total, "keep={keep}, chunk={chunk}");
    for (i, b) in bytes.iter().enumerate() {
        assert_eq!(*b, expected(i), "byte {i}, keep={keep}, chunk={chunk}");
    }
}

#[test]
fn empty_stream() {
    let c = capture(0, 4096, 4096);
    assert_eq!(c.total(), 0);
    assert!(c.into_bytes().is_empty());
}

#[test]
fn lossless_within_keep() {
    for &chunk in &[1usize, 7, 8192] {
        assert_roundtrip(100, 4096, chunk);
    }
}

#[test]
fn lossless_between_keep_and_double_keep() {
    for &chunk in &[1usize, 3, 100, 8192] {
        for &(total, keep) in &[
            (4097usize, 4096),
            (6000, 4096),
            (8191, 4096),
            (8192, 4096), // exactly 2 * keep
        ] {
            assert_roundtrip(total, keep, chunk);
        }
    }
}

#[test]
fn one_byte_past_double_keep_truncates_one_byte() {
    for &chunk in &[1usize, 7, 8192] {
        let bytes = capture(8193, 4096, chunk).into_bytes();
        assert_eq!(bytes[0], expected(0));
        assert_eq!(*bytes.last().unwrap(), expected(8192));
        assert!(
            String::from_utf8_lossy(&bytes).contains("1 bytes truncated (8193 total"),
            "chunk={chunk}"
        );
    }
}

#[test]
fn oversized_is_bounded_with_head_tail_and_count() {
    for &chunk in &[1usize, 7, 8192] {
        let c = capture(1_000_000, 4096, chunk);
        assert_eq!(c.total(), 1_000_000);
        let bytes = c.into_bytes();
        // head + marker + tail — far smaller than the stream.
        assert!(bytes.len() < 4096 * 2 + 256, "chunk={chunk}");
        // head intact
        assert_eq!(
            &bytes[..16],
            &(0..16).map(expected).collect::<Vec<u8>>()[..]
        );
        // marker reports the true total (lossy text keeps the ASCII marker)
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("1000000 total"), "chunk={chunk}");
        // tail intact: `into_bytes` appends the last `keep` bytes last
        assert_eq!(
            &bytes[bytes.len() - 4096..],
            &(995_904..1_000_000).map(expected).collect::<Vec<u8>>()[..],
            "chunk={chunk}"
        );
    }
}

#[test]
fn keep_one_edge_cases() {
    assert_eq!(capture(0, 1, 1).into_bytes(), Vec::<u8>::new());
    assert_eq!(capture(1, 1, 1).into_bytes(), vec![expected(0)]);
    // total == 2 * keep: lossless
    assert_eq!(
        capture(2, 1, 1).into_bytes(),
        vec![expected(0), expected(1)]
    );
    // total > 2 * keep: head + marker + tail
    let bytes = capture(3, 1, 1).into_bytes();
    assert_eq!(bytes[0], expected(0));
    assert_eq!(*bytes.last().unwrap(), expected(2));
    assert!(String::from_utf8_lossy(&bytes).contains("1 bytes truncated (3 total"));
}

#[test]
fn materialize_is_bounded_and_ordered() {
    let c = capture(1_000_000, 4096, 8192);
    let m = c.materialize();
    assert!(m.len() <= 8192);
    assert_eq!(m[0], expected(0));
    assert_eq!(*m.last().unwrap(), expected(999_999));
}

#[test]
fn zero_keep_truncates_everything() {
    let c = capture(3, 0, 3);
    assert_eq!(c.total(), 3);
    assert!(c.materialize().is_empty());
    let bytes = c.into_bytes();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("3 bytes truncated (3 total, cap 0)"));
}
