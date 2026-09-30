//! Port of `Tests/ConfigTests.fs`.
//!
//! F#'s triple-quoted strings drop the newline that immediately follows `"""`,
//! Rust raw strings keep it. The extra leading blank line is insignificant
//! whitespace to a TOML parser, so every case below parses exactly the same
//! document as the original.

#![allow(non_snake_case)]

use marksman::config::{self, ComplWikiStyle, Config};

#[test]
fn testParse_0() {
    let content = r#"
"#;

    let actual = config::try_parse(content);

    let expected = Config::empty();

    assert_eq!(Some(expected), actual);
}

#[test]
fn testParse_1() {
    let content = r#"
[code_action]
"#;

    let actual = config::try_parse(content);
    let expected = Config::empty();
    assert_eq!(Some(expected), actual);
}

#[test]
fn testParse_2() {
    let content = r#"
[code_action]
toc.enable = false
"#;

    let actual = config::try_parse(content);

    let expected = Config { ca_toc_enable: Some(false), ..Config::empty() };

    assert_eq!(Some(expected), actual);
}

#[test]
fn testParse_tocInclude() {
    let content = r#"
[code_action]
toc.include = [2, 3, 4]
"#;

    let actual = config::try_parse(content);

    let expected = Config { ca_toc_include: Some(vec![2, 3, 4]), ..Config::empty() };

    assert_eq!(Some(expected), actual);
}

#[test]
fn testParse_3() {
    let content = r#"
[completion]
wiki.style = "file-stem"
"#;

    let actual = config::try_parse(content);

    let expected =
        Config { compl_wiki_style: Some(ComplWikiStyle::FileStem), ..Config::empty() };

    assert_eq!(Some(expected), actual);
}

#[test]
fn testParse_4() {
    let content = r#"
[core]
text_sync = "incremental"
"#;

    let actual = config::try_parse(content);

    let expected =
        Config { core_text_sync: Some(config::TextSync::Incremental), ..Config::empty() };

    assert_eq!(Some(expected), actual);
}

#[test]
fn testParse_5() {
    let content = r#"
[core]
incremental_references = true
"#;

    let actual = config::try_parse(content);

    let expected =
        Config { core_incremental_references: Some(true), ..Config::empty() };

    assert_eq!(Some(expected), actual);
}

#[test]
fn testParse_6() {
    let content = r#"
[core]
paranoid = true
"#;

    let actual = config::try_parse(content);

    let expected = Config { core_paranoid: Some(true), ..Config::empty() };

    assert_eq!(Some(expected), actual);
}

#[test]
fn testParse_7() {
    let content = r#"
[completion]
candidates = 100
"#;

    let actual = config::try_parse(content);

    let expected = Config { compl_candidates: Some(100), ..Config::empty() };

    assert_eq!(Some(expected), actual);
}

#[test]
fn testParse_8() {
    let content = r#"
[core]
markdown.glfm_heading_ids.enable = true
"#;

    let actual = config::try_parse(content);

    let expected = Config {
        core_markdown_glfm_heading_ids_enable: Some(true),
        ..Config::empty()
    };

    assert_eq!(Some(expected), actual);
}

#[test]
fn testParse_broken_0() {
    let content = r#"
blah
"#;

    let actual = config::try_parse(content);
    assert_eq!(None, actual);
}

#[test]
fn testParse_broken_1() {
    let content = r#"
[core]
markdown.file_extensions = [1, 2]
"#;

    let actual = config::try_parse(content);
    assert_eq!(None, actual);
}

#[test]
fn testParse_broken_2() {
    let content = r#"
[core]
markdown.file_extensions = [["md"], "markdown"]
"#;

    let actual = config::try_parse(content);
    assert_eq!(None, actual);
}

#[test]
fn testParse_broken_3() {
    let content = r#"
[core]
markdown.file_extensions = [["md"], ["markdown"]]
"#;

    let actual = config::try_parse(content);
    assert_eq!(None, actual);
}

#[test]
fn testParse_broken_4() {
    let content = r#"
[completion]
candidates = "fifty"
"#;

    let actual = config::try_parse(content);
    assert_eq!(None, actual);
}

#[test]
fn testParse_broken_5() {
    let content = r#"
[completion]
candidates = -1
"#;

    let actual = config::try_parse(content);
    assert_eq!(None, actual);
}

#[test]
fn testParse_broken_6() {
    let content = r#"
[core]
markdown.glfm_heading_ids.enable = -1
"#;

    let actual = config::try_parse(content);
    assert_eq!(None, actual);
}

#[test]
fn testParse_broken_tocInclude() {
    let content = r#"
[code_action]
toc.include = [1, -1]
"#;

    let actual = config::try_parse(content);
    assert_eq!(None, actual);
}

#[test]
fn testDefault() {
    // The original reads the `default.marksman.toml` manifest resource embedded
    // in the test assembly; the Rust port ships no such resource, so the very
    // same document is inlined here.
    let content = r#"
# This file lists all configuration options with their default values.
# You do not need to duplicate all the values in your own user or project config.
# Only override what is needed.

[core]
markdown.file_extensions = ["md", "markdown"]
# Enable GitLab Flavored Markdown heading ID disambiguation method described
# in https://docs.gitlab.com/ee/user/markdown.html#heading-ids-and-links.
# This enables multiple headings with equal IDs to be deterministically
# referenced by links and impacts ID generation in "Table of Contents"
# code action.
markdown.glfm_heading_ids.enable = true
# Configures text sync protocol between the editor (LSP client)
# and Marksman (LSP server).
# Can be either 'full' or `incremental`:
# * full: the whole copy of a document is sent by the editor
#   on every update,
# * incremental: only the changed parts are sent by
#   the editor. This will result in less trafic between
#   the editor and Marksman, but the overall performance
#   impact is marginal.
# Defaults to `full` because the editors have bugs in incremental
# sync which result in slightly correpted state and are really hard
# to diagnose.
text_sync = "full"
# When set to true, level 1 headings will be treated as document titles
# (this includes an assumption of having a single title in the document).
# Setting this to false automatically changes the default wiki link
# completion style to a file-based one.
title_from_heading = true
# Use incremental resolution of project-wide references.
# This is much more efficient but is currently experimental
incremental_references = false
# For debugging only! Enables extra validation checks around
# incremental state updates. SIGNIFICANTLY IMPACTS PERFORMANCE
paranoid = false

[code_action]
# Enable/disable "Table of Contents" code action
toc.enable = true
# Heading levels to include when generating a Table of Contents
toc.include = [1, 2, 3, 4, 5, 6]

# Enable/disable "Create missing linked file" code action
create_missing_file.enable = true

[completion]
# The maximum number of candidates returned for a completion
candidates = 50
# The style of wiki links completion.
# Other values include:
# * "file-stem" to complete using file name without an extension,
# * "file-path-stem" same as above but using file path.
wiki.style = "title-slug"
"#;

    let parsed = config::try_parse(content);
    assert_eq!(Some(Config::default_config()), parsed);
}

#[test]
fn testDefault_titleVsCompletionStyle() {
    let content = r#"
[core]
title_from_heading = false
"#;

    let actual = config::try_parse(content).expect("Expected a successful parse");

    assert!(!actual.core_title_from_heading());
    assert_eq!(ComplWikiStyle::FileStem, actual.compl_wiki_style());
}
