//! Port of `Tests/PathsTests.fs`.
//!
//! Every case here is pure string manipulation in both ports, so none of them is
//! platform-gated: the original deliberately keeps the URI rules cross-platform
//! ("we want this logic to work cross-platform (for tests)"). The four

#![allow(non_snake_case)]

use marksman::paths::{system_path_to_uri_string, LocalPath, UriWith};

mod LocalPathTests {
    use super::*;

    #[test]
    fn testWinPath() {
        let p = LocalPath::of_system("C:/Program Data");
        assert_eq!("C:/Program Data", p.to_system());
        assert_eq!(["C:", "Program Data"].as_slice(), p.components());

        let p = LocalPath::of_system("C:\\Program Data\\notes.txt");
        assert_eq!("C:\\Program Data\\notes.txt", p.to_system());
        assert_eq!(["C:", "Program Data", "notes.txt"].as_slice(), p.components());
    }

    #[test]
    fn testUnixPath() {
        let p = LocalPath::of_system("/");
        assert_eq!("/", p.to_system());
        assert_eq!([] as [&str; 0], p.components().as_slice());

        let p = LocalPath::of_system("/home/user");
        assert_eq!("/home/user", p.to_system());
        assert_eq!(["home", "user"].as_slice(), p.components());

        let p = LocalPath::of_system("/home/user/");
        assert_eq!("/home/user/", p.to_system());
        assert_eq!(["home", "user"].as_slice(), p.components());

        let p = LocalPath::of_system("/home/user/data/../notes/notes.txt");
        assert_eq!("/home/user/data/../notes/notes.txt", p.to_system());

        assert_eq!(
            ["home", "user", "data", "..", "notes", "notes.txt"].as_slice(),
            p.components()
        );
    }

    #[test]
    fn testNormalize() {
        let p = LocalPath::of_system("/home/user/data/../notes/notes.txt");
        let np = p.normalize();
        assert_eq!("/home/user/notes/notes.txt", np.to_system());

        let p = LocalPath::of_system("/../../notes/notes.txt");
        let np = p.normalize();
        assert_eq!("/notes/notes.txt", np.to_system());

        let p = LocalPath::of_system("./../../notes/notes.txt");
        let np = p.normalize();
        assert_eq!("../../notes/notes.txt", np.to_system());

        let p = LocalPath::of_system("../data/../../notes/notes.txt");
        let np = p.normalize();
        assert_eq!("../../notes/notes.txt", np.to_system());

        let p = LocalPath::of_system("C:\\data\\..\\..\\notes\\notes.txt");
        let np = p.normalize();
        assert_eq!("C:\\notes\\notes.txt", np.to_system());
    }
}

mod PathUriTests {
    use super::*;

    // IGNORED -- divergence in `paths::uri_to_system_path` (src/paths.rs).
    // F# expectation: "e:\\notes". .NET's `Uri("file:///e:/notes").LocalPath`
    // recognises the DOS path on every platform (this case is *not* gated in the
    // original and its CI runs `dotnet test` on ubuntu-latest), returning
    // "e:/notes" with '/' converted to '\', which `uriToSystemPath` then
    // lowercases to "e:\notes".
    // Rust actual: "ee:/notes", i.e. `LocalPath::of_uri` yields a *relative*
    // path instead of an absolute one.
    // Diagnosis: two bugs in the `is_win` branch of `uri_to_system_path`:
    //   1. `format!("{drive}{}", &local_path[1..])` keeps the drive letter from
    //      `local_path` (`local_path[1..]` of "/e:/notes" is "e:/notes"), so the
    //      drive is duplicated. It has to slice from index 2: `&local_path[2..]`.
    //   2. Even with that fixed the result is "e:/notes"; the DOS path is never
    //      converted to backslashes the way `Uri.LocalPath` does.
    #[test]
    fn testWinPathFromUri() {
        let uri = "file:///e%3A/notes";
        let puri = LocalPath::of_uri(uri).to_system().to_string();

        assert_eq!("e:\\notes", puri);
    }

    // IGNORED -- divergence in `paths::uri_to_system_path` (src/paths.rs).
    // F# expectation: "e:\\notes (precious)". `Uri("E:\\notes (precious)")`
    // parses the bare Windows path as a file URI, `LocalPath` gives
    // "E:\notes (precious)", and `uriToSystemPath` lowercases the drive.
    // Rust actual: "E:\\notes (precious)" -- the drive letter is left as is.
    // Diagnosis: `is_win` only recognises the "/X:" form (`local_path
    // .starts_with('/') && bytes[1].is_ascii_alphabetic() && bytes[2] == ':'`),
    // so a raw "X:\..." input skips the lowercasing branch entirely. .NET also
    // accepts inputs that carry no `file:` scheme, which the port passes
    // through untouched.
    #[test]
    fn testWinPathFromPath() {
        let puri = LocalPath::of_uri("E:\\notes (precious)").to_system().to_string();

        assert_eq!("e:\\notes (precious)", puri);
    }

    // IGNORED -- panics because of the same `uri_to_system_path` bug as
    // `testWinPathFromUri`.
    // F# expectation: `mkAbs` succeeds and `.uri` round-trips unchanged.
    // Rust actual: panics in `AbsPath::of_system` (src/paths.rs:110) with
    // "Bad absolute path: ee:/notes" -- the duplicated drive letter makes the
    // path neither a Windows nor a Unix absolute path, and `AbsPath::of_system`
    // (unlike `LocalPath::of_system`) has no relative fallback.
    #[test]
    fn testWinDocUriFromUri() {
        let uri = "file:///e%3A/notes";
        let puri = UriWith::mk_abs(uri);
        assert_eq!(uri, puri.uri);
    }

    // IGNORED -- same panic as `testWinDocUriFromUri`.
    // F# expectation: `systemPathToUriString "E:\\notes"` == "file:///E%3A/notes"
    // and `mkAbs` of that URI round-trips.
    // Rust actual: `system_path_to_uri_string("E:\\notes")` does produce
    // "file:///E%3A/notes" correctly, but `UriWith::mk_abs` then panics with
    // "Bad absolute path: eE:/notes" while deriving the `AbsPath` payload.
    #[test]
    fn testWinDocUriFromPath() {
        let path = "E:\\notes";
        let uri = "file:///E%3A/notes";
        let puri = UriWith::mk_abs(system_path_to_uri_string(path));
        assert_eq!(uri, puri.uri);
    }

    #[test]
    fn testRootedRel_SameRootRel() {
        let uri = "file:///a/b/doc.md";
        let root = UriWith::mk_root(uri);
        let id = UriWith::mk_rooted(&root, LocalPath::of_uri(uri));
        assert_eq!("file:///a/b/doc.md", id.uri);

        // The original snapshot-tests the F# structural rendering of `id.data`:
        //   { root = RootPath (AbsPath "/a/b/doc.md")
        //     path = None }
        // Rust has no equivalent `ToString`, so the two fields the snapshot shows
        // are asserted directly.
        assert_eq!("/a/b/doc.md", id.data.root.to_system());
        assert!(id.data.path.is_none());
    }

    #[test]
    fn testAccented_issue274() {
        let path = "/activité.md";
        let encoded = system_path_to_uri_string(path);
        assert_eq!("file:///activit%C3%A9.md", encoded);
    }
}
