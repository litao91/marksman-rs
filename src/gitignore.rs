//! `.gitignore`-style glob matching.
//!
//! Port of `Marksman.GitIgnore`.

use globset::{Glob, GlobMatcher};
use log::warn;

#[derive(Clone, Debug)]
pub enum GlobPattern {
    Include(GlobMatcher),
    Exclude(GlobMatcher),
}

pub fn pattern_to_glob(pat: &str) -> Vec<GlobMatcher> {
    if pat.trim().is_empty() {
        return Vec::new();
    }

    let first_slash_idx = pat.find('/');
    let is_absolute = first_slash_idx != Some(pat.len() - 1);
    let is_dir = pat.ends_with('/');
    let pat = pat.strip_prefix('/').unwrap_or(pat);
    let pat = if is_absolute { pat.to_string() } else { format!("**/{pat}") };

    let mut out = Vec::new();
    if is_dir {
        if let Ok(g) = Glob::new(&format!("{pat}**")) {
            out.push(g.compile_matcher());
        }
        if let Ok(g) = Glob::new(&pat[..pat.len() - 1]) {
            out.push(g.compile_matcher());
        }
    } else if let Ok(g) = Glob::new(&pat) {
        out.push(g.compile_matcher());
    } else {
        warn!("Unsupported glob pattern: pat={pat}");
    }

    out
}

pub fn mk_glob_pattern(pat: &str) -> Vec<GlobPattern> {
    if pat.starts_with('#') {
        Vec::new()
    } else if let Some(negated) = pat.strip_prefix('!') {
        pattern_to_glob(negated).into_iter().map(GlobPattern::Include).collect()
    } else {
        pattern_to_glob(pat).into_iter().map(GlobPattern::Exclude).collect()
    }
}

#[derive(Clone, Debug)]
pub struct Matcher {
    pub root: String,
    pub patterns: Vec<GlobPattern>,
}

impl Matcher {
    pub fn mk(root: &str, lines: &[String]) -> Matcher {
        let patterns = lines.iter().flat_map(|l| mk_glob_pattern(l)).collect();
        Matcher { root: root.to_string(), patterns }
    }

    pub fn mk_default(root: &str) -> Matcher {
        Self::mk(
            root,
            &[".git".to_string(), ".hg".to_string()],
        )
    }

    pub fn ignores(&self, path: &str) -> bool {
        let rel_path = relative_to(&self.root, path);

        for g in &self.patterns {
            match g {
                GlobPattern::Include(glob) => {
                    if glob.is_match(&rel_path) {
                        return false;
                    }
                }
                GlobPattern::Exclude(glob) => {
                    if glob.is_match(&rel_path) {
                        return true;
                    }
                }
            }
        }
        false
    }

    pub fn ignores_any(matchers: &[Matcher], path: &str) -> bool {
        matchers.iter().any(|m| m.ignores(path))
    }
}

fn relative_to(root: &str, path: &str) -> String {
    let root = root.trim_end_matches(['/', '\\']);
    if let Some(rest) = path.strip_prefix(root) {
        rest.trim_start_matches(['/', '\\']).to_string()
    } else {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/Users/john/notes";

    #[test]
    fn empty_pattern_produces_no_globs() {
        assert!(mk_glob_pattern("").is_empty());
        assert!(pattern_to_glob("").is_empty());
        assert!(pattern_to_glob("   ").is_empty());
    }

    #[test]
    fn absolute_glob_is_anchored_to_the_root() {
        let glob = Matcher::mk(ROOT, &["/node_modules".to_string()]);
        assert!(glob.ignores("/Users/john/notes/node_modules"));
        assert!(!glob.ignores("/Users/john/notes/real.md"));
    }

    #[test]
    fn directory_glob_matches_at_any_depth_including_contents() {
        let glob = Matcher::mk(ROOT, &["node_modules/".to_string()]);
        assert!(glob.ignores("/Users/john/notes/node_modules"));
        assert!(glob.ignores("/Users/john/notes/node_modules/"));
        assert!(glob.ignores("/Users/john/notes/node_modules/foo.md"));
        assert!(!glob.ignores("/Users/john/notes/real.md"));
    }

    #[test]
    fn directory_glob_matches_nested_directories() {
        let glob = Matcher::mk(ROOT, &["node_modules/".to_string()]);
        assert!(glob.ignores("/Users/john/notes/sub/node_modules"));
        assert!(glob.ignores("/Users/john/notes/sub/node_modules/"));
        assert!(glob.ignores("/Users/john/notes/sub/node_modules/foo.md"));
        assert!(!glob.ignores("/Users/john/notes/sub/real.md"));
    }

    #[test]
    fn interior_slash_anchors_the_glob() {
        let glob = Matcher::mk(ROOT, &["a/b".to_string()]);
        assert!(glob.ignores("/Users/john/notes/a/b"));
        assert!(!glob.ignores("/Users/john/notes/a/real.md"));
    }

    #[test]
    fn bare_name_is_anchored_to_the_root() {
        // `pat.IndexOf('/')` is -1 for a bare name, which the original treats as
        // absolute, so `.git` only matches at the workspace root.
        let glob = Matcher::mk_default(ROOT);
        assert!(glob.ignores("/Users/john/notes/.git"));
        assert!(glob.ignores("/Users/john/notes/.hg"));
        assert!(!glob.ignores("/Users/john/notes/sub/.git"));
        assert!(!glob.ignores("/Users/john/notes/real.md"));
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let glob = Matcher::mk(ROOT, &["# comment".to_string(), "".to_string()]);
        assert!(!glob.ignores("/Users/john/notes/anything.md"));
    }

    #[test]
    fn first_matching_pattern_wins() {
        // Unlike git, the matcher stops at the first pattern that applies, so a
        // negation only takes effect when it is listed before what it cancels.
        let glob = Matcher::mk(ROOT, &["!keep.md".to_string(), "*.md".to_string()]);
        assert!(glob.ignores("/Users/john/notes/drop.md"));
        assert!(!glob.ignores("/Users/john/notes/keep.md"));
    }

    #[test]
    fn ignores_any_short_circuits_over_matchers() {
        let a = Matcher::mk(ROOT, &["/skip".to_string()]);
        let b = Matcher::mk(ROOT, &["/other".to_string()]);
        assert!(Matcher::ignores_any(&[a.clone(), b.clone()], "/Users/john/notes/skip"));
        assert!(!Matcher::ignores_any(&[a, b], "/Users/john/notes/keep.md"));
        assert!(!Matcher::ignores_any(&[], "/Users/john/notes/skip"));
    }
}
