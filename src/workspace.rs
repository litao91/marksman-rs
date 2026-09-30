//! The set of workspace folders and the user-level configuration shared by them.
//!
//! Port of `Marksman.Workspace`.

use std::collections::BTreeMap;

use crate::config::Config;
use crate::folder::Folder;
use crate::names::FolderId;
use crate::paths::{AbsPath, RootPath};

#[derive(Clone, Debug, Default)]
pub struct Workspace {
    pub config: Option<Config>,
    pub folders: BTreeMap<FolderId, Folder>,
}

fn merge_folder_config(user_config: &Option<Config>, folder: Folder) -> Folder {
    let merged = Config::merge_opt(folder.config(), user_config.clone());
    folder.with_config(merged)
}

impl Workspace {
    pub fn of_folders(user_config: Option<Config>, folders: impl IntoIterator<Item = Folder>) -> Workspace {
        let mut map = BTreeMap::new();
        for folder in folders {
            let merged = merge_folder_config(&user_config, folder);
            map.insert(merged.id(), merged);
        }
        Workspace { config: user_config, folders: map }
    }

    pub fn folders(&self) -> impl Iterator<Item = &Folder> {
        self.folders.values()
    }

    pub fn user_config(&self) -> Option<Config> {
        self.config.clone()
    }

    pub fn try_find_folder_enclosing(&self, inner_path: &AbsPath) -> Option<&Folder> {
        self.folders.iter().find_map(|(folder_id, folder)| {
            let folder_path: &RootPath = &folder_id.data;
            if folder_path.contains(&crate::paths::LocalPath::Abs(inner_path.clone())) {
                Some(folder)
            } else {
                None
            }
        })
    }

    pub fn without_folder(&self, key_path: &FolderId) -> Workspace {
        let mut folders = self.folders.clone();
        folders.remove(key_path);
        Workspace { folders, ..self.clone() }
    }

    pub fn without_folders(&self, roots: &[FolderId]) -> Workspace {
        let mut folders = self.folders.clone();
        for root in roots {
            folders.remove(root);
        }
        Workspace { folders, ..self.clone() }
    }

    pub fn with_folder(&self, new_folder: Folder) -> Workspace {
        let new_folder = merge_folder_config(&self.config, new_folder);
        let mut folders = self.folders.clone();

        if new_folder.is_single_file() {
            folders.insert(new_folder.id(), new_folder);
        } else {
            let new_root = new_folder.root_path();

            // A multi-file folder subsumes any single-file folders inside it.
            let enclosed: Vec<FolderId> = folders
                .iter()
                .filter(|(_, existing)| {
                    if existing.is_single_file() {
                        let existing_root = AbsPath(existing.root_path().to_system().to_string());
                        new_root.contains(&crate::paths::LocalPath::Abs(existing_root))
                    } else {
                        false
                    }
                })
                .map(|(id, _)| id.clone())
                .collect();

            for id in enclosed {
                folders.remove(&id);
            }
            folders.insert(new_folder.id(), new_folder);
        }

        Workspace { folders, ..self.clone() }
    }

    pub fn with_folders(&self, folders: impl IntoIterator<Item = Folder>) -> Workspace {
        folders.into_iter().fold(self.clone(), |ws, folder| ws.with_folder(folder))
    }

    pub fn doc_count(&self) -> usize {
        self.folders.values().map(Folder::doc_count).sum()
    }

