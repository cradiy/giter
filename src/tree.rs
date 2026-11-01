use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsString,
    path::PathBuf,
};

use crate::git::File;

pub struct Row {
    pub path: PathBuf,
    pub prefix: String,
    pub directory: bool,
    pub expanded: bool,
    pub files: Vec<usize>,
}

#[derive(Default)]
struct Directory {
    children: BTreeMap<OsString, Directory>,
    direct: Vec<usize>,
    files: Vec<usize>,
}

pub fn rows(files: &[File], indices: &[usize], collapsed: &HashSet<PathBuf>) -> Vec<Row> {
    let mut root = Directory::default();
    for &index in indices {
        let mut directory = &mut root;
        directory.files.push(index);
        if let Some(parent) = files[index].path.parent() {
            for component in parent.components() {
                directory = directory
                    .children
                    .entry(component.as_os_str().to_owned())
                    .or_default();
                directory.files.push(index);
            }
        }
        directory.direct.push(index);
    }
    let mut rows = Vec::new();
    append(
        &root,
        PathBuf::new(),
        String::new(),
        String::new(),
        files,
        collapsed,
        &mut rows,
    );
    rows
}

fn append(
    directory: &Directory,
    path: PathBuf,
    prefix: String,
    child_prefix: String,
    files: &[File],
    collapsed: &HashSet<PathBuf>,
    rows: &mut Vec<Row>,
) {
    let expanded = !collapsed.contains(&path);
    rows.push(Row {
        path: path.clone(),
        prefix,
        directory: true,
        expanded,
        files: directory.files.clone(),
    });
    if !expanded {
        return;
    }
    enum Child<'a> {
        Directory(&'a OsString, &'a Directory),
        File(usize),
    }
    let mut children = directory
        .children
        .iter()
        .map(|(name, child)| Child::Directory(name, child))
        .chain(directory.direct.iter().map(|&index| Child::File(index)))
        .collect::<Vec<_>>();
    children.sort_by_key(|child| match child {
        Child::Directory(name, child) => (
            !child.files.iter().any(|i| files[*i].conflicted()),
            false,
            name.to_os_string(),
        ),
        Child::File(index) => (
            !files[*index].conflicted(),
            true,
            files[*index]
                .path
                .file_name()
                .unwrap_or_default()
                .to_owned(),
        ),
    });
    let count = children.len();
    for (i, child) in children.into_iter().enumerate() {
        let last = i + 1 == count;
        let prefix = format!("{child_prefix}{}", if last { "└─ " } else { "├─ " });
        match child {
            Child::Directory(name, child) => append(
                child,
                path.join(name),
                prefix,
                format!("{child_prefix}{}", if last { "   " } else { "│  " }),
                files,
                collapsed,
                rows,
            ),
            Child::File(index) => rows.push(Row {
                path: files[index].path.clone(),
                prefix,
                directory: false,
                expanded: false,
                files: vec![index],
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapsed_directory_retains_its_exact_file_scope_and_sibling_paths() {
        let files: Vec<_> = ["src/a.rs", "src/nested/b.rs", "src-other/c.rs", "顶层.txt"]
            .into_iter()
            .map(|path| File {
                path: path.into(),
                original: None,
                index: '?',
                worktree: '?',
            })
            .collect();
        let collapsed = HashSet::from([PathBuf::from("src")]);
        let rows = rows(&files, &[0, 1, 2, 3], &collapsed);
        assert_eq!(rows[0].files, [0, 1, 2, 3]);
        let src = rows
            .iter()
            .find(|r| r.path == std::path::Path::new("src"))
            .unwrap();
        assert!(!src.expanded);
        assert_eq!(src.files, [0, 1]);
        assert!(
            !rows
                .iter()
                .any(|r| r.path == std::path::Path::new("src/a.rs"))
        );
        assert!(
            rows.iter()
                .any(|r| r.path == std::path::Path::new("src-other/c.rs"))
        );
    }
}
