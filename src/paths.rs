//! Filesystem path and URI types.
//!
//! Port of `Marksman.Paths`. Paths are kept as strings because Markdown links
//! reference documents by their textual path, and the original relies on .NET's
//! lexical path operations rather than filesystem inspection.

use crate::misc;

pub const UNIX_SEP: char = '/';

pub fn path_components(path: &str) -> Vec<&str> {
    path.split(['\\', '/']).filter(|c| !c.is_empty()).collect()
}

/// `Path.GetFullPath`: lexically resolves `.` and `..` without touching the filesystem.
fn lexically_resolve(path: &str) -> String {
    let sep = if path.contains('\\') && path.matches('\\').count() > path.matches('/').count() {
        '\\'
    } else {
        '/'
    };
    let is_abs = path.starts_with('/') || AbsPath::is_raw_win_abs_path(path);
    let prefix = if path.starts_with('/') {
        "/"
    } else if AbsPath::is_raw_win_abs_path(path) {
        &path[..2]
    } else {
        ""
    };

    let mut comps: Vec<&str> = Vec::new();
    for comp in path.split(['/', '\\']) {
        match comp {
            "" | "." => {}
            ".." => {
                if !comps.is_empty() && *comps.last().unwrap() != ".." {
                    comps.pop();
                } else if !is_abs {
                    comps.push("..");
                }
            }
            c => comps.push(c),
        }
    }

    let joined = comps.join(&sep.to_string());
    if prefix.is_empty() {
        joined
    } else if prefix == "/" {
        format!("/{joined}")
    } else {
        // Keep the drive separator for Windows-style roots.
        if joined.is_empty() {
            format!("{prefix}\\")
        } else {
            format!("{prefix}\\{joined}")
        }
    }
}