    pub fn folder_count(&self) -> usize {
        self.folders.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ParserSettings;
    use crate::doc::Doc;
    use crate::paths::LocalPath;
    use crate::text::mk_text;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("marksman-ws-{name}"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &std::path::Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn folder(dir: &std::path::Path, name: &str) -> Folder {
        folder_with_config(dir, name, None)
    }

    fn folder_with_config(dir: &std::path::Path, name: &str, config: Option<Config>) -> Folder {
        let folder_id =
            FolderId::of_uri(crate::paths::system_path_to_uri_string(dir.to_str().unwrap()));
        Folder::try_load(config, name, folder_id).unwrap()
    }

    fn single_file_doc(dir: &std::path::Path, rel: &str) -> Doc {
        let root = dir.to_str().unwrap();
        let folder_id = FolderId::of_uri(crate::paths::system_path_to_uri_string(root));
        Doc::mk(
            &ParserSettings::default(),
            DocId::mk_rooted(&folder_id, LocalPath::of_system(&format!("{root}/{rel}"))),
            Some(1),
            mk_text("# T\n"),
        )
        .unwrap()
    }

    use crate::names::DocId;

    #[test]
    fn counts_documents_across_folders() {
        let a = temp_dir("a");
        let b = temp_dir("b");
        write(&a, "x.md", "# X\n");
        write(&b, "y.md", "# Y\n");
        write(&b, "z.md", "# Z\n");

        let ws = Workspace::of_folders(None, vec![folder(&a, "a"), folder(&b, "b")]);
        assert_eq!(ws.folder_count(), 2);
        assert_eq!(ws.doc_count(), 3);
        std::fs::remove_dir_all(&a).ok();
        std::fs::remove_dir_all(&b).ok();
    }

    #[test]
    fn finds_the_folder_enclosing_a_path() {
        let a = temp_dir("enc-a");
        let b = temp_dir("enc-b");
        write(&a, "x.md", "# X\n");
        write(&b, "y.md", "# Y\n");

        let ws = Workspace::of_folders(None, vec![folder(&a, "a"), folder(&b, "b")]);
        let found = ws.try_find_folder_enclosing(&AbsPath::of_system(
            a.join("x.md").to_str().unwrap(),
        ));
        assert!(found.is_some());
        assert_eq!(found.unwrap().doc_count(), 1);
        assert!(ws
            .try_find_folder_enclosing(&AbsPath::of_system("/nowhere/x.md"))
            .is_none());
        std::fs::remove_dir_all(&a).ok();
        std::fs::remove_dir_all(&b).ok();
    }

    #[test]
    fn user_config_is_merged_into_folders() {
        let dir = temp_dir("cfg");
        write(&dir, "a.mdown", "# A\n");
        let user_config = Config {
            core_markdown_file_extensions: Some(vec!["mdown".into()]),
            ..Config::empty()
        };

        // Folders are loaded with the user config in hand, exactly as the server
        // does when it learns about workspace folders during initialize.
        let ws = Workspace::of_folders(
            Some(user_config.clone()),
            vec![folder_with_config(&dir, "d", Some(user_config))],
        );
        let f = ws.folders().next().unwrap();
        assert_eq!(f.configured_markdown_exts(), vec!["mdown".to_string()]);
        assert_eq!(f.doc_count(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn folders_can_be_removed() {
        let a = temp_dir("rm-a");
        write(&a, "x.md", "# X\n");
        let f = folder(&a, "a");
        let id = f.id();

        let ws = Workspace::of_folders(None, vec![f]);
        assert_eq!(ws.folder_count(), 1);
        assert_eq!(ws.without_folder(&id).folder_count(), 0);
        assert_eq!(ws.without_folders(&[id]).folder_count(), 0);
        std::fs::remove_dir_all(&a).ok();
    }

    #[test]
    fn multi_file_folder_subsumes_enclosed_single_file_folders() {
        let dir = temp_dir("subsume");
        write(&dir, "a.md", "# A\n");

        let single = Folder::single_file(single_file_doc(&dir, "a.md"), None);
        let ws = Workspace::of_folders(None, vec![single]);
        assert_eq!(ws.folder_count(), 1);

        let multi = folder(&dir, "multi");
        let ws = ws.with_folder(multi);
        assert_eq!(ws.folder_count(), 1);
        assert!(!ws.folders().next().unwrap().is_single_file());
        std::fs::remove_dir_all(&dir).ok();
    }
}
