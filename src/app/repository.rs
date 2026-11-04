use super::*;
use crate::tree::{Order, Row};

pub(super) struct Tree {
    source: Arc<[File]>,
    extra: Option<PathBuf>,
    filter: String,
    collapsed: HashSet<PathBuf>,
    files: Arc<[File]>,
    rows: Arc<[Row]>,
}

impl App {
    fn repository_tree(&self) -> Arc<Tree> {
        let source = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.committed_files.clone())
            .unwrap_or_default();
        let extra = self
            .file_history
            .as_ref()
            .filter(|history| history.start.is_some())
            .map(|history| &history.path);
        let mut cache = self.repository_cache.borrow_mut();
        if let Some(tree) = cache.as_ref()
            && (Arc::ptr_eq(&tree.source, &source) || (tree.source.is_empty() && source.is_empty()))
            && tree.extra.as_ref() == extra
            && tree.filter == self.repository_filter
            && tree.collapsed == self.collapsed[2]
        {
            return tree.clone();
        }
        let files = if let Some(path) = extra
            && !source.iter().any(|file| &file.path == path)
        {
            let mut files = source.to_vec();
            files.push(File {
                path: path.clone(),
                original: None,
                index: ' ',
                worktree: ' ',
            });
            files.into()
        } else {
            source.clone()
        };
        let query = self.repository_filter.to_lowercase();
        let indices: Vec<_> = files
            .iter()
            .enumerate()
            .filter(|(_, file)| query.is_empty() || file.label().to_lowercase().contains(&query))
            .map(|(index, _)| index)
            .collect();
        let rows = crate::tree::rows(
            &files,
            &indices,
            &self.collapsed[2],
            Order::DirectoriesFirst,
        )
        .into();
        let tree = Arc::new(Tree {
            source,
            extra: extra.cloned(),
            filter: self.repository_filter.clone(),
            collapsed: self.collapsed[2].clone(),
            files,
            rows,
        });
        *cache = Some(tree.clone());
        tree
    }

    pub fn repository_files(&self) -> Arc<[File]> {
        self.repository_tree().files.clone()
    }

    pub fn repository_rows(&self) -> Arc<[Row]> {
        self.repository_tree().rows.clone()
    }

    pub(super) fn select_repository_path(&mut self, path: &std::path::Path) {
        let rows = self.repository_rows();
        if rows
            .get(self.repository_selected)
            .is_some_and(|row| !row.directory && row.path == path)
        {
            return;
        }
        if let Some(index) = rows
            .iter()
            .position(|row| !row.directory && row.path == path)
        {
            self.repository_selected = index;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn app(paths: impl Iterator<Item = PathBuf>) -> App {
        let mut app = App::new(Repo {
            root: "/nonexistent-repository-cache-test".into(),
        });
        app.snapshot = Some(Snapshot {
            branch: "main".into(),
            operation: String::new(),
            files: vec![],
            committed_files: paths
                .map(|path| File {
                    path,
                    original: None,
                    index: ' ',
                    worktree: ' ',
                })
                .collect::<Vec<_>>()
                .into(),
            head: Some("1".repeat(40)),
            stashes: vec![],
            branches: vec![],
        });
        app.tab = Tab::Files;
        app.file_tree_focus = true;
        app
    }

    #[test]
    fn repository_cache_tracks_filter_folds_file_inventory_and_historical_paths() {
        let mut app = app(["src/a.rs", "src/b.rs", "src/c.rs"]
            .into_iter()
            .map(PathBuf::from));
        let initial = app.repository_rows();
        assert!(Arc::ptr_eq(&initial, &app.repository_rows()));
        app.repository_filter = "b.rs".into();
        let filtered = app.repository_rows();
        assert!(!Arc::ptr_eq(&initial, &filtered));
        assert_eq!(
            filtered.last().unwrap().path,
            std::path::Path::new("src/b.rs")
        );
        app.collapsed[2].insert("src".into());
        let folded = app.repository_rows();
        assert_eq!(folded.last().unwrap().path, std::path::Path::new("src"));
        assert_eq!(folded[0].files.len(), 1);
        app.repository_filter.clear();
        assert_eq!(app.repository_rows()[0].files.len(), 3);
        app.collapsed[2].clear();
        let mut files = app.snapshot.as_ref().unwrap().committed_files.to_vec();
        files.push(File {
            path: "new.rs".into(),
            original: None,
            index: ' ',
            worktree: ' ',
        });
        app.snapshot.as_mut().unwrap().committed_files = files.into();
        assert_eq!(app.repository_rows()[0].files.len(), 4);
        app.open_file_history(
            "old/deleted.rs".into(),
            "old/deleted.rs".into(),
            Some("2".repeat(40)),
        );
        assert_eq!(app.repository_rows()[0].files.len(), 5);
        assert!(
            app.repository_rows()
                .iter()
                .any(|row| row.path == std::path::Path::new("old/deleted.rs"))
        );
        app.open_file_history(
            "old/other.rs".into(),
            "old/other.rs".into(),
            Some("2".repeat(40)),
        );
        let rows = app.repository_rows();
        assert!(
            !rows
                .iter()
                .any(|row| row.path == std::path::Path::new("old/deleted.rs"))
        );
        assert!(
            rows.iter()
                .any(|row| row.path == std::path::Path::new("old/other.rs"))
        );
    }

    #[test]
    fn repository_viewport_keeps_selection_past_u16_offsets_after_resize() {
        let mut app = app((0..70000).map(|index| PathBuf::from(format!("f{index:05}.rs"))));
        app.repository_selected = 70000;
        let rows = app.repository_rows();
        for (width, height) in [(80, 18), (120, 24)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| crate::ui::draw(frame, &mut app))
                .unwrap();
            let text: String = (1..height - 2)
                .flat_map(|y| (0..width / 2).map(move |x| (x, y)))
                .map(|position| terminal.backend().buffer()[position].symbol())
                .collect();
            assert!(text.contains("f69999.rs"));
            assert!(app.repository_state.offset() > usize::from(u16::MAX));
            assert_eq!(app.repository_state.selected(), Some(70000));
            assert!(Arc::ptr_eq(&rows, &app.repository_rows()));
        }
        app.event(Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)));
        assert!(
            matches!(app.selection(), Some(Selection::RepositoryFile(file)) if file.path == std::path::Path::new("f69998.rs"))
        );
        assert!(Arc::ptr_eq(&rows, &app.repository_rows()));
    }
}
