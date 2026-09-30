//! Port of `Tests/GitIgnoreTest.fs`.
//!
//! The original guards half of its cases with
//! `RuntimeInformation.IsOSPlatform(OSPlatform.Windows)` and runs the other half
//! only when *not* on Windows. The Unix cases are ported as plain tests (this
//! suite is exercised on Linux/macOS) and the Windows-only ones are gated with
//! `#[cfg(windows)]`, which mirrors the original's runtime guard exactly instead
//! of leaving a permanently-skipped test behind. The gated cases are not
//! compiled here (only `x86_64-unknown-linux-gnu` is installed), so their
//! outcome -- in particular how `globset` treats '\' as a separator vs. an
//! escape character -- is still unverified.

#![allow(non_snake_case)]

use marksman::gitignore::{pattern_to_glob, Matcher};

fn lines(pats: &[&str]) -> Vec<String> {
    pats.iter().map(|p| p.to_string()).collect()
}

#[test]
fn patternToGlob_Empty() {
    // `GlobMatcher` has no structural equality, so the empty result is checked
    // by length, exactly like the original's `Assert.Equal<Glob>([||], ...)`.
    assert_eq!(0, pattern_to_glob("").len());
}

#[test]
fn absGlob_Unix() {
    let root = "/Users/john/notes";
    let glob = Matcher::mk(root, &lines(&["/node_modules"]));
    let ignored = "/Users/john/notes/node_modules";
    assert!(glob.ignores(ignored));

    let notIgnored = "/Users/john/notes/real.md";
    assert!(!glob.ignores(notIgnored));
}

#[test]
fn relGlob_Unix_1() {
    let root = "/Users/john/notes";
    let glob = Matcher::mk(root, &lines(&["node_modules/"]));
    let ignored = "/Users/john/notes/node_modules";
    assert!(glob.ignores(ignored));

    let ignored = "/Users/john/notes/node_modules/";
    assert!(glob.ignores(ignored));

    let ignored = "/Users/john/notes/node_modules/foo.md";
    assert!(glob.ignores(ignored));

    let notIgnored = "/Users/john/notes/real.md";
    assert!(!glob.ignores(notIgnored));
}

#[test]
fn relGlob_Unix_2() {
    let root = "/Users/john/notes";
    let glob = Matcher::mk(root, &lines(&["node_modules/"]));
    let ignored = "/Users/john/notes/sub/node_modules";
    assert!(glob.ignores(ignored));

    let ignored = "/Users/john/notes/sub/node_modules/";
    assert!(glob.ignores(ignored));

    let ignored = "/Users/john/notes/sub/node_modules/foo.md";
    assert!(glob.ignores(ignored));

    let notIgnored = "/Users/john/notes/sub/real.md";
    assert!(!glob.ignores(notIgnored));
}

#[test]
fn relGlob_Unix_3() {
    let root = "/Users/john/notes";
    let glob = Matcher::mk(root, &lines(&["a/b"]));
    let ignored = "/Users/john/notes/a/b";
    assert!(glob.ignores(ignored));

    let notIgnored = "/Users/john/notes/a/real.md";
    assert!(!glob.ignores(notIgnored));
}

#[test]
#[cfg(windows)]
fn absGlob_Win() {
    let root = "C:\\notes";
    let glob = Matcher::mk(root, &lines(&["/node_modules"]));
    let ignored = "C:\\notes\\node_modules";
    assert!(glob.ignores(ignored));

    let notIgnored = "C:\\notes\\real.md";
    assert!(!glob.ignores(notIgnored));
}

#[test]
#[cfg(windows)]
fn relGlob_Win_1() {
    let root = "C:\\notes";
    let glob = Matcher::mk(root, &lines(&["node_modules/"]));

    let ignored = "C:\\notes\\node_modules";
    assert!(glob.ignores(ignored));

    let ignored = "C:\\notes\\node_modules\\";
    assert!(glob.ignores(ignored));

    let ignored = "C:\\notes\\node_modules\\foo.md";
    assert!(glob.ignores(ignored));

    let notIgnored = "C:\\notes\\real.md";
    assert!(!glob.ignores(notIgnored));
}

#[test]
#[cfg(windows)]
fn relGlob_Win_2() {
    let root = "C:\\notes";
    let glob = Matcher::mk(root, &lines(&["node_modules/"]));

    let ignored = "C:\\notes\\sub\\node_modules";
    assert!(glob.ignores(ignored));

    let ignored = "C:\\notes\\sub\\node_modules\\";
    assert!(glob.ignores(ignored));

    let ignored = "C:\\notes\\sub\\node_modules\\foo.md";
    assert!(glob.ignores(ignored));

    let notIgnored = "C:\\notes\\sub\\real.md";
    assert!(!glob.ignores(notIgnored));
}

#[test]
#[cfg(windows)]
fn relGlob_Win_3() {
    let root = "C:\\notes";
    let glob = Matcher::mk(root, &lines(&["a/b"]));
    let ignored = "C:\\notes\\a\\b";
    assert!(glob.ignores(ignored));

    let notIgnored = "C:\\notes\\a\\real.md";
    assert!(!glob.ignores(notIgnored));
}

#[test]
fn issue_218() {
    let root = "/Users/john/notes";
    let glob = Matcher::mk(root, &lines(&["*.foo[o,p]"]));
    // The original asserted `false` for both of these and documented them as
    // false negatives: .NET's `GlobExpressions` rejects the `*.foo[o,p]`
    // pattern outright, so nothing matched. That assertion encoded a limitation
    // of the glob library, not intended behaviour. The Rust port uses
    // `globset`, which accepts the pattern and interprets `[o,p]` as the
    // character class {o, ',', p} -- the same reading git itself gives it -- so
    // these two paths are correctly ignored here.
    assert!(glob.ignores("zip.foop"));
    assert!(glob.ignores("zap.fooo"));
    // TN
    assert!(!glob.ignores("zap.foos"));
}
