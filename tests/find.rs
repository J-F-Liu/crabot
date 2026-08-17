use crabot::tools::find::PatternMatcher;

fn matcher(pattern: &str) -> PatternMatcher {
    PatternMatcher::new(pattern).unwrap()
}

#[test]
fn bare_pattern_matches_basename_anywhere() {
    let m = matcher("*.rs");
    assert!(m.matches("mod.rs"));
    assert!(m.matches("src/tools/mod.rs"));
    assert!(!m.matches("src/mod.rst"));
    assert!(!m.matches("src/tools/mod"));
}

#[test]
fn exact_basename_matches_everywhere() {
    let m = matcher("Cargo.toml");
    assert!(m.matches("Cargo.toml"));
    assert!(m.matches("sub/dir/Cargo.toml"));
    assert!(!m.matches("src/Cargo.toml.bak"));
}

#[test]
fn star_does_not_cross_separators() {
    let m = matcher("src/*.rs");
    assert!(m.matches("src/main.rs"));
    assert!(!m.matches("src/tools/mod.rs"));
}

#[test]
fn backslashes_are_treated_as_separators() {
    let m = matcher(r"src\*.rs");
    assert!(m.matches("src/main.rs"));
    assert!(!m.matches("src/tools/mod.rs"));
}

#[test]
fn double_star_crosses_separators() {
    let m = matcher("src/**/*.rs");
    assert!(m.matches("src/a.rs"));
    assert!(m.matches("src/tools/mod.rs"));
    assert!(m.matches("src/a/b/c.rs"));
    assert!(!m.matches("tests/foo.rs"));
}

#[test]
fn invalid_glob_is_an_error() {
    assert!(PatternMatcher::new("[abc").is_err());
}

#[test]
fn matching_is_case_insensitive_by_default() {
    let m = matcher("*.rs");
    assert!(m.matches("FOO.RS"));
    assert!(m.matches("src/tools/Mod.Rs"));
}

#[test]
fn uppercase_in_pattern_switches_to_case_sensitive() {
    let m = matcher("*.RS");
    assert!(m.matches("foo.RS"));
    assert!(!m.matches("foo.rs"));
}

#[test]
fn uppercase_in_path_pattern_switches_to_case_sensitive() {
    let m = matcher("src/Tools/*.rs");
    assert!(m.matches("src/Tools/mod.rs"));
    assert!(!m.matches("src/tools/mod.rs"));
}

#[test]
fn non_ascii_uppercase_keeps_case_insensitive() {
    // globset folds ASCII case only, so an Ä in the pattern must not switch
    // to case-sensitive matching.
    let m = matcher("*Ä*.rs");
    assert!(m.matches("FOOÄBAR.RS"));
}
