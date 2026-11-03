use std::{
    borrow::Cow,
    collections::HashSet,
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{Receiver, Sender},
    },
};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;

use crate::{
    branches::Browser,
    git::{
        self, Commit, CommitDetails, File, FileCommit, Preview, Repo, Request, Response, Selection,
        Snapshot, Stash, StashDetails,
    },
    input::Input,
};

mod branches;
mod changes;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Changes,
    Staged,
    Files,
    Stash,
    Branches,
}

impl Tab {
    pub const ALL: [Self; 4] = [Self::Changes, Self::Branches, Self::Files, Self::Stash];
}

#[derive(Default)]
pub struct ChangeState {
    pub selected: usize,
    pub filter: String,
    pub list_state: ListState,
}

pub enum Mode {
    Normal,
    Search(Input),
    Commit(Input),
    Help,
}

pub enum Action {
    None,
    Quit,
    Editor(PathBuf),
}

pub struct History {
    pub commit: Commit,
    pub details: Option<CommitDetails>,
    token: u64,
    return_selected: usize,
    return_filter: String,
    return_list_state: ListState,
}

pub struct FileHistory {
    pub path: PathBuf,
    pub entries: Option<Vec<FileCommit>>,
    query_path: PathBuf,
    start: Option<String>,
    token: u64,
    return_tab: Tab,
    return_selected: usize,
    return_filter: String,
    return_list_state: ListState,
    return_focus: bool,
    refreshing: bool,
}

pub struct StashView {
    pub item: Stash,
    pub details: Option<Arc<StashDetails>>,
    token: u64,
    return_selected: usize,
    return_filter: String,
    return_list_state: ListState,
}

pub struct App {
    pub repo: Repo,
    pub snapshot: Option<Snapshot>,
    pub tab: Tab,
    pub list_state: ListState,
    pub selected: usize,
    pub preview: Option<Preview>,
    pub preview_loading: bool,
    pub list_focus: bool,
    pub fullscreen: bool,
    pub scroll: usize,
    pub horizontal: u16,
    pub chunk: usize,
    pub base: bool,
    pub whole_sources: bool,
    pub mode: Mode,
    pub filter: String,
    pub message: String,
    pub error: bool,
    pub busy: bool,
    pub page_height: usize,
    pub history: Option<History>,
    pub stash: Option<StashView>,
    pub branches: Browser,
    pub graphics: crate::graphics::Graphics,
    pub file_history: Option<FileHistory>,
    pub file_tree_focus: bool,
    pub repository_selected: usize,
    pub repository_filter: String,
    pub repository_state: ListState,
    pub change_panes: [ChangeState; 2],
    change_tab: Tab,
    collapsed: [HashSet<PathBuf>; 5],
    history_generation: u64,
    sender: Sender<Request>,
    receiver: Receiver<Response>,
    generation: u64,
    pending_g: bool,
    committing: bool,
    watch_pending: bool,
    watch_refreshing: bool,
}

impl App {
    pub fn new(repo: Repo) -> Self {
        let (sender, receiver) = git::worker(repo.clone());
        let app = Self {
            repo,
            snapshot: None,
            tab: Tab::Changes,
            list_state: ListState::default(),
            selected: 0,
            preview: None,
            preview_loading: false,
            list_focus: true,
            fullscreen: false,
            scroll: 0,
            horizontal: 0,
            chunk: 0,
            base: false,
            whole_sources: false,
            mode: Mode::Normal,
            filter: String::new(),
            message: "Loading repository…".into(),
            error: false,
            busy: true,
            page_height: 10,
            history: None,
            stash: None,
            branches: Browser::default(),
            graphics: crate::graphics::Graphics::default(),
            file_history: None,
            file_tree_focus: false,
            repository_selected: 0,
            repository_filter: String::new(),
            repository_state: ListState::default(),
            change_panes: std::array::from_fn(|_| ChangeState::default()),
            change_tab: Tab::Changes,
            collapsed: std::array::from_fn(|_| HashSet::new()),
            history_generation: 0,
            sender,
            receiver,
            generation: 0,
            pending_g: false,
            committing: false,
            watch_pending: false,
            watch_refreshing: false,
        };
        let _ = app.sender.send(Request::Refresh);
        app
    }

    pub fn stash_list(&self) -> bool {
        self.tab == Tab::Stash && self.stash.is_none()
    }

    pub fn commit_list(&self) -> bool {
        self.file_history.is_some() || self.branch_graph_view()
    }

    pub fn commit_at(&self, index: usize) -> Option<&Commit> {
        if let Some(history) = &self.file_history {
            history
                .entries
                .as_ref()?
                .get(index)
                .map(|entry| &entry.commit)
        } else if self.tab == Tab::Branches {
            self.branches
                .graph
                .as_ref()?
                .entries
                .get(index)
                .map(|entry| &entry.commit)
        } else {
            None
        }
    }

    fn commit_count(&self) -> usize {
        if let Some(history) = &self.file_history {
            history.entries.as_ref().map_or(0, Vec::len)
        } else if self.tab == Tab::Branches {
            self.branches
                .graph
                .as_ref()
                .map_or(0, |graph| graph.entries.len())
        } else {
            0
        }
    }

    pub fn files(&self) -> &[File] {
        if let Some(stash) = &self.stash {
            return stash
                .details
                .as_ref()
                .map_or(&[], |details| details.files.as_slice());
        }
        if let Some(history) = &self.history {
            history
                .details
                .as_ref()
                .map_or(&[], |details| details.files.as_slice())
        } else {
            self.snapshot.as_ref().map_or(&[], |snapshot| {
                if self.tab == Tab::Files {
                    snapshot.committed_files.as_slice()
                } else {
                    snapshot.files.as_slice()
                }
            })
        }
    }

    fn tree_index(&self) -> usize {
        match self.tab {
            Tab::Changes => 0,
            Tab::Staged => 1,
            Tab::Files => 2,
            Tab::Stash => 3,
            Tab::Branches => 4,
        }
    }

