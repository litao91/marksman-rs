//! Marksman configuration from TOML files.
//!
//! Port of `Marksman.Config`. Every knob is optional so that user, project and
//! default configurations can be layered by field.

use std::path::PathBuf;

use log::error;
use toml::Value;

#[derive(Clone, Debug)]
pub enum LookupError {
    NotFound(Vec<String>),
    WrongType(Vec<String>, Value, String),
    WrongValue(Vec<String>, Value, String),
}

impl std::fmt::Display for LookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LookupError::NotFound(path) => write!(f, "not found: {}", path.join(".")),
            LookupError::WrongType(path, value, expected) => {
                write!(f, "{}: expected {expected}, got {value:?}", path.join("."))
            }
            LookupError::WrongValue(path, value, err) => {
                write!(f, "{}: {err}, got {value:?}", path.join("."))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ComplWikiStyle {
    /// Document title's slug, e.g. "A B C" -> "a-b-c"
    TitleSlug,
    /// File name without an extension, e.g. "path/to/doc.md" -> "doc"
    FileStem,
    /// File path without an extension, e.g. "path/to/doc.md" -> "path/to/doc"
    FilePathStem,
}

impl ComplWikiStyle {
    pub fn of_string(input: &str) -> Result<ComplWikiStyle, String> {
        match input.to_lowercase().as_str() {
            "title-slug" => Ok(ComplWikiStyle::TitleSlug),
            "file-stem" => Ok(ComplWikiStyle::FileStem),
            "file-path-stem" => Ok(ComplWikiStyle::FilePathStem),
            other => Err(format!("Unknown ComplWikiStyle: {other}")),
        }
    }

    pub fn of_string_opt(input: &str) -> Option<ComplWikiStyle> {
        Self::of_string(input).ok()
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ComplWikiStyle::TitleSlug => "title-slug",
            ComplWikiStyle::FileStem => "file-stem",
            ComplWikiStyle::FilePathStem => "file-path-stem",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TextSync {
    Full,
    Incremental,
}

impl TextSync {
    pub fn of_string(input: &str) -> Result<TextSync, String> {
        match input.to_lowercase().as_str() {
            "full" => Ok(TextSync::Full),
            "incremental" => Ok(TextSync::Incremental),
            other => Err(format!("Unknown text sync setting: {other}")),
        }
    }

    pub fn of_string_opt(input: &str) -> Option<TextSync> {
        Self::of_string(input).ok()
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            TextSync::Full => "full",
            TextSync::Incremental => "incremental",
        }
    }
}

/// Configuration knobs for the Marksman LSP. All options are laid out flat to
/// make working with the config without lenses manageable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub ca_toc_enable: Option<bool>,
    pub ca_toc_include: Option<Vec<i32>>,
    pub ca_create_missing_file_enable: Option<bool>,
    pub core_markdown_file_extensions: Option<Vec<String>>,
    pub core_markdown_glfm_heading_ids_enable: Option<bool>,
    pub core_text_sync: Option<TextSync>,
    pub core_title_from_heading: Option<bool>,
    pub core_incremental_references: Option<bool>,
    pub core_paranoid: Option<bool>,
    pub compl_wiki_style: Option<ComplWikiStyle>,
    pub compl_candidates: Option<i64>,
}

impl Default for Config {
    fn default() -> Self {
        Config::default_config()
    }
}

impl Config {
    pub fn default_config() -> Config {
        Config {
            ca_toc_enable: Some(true),
            ca_toc_include: Some(vec![1, 2, 3, 4, 5, 6]),
            ca_create_missing_file_enable: Some(true),
            core_markdown_file_extensions: Some(vec!["md".into(), "markdown".into()]),
            core_markdown_glfm_heading_ids_enable: Some(true),
            core_text_sync: Some(TextSync::Full),
            core_title_from_heading: Some(true),
            core_incremental_references: Some(false),
            core_paranoid: Some(false),
            compl_wiki_style: Some(ComplWikiStyle::TitleSlug),
            compl_candidates: Some(50),
        }
    }

    pub fn empty() -> Config {
        Config {
            ca_toc_enable: None,
            ca_toc_include: None,
            ca_create_missing_file_enable: None,
            core_markdown_file_extensions: None,
            core_markdown_glfm_heading_ids_enable: None,
            core_text_sync: None,
            core_title_from_heading: None,
            core_incremental_references: None,
            core_paranoid: None,
            compl_wiki_style: None,
            compl_candidates: None,
        }
    }

    pub fn ca_toc_enable(&self) -> bool {
        self.ca_toc_enable.or(Config::default_config().ca_toc_enable).unwrap()
    }

    pub fn ca_toc_include(&self) -> Vec<i32> {
        self.ca_toc_include
            .clone()
            .or(Config::default_config().ca_toc_include)
            .unwrap()
    }

    pub fn ca_create_missing_file_enable(&self) -> bool {
        self.ca_create_missing_file_enable
            .or(Config::default_config().ca_create_missing_file_enable)
            .unwrap()
    }

    pub fn core_markdown_file_extensions(&self) -> Vec<String> {
        self.core_markdown_file_extensions
            .clone()
            .or(Config::default_config().core_markdown_file_extensions)
            .unwrap()
    }

    pub fn core_markdown_glfm_heading_ids_enable(&self) -> bool {
        self.core_markdown_glfm_heading_ids_enable
            .or(Config::default_config().core_markdown_glfm_heading_ids_enable)
            .unwrap()
    }

    pub fn core_text_sync(&self) -> TextSync {
        self.core_text_sync.or(Config::default_config().core_text_sync).unwrap()
    }

    pub fn core_title_from_heading(&self) -> bool {
        self.core_title_from_heading
            .or(Config::default_config().core_title_from_heading)
            .unwrap()
    }

    pub fn core_incremental_references(&self) -> bool {
        self.core_incremental_references
            .or(Config::default_config().core_incremental_references)
            .unwrap()
    }

    pub fn core_paranoid(&self) -> bool {
        self.core_paranoid.or(Config::default_config().core_paranoid).unwrap()
    }

    pub fn compl_wiki_style(&self) -> ComplWikiStyle {
        match self.compl_wiki_style {
            Some(x) => x,
            None => {
                if self.core_title_from_heading() {
                    Config::default_config().compl_wiki_style.unwrap()
                } else {
                    ComplWikiStyle::FileStem
                }
            }
        }
    }

    pub fn compl_candidates(&self) -> i64 {
        self.compl_candidates.or(Config::default_config().compl_candidates).unwrap()
    }

    pub fn merge(hi: &Config, low: &Config) -> Config {
        Config {
            ca_toc_enable: hi.ca_toc_enable.or(low.ca_toc_enable),
            ca_toc_include: hi.ca_toc_include.clone().or_else(|| low.ca_toc_include.clone()),
            ca_create_missing_file_enable: hi.ca_create_missing_file_enable.or(low.ca_create_missing_file_enable),
            core_markdown_file_extensions: hi
                .core_markdown_file_extensions
                .clone()
                .or_else(|| low.core_markdown_file_extensions.clone()),
            core_markdown_glfm_heading_ids_enable: hi
                .core_markdown_glfm_heading_ids_enable
                .or(low.core_markdown_glfm_heading_ids_enable),
            core_text_sync: hi.core_text_sync.or(low.core_text_sync),
            core_title_from_heading: hi.core_title_from_heading.or(low.core_title_from_heading),
            core_incremental_references: hi
                .core_incremental_references
                .or(low.core_incremental_references),
            core_paranoid: hi.core_paranoid.or(low.core_paranoid),
            compl_wiki_style: hi.compl_wiki_style.or(low.compl_wiki_style),
            compl_candidates: hi.compl_candidates.or(low.compl_candidates),
        }
    }

    pub fn merge_opt(hi: Option<Config>, low: Option<Config>) -> Option<Config> {
        match low {
            None => hi,
            Some(low) => match hi {
                None => Some(low),
                Some(hi) => Some(Config::merge(&hi, &low)),
            },
        }
    }

    pub fn or_default(config: Option<Config>) -> Config {
        config.unwrap_or_default()
    }
}

fn get_from_table<'a>(table: &'a Value, path: &[&str]) -> Result<Option<&'a Value>, LookupError> {
    let mut current = table;
    for (idx, key) in path.iter().enumerate() {
        let seen: Vec<String> = path[..=idx].iter().map(|s| s.to_string()).collect();
        match current.get(key) {
            Some(value) => current = value,
            None => return Ok(None),
        }
        if idx + 1 < path.len() && !current.is_table() {
            return Err(LookupError::WrongType(seen, current.clone(), "table".into()));
        }
    }
    Ok(Some(current))
}