/// `Path.GetRelativePath` for two absolute paths.
fn relative_path(from: &str, to: &str) -> String {
    let from_comps: Vec<&str> = from.split(['/', '\\']).filter(|c| !c.is_empty()).collect();
    let to_comps: Vec<&str> = to.split(['/', '\\']).filter(|c| !c.is_empty()).collect();

    let mut common = 0;
    while common < from_comps.len() && common < to_comps.len() {
        if from_comps[common] != to_comps[common] {
            break;
        }
        common += 1;
    }

    let ups = from_comps.len() - common;
    let mut parts: Vec<String> = vec!["..".to_string(); ups];
    parts.extend(to_comps[common..].iter().map(|c| c.to_string()));
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join(std::path::MAIN_SEPARATOR_STR)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AbsPath(pub String);

impl AbsPath {
    pub fn is_raw_win_abs_path(str_: &str) -> bool {
        let b = str_.as_bytes();
        b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
    }

    pub fn is_raw_unix_abs_path(str_: &str) -> bool {
        str_.starts_with('/')
    }

    pub fn is_root_component(str_: &str) -> bool {
        str_ == "/" || (str_.len() == 2 && str_.as_bytes()[0].is_ascii_alphabetic() && str_.as_bytes()[1] == b':')
    }

    pub fn try_of_system(str_: &str) -> Option<AbsPath> {
        if Self::is_raw_win_abs_path(str_) || Self::is_raw_unix_abs_path(str_) {
            Some(AbsPath(str_.to_string()))
        } else {
            None
        }
    }

    pub fn of_system(str_: &str) -> AbsPath {
        Self::try_of_system(str_).unwrap_or_else(|| panic!("Bad absolute path: {str_}"))
    }

    pub fn of_uri(raw_uri: &str) -> AbsPath {
        Self::of_system(&uri_to_system_path(raw_uri))
    }

    pub fn to_system(&self) -> &str {
        &self.0
    }

    pub fn to_uri(&self) -> String {
        system_path_to_uri_string(&self.0)
    }

    pub fn append_file(&self, filename: &str) -> AbsPath {
        AbsPath(combine_sys(&self.0, filename))
    }

    pub fn append(&self, rel: &RelPath) -> AbsPath {
        AbsPath(combine_sys(&self.0, &rel.0))
    }

    pub fn resolve(&self) -> AbsPath {
        AbsPath(lexically_resolve(&self.0))
    }

    pub fn contains(&self, inner: &AbsPath) -> bool {
        inner.0.starts_with(&self.0)
    }

    pub fn filename(&self) -> &str {
        misc::file_name(&self.0)
    }

    pub fn filename_stem(&self) -> &str {
        misc::file_name_stem(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelPath(pub String);

impl RelPath {
    pub fn of_string_unchecked(str_: impl Into<String>) -> RelPath {
        RelPath(str_.into())
    }

    pub fn to_system(&self) -> &str {
        &self.0
    }

    pub fn filename(&self) -> &str {
        misc::file_name(&self.0)
    }

    pub fn filename_stem(&self) -> &str {
        misc::file_name_stem(&self.0)
    }

    pub fn directory(&self) -> RelPath {
        RelPath(directory_of(&self.0))
    }

    pub fn has_extension(&self) -> bool {
        !misc::path_extension(&self.0).is_empty()
    }
}

fn directory_of(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    match normalized.rfind('/') {
        Some(0) => "/".to_string(),
        Some(idx) => normalized[..idx].to_string(),
        None => String::new(),
    }
}

fn combine_sys(p1: &str, p2: &str) -> String {
    if p2.starts_with('/') || AbsPath::is_raw_win_abs_path(p2) {
        return p2.to_string();
    }
    if p1.is_empty() {
        return p2.to_string();
    }
    let ends_with_sep = p1.ends_with('/') || p1.ends_with('\\');
    if ends_with_sep {
        format!("{p1}{p2}")
    } else {
        format!("{p1}{UNIX_SEP}{p2}")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LocalPath {
    Abs(AbsPath),
    Rel(RelPath),
}

impl LocalPath {
    pub fn raw(&self) -> &str {
        match self {
            LocalPath::Abs(AbsPath(s)) | LocalPath::Rel(RelPath(s)) => s,
        }
    }

    pub fn try_of_system(str_: &str) -> Option<LocalPath> {
        if str_.is_empty() {
            None
        } else {
            match AbsPath::try_of_system(str_) {
                Some(path) => Some(LocalPath::Abs(path)),
                None => Some(LocalPath::Rel(RelPath::of_string_unchecked(str_))),
            }
        }
    }

    pub fn of_system(str_: &str) -> LocalPath {
        Self::try_of_system(str_)
            .unwrap_or_else(|| panic!("String {str_} couldn't be converted to a path"))
    }

    pub fn to_system(&self) -> &str {
        self.raw()
    }

    pub fn is_absolute(&self) -> bool {
        matches!(self, LocalPath::Abs(_))
    }

    pub fn as_absolute(&self) -> &AbsPath {
        match self {
            LocalPath::Abs(p) => p,
            LocalPath::Rel(p) => panic!("Path {} is not absolute", p.0),
        }
    }

    pub fn is_relative(&self) -> bool {
        matches!(self, LocalPath::Rel(_))
    }

    pub fn components(&self) -> Vec<&str> {
        path_components(self.raw())
    }

    pub fn has_dot_components(&self) -> bool {
        self.components().iter().any(|c| *c == "." || *c == "..")
    }

    pub fn of_components(comps: &[&str]) -> LocalPath {
        assert!(!comps.is_empty());
        let sys_path = if comps[0] == "/" {
            format!("/{}", comps[1..].join(std::path::MAIN_SEPARATOR_STR))
        } else {
            comps.join(std::path::MAIN_SEPARATOR_STR)
        };
        Self::of_system(&sys_path)
    }

    pub fn dominating_directory_separator(&self) -> char {
        let raw = self.raw();
        let num_forward = raw.matches('/').count();
        let num_backward = raw.matches('\\').count();
        if num_forward >= num_backward {
            '/'
        } else {
            '\\'
        }
    }

    pub fn starts_with_string(&self, str_: &str) -> bool {
        self.raw().starts_with(str_)
    }

    pub fn normalize(&self) -> LocalPath {
        let is_abs = self.is_absolute();
        let comps = self.components();

        let mut acc: Vec<&str> = Vec::new();
        for comp in comps {
            match (acc.len(), comp) {
                (0, "..") if is_abs => {}
                (0, ".") => {}
                (_, ".") => {}
                (1, "..") if AbsPath::is_root_component(acc[0]) => {}
                (_, "..") => {
                    if acc.last() == Some(&"..") {
                        acc.push("..");
                    } else if acc.is_empty() {
                        acc.push("..");
                    } else {
                        acc.pop();
                    }
                }
                (_, _) => acc.push(comp),
            }
        }

        let sep = self.dominating_directory_separator().to_string();
        let mut new_path = acc.join(&sep);
        if self.starts_with_string("/") {
            new_path = format!("/{new_path}");
        }

        Self::try_of_system(&new_path).expect("Path normalization failed")
    }

    pub fn of_uri(raw_uri: &str) -> LocalPath {
        Self::of_system(&uri_to_system_path(raw_uri))
    }

    pub fn to_uri(&self) -> String {
        system_path_to_uri_string(self.raw())
    }

    pub fn append_file(&self, filename: &str) -> LocalPath {
        let extended = combine_sys(self.raw(), filename);
        match self {
            LocalPath::Abs(_) => LocalPath::Abs(AbsPath(extended)),
            LocalPath::Rel(_) => LocalPath::Rel(RelPath(extended)),
        }
    }

    pub fn combine(p1: &LocalPath, p2: &LocalPath) -> LocalPath {
        match p2 {
            LocalPath::Abs(_) => p2.clone(),
            LocalPath::Rel(_) => {
                let combined = combine_sys(p1.raw(), p2.raw());
                match p1 {
                    LocalPath::Rel(_) => LocalPath::Rel(RelPath(combined)),
                    LocalPath::Abs(_) => LocalPath::Abs(AbsPath(combined)),
                }
            }
        }
    }

    pub fn filename(&self) -> &str {
        misc::file_name(self.raw())
    }

    pub fn filename_stem(&self) -> &str {
        misc::file_name_stem(self.raw())
    }

    pub fn directory(&self) -> LocalPath {
        Self::of_system(&directory_of(self.raw()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RootPath(pub AbsPath);

impl RootPath {
    pub fn of_path(path: AbsPath) -> RootPath {
        RootPath(path)
    }

    pub fn path(&self) -> &AbsPath {
        &self.0
    }

    pub fn to_local(&self) -> LocalPath {
        LocalPath::Abs(self.0.clone())
    }

    pub fn to_abs(&self) -> AbsPath {
        self.0.clone()
    }

    pub fn to_system(&self) -> &str {
        self.0.to_system()
    }

    pub fn to_uri(&self) -> String {
        self.0.to_uri()
    }

    pub fn resolve(&self) -> AbsPath {
        self.0.resolve()
    }

    pub fn append(&self, rel_path: &RelPath) -> AbsPath {
        self.0.append(rel_path)
    }

    pub fn append_file(&self, filename: &str) -> AbsPath {
        self.0.append_file(filename)
    }

    pub fn contains(&self, path: &LocalPath) -> bool {
        match path {
            LocalPath::Rel(_) => true,
            LocalPath::Abs(enclosed) => {
                let enclosing = self.0.resolve();
                let enclosed = enclosed.resolve();
                enclosed.0.starts_with(&enclosing.0)
            }
        }
    }

    pub fn filename(&self) -> &str {
        self.0.filename()
    }

    pub fn filename_stem(&self) -> &str {
        self.0.filename_stem()
    }
}

/// A path relative to a workspace root. `None` means the root itself.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RootedRelPath {
    pub root: RootPath,
    pub path: Option<RelPath>,
}

impl RootedRelPath {
    pub fn mk(root: RootPath, path: LocalPath) -> RootedRelPath {
        match path {
            LocalPath::Rel(p) => RootedRelPath { root, path: Some(p) },
            LocalPath::Abs(p) => {
                let sys_root = root.resolve();
                let sys_other = p.resolve();
                if sys_root == sys_other {
                    RootedRelPath { root, path: None }
                } else {
                    let rel = relative_path(sys_root.to_system(), sys_other.to_system());
                    RootedRelPath { root, path: Some(RelPath(rel)) }
                }
            }
        }
    }

    pub fn to_abs(&self) -> AbsPath {
        match &self.path {
            None => self.root.to_abs(),
            Some(path) => self.root.append(path),
        }
    }

    pub fn to_local(&self) -> LocalPath {
        LocalPath::Abs(self.to_abs())
    }

    pub fn to_system(&self) -> String {
        self.to_abs().to_system().to_string()
    }

    pub fn to_uri(&self) -> String {
        self.to_local().to_uri()
    }

    pub fn filename(&self) -> String {
        misc::file_name(&self.to_system()).to_string()
    }

    pub fn filename_stem(&self) -> String {
        misc::file_name_stem(&self.to_system()).to_string()
    }

    pub fn directory(&self) -> Option<RootedRelPath> {
        self.path.as_ref().map(|rel| RootedRelPath {
            root: self.root.clone(),
            path: Some(rel.directory()),
        })
    }

    pub fn combine(&self, path: &LocalPath) -> Option<RootedRelPath> {
        match LocalPath::combine(&self.to_local(), path) {
            LocalPath::Rel(_) => None,
            LocalPath::Abs(out) => {
                if self.root.contains(&LocalPath::Abs(out.clone())) {
                    Some(RootedRelPath::mk(self.root.clone(), LocalPath::Abs(out)))
                } else {
                    None
                }
            }
        }
    }

    pub fn root_path(&self) -> &RootPath {
        &self.root
    }

    pub fn rel_path_forced(&self) -> RelPath {
        self.path.clone().unwrap_or_else(|| RelPath(".".to_string()))
    }
}

/// A value paired with the URI it was derived from.
#[derive(Clone, Debug)]
pub struct UriWith<T> {
    pub uri: String,
    pub data: T,
}

impl<T: PartialEq> PartialEq for UriWith<T> {
    fn eq(&self, other: &Self) -> bool {
        self.uri == other.uri
    }
}
impl<T: Eq> Eq for UriWith<T> {}
impl<T: Eq> PartialOrd for UriWith<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.uri.cmp(&other.uri))
    }
}
impl<T: Eq> Ord for UriWith<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.uri.cmp(&other.uri)
    }
}
impl<T> std::hash::Hash for UriWith<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.uri.hash(state);
    }
}

impl UriWith<AbsPath> {
    pub fn mk_abs(uri: impl Into<String>) -> UriWith<AbsPath> {
        let uri = uri.into();
        let data = AbsPath::of_uri(&uri);
        UriWith { uri, data }
    }
}

impl UriWith<RootPath> {
    pub fn mk_root(uri: impl Into<String>) -> UriWith<RootPath> {
        let uri = uri.into();
        let data = RootPath(AbsPath::of_uri(&uri));
        UriWith { uri, data }
    }
}

impl UriWith<RootedRelPath> {
    pub fn mk_rooted(root: &UriWith<RootPath>, path: LocalPath) -> UriWith<RootedRelPath> {
        let rel_path = RootedRelPath::mk(root.data.clone(), path);
        let uri = rel_path.to_uri();
        UriWith { uri, data: rel_path }
    }

    pub fn rooted_rel_to_abs(&self) -> UriWith<LocalPath> {
        UriWith {
            uri: self.uri.clone(),
            data: self.data.to_local(),
        }
    }
}

/// A document path with any configured Markdown extension removed. Wiki-links
/// and inline links may address a document with or without its extension, so
/// canonical form is what makes them comparable.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonDocPath(String);

impl CanonDocPath {
    pub fn mk(md_exts: &[String], rel_path: &RelPath) -> CanonDocPath {
        CanonDocPath(misc::chop_markdown_ext(md_exts, &rel_path.0))
    }

    pub fn components(&self) -> Vec<String> {
        path_components(&self.0).into_iter().map(|s| s.to_string()).collect()
    }

    pub fn to_rel(&self) -> RelPath {
        RelPath(self.0.clone())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Converts a local filesystem path into a `file:` URI, leaving non-Latin-1
/// characters unescaped exactly as the original does so that round-trips through
/// editors that send raw URIs keep matching.
pub fn system_path_to_uri_string(file_path: &str) -> String {
    let mut uri = String::with_capacity(file_path.len());

    for c in file_path.chars() {
        if c.is_ascii_alphanumeric()
            || matches!(c, '+' | '/' | '.' | '-' | '_' | '~')
            || (c as u32) > 0xFF
        {
            uri.push(c);
        } else if c == '\\' {
            uri.push('/');
        } else {
            uri.push_str(&misc::url_encode(&c.to_string()));
        }
    }

    let chars: Vec<char> = uri.chars().collect();
    if chars.len() >= 2 && chars[0] == '/' && chars[1] == '/' {
        // UNC path
        format!("file:{uri}")
    } else {
        format!("file:///{}", uri.trim_start_matches('/'))
    }
}

/// A single-letter "scheme" is a Windows drive letter: `System.Uri` reads both
/// `c:/notes` and `E:\notes` as file paths.
fn is_drive_prefix(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// A path of the form `/x:/...`, which `Uri.LocalPath` renders with backslashes.
fn is_dos_path(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':'
}

/// Splits a URI into its authority and path the way `System.Uri` does.
fn split_authority(uri: &str) -> (String, String) {
    if let Some(rest) = uri.strip_prefix("file://") {
        return match rest.find('/') {
            Some(idx) => (rest[..idx].to_string(), rest[idx..].to_string()),
            None => (rest.to_string(), "/".to_string()),
        };
    }

    if let Some(idx) = uri.find("://") {
        let rest = &uri[idx + 3..];
        return match rest.find('/') {
            Some(j) => (rest[..j].to_string(), rest[j..].to_string()),
            None => (rest.to_string(), "/".to_string()),
        };
    }

    if let Some(rest) = uri.strip_prefix("file:") {
        return (String::new(), rest.to_string());
    }

    if is_drive_prefix(uri) {
        return (String::new(), format!("/{}{}", &uri[..2], uri[2..].replace('\\', "/")));
    }

    (String::new(), uri.to_string())
}

pub fn uri_to_system_path(uri: &str) -> String {
    let unescaped = misc::url_decode(uri);
    let (authority, path) = split_authority(&unescaped);

    // A fragment or query is not part of the local path. Because the whole URI
    // is unescaped first, a `%23` in a file name is read as a fragment
    // separator here exactly as `System.Uri` reads it in the original.
    let path = match path.find(['#', '?']) {
        Some(idx) => &path[..idx],
        None => &path,
    };

    let local_path = if !authority.is_empty() {
        format!("\\\\{authority}{}", path.replace('/', "\\"))
    } else if is_dos_path(path) {
        path[1..].replace('/', "\\")
    } else {
        path.to_string()
    };

    // The original normalizes the drive letter to lower case and leaves the rest
    // of the path alone.
    if is_drive_prefix(&local_path) {
        format!(
            "{}{}",
            local_path.as_bytes()[0].to_ascii_lowercase() as char,
            &local_path[1..]
        )
    } else {
        local_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_roundtrip_for_unix_path() {
        assert_eq!(system_path_to_uri_string("/home/u/a.md"), "file:///home/u/a.md");
        assert_eq!(uri_to_system_path("file:///home/u/a.md"), "/home/u/a.md");
    }

    #[test]
    fn uri_escapes_spaces() {
        assert_eq!(
            system_path_to_uri_string("/a b/c d.md"),
            "file:///a%20b/c%20d.md"
        );
        assert_eq!(uri_to_system_path("file:///a%20b/c%20d.md"), "/a b/c d.md");
    }

    #[test]
    fn uri_keeps_non_latin1_characters_raw() {
        assert_eq!(system_path_to_uri_string("/notes/日本.md"), "file:///notes/日本.md");
        assert_eq!(uri_to_system_path("file:///notes/日本.md"), "/notes/日本.md");
    }

    #[test]
    fn uri_to_system_path_matches_dotnet() {
        // Expected values are `Uri.LocalPath` as measured from .NET on Linux,
        // with the original's drive-letter lowercasing applied on top.
        assert_eq!(uri_to_system_path("file:///e%3A/notes"), "e:\\notes");
        assert_eq!(uri_to_system_path("file:///E:/notes"), "e:\\notes");
        assert_eq!(uri_to_system_path("file:///e:/a/b/c"), "e:\\a\\b\\c");
        assert_eq!(uri_to_system_path("E:\\notes (precious)"), "e:\\notes (precious)");
        assert_eq!(uri_to_system_path("C:/Program Data"), "c:\\Program Data");
        assert_eq!(uri_to_system_path("c:/notes"), "c:\\notes");
        assert_eq!(uri_to_system_path("file:///c:/x/y:z"), "c:\\x\\y:z");
        assert_eq!(uri_to_system_path("file://server/share/doc.md"), "\\\\server\\share\\doc.md");
        assert_eq!(
            uri_to_system_path("file:///e%3A/notes%20(precious)"),
            "e:\\notes (precious)"
        );
    }

    #[test]
    fn uri_to_system_path_drops_a_fragment() {
        assert_eq!(uri_to_system_path("file:///c:/x/y#z"), "c:\\x\\y");
        assert_eq!(uri_to_system_path("file:///a/b.md#heading"), "/a/b.md");
    }

    #[test]
    fn rooted_rel_path_is_relative_to_root() {
        let root = RootPath(AbsPath::of_system("/w/space"));
        let rooted = RootedRelPath::mk(root.clone(), LocalPath::of_system("/w/space/a/b.md"));
        assert_eq!(rooted.rel_path_forced().to_system(), "a/b.md");
        assert_eq!(rooted.to_uri(), "file:///w/space/a/b.md");
    }

    #[test]
    fn root_itself_has_no_rel_path() {
        let root = RootPath(AbsPath::of_system("/w/space"));
        let rooted = RootedRelPath::mk(root, LocalPath::of_system("/w/space"));
        assert!(rooted.path.is_none());
        assert_eq!(rooted.rel_path_forced().to_system(), ".");
    }

    #[test]
    fn rooted_combine_rejects_escaping_root() {
        let root = RootPath(AbsPath::of_system("/w/space"));
        let rooted = RootedRelPath::mk(root, LocalPath::of_system("/w/space/a.md"));
        assert!(rooted.combine(&LocalPath::of_system("b.md")).is_some());
        assert!(rooted.combine(&LocalPath::of_system("../../etc/passwd")).is_none());
    }

    #[test]
    fn canon_path_strips_markdown_extension() {
        let exts: Vec<String> = vec!["md".into(), "markdown".into()];
        let canon = CanonDocPath::mk(&exts, &RelPath("a/b.md".into()));
        assert_eq!(canon.as_str(), "a/b");
        assert_eq!(canon.components(), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn normalize_resolves_dot_components() {
        let p = LocalPath::of_system("./a/../b/./c.md");
        assert_eq!(p.normalize().raw(), "b/c.md");
        let abs = LocalPath::of_system("/a/../b");
        assert_eq!(abs.normalize().raw(), "/b");
    }

    #[test]
    fn root_path_contains_only_its_subtree() {
        let root = RootPath(AbsPath::of_system("/w/space"));
        assert!(root.contains(&LocalPath::of_system("/w/space/a.md")));
        assert!(!root.contains(&LocalPath::of_system("/w/other/a.md")));
        assert!(root.contains(&LocalPath::of_system("relative.md")));
    }

    #[test]
    fn uri_with_equality_ignores_payload() {
        let a = UriWith::mk_root("file:///w/space");
        let b = UriWith::mk_root("file:///w/space");
        assert_eq!(a, b);
        assert_eq!(a.data.to_system(), "/w/space");
    }
}