    pub fn items(&self) -> Vec<usize> {
        let query = self.filter.to_lowercase();
        if self.stash_list() {
            self.snapshot
                .as_ref()
                .map(|snapshot| {
                    snapshot
                        .stashes
                        .iter()
                        .enumerate()
                        .filter(|(_, stash)| {
                            format!(
                                "{} {} {} {}",
                                stash.id, stash.reference, stash.subject, stash.author
                            )
                            .to_lowercase()
                            .contains(&query)
                        })
                        .map(|(index, _)| index)
                        .collect()
                })
                .unwrap_or_default()
        } else if self.commit_list() {
            (0..self.commit_count())
                .filter(|index| {
                    self.commit_at(*index).is_some_and(|commit| {
                        format!("{} {} {}", commit.id, commit.subject, commit.author)
                            .to_lowercase()
                            .contains(&query)
                    })
                })
                .collect()
        } else {
            self.files()
                .iter()
                .enumerate()
                .filter(|(_, file)| {
                    (self.history.is_some()
                        || self.stash.is_some()
                        || self.tab == Tab::Files
                        || if self.tab == Tab::Changes {
                            file.unstaged()
                        } else {
                            file.staged()
                        })
                        && file.label().to_lowercase().contains(&query)
                })
                .map(|(i, _)| i)
                .collect()
        }
    }

    pub fn file_history_selection(&self) -> Option<Selection> {
        let history = self.file_history.as_ref()?;
        let index = *self.items().get(self.selected)?;
        let entry = history.entries.as_ref()?.get(index)?;
        Some(Selection::CommitFile {
            id: entry.commit.id.clone(),
            parent: entry.parent.clone(),
            file: entry.file.clone(),
        })
    }

    pub fn selection(&self) -> Option<Selection> {
        if self.branch_list() {
            return None;
        }
        if self.stash_list() {
            let snapshot = self.snapshot.as_ref()?;
            return self
                .items()
                .get(self.selected)
                .map(|index| Selection::Stash(snapshot.stashes[*index].clone()));
        }
        if self.tab == Tab::Files && (self.file_tree_focus || self.file_history.is_none()) {
            return self.repository_selection();
        }
        if self.file_history.is_some() {
            return self.file_history_selection();
        }
        if self.commit_list() {
            let index = *self.items().get(self.selected)?;
            return self
                .commit_at(index)
                .map(|commit| Selection::Commit(commit.id.clone()));
        }
        let rows = self.tree_rows();
        let row = rows.get(self.selected)?;
        let files = self.files();
        if let Some(stash) = &self.stash {
            let details = stash.details.as_ref()?.clone();
            return Some(if row.directory {
                Selection::StashDirectory {
                    details,
                    path: row.path.clone(),
                    files: row
                        .files
                        .iter()
                        .map(|index| files[*index].clone())
                        .collect(),
                }
            } else {
                Selection::StashFile {
                    details,
                    file: files[row.files[0]].clone(),
                }
            });
        }
        if self.tab == Tab::Files {
            return Some(if row.directory {
                Selection::RepositoryDirectory(
                    row.path.clone(),
                    row.files
                        .iter()
                        .map(|index| files[*index].clone())
                        .collect(),
                )
            } else {
                Selection::RepositoryFile(files[row.files[0]].clone())
            });
        }
        if let Some(history) = &self.history {
            let details = history.details.as_ref()?;
            return Some(if row.directory {
                Selection::CommitDirectory {
                    id: history.commit.id.clone(),
                    path: row.path.clone(),
                    files: row.files.iter().map(|i| files[*i].clone()).collect(),
                }
            } else {
                Selection::CommitFile {
                    id: history.commit.id.clone(),
                    parent: details.parent.clone(),
                    file: files[row.files[0]].clone(),
                }
            });
        }
        Some(if row.directory {
            Selection::Directory(
                row.path.clone(),
                row.files.iter().map(|i| files[*i].clone()).collect(),
                self.tab == Tab::Staged,
            )
        } else {
            Selection::File(files[row.files[0]].clone(), self.tab == Tab::Staged)
        })
    }

    pub fn tree_rows(&self) -> Vec<crate::tree::Row> {
        if self.changes_view() {
            return self.change_rows(self.tab);
        }
        if self.commit_list()
            || self.branch_list()
            || self.stash_list()
            || self
                .stash
                .as_ref()
                .is_some_and(|stash| stash.details.is_none())
            || self.snapshot.is_none()
            || self.history.as_ref().is_some_and(|h| h.details.is_none())
        {
            return Vec::new();
        }
        crate::tree::rows(
            self.files(),
            &self.items(),
            &self.collapsed[self.tree_index()],
        )
    }

    fn item_count(&self) -> usize {
        if self.commit_list() || self.stash_list() {
            self.items().len()
        } else {
            self.tree_rows().len()
        }
    }

    fn switch_tab(&mut self, tab: Tab) {
        self.save_change_state();
        let tab = if tab == Tab::Changes {
            self.change_tab
        } else {
            tab
        };
        self.fullscreen = false;
        self.history = None;
        self.stash = None;
        self.file_history = None;
        self.tab = tab;
        if tab == Tab::Branches {
            self.branches = Browser::default();
            self.restore_branch(None);
        }
        self.file_tree_focus = tab == Tab::Files;
        if tab == Tab::Files {
            self.repository_selected = 0;
            self.repository_filter.clear();
            self.repository_state = ListState::default();
        }
        self.selected = 0;
        self.filter.clear();
        self.list_focus = true;
        self.list_state = ListState::default();
        if self.changes_view() {
            self.restore_change_state();
        }
        self.load_preview();
    }

