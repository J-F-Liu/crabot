//! Integration tests for `crabot::tools::group_repeated_paths`.

use crabot::tools::group_repeated_paths;

/// Normalize `input`, expecting a regrouping to happen.
fn grouped(input: &str) -> String {
    group_repeated_paths(input).expect("expected the paths to be grouped")
}

#[test]
fn groups_repeated_paths() {
    let input = concat!(
        "src/foo.rs:10:    let x = ...\n",
        "src/foo.rs:18:    let y = ...\n",
        "src/foo.rs:27:    let z = ...\n",
        "src/bar.rs:5:    ...\n",
        "src/bar.rs:12:    ...\n",
    );
    let expected = concat!(
        "src/foo.rs:\n",
        "10:    let x = ...\n",
        "18:    let y = ...\n",
        "27:    let z = ...\n",
        "\n",
        "src/bar.rs:\n",
        "5:    ...\n",
        "12:    ...\n",
    );
    assert_eq!(grouped(input), expected);
}

/// Context lines (`grep -A/-B/-C`) carry `-` separators and join the run of
/// their file; surrounding non-grep output is left alone.
#[test]
fn groups_context_lines() {
    let input = concat!(
        "scanhead.rs\n",
        "scanner.rs\n",
        "state.rs\n",
        "traits.rs\n",
        "udm_mock.rs\n",
        "/d/Rust/InnoProjector/hal/src/traits.rs:93:    fn project(&self, udm_data: &[u8]) -> Result<(), DeviceError>;\n",
        "/d/Rust/InnoProjector/hal/src/traits.rs-94-\n",
        "/d/Rust/InnoProjector/hal/src/traits.rs-95-    // 设备控制\n",
    );
    let expected = concat!(
        "scanhead.rs\n",
        "scanner.rs\n",
        "state.rs\n",
        "traits.rs\n",
        "udm_mock.rs\n",
        "\n",
        "/d/Rust/InnoProjector/hal/src/traits.rs:\n",
        "93:    fn project(&self, udm_data: &[u8]) -> Result<(), DeviceError>;\n",
        "94-\n",
        "95-    // 设备控制\n",
    );
    assert_eq!(grouped(input), expected);
}

#[test]
fn keeps_lone_lines() {
    assert!(group_repeated_paths("src/foo.rs:10: x\n").is_none());
    assert!(group_repeated_paths("").is_none());
}

#[test]
fn breaks_runs_on_other_lines() {
    let input = concat!(
        "src/foo.rs:10: a\n",
        "src/foo.rs:18: b\n",
        "--\n",
        "src/foo.rs:30: c\n",
        "src/foo.rs:31: d\n",
    );
    let expected = concat!(
        "src/foo.rs:\n",
        "10: a\n",
        "18: b\n",
        "--\n",
        "\n",
        "src/foo.rs:\n",
        "30: c\n",
        "31: d\n",
    );
    assert_eq!(grouped(input), expected);
}

#[test]
fn ignores_non_path_prefixes() {
    let input = concat!(
        "2026-01-01 10:00:00,000 ERROR device timed out\n",
        "2026-01-01 10:00:00,001 ERROR retrying\n",
        "step-1-done\n",
        "step-1-failed\n",
        "error[E0425]: cannot find value `x` in this scope\n",
        "note: this error originates in a macro\n",
    );
    assert!(group_repeated_paths(input).is_none());
}

#[test]
fn keeps_log_lines_around_a_group() {
    let input = concat!(
        "2026-01-01 10:00:00 start\n",
        "src/foo.rs:10: a\n",
        "src/foo.rs:18: b\n",
        "2026-01-01 10:00:01 done\n",
    );
    let expected = concat!(
        "2026-01-01 10:00:00 start\n",
        "\n",
        "src/foo.rs:\n",
        "10: a\n",
        "18: b\n",
        "2026-01-01 10:00:01 done\n",
    );
    assert_eq!(grouped(input), expected);
}

#[test]
fn handles_windows_paths() {
    let input = concat!("C:\\src\\foo.rs:10: a\n", "C:\\src\\foo.rs:18: b\n",);
    let expected = concat!("C:\\src\\foo.rs:\n", "10: a\n", "18: b\n");
    assert_eq!(grouped(input), expected);
}

#[test]
fn keeps_stderr_and_exit_code() {
    let input = concat!(
        "src/foo.rs:10: a\n",
        "src/foo.rs:18: b\n",
        "STDERR:\n",
        "note: something happened\n",
        "src/foo.rs:20: c\n",
        "Exit code: 1\n",
    );
    let expected = concat!(
        "src/foo.rs:\n",
        "10: a\n",
        "18: b\n",
        "STDERR:\n",
        "note: something happened\n",
        "src/foo.rs:20: c\n",
        "Exit code: 1\n",
    );
    assert_eq!(grouped(input), expected);
}

#[test]
fn groups_matches_with_empty_text() {
    let input = concat!("src/foo.rs:10:\n", "src/foo.rs-11-\n", "src/foo.rs:12:\n",);
    let expected = concat!("src/foo.rs:\n", "10:\n", "11-\n", "12:\n");
    assert_eq!(grouped(input), expected);
}

#[test]
fn preserves_missing_trailing_newline() {
    let input = "src/foo.rs:10: a\nsrc/foo.rs:18: b";
    assert_eq!(grouped(input), "src/foo.rs:\n10: a\n18: b");
}

#[test]
fn is_idempotent() {
    let input = concat!(
        "src/foo.rs:10:    let x = ...\n",
        "src/foo.rs:18:    let y = ...\n",
        "src/bar.rs:5:    ...\n",
        "src/bar.rs:12:    ...\n",
    );
    let once = grouped(input);
    assert!(group_repeated_paths(&once).is_none());
}
