use super::*;

impl App {
    pub fn changes_view(&self) -> bool {
        matches!(self.tab, Tab::Changes | Tab::Staged)
    }

    pub fn main_tab(&self) -> Tab {
        if self.changes_view() {
            Tab::Changes
        } else {
            self.tab
        }
    }

    pub fn change_filter(&self, tab: Tab) -> &str {
        if self.tab == tab {
            &self.filter
        } else {
            &self.change_panes[usize::from(tab == Tab::Staged)].filter
        }
    }

    pub fn change_rows(&self, tab: Tab) -> Vec<crate::tree::Row> {
        let files = self
            .snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| snapshot.files.as_slice());
        let query = self.change_filter(tab).to_lowercase();
        let indices: Vec<_> = files
            .iter()
            .enumerate()
            .filter(|(_, file)| {
                (if tab == Tab::Staged {
                    file.staged()
                } else {
                    file.unstaged()
                }) && file.label().to_lowercase().contains(&query)
            })
            .map(|(index, _)| index)
            .collect();
        if indices.is_empty() {
            return Vec::new();
        }
        crate::tree::rows(
            files,
            &indices,
            &self.collapsed[usize::from(tab == Tab::Staged)],
        )
    }

    pub(super) fn save_change_state(&mut self) {
        if self.changes_view() {
            self.change_tab = self.tab;
            self.change_panes[usize::from(self.tab == Tab::Staged)] = ChangeState {
                selected: self.selected,
                filter: self.filter.clone(),
                list_state: self.list_state,
            };
        }
    }

    pub(super) fn restore_change_state(&mut self) {
        let state = &self.change_panes[usize::from(self.tab == Tab::Staged)];
        self.selected = state.selected;
        self.filter = state.filter.clone();
        self.list_state = state.list_state;
    }

    pub(super) fn focus_changes(&mut self, tab: Tab, at_edge: bool) {
        if !self.changes_view() {
            return;
        }
        self.fullscreen = false;
        self.list_focus = true;
        if self.tab != tab {
            self.save_change_state();
            self.tab = tab;
            self.change_tab = tab;
            self.restore_change_state();
            if at_edge {
                self.selected = if tab == Tab::Staged {
                    0
                } else {
                    self.item_count().saturating_sub(1)
                };
            }
            self.load_preview();
        }
    }

    pub(super) fn move_step(&mut self, amount: isize) {
        if self.changes_view() && self.list_focus {
            if amount > 0
                && self.tab == Tab::Changes
                && self.selected >= self.item_count().saturating_sub(1)
            {
                self.focus_changes(Tab::Staged, true);
                return;
            }
            if amount < 0 && self.tab == Tab::Staged && self.selected == 0 {
                self.focus_changes(Tab::Changes, true);
                return;
            }
        }
        self.move_by(amount);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn app() -> App {
        let mut app = App::new(Repo {
            root: "/nonexistent-giter-changes-test".into(),
        });
        app.snapshot = Some(Snapshot {
            branch: "main".into(),
            operation: String::new(),
            files: [
                ("src/both.rs", 'A', 'M'),
                ("src/working.rs", ' ', 'D'),
                ("src/staged.rs", 'M', ' '),
            ]
            .into_iter()
            .map(|(path, index, worktree)| File {
                path: path.into(),
                original: None,
                index,
                worktree,
            })
            .collect(),
            committed_files: vec![],
            head: None,
            stashes: vec![],
            branches: vec![],
        });
        app
    }

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.event(Event::Key(KeyEvent::new(code, modifiers)));
    }

    fn select(app: &mut App, path: &str) {
        app.selected = app
            .tree_rows()
            .iter()
            .position(|row| row.path == std::path::Path::new(path))
            .unwrap();
    }

    #[test]
    fn split_changes_preserves_tree_state_and_uses_the_selected_git_side() {
        let mut app = app();
        select(&mut app, "src/both.rs");
        assert!(matches!(app.selection(), Some(Selection::File(_, false))));
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text: Vec<String> = (0..30)
            .map(|y| (0..40).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        let upper = text
            .iter()
            .position(|row| row.contains("M both.rs"))
            .unwrap();
        let lower = text
            .iter()
            .position(|row| row.contains("A both.rs"))
            .unwrap();
        assert!(upper < lower);
        let upper_x = text[upper].chars().position(|ch| ch == 'M').unwrap() as u16;
        let lower_x = text[lower].chars().position(|ch| ch == 'A').unwrap() as u16;
        assert_eq!(buffer[(upper_x, upper as u16)].fg, crate::theme::AMBER);
        assert_eq!(buffer[(lower_x, lower as u16)].fg, crate::theme::GREEN);
        app.update_filter("working.rs".into());
        select(&mut app, "src/working.rs");
        press(&mut app, KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert!(app.main_tab() == Tab::Changes);
        assert!(app.filter.is_empty());
        select(&mut app, "src/both.rs");
        assert!(matches!(app.selection(), Some(Selection::File(_, true))));
        select(&mut app, "src");
        app.tree_key(false, false);
        assert!(
            !app.tree_rows()
                .iter()
                .any(|row| row.path == std::path::Path::new("src/both.rs"))
        );
        press(&mut app, KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(app.filter, "working.rs");
        assert!(
            matches!(app.selection(), Some(Selection::File(file, false)) if file.path == std::path::Path::new("src/working.rs"))
        );
        press(&mut app, KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert!(
            !app.tree_rows()
                .iter()
                .any(|row| row.path == std::path::Path::new("src/both.rs"))
        );
        press(&mut app, KeyCode::Char('l'), KeyModifiers::CONTROL);
        assert!(app.tab == Tab::Branches);
        press(&mut app, KeyCode::Char('h'), KeyModifiers::CONTROL);
        assert!(app.tab == Tab::Staged);
        assert!(app.filter.is_empty());
        assert!(
            app.tree_rows()
                .iter()
                .find(|row| row.path == std::path::Path::new("src"))
                .is_some_and(|row| !row.expanded)
        );
        press(&mut app, KeyCode::Char('k'), KeyModifiers::CONTROL);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.fullscreen);
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert!(app.tab == Tab::Changes && app.fullscreen);
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.fullscreen && app.filter == "working.rs");
    }

    #[test]
    fn split_changes_crosses_adjacent_edges_and_empty_lists_without_wrapping() {
        for (down, up) in [
            (KeyCode::Char('j'), KeyCode::Char('k')),
            (KeyCode::Down, KeyCode::Up),
        ] {
            let mut app = app();
            press(&mut app, up, KeyModifiers::NONE);
            assert!(app.tab == Tab::Changes && app.selected == 0);
            press(&mut app, down, KeyModifiers::NONE);
            assert!(app.tab == Tab::Changes && app.selected == 1);
            press(&mut app, KeyCode::Char('G'), KeyModifiers::NONE);
            press(&mut app, down, KeyModifiers::NONE);
            assert!(app.tab == Tab::Staged && app.selected == 0);
            press(&mut app, up, KeyModifiers::NONE);
            assert!(app.tab == Tab::Changes && app.selected == app.item_count() - 1);
            press(&mut app, down, KeyModifiers::NONE);
            press(&mut app, KeyCode::Char('G'), KeyModifiers::NONE);
            let last = app.selected;
            press(&mut app, down, KeyModifiers::NONE);
            assert!(app.tab == Tab::Staged && app.selected == last);
            app.snapshot.as_mut().unwrap().files.clear();
            app.load_preview();
            press(&mut app, up, KeyModifiers::NONE);
            assert!(app.tab == Tab::Changes && app.selected == 0);
            press(&mut app, down, KeyModifiers::NONE);
            assert!(app.tab == Tab::Staged && app.selected == 0);
        }
    }
}