    pub fn repository_files(&self) -> Cow<'_, [File]> {
        let files = self
            .snapshot
            .as_ref()
            .map_or(&[][..], |snapshot| snapshot.committed_files.as_slice());
        if let Some(history) = &self.file_history
            && history.start.is_some()
            && !files.iter().any(|file| file.path == history.path)
        {
            let mut files = files.to_vec();
            files.push(File {
                path: history.path.clone(),
                original: None,
                index: ' ',
                worktree: ' ',
            });
            Cow::Owned(files)
        } else {
            Cow::Borrowed(files)
        }
    }

    pub fn repository_rows(&self) -> Vec<crate::tree::Row> {
        let files = self.repository_files();
        let query = self.repository_filter.to_lowercase();
        let indices: Vec<_> = files
            .iter()
            .enumerate()
            .filter(|(_, file)| file.label().to_lowercase().contains(&query))
            .map(|(index, _)| index)
            .collect();
        crate::tree::rows(&files, &indices, &self.collapsed[2])
    }

    pub fn file_history_has_origin(&self) -> bool {
        self.file_history
            .as_ref()
            .is_some_and(|view| view.return_tab != Tab::Files)
    }

    fn repository_selection(&self) -> Option<Selection> {
        let files = self.repository_files();
        let rows = self.repository_rows();
        let row = rows.get(self.repository_selected)?;
        Some(if row.directory {
            Selection::RepositoryDirectory(
                row.path.clone(),
                row.files
                    .iter()
                    .map(|index| files[*index].clone())
                    .collect(),
            )
        } else {
            Selection::RepositoryFile(files[row.files[0]].clone())
        })
    }

    fn sync_repository_history(&mut self) {
        if let Some(Selection::RepositoryFile(file)) = self.repository_selection() {
            if self
                .file_history
                .as_ref()
                .is_none_or(|history| history.path != file.path)
            {
                let query = file.original.clone().unwrap_or_else(|| file.path.clone());
                if self.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot
                        .committed_files
                        .iter()
                        .any(|committed| committed.path == query)
                }) {
                    self.open_file_history(query.clone(), query, None);
                } else {
                    self.switch_tab(Tab::Files);
                    self.notice("This file has no committed history", false);
                }
            }
        } else if self.file_history.is_none() {
            self.request_preview(None);
        }
        self.file_tree_focus = true;
        self.list_focus = true;
    }

    fn repository_key(&mut self, open: bool, toggle: bool) {
        let rows = self.repository_rows();
        let Some(row) = rows.get(self.repository_selected) else {
            return;
        };
        if row.directory && (toggle || (open && !row.expanded) || (!open && row.expanded)) {
            if row.expanded {
                self.collapsed[2].insert(row.path.clone());
            } else {
                self.collapsed[2].remove(&row.path);
            }
        } else if open {
            if row.directory && !row.files.is_empty() && self.repository_selected + 1 < rows.len() {
                self.repository_selected += 1;
            } else if !row.directory {
                self.enter_file_history();
                return;
            }
        } else if let Some(parent) = row.path.parent()
            && let Some(index) = rows
                .iter()
                .position(|row| row.directory && row.path == parent)
        {
            self.repository_selected = index;
        }
        self.sync_repository_history();
    }

    fn enter_file_history(&mut self) {
        if self.file_history.is_some() {
            self.file_tree_focus = false;
            self.list_focus = true;
            if self.preview.is_none() {
                self.load_preview();
            }
        }
    }

    fn current_filter(&self) -> &str {
        if self.branch_list() {
            &self.branches.filter
        } else if self.tab == Tab::Files && self.file_tree_focus {
            &self.repository_filter
        } else {
            &self.filter
        }
    }

    fn update_filter(&mut self, filter: String) {
        if self.branch_list() {
            self.branches.filter = filter;
            self.branches.selected = self
                .branch_rows()
                .iter()
                .position(|row| matches!(row, crate::branches::Row::Branch(_)))
                .unwrap_or(0);
            self.branches.state = ListState::default();
            self.sync_branches(false, false);
        } else if self.tab == Tab::Files && self.file_tree_focus {
            self.repository_filter = filter;
            self.repository_selected = 0;
            if let Some(snapshot) = &self.snapshot {
                let query = self.repository_filter.to_lowercase();
                let parents: Vec<_> = snapshot
                    .committed_files
                    .iter()
                    .filter(|file| file.label().to_lowercase().contains(&query))
                    .flat_map(|file| {
                        file.path
                            .ancestors()
                            .skip(1)
                            .map(|parent| parent.to_owned())
                    })
                    .collect();
                for parent in parents {
                    self.collapsed[2].remove(&parent);
                }
            }
            self.sync_repository_history();
        } else {
            self.filter = filter;
            self.selected = 0;
            self.reveal_matches();
            self.load_preview();
        }
    }

    fn open_selected_file_history(&mut self) {
        if self.tab == Tab::Files {
            if self.file_tree_focus {
                self.sync_repository_history();
                if matches!(
                    self.repository_selection(),
                    Some(Selection::RepositoryFile(_))
                ) {
                    self.enter_file_history();
                } else {
                    self.repository_key(true, false);
                }
            }
            return;
        }
        match self.selection() {
            Some(Selection::File(file, _) | Selection::RepositoryFile(file)) => {
                let query = file.original.clone().unwrap_or_else(|| file.path.clone());
                self.open_file_history(file.path, query, None);
            }
            Some(Selection::CommitFile { id, file, .. }) => {
                self.open_file_history(file.path.clone(), file.path, Some(id))
            }
            _ => self.switch_tab(Tab::Files),
        }
    }

    fn open_file_history(&mut self, path: PathBuf, query_path: PathBuf, start: Option<String>) {
        self.save_change_state();
        self.fullscreen = false;
        let from_repository = self.tab == Tab::Files;
        if !from_repository {
            self.repository_filter.clear();
            self.repository_state = ListState::default();
        }
        for parent in path.ancestors().skip(1) {
            self.collapsed[2].remove(parent);
        }
        if let Some(index) = self
            .repository_rows()
            .iter()
            .position(|row| !row.directory && row.path == path)
        {
            self.repository_selected = index;
        }
        self.history_generation += 1;
        let mut view = self.file_history.take().unwrap_or_else(|| FileHistory {
            path: path.clone(),
            entries: None,
            query_path: query_path.clone(),
            start: start.clone(),
            token: 0,
            return_tab: self.tab,
            return_selected: self.selected,
            return_filter: std::mem::take(&mut self.filter),
            return_list_state: std::mem::take(&mut self.list_state),
            return_focus: self.list_focus,
            refreshing: false,
        });
        view.path = path;
        view.query_path = query_path.clone();
        view.start = start.clone();
        view.entries = None;
        view.refreshing = false;
        view.token = self.history_generation;
        self.file_history = Some(view);
        if let Some(index) = self.repository_rows().iter().position(|row| {
            !row.directory
                && self
                    .file_history
                    .as_ref()
                    .is_some_and(|view| row.path == view.path)
        }) {
            self.repository_selected = index;
        }
        self.tab = Tab::Files;
        self.file_tree_focus = false;
        self.selected = 0;
        self.filter.clear();
        self.list_state = ListState::default();
        self.list_focus = true;
        self.request_preview(None);
        self.notice("Loading file history…", false);
        let _ = self.sender.send(Request::FileHistory(
            self.history_generation,
            query_path,
            start,
        ));
    }

    fn leave_file_history(&mut self) {
        if let Some(view) = self.file_history.take() {
            self.tab = view.return_tab;
            self.selected = view.return_selected;
            self.filter = view.return_filter;
            self.list_state = view.return_list_state;
            self.list_focus = view.return_focus;
            self.file_tree_focus = self.tab == Tab::Files;
            if self.file_tree_focus {
                self.request_preview(None);
            } else {
                self.load_preview();
            }
            self.notice("Returned from file history", false);
        }
    }

    fn open_stash(&mut self) {
        let Some(Selection::Stash(item)) = self.selection() else {
            return;
        };
        self.history_generation += 1;
        let id = item.id.clone();
        self.stash = Some(StashView {
            item,
            details: None,
            token: self.history_generation,
            return_selected: self.selected,
            return_filter: std::mem::take(&mut self.filter),
            return_list_state: std::mem::take(&mut self.list_state),
        });
        self.selected = 0;
        self.collapsed[3].clear();
        self.list_focus = true;
        self.load_preview();
        self.notice("Loading stash files…", false);
        let _ = self
            .sender
            .send(Request::StashFiles(self.history_generation, id));
    }

    fn leave_stash(&mut self) {
        if let Some(view) = self.stash.take() {
            self.filter = view.return_filter;
            self.selected = self
                .items()
                .iter()
                .position(|index| {
                    self.snapshot
                        .as_ref()
                        .is_some_and(|snapshot| snapshot.stashes[*index].id == view.item.id)
                })
                .unwrap_or(view.return_selected);
            self.list_state = view.return_list_state;
            self.list_focus = true;
            self.load_preview();
            self.notice("Stashes · Enter opens changed files", false);
        }
    }

    fn open_history(&mut self) {
        let Some(Selection::Commit(id)) = self.selection() else {
            return;
        };
        let Some(commit) = self
            .items()
            .into_iter()
            .filter_map(|index| self.commit_at(index))
            .find(|commit| commit.id == id)
            .cloned()
        else {
            return;
        };
        self.history_generation += 1;
        self.history = Some(History {
            commit,
            details: None,
            token: self.history_generation,
            return_selected: self.selected,
            return_filter: std::mem::take(&mut self.filter),
            return_list_state: std::mem::take(&mut self.list_state),
        });
        self.collapsed[self.tree_index()].clear();
        self.selected = 0;
        self.list_focus = true;
        self.load_preview();
        self.notice("Loading commit files…", false);
        let _ = self
            .sender
            .send(Request::CommitFiles(self.history_generation, id));
    }

    fn leave_history(&mut self) {
        if let Some(history) = self.history.take() {
            self.filter = history.return_filter;
            self.selected = self
                .items()
                .iter()
                .position(|index| {
                    self.commit_at(*index)
                        .is_some_and(|commit| commit.id == history.commit.id)
                })
                .unwrap_or(history.return_selected);
            self.list_state = history.return_list_state;
            self.list_focus = true;
            self.sync_branches(false, false);
            self.load_preview();
            self.notice("History · Enter opens commit files", false);
        }
    }

    fn reveal_matches(&mut self) {
        if self.filter.is_empty() || self.commit_list() || self.stash_list() {
            return;
        }
        let parents: Vec<_> = self
            .items()
            .iter()
            .flat_map(|index| {
                self.files()[*index]
                    .path
                    .ancestors()
                    .skip(1)
                    .map(|p| p.to_owned())
            })
            .collect();
        let tree = self.tree_index();
        for parent in parents {
            self.collapsed[tree].remove(&parent);
        }
    }

    fn tree_key(&mut self, open: bool, toggle: bool) {
        if !self.list_focus {
            self.list_focus = true;
            return;
        }
        let rows = self.tree_rows();
        let Some(row) = rows.get(self.selected) else {
            if open {
                self.list_focus = false;
            }
            return;
        };
        let tree = self.tree_index();
        let collapsed = &mut self.collapsed[tree];
        if row.directory && (toggle || (open && !row.expanded) || (!open && row.expanded)) {
            if row.expanded {
                collapsed.insert(row.path.clone());
            } else {
                collapsed.remove(&row.path);
            }
        } else if open {
            if row.directory && !row.files.is_empty() && self.selected + 1 < rows.len() {
                self.selected += 1;
            } else if !row.directory {
                self.open_fullscreen();
                return;
            }
        } else if let Some(parent) = row.path.parent()
            && let Some(index) = rows
                .iter()
                .position(|row| row.directory && row.path == parent)
        {
            self.selected = index;
        }
        self.load_preview();
    }

    pub fn notice(&mut self, message: impl Into<String>, error: bool) {
        self.message = message.into();
        self.error = error;
    }

    pub fn load_preview(&mut self) {
        if self.branch_list() {
            self.generation += 1;
            self.preview = None;
            self.preview_loading = false;
            self.sync_branches(false, false);
            return;
        }
        if self.tab == Tab::Files && self.file_tree_focus {
            self.sync_repository_history();
            return;
        }
        self.selected = self.selected.min(self.item_count().saturating_sub(1));
        self.request_preview(self.selection());
    }

    fn request_preview(&mut self, selection: Option<Selection>) {
        if self.fullscreen && !Self::file_preview(selection.as_ref()) {
            self.fullscreen = false;
            self.list_focus = true;
        }
        self.generation += 1;
        self.preview = None;
        self.scroll = 0;
        self.horizontal = 0;
        self.chunk = 0;
        self.whole_sources = false;
        self.preview_loading = self
            .file_history
            .as_ref()
            .is_some_and(|h| h.entries.is_none())
            || self.history.as_ref().is_some_and(|h| h.details.is_none())
            || self
                .stash
                .as_ref()
                .is_some_and(|stash| stash.details.is_none());
        if let Some(selection) = selection {
            self.preview_loading = true;
            let _ = self
                .sender
                .send(Request::Preview(self.generation, selection));
        }
    }

    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(response) = self.receiver.try_recv() {
            changed = true;
            match response {
                Response::Snapshot(result, message, automatic) => {
                    if automatic {
                        self.watch_refreshing = false;
                        if self.busy || !matches!(self.mode, Mode::Normal) {
                            self.watch_pending = true;
                            continue;
                        }
                    } else {
                        self.busy = false;
                    }
                    // A successful write must not be retried if the following refresh fails.
                    if self.committing && message.is_some() {
                        self.mode = Mode::Normal;
                        self.committing = false;
                    }
                    match result {
                        Ok(snapshot) => {
                            let branch_before = self
                                .selected_branch()
                                .map(|branch| branch.reference.clone());
                            let branch_group = self
                                .branch_rows()
                                .get(self.branches.selected)
                                .copied()
                                .filter(|row| !matches!(row, crate::branches::Row::Branch(_)));
                            let head_changed = self.snapshot.as_ref().and_then(|s| s.head.as_ref())
                                != snapshot.head.as_ref();
                            let previous = self.selection();
                            let change_anchors: Vec<_> = [Tab::Changes, Tab::Staged]
                                .into_iter()
                                .filter(|tab| *tab != self.tab)
                                .map(|tab| {
                                    let selected =
                                        self.change_panes[usize::from(tab == Tab::Staged)].selected;
                                    let anchor = self
                                        .change_rows(tab)
                                        .get(selected)
                                        .map(|row| (row.path.clone(), row.directory));
                                    (tab, anchor)
                                })
                                .collect();
                            self.snapshot = Some(snapshot);
                            for (tab, anchor) in change_anchors {
                                let rows = self.change_rows(tab);
                                let state = &mut self.change_panes[usize::from(tab == Tab::Staged)];
                                state.selected = anchor
                                    .and_then(|(path, directory)| {
                                        rows.iter()
                                            .position(|row| {
                                                row.path == path && row.directory == directory
                                            })
                                            .or_else(|| {
                                                path.ancestors().skip(1).find_map(|parent| {
                                                    rows.iter().position(|row| {
                                                        row.directory && row.path == parent
                                                    })
                                                })
                                            })
                                    })
                                    .unwrap_or(state.selected)
                                    .min(rows.len().saturating_sub(1));
                            }
                            if self.tab == Tab::Branches {
                                if message.as_deref() == Some("Branch switched") {
                                    self.branches.filter.clear();
                                    self.branches.state = ListState::default();
                                }
                                self.restore_branch(
                                    if message.as_deref() == Some("Branch switched") {
                                        None
                                    } else {
                                        branch_before
                                    },
                                );
                                if message.as_deref() != Some("Branch switched")
                                    && let Some(group) = branch_group
                                    && let Some(index) =
                                        self.branch_rows().iter().position(|row| *row == group)
                                {
                                    self.branches.selected = index;
                                }
                            }
                            if let Some(stash) = &mut self.stash {
                                stash.item.reference = self
                                    .snapshot
                                    .as_ref()
                                    .and_then(|snapshot| {
                                        snapshot
                                            .stashes
                                            .iter()
                                            .find(|item| item.id == stash.item.id)
                                    })
                                    .map_or_else(
                                        || "Dropped stash".into(),
                                        |item| item.reference.clone(),
                                    );
                            }
                            if let Some(previous) = previous {
                                match previous {
                                    Selection::Stash(item) => {
                                        if let Some(index) = self.items().iter().position(|index| {
                                            self.snapshot.as_ref().is_some_and(|snapshot| {
                                                snapshot.stashes[*index].id == item.id
                                            })
                                        }) {
                                            self.selected = index;
                                        }
                                    }
                                    Selection::Commit(id) => {
                                        if let Some(position) = self.items().iter().position(|i| {
                                            self.commit_at(*i).is_some_and(|commit| commit.id == id)
                                        }) {
                                            if self.tab == Tab::Files && self.file_tree_focus {
                                                self.repository_selected = position;
                                            } else {
                                                self.selected = position;
                                            }
                                        }
                                    }
                                    selection => {
                                        let (path, directory) = match selection {
                                            Selection::File(file, _)
                                            | Selection::RepositoryFile(file)
                                            | Selection::StashFile { file, .. }
                                            | Selection::CommitFile { file, .. } => {
                                                (file.path, false)
                                            }
                                            Selection::Directory(path, ..)
                                            | Selection::RepositoryDirectory(path, _)
                                            | Selection::StashDirectory { path, .. }
                                            | Selection::CommitDirectory { path, .. } => {
                                                (path, true)
                                            }
                                            _ => unreachable!(),
                                        };
                                        let rows = if self.tab == Tab::Files && self.file_tree_focus
                                        {
                                            self.repository_rows()
                                        } else {
                                            self.tree_rows()
                                        };
                                        if let Some(position) = rows
                                            .iter()
                                            .position(|row| {
                                                row.path == path && row.directory == directory
                                            })
                                            .or_else(|| {
                                                path.ancestors().skip(1).find_map(|parent| {
                                                    rows.iter().position(|row| {
                                                        row.directory && row.path == parent
                                                    })
                                                })
                                            })
                                        {
                                            if self.tab == Tab::Files && self.file_tree_focus {
                                                self.repository_selected = position;
                                            } else {
                                                self.selected = position;
                                            }
                                        }
                                    }
                                }
                            }
                            if self.committing {
                                self.mode = Mode::Normal;
                                self.committing = false;
                            }
                            if automatic {
                                if head_changed {
                                    self.reload_file_history();
                                }
                                if self.history.is_none() && self.stash.is_none() {
                                    self.reload_preview();
                                }
                            } else {
                                self.notice(message.unwrap_or_default(), false);
                                self.load_preview();
                            }
                            self.sync_branches(false, false);
                        }
                        Err(error) => {
                            self.committing = false;
                            self.notice(error, true);
                        }
                    }
                }
                Response::Preview(id, result) if id == self.generation => {
                    self.preview_loading = false;
                    match result {
                        Ok(preview) => {
                            self.chunk = self.chunk.min(
                                preview
                                    .conflict
                                    .as_ref()
                                    .map_or(0, |conflict| conflict.chunks.len().saturating_sub(1)),
                            );
                            self.preview = Some(preview);
                            self.scroll = self.scroll.min(self.max_scroll());
                        }
                        Err(error) => self.notice(error, true),
                    }
                }
                Response::CommitFiles(token, result)
                    if self
                        .history
                        .as_ref()
                        .is_some_and(|history| history.token == token) =>
                {
                    match result {
                        Ok(details) => {
                            if let Some(history) = &mut self.history {
                                history.details = Some(details);
                            }
                            self.notice("Commit files · Esc returns to commits", false);
                            self.load_preview();
                        }
                        Err(error) => {
                            self.leave_history();
                            self.notice(error, true);
                        }
                    }
                }
                Response::FileHistory(token, result)
                    if self
                        .file_history
                        .as_ref()
                        .is_some_and(|view| view.token == token) =>
                {
                    match result {
                        Ok(entries) => {
                            let count = entries.len();
                            let selected_id = self
                                .items()
                                .get(self.selected)
                                .and_then(|index| self.commit_at(*index))
                                .map(|commit| commit.id.clone());
                            let refreshing = self
                                .file_history
                                .as_ref()
                                .is_some_and(|view| view.refreshing);
                            if let Some(view) = &mut self.file_history {
                                view.entries = Some(entries);
                                view.refreshing = false;
                            }
                            if refreshing {
                                self.selected = selected_id
                                    .and_then(|id| {
                                        self.items().iter().position(|index| {
                                            self.commit_at(*index)
                                                .is_some_and(|commit| commit.id == id)
                                        })
                                    })
                                    .unwrap_or(0);
                                self.reload_preview();
                            } else if self.tab == Tab::Files && self.file_tree_focus {
                                self.notice(format!("{count} commits · Tab switches panes"), false);
                                self.request_preview(None);
                            } else {
                                self.notice(format!("{count} commits · Tab switches panes"), false);
                                self.request_preview(self.file_history_selection());
                            }
                        }
                        Err(error) => {
                            self.leave_file_history();
                            self.notice(error, true);
                        }
                    }
                }
                Response::StashFiles(token, result)
                    if self
                        .stash
                        .as_ref()
                        .is_some_and(|stash| stash.token == token) =>
                {
                    match result {
                        Ok(details) => {
                            if let Some(stash) = &mut self.stash {
                                stash.details = Some(details);
                            }
                            self.notice("Stash files · Esc returns to stashes", false);
                            self.load_preview();
                        }
                        Err(error) => {
                            self.leave_stash();
                            self.notice(error, true);
                        }
                    }
                }
                Response::BranchGraph(token, result) if self.branches.token == token => {
                    self.apply_graph(result);
                }
                Response::Failed(error) => {
                    self.busy = false;
                    self.committing = false;
                    self.notice(error, true);
                }
                _ => {}
            }
        }
        if self.watch_pending
            && !self.watch_refreshing
            && !self.busy
            && !self.preview_loading
            && matches!(self.mode, Mode::Normal)
        {
            self.watch_pending = false;
            self.watch_refreshing = true;
            let _ = self.sender.send(Request::AutoRefresh);
        }
        changed
    }

    pub fn repository_changed(&mut self) {
        self.watch_pending = true;
    }

    fn reload_preview(&mut self) {
        if self.branch_list() {
            self.sync_branches(false, false);
            return;
        }
        if self.tab == Tab::Files && self.file_tree_focus {
            self.sync_repository_history();
            return;
        }
        self.selected = self.selected.min(self.item_count().saturating_sub(1));
        if self.fullscreen && !Self::file_preview(self.selection().as_ref()) {
            self.fullscreen = false;
            self.list_focus = true;
            self.load_preview();
            return;
        }
        if let Some(selection) = self.selection() {
            self.generation += 1;
            self.preview_loading = true;
            let _ = self
                .sender
                .send(Request::Preview(self.generation, selection));
        } else {
            self.load_preview();
        }
    }

    fn reload_file_history(&mut self) {
        if let Some(view) = &mut self.file_history
            && view.start.is_none()
            && view.entries.is_some()
        {
            self.history_generation += 1;
            view.token = self.history_generation;
            view.refreshing = true;
            let _ = self.sender.send(Request::FileHistory(
                view.token,
                view.query_path.clone(),
                None,
            ));
        }
    }

    fn move_by(&mut self, amount: isize) {
        if self.branch_list() {
            self.branches.selected = self
                .branches
                .selected
                .saturating_add_signed(amount)
                .min(self.branch_rows().len().saturating_sub(1));
            self.sync_branches(false, false);
            return;
        }
        if self.tab == Tab::Files && self.file_tree_focus {
            let next = self
                .repository_selected
                .saturating_add_signed(amount)
                .min(self.repository_rows().len().saturating_sub(1));
            if next != self.repository_selected {
                self.repository_selected = next;
                self.sync_repository_history();
            }
            return;
        }
        if self.list_focus {
            let next = self
                .selected
                .saturating_add_signed(amount)
                .min(self.item_count().saturating_sub(1));
            if next != self.selected {
                self.selected = next;
                self.load_preview();
            }
            self.more_branches();
        } else {
            self.scroll = self
                .scroll
                .saturating_add_signed(amount)
                .min(self.max_scroll());
        }
    }

    pub fn max_scroll(&self) -> usize {
        let Some(preview) = &self.preview else {
            return 0;
        };
        if let Some(conflict) = &preview.conflict {
            let start = conflict.chunks.get(self.chunk).map_or(0, |c| c.lines.start);
            let styled = preview.styled();
            let sources = styled
                .sources
                .as_ref()
                .expect("Conflict sources are prepared");
            styled
                .lines
                .len()
                .saturating_sub(start)
                .max(sources.ours.len())
                .max(sources.theirs.len())
                .max(sources.base.len())
                .saturating_sub(1)
        } else {
            preview.styled().lines.len().saturating_sub(1)
        }
    }

    fn file_preview(selection: Option<&Selection>) -> bool {
        matches!(
            selection,
            Some(Selection::File(..) | Selection::CommitFile { .. } | Selection::StashFile { .. })
        )
    }

    fn open_fullscreen(&mut self) {
        if Self::file_preview(self.selection().as_ref()) {
            self.fullscreen = true;
            self.list_focus = false;
        }
    }

    pub fn refresh(&mut self) {
        if self.tab == Tab::Branches && self.history.is_none() {
            self.sync_branches(true, false);
        }
        if let Some(view) = &self.file_history {
            if view.entries.is_some() {
                let tree_focus = self.file_tree_focus;
                let list_focus = self.list_focus;
                self.open_file_history(
                    view.path.clone(),
                    view.query_path.clone(),
                    view.start.clone(),
                );
                self.file_tree_focus = tree_focus;
                self.list_focus = list_focus;
            }
            return;
        }
        if !self.busy {
            self.busy = true;
            let _ = self.sender.send(Request::Refresh);
        }
    }

    pub fn event(&mut self, event: Event) -> Action {
        match event {
            Event::Paste(text) => {
                match &mut self.mode {
                    Mode::Commit(input) if !self.committing => input.insert(&text, true),
                    Mode::Search(input) => {
                        input.insert(&text, false);
                        let filter = input.text.clone();
                        self.update_filter(filter);
                    }
                    _ => {}
                }
                Action::None
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => self.key(key),
            _ => Action::None,
        }
    }

    fn key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            return Action::Quit;
        }
        match &mut self.mode {
            Mode::Help => {
                if matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')
                ) {
                    self.mode = Mode::Normal;
                }
                return Action::None;
            }
            Mode::Commit(input) => {
                if self.committing {
                    return Action::None;
                }
                if key.code == KeyCode::Esc {
                    self.mode = Mode::Normal;
                } else if ctrl && key.code == KeyCode::Char('s') {
                    if !input.text.trim().is_empty() {
                        let _ = self.sender.send(Request::Commit(input.text.clone()));
                        self.busy = true;
                        self.committing = true;
                        self.notice("Creating commit…", false);
                    }
                } else {
                    input.key(key, true);
                }
                return Action::None;
            }
            Mode::Search(input) => {
                match key.code {
                    KeyCode::Esc => {
                        self.mode = Mode::Normal;
                        self.update_filter(String::new());
                    }
                    KeyCode::Enter => self.mode = Mode::Normal,
                    _ => {
                        input.key(key, false);
                        let filter = input.text.clone();
                        if filter != self.current_filter() {
                            self.update_filter(filter);
                        }
                    }
                }
                return Action::None;
            }
            Mode::Normal => {}
        }

        let was_g = self.pending_g;
        self.pending_g = false;
        if ctrl {
            match key.code {
                KeyCode::Char('l' | 'h') => {
                    let index = Tab::ALL
                        .iter()
                        .position(|tab| *tab == self.main_tab())
                        .unwrap_or(0);
                    let offset = if key.code == KeyCode::Char('l') {
                        1
                    } else {
                        Tab::ALL.len() - 1
                    };
                    self.switch_tab(Tab::ALL[(index + offset) % Tab::ALL.len()]);
                }
                KeyCode::Char('d') => self.move_by((self.page_height / 2).max(1) as isize),
                KeyCode::Char('u') => self.move_by(-((self.page_height / 2).max(1) as isize)),
                KeyCode::Char('j') => self.focus_changes(Tab::Staged, false),
                KeyCode::Char('k') => self.focus_changes(Tab::Changes, false),
                _ => {}
            }
            return Action::None;
        }
        match key.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Esc
            | KeyCode::Backspace
            | KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Char('h')
            | KeyCode::Left
                if self.fullscreen =>
            {
                self.fullscreen = false;
                self.list_focus = true;
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right if self.fullscreen => {}
            KeyCode::Char('1' | '2' | '3' | '4') => {
                let tab = match key.code {
                    KeyCode::Char('1') => Tab::Changes,
                    KeyCode::Char('2') => Tab::Branches,
                    KeyCode::Char('3') => Tab::Files,
                    _ => Tab::Stash,
                };
                self.switch_tab(tab);
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_step(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_step(-1),
            KeyCode::Enter | KeyCode::Char('l' | ' ') | KeyCode::Right if self.branch_list() => {
                self.branch_key(
                    true,
                    matches!(key.code, KeyCode::Enter | KeyCode::Char(' ')),
                );
            }
            KeyCode::Tab | KeyCode::BackTab if self.branch_list() => self.branch_key(true, false),
            KeyCode::Char('h') | KeyCode::Left if self.branch_list() => {
                self.branch_key(false, false)
            }
            KeyCode::Char('h') | KeyCode::Left if self.branch_graph_view() => {
                if self.list_focus {
                    self.leave_branches_graph();
                } else {
                    self.list_focus = true;
                }
            }
            KeyCode::Char('f' | 'F') => self.open_selected_file_history(),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right
                if self.tab == Tab::Files && self.file_tree_focus =>
            {
                self.repository_key(true, key.code == KeyCode::Enter)
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right if self.tab == Tab::Files => {
                self.open_fullscreen()
            }
            KeyCode::Char('h') | KeyCode::Left if self.tab == Tab::Files => {
                if self.file_tree_focus {
                    self.repository_key(false, false);
                } else if self.list_focus {
                    self.file_tree_focus = true;
                } else {
                    self.list_focus = true;
                }
            }
            KeyCode::Tab | KeyCode::BackTab if self.tab == Tab::Files => {
                if self.file_tree_focus {
                    self.enter_file_history();
                } else {
                    self.open_fullscreen();
                }
            }
            KeyCode::Char(' ') if self.tab == Tab::Files && self.file_tree_focus => {
                self.repository_key(true, true)
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right
                if self.list_focus && self.stash_list() =>
            {
                self.open_stash()
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right
                if self.list_focus && self.commit_list() =>
            {
                self.open_history()
            }
            KeyCode::Char('h') | KeyCode::Left => self.tree_key(false, false),
            KeyCode::Char('l') | KeyCode::Right if self.list_focus => self.tree_key(true, false),
            KeyCode::Char('l') | KeyCode::Right => {}
            KeyCode::Enter | KeyCode::Char(' ')
                if self.list_focus && !self.commit_list() && !self.stash_list() =>
            {
                self.tree_key(true, true)
            }
            KeyCode::Enter => self.list_focus = false,
            KeyCode::Tab | KeyCode::BackTab => {
                if self.list_focus && Self::file_preview(self.selection().as_ref()) {
                    self.open_fullscreen();
                } else {
                    self.list_focus = !self.list_focus;
                }
            }
            KeyCode::Char('g') if was_g => {
                if self.branch_list() {
                    self.branches.selected = 0;
                    self.sync_branches(false, false);
                } else if self.tab == Tab::Files && self.file_tree_focus {
                    self.repository_selected = 0;
                    self.sync_repository_history();
                } else if self.list_focus {
                    self.selected = 0;
                    self.load_preview();
                } else {
                    self.scroll = 0;
                    self.horizontal = 0;
                }
            }
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => {
                if self.branch_list() {
                    self.branches.selected = self.branch_rows().len().saturating_sub(1);
                    self.sync_branches(false, false);
                } else if self.tab == Tab::Files && self.file_tree_focus {
                    self.repository_selected = self.repository_rows().len().saturating_sub(1);
                    self.sync_repository_history();
                } else if self.list_focus {
                    self.selected = self.item_count().saturating_sub(1);
                    self.load_preview();
                    self.more_branches();
                } else {
                    self.scroll = self.max_scroll();
                }
            }
            KeyCode::PageDown => self.move_by(self.page_height as isize),
            KeyCode::PageUp => self.move_by(-(self.page_height as isize)),
            KeyCode::Char('H') => self.horizontal = self.horizontal.saturating_sub(8),
            KeyCode::Char('L') => self.horizontal = self.horizontal.saturating_add(8),
            KeyCode::Char(']') | KeyCode::Char('[') => {
                if let Some(conflict) = self.preview.as_ref().and_then(|p| p.conflict.as_ref()) {
                    let amount = if key.code == KeyCode::Char(']') {
                        1
                    } else {
                        -1
                    };
                    self.chunk = self
                        .chunk
                        .saturating_add_signed(amount)
                        .min(conflict.chunks.len().saturating_sub(1));
                    self.scroll = 0;
                }
            }
            KeyCode::Char('b') => self.base = !self.base,
            KeyCode::Char('v') => {
                self.whole_sources = !self.whole_sources;
                self.scroll = 0;
            }
            KeyCode::Char('/') if !self.fullscreen => {
                let mut input = Input::default();
                input.insert(self.current_filter(), false);
                self.mode = Mode::Search(input);
            }
            KeyCode::Esc | KeyCode::Backspace if self.tab == Tab::Files => {
                if !self.file_tree_focus {
                    self.file_tree_focus = true;
                    self.list_focus = true;
                } else if self
                    .file_history
                    .as_ref()
                    .is_some_and(|view| view.return_tab != Tab::Files)
                {
                    self.leave_file_history();
                } else {
                    self.update_filter(String::new());
                }
            }
            KeyCode::Esc | KeyCode::Backspace if self.stash.is_some() => self.leave_stash(),
            KeyCode::Esc | KeyCode::Backspace if self.history.is_some() => {
                self.leave_history();
            }
            KeyCode::Esc | KeyCode::Backspace if self.branch_graph_view() => {
                self.leave_branches_graph()
            }
            KeyCode::Esc | KeyCode::Backspace if self.branch_list() => {
                self.update_filter(String::new())
            }
            KeyCode::Esc => {
                self.filter.clear();
                self.list_focus = true;
                self.load_preview();
            }
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('s') if self.branch_list() => self.switch_selected_branch(),
            KeyCode::Char('s' | 'u' | 'c' | 'e')
                if matches!(self.tab, Tab::Files | Tab::Stash | Tab::Branches) =>
            {
                self.notice("History is read-only", false);
            }
            KeyCode::Char('s' | 'u') if !self.busy => {
                let selection = match self.selection() {
                    Some(Selection::File(file, staged)) => Some((vec![file], staged)),
                    Some(Selection::Directory(_, files, staged)) => Some((files, staged)),
                    _ => None,
                };
                if let Some((files, staged)) = selection
                    && !files.is_empty()
                {
                    if key.code == KeyCode::Char('s') && !staged {
                        let _ = self.sender.send(Request::Stage(files));
                        self.busy = true;
                    } else if key.code == KeyCode::Char('u') && staged {
                        let _ = self.sender.send(Request::Unstage(files));
                        self.busy = true;
                    }
                }
            }
            KeyCode::Char('c') if !self.busy => {
                if let Some(snapshot) = &self.snapshot {
                    if snapshot.files.iter().any(|f| f.conflicted()) {
                        self.notice("Resolve and stage all conflicts before committing", true);
                    } else if snapshot.files.iter().any(|f| f.staged())
                        || !snapshot.operation.is_empty()
                    {
                        self.mode = Mode::Commit(Input::default());
                    } else {
                        self.notice("Stage changes before committing", true);
                    }
                }
            }
            KeyCode::Char('e') if !self.busy => {
                if let Some(Selection::File(file, _)) = self.selection() {
                    return Action::Editor(file.path);
                }
            }
            _ => {}
        }
        Action::None
    }
}