fn as_bool(table: &Value, path: &[&str]) -> Result<Option<bool>, LookupError> {
    match get_from_table(table, path)? {
        None => Ok(None),
        Some(Value::Boolean(b)) => Ok(Some(*b)),
        Some(v) => Err(LookupError::WrongType(
            path.iter().map(|s| s.to_string()).collect(),
            v.clone(),
            "bool".into(),
        )),
    }
}

fn as_string(table: &Value, path: &[&str]) -> Result<Option<String>, LookupError> {
    match get_from_table(table, path)? {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(v) => Err(LookupError::WrongType(
            path.iter().map(|s| s.to_string()).collect(),
            v.clone(),
            "string".into(),
        )),
    }
}

fn as_string_array(table: &Value, path: &[&str]) -> Result<Option<Vec<String>>, LookupError> {
    match get_from_table(table, path)? {
        None => Ok(None),
        Some(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::String(s) => out.push(s.clone()),
                    other => {
                        return Err(LookupError::WrongType(
                            path.iter().map(|s| s.to_string()).collect(),
                            other.clone(),
                            "string".into(),
                        ))
                    }
                }
            }
            Ok(Some(out))
        }
        Some(v) => Err(LookupError::WrongType(
            path.iter().map(|s| s.to_string()).collect(),
            v.clone(),
            "array of strings".into(),
        )),
    }
}

fn as_non_negative_int_array(table: &Value, path: &[&str]) -> Result<Option<Vec<i32>>, LookupError> {
    match get_from_table(table, path)? {
        None => Ok(None),
        Some(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item.as_integer() {
                    Some(v) if v >= 0 => out.push(v as i32),
                    _ => {
                        return Err(LookupError::WrongValue(
                            path.iter().map(|s| s.to_string()).collect(),
                            Value::Array(items.clone()),
                            "expected non-negative integers".into(),
                        ))
                    }
                }
            }
            Ok(Some(out))
        }
        Some(v) => Err(LookupError::WrongType(
            path.iter().map(|s| s.to_string()).collect(),
            v.clone(),
            "array of integers".into(),
        )),
    }
}

fn as_int(table: &Value, path: &[&str]) -> Result<Option<i64>, LookupError> {
    match get_from_table(table, path)? {
        None => Ok(None),
        Some(v) => match v.as_integer() {
            Some(i) => Ok(Some(i)),
            None => Err(LookupError::WrongType(
                path.iter().map(|s| s.to_string()).collect(),
                v.clone(),
                "integer".into(),
            )),
        },
    }
}

fn config_of_table(table: &Value) -> Result<Config, LookupError> {
    let ca_toc_enable = as_bool(table, &["code_action", "toc", "enable"])?;
    let ca_toc_include = as_non_negative_int_array(table, &["code_action", "toc", "include"])?;
    let ca_create_missing_file_enable =
        as_bool(table, &["code_action", "create_missing_file", "enable"])?;
    let core_markdown_file_extensions =
        as_string_array(table, &["core", "markdown", "file_extensions"])?;
    let core_markdown_glfm_heading_ids_enable =
        as_bool(table, &["core", "markdown", "glfm_heading_ids", "enable"])?;
    let core_text_sync = as_string(table, &["core", "text_sync"])?
        .and_then(|s| TextSync::of_string_opt(&s));
    let core_title_from_heading = as_bool(table, &["core", "title_from_heading"])?;
    let core_incremental_references = as_bool(table, &["core", "incremental_references"])?;
    let core_paranoid = as_bool(table, &["core", "paranoid"])?;
    let compl_wiki_style = as_string(table, &["completion", "wiki", "style"])?
        .and_then(|s| ComplWikiStyle::of_string_opt(&s));

    let candidates_path = ["completion", "candidates"];
    let compl_candidates = match as_int(table, &candidates_path)? {
        None => None,
        Some(v) => {
            if v > 0 {
                Some(v)
            } else {
                return Err(LookupError::WrongValue(
                    candidates_path.iter().map(|s| s.to_string()).collect(),
                    Value::Integer(v),
                    "expected a non-negative number".into(),
                ));
            }
        }
    };

    Ok(Config {
        ca_toc_enable,
        ca_toc_include,
        ca_create_missing_file_enable,
        core_markdown_file_extensions,
        core_markdown_glfm_heading_ids_enable,
        core_text_sync,
        core_title_from_heading,
        core_incremental_references,
        core_paranoid,
        compl_wiki_style,
        compl_candidates,
    })
}

pub fn try_parse(content: &str) -> Option<Config> {
    match toml::from_str::<Value>(content) {
        Ok(table) => match config_of_table(&table) {
            Ok(parsed) => Some(parsed),
            Err(err) => {
                error!("Failed to parse configuration: {err}");
                None
            }
        },
        Err(_) => {
            log::trace!("Parsing as TOML failed");
            None
        }
    }
}

pub fn read(filepath: &str) -> Option<Config> {
    let content = std::fs::read_to_string(filepath).ok()?;
    try_parse(&content)
}

pub fn user_config_dir() -> PathBuf {
    let base = if cfg!(target_os = "macos") {
        dirs_base("Library/Application Support")
    } else if cfg!(target_os = "windows") {
        std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| dirs_base("AppData/Roaming"))
    } else {
        std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| dirs_base(".config"))
    };
    base.join("marksman")
}

fn dirs_base(relative: &str) -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(relative)
}

pub fn user_config_file() -> PathBuf {
    user_config_dir().join("config.toml")
}

pub fn default_markdown_extensions() -> Vec<String> {
    Config::default_config().core_markdown_file_extensions()
}

/// The subset of configuration that affects how a document is parsed. Changing
/// any of these requires re-parsing every document in the folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParserSettings {
    pub md_file_ext: Vec<String>,
    pub title_from_heading: bool,
    pub glfm_heading_ids: bool,
}

impl ParserSettings {
    pub fn of_config(config: &Config) -> ParserSettings {
        ParserSettings {
            md_file_ext: config.core_markdown_file_extensions(),
            title_from_heading: config.core_title_from_heading(),
            glfm_heading_ids: config.core_markdown_glfm_heading_ids_enable(),
        }
    }
}

impl Default for ParserSettings {
    fn default() -> Self {
        ParserSettings::of_config(&Config::default_config())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_yields_defaults() {
        let config = Config::empty();
        assert_eq!(config.ca_toc_include(), vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(config.core_markdown_file_extensions(), vec!["md", "markdown"]);
        assert_eq!(config.compl_candidates(), 50);
        assert_eq!(config.compl_wiki_style(), ComplWikiStyle::TitleSlug);
        assert_eq!(config.core_text_sync(), TextSync::Full);
    }

    #[test]
    fn wiki_style_follows_title_from_heading_when_unset() {
        let config = Config { core_title_from_heading: Some(false), ..Config::empty() };
        assert_eq!(config.compl_wiki_style(), ComplWikiStyle::FileStem);
    }

    #[test]
    fn parses_nested_options() {
        let content = r#"
            [code_action.toc]
            enable = false
            include = [1, 2]

            [core]
            text_sync = "incremental"
            title_from_heading = false
            incremental_references = true
            paranoid = true

            [core.markdown]
            file_extensions = ["md", "mdown"]

            [core.markdown.glfm_heading_ids]
            enable = false

            [completion]
            candidates = 10

            [completion.wiki]
            style = "file-path-stem"
        "#;
        let config = try_parse(content).expect("valid config");
        assert_eq!(config.ca_toc_enable, Some(false));
        assert_eq!(config.ca_toc_include, Some(vec![1, 2]));
        assert_eq!(
            config.core_markdown_file_extensions,
            Some(vec!["md".to_string(), "mdown".to_string()])
        );
        assert_eq!(config.core_markdown_glfm_heading_ids_enable, Some(false));
        assert_eq!(config.core_text_sync, Some(TextSync::Incremental));
        assert_eq!(config.core_title_from_heading, Some(false));
        assert_eq!(config.core_incremental_references, Some(true));
        assert_eq!(config.core_paranoid, Some(true));
        assert_eq!(config.compl_candidates, Some(10));
        assert_eq!(config.compl_wiki_style, Some(ComplWikiStyle::FilePathStem));
    }

    #[test]
    fn unknown_enum_values_are_ignored() {
        let config = try_parse("core.text_sync = \"bogus\"").expect("parses");
        assert_eq!(config.core_text_sync, None);
    }

    #[test]
    fn rejects_bad_types_and_values() {
        assert!(matches!(
            try_parse("completion.candidates = 0"),
            None
        ));
        assert!(try_parse("not toml [[[").is_none());
        assert!(try_parse("core.paranoid = \"yes\"").is_none());
    }

    #[test]
    fn merge_prefers_higher_precedence() {
        let hi = Config { compl_candidates: Some(5), ..Config::empty() };
        let low = Config {
            compl_candidates: Some(99),
            core_paranoid: Some(true),
            ..Config::empty()
        };
        let merged = Config::merge(&hi, &low);
        assert_eq!(merged.compl_candidates, Some(5));
        assert_eq!(merged.core_paranoid, Some(true));
    }

    #[test]
    fn parser_settings_follow_config() {
        let config = Config {
            core_markdown_file_extensions: Some(vec!["mdown".into()]),
            core_title_from_heading: Some(false),
            ..Config::empty()
        };
        let settings = ParserSettings::of_config(&config);
        assert_eq!(settings.md_file_ext, vec!["mdown"]);
        assert!(!settings.title_from_heading);
        assert!(settings.glfm_heading_ids);
    }
}
