use std::{
    borrow::Cow,
    collections::HashSet,
    path::PathBuf,
    sync::mpsc::{Receiver, Sender},
};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;

use crate::{
    git::{
        self, Commit, CommitDetails, File, FileCommit, Preview, Repo, Request, Response, Selection,
        Snapshot,
    },
    input::Input,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Changes,
    Staged,
    Log,
    Files,
}

impl Tab {
    pub const ALL: [Self; 4] = [Self::Changes, Self::Staged, Self::Log, Self::Files];
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
    pub file_history: Option<FileHistory>,
    pub file_tree_focus: bool,
    pub repository_selected: usize,
    pub repository_filter: String,
    pub repository_state: ListState,
    collapsed: [HashSet<PathBuf>; 4],
    history_generation: u64,
    sender: Sender<Request>,
    receiver: Receiver<Response>,
    generation: u64,
    pending_g: bool,
    committing: bool,
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
            file_history: None,
            file_tree_focus: false,
            repository_selected: 0,
            repository_filter: String::new(),
            repository_state: ListState::default(),
            collapsed: std::array::from_fn(|_| HashSet::new()),
            history_generation: 0,
            sender,
            receiver,
            generation: 0,
            pending_g: false,
            committing: false,
        };
        let _ = app.sender.send(Request::Refresh);
        app
    }

    pub fn commit_list(&self) -> bool {
        self.file_history.is_some() || (self.tab == Tab::Log && self.history.is_none())
    }

    pub fn commit_at(&self, index: usize) -> Option<&Commit> {
        if let Some(history) = &self.file_history {
            history
                .entries
                .as_ref()?
                .get(index)
                .map(|entry| &entry.commit)
        } else {
            self.snapshot.as_ref()?.commits.get(index)
        }
    }

    fn commit_count(&self) -> usize {
        if let Some(history) = &self.file_history {
            history.entries.as_ref().map_or(0, Vec::len)
        } else {
            self.snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.commits.len())
        }
    }

    pub fn files(&self) -> &[File] {
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

    pub fn file_status(&self, file: &File) -> &'static str {
        file.status(if self.tab == Tab::Files {
            file.worktree == ' '
        } else {
            self.tab != Tab::Changes
        })
    }

    fn tree_index(&self) -> usize {
        match self.tab {
            Tab::Changes => 0,
            Tab::Staged => 1,
            Tab::Log => 2,
            Tab::Files => 3,
        }
    }

    pub fn items(&self) -> Vec<usize> {
        let query = self.filter.to_lowercase();
        if self.commit_list() {
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
        if self.tab == Tab::Files && (self.file_tree_focus || self.file_history.is_none()) {
            return self.repository_selection();
        }
        if self.file_history.is_some() {
            return self.file_history_selection();
        }
        if self.commit_list() {
            let snapshot = self.snapshot.as_ref()?;
            return self
                .items()
                .get(self.selected)
                .map(|i| Selection::Commit(snapshot.commits[*i].id.clone()));
        }
        let rows = self.tree_rows();
        let row = rows.get(self.selected)?;
        let files = self.files();
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
        if self.commit_list()
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
        if self.commit_list() {
            self.items().len()
        } else {
            self.tree_rows().len()
        }
    }

    fn switch_tab(&mut self, tab: Tab) {
        self.history = None;
        self.file_history = None;
        self.tab = tab;
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
        crate::tree::rows(&files, &indices, &self.collapsed[3])
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
                self.collapsed[3].insert(row.path.clone());
            } else {
                self.collapsed[3].remove(&row.path);
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
        if self.tab == Tab::Files && self.file_tree_focus {
            &self.repository_filter
        } else {
            &self.filter
        }
    }

    fn update_filter(&mut self, filter: String) {
        if self.tab == Tab::Files && self.file_tree_focus {
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
                    self.collapsed[3].remove(&parent);
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
        let from_repository = self.tab == Tab::Files;
        if !from_repository {
            self.repository_filter.clear();
            self.repository_state = ListState::default();
        }
        for parent in path.ancestors().skip(1) {
            self.collapsed[3].remove(parent);
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
        });
        view.path = path;
        view.query_path = query_path.clone();
        view.start = start.clone();
        view.entries = None;
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

    fn open_history(&mut self) {
        let Some(Selection::Commit(id)) = self.selection() else {
            return;
        };
        let Some(commit) = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.commits.iter().find(|c| c.id == id))
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
        self.collapsed[2].clear();
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
                    self.snapshot
                        .as_ref()
                        .is_some_and(|snapshot| snapshot.commits[*index].id == history.commit.id)
                })
                .unwrap_or(history.return_selected);
            self.list_state = history.return_list_state;
            self.list_focus = true;
            self.load_preview();
            self.notice("History · Enter opens commit files", false);
        }
    }

    fn reveal_matches(&mut self) {
        if self.filter.is_empty() || self.commit_list() {
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
                self.list_focus = false;
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
        if self.tab == Tab::Files && self.file_tree_focus {
            self.sync_repository_history();
            return;
        }
        self.selected = self.selected.min(self.item_count().saturating_sub(1));
        self.request_preview(self.selection());
    }

    fn request_preview(&mut self, selection: Option<Selection>) {
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
            || self.history.as_ref().is_some_and(|h| h.details.is_none());
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
                Response::Snapshot(result, message) => {
                    self.busy = false;
                    // A successful write must not be retried if the following refresh fails.
                    if self.committing && message.is_some() {
                        self.mode = Mode::Normal;
                        self.committing = false;
                    }
                    match result {
                        Ok(snapshot) => {
                            let previous = self.selection();
                            self.snapshot = Some(snapshot);
                            if let Some(previous) = previous {
                                match previous {
                                    Selection::Commit(id) => {
                                        if let Some(position) = self.items().iter().position(|i| {
                                            self.snapshot
                                                .as_ref()
                                                .is_some_and(|s| s.commits[*i].id == id)
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
                                            | Selection::CommitFile { file, .. } => {
                                                (file.path, false)
                                            }
                                            Selection::Directory(path, ..)
                                            | Selection::RepositoryDirectory(path, _)
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
                                            self.selected = position;
                                        }
                                    }
                                }
                            }
                            if self.committing {
                                self.mode = Mode::Normal;
                                self.committing = false;
                            }
                            self.notice(
                                message.unwrap_or_else(|| "Repository refreshed".into()),
                                false,
                            );
                            self.load_preview();
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
                        Ok(preview) => self.preview = Some(preview),
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
                            if let Some(view) = &mut self.file_history {
                                view.entries = Some(entries);
                            }
                            self.notice(format!("{count} commits · Tab switches panes"), false);
                            if self.tab == Tab::Files && self.file_tree_focus {
                                self.request_preview(None);
                            } else {
                                self.request_preview(self.file_history_selection());
                            }
                        }
                        Err(error) => {
                            self.leave_file_history();
                            self.notice(error, true);
                        }
                    }
                }
                Response::Failed(error) => {
                    self.busy = false;
                    self.committing = false;
                    self.notice(error, true);
                }
                _ => {}
            }
        }
        changed
    }

    fn move_by(&mut self, amount: isize) {
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
            conflict
                .working
                .lines()
                .count()
                .saturating_sub(start)
                .max(conflict.ours.lines().count())
                .max(conflict.theirs.lines().count())
                .max(conflict.base.lines().count())
                .saturating_sub(1)
                .min(u16::MAX as usize)
        } else {
            preview
                .styled()
                .lines
                .len()
                .saturating_sub(1)
                .min(u16::MAX as usize)
        }
    }

    pub fn refresh(&mut self) {
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
                        .position(|tab| *tab == self.tab)
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
                _ => {}
            }
            return Action::None;
        }
        match key.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('1' | '2' | '3' | '4') => {
                let tab = match key.code {
                    KeyCode::Char('1') => Tab::Changes,
                    KeyCode::Char('2') => Tab::Staged,
                    KeyCode::Char('3') => Tab::Log,
                    _ => Tab::Files,
                };
                self.switch_tab(tab);
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_by(-1),
            KeyCode::Char('f' | 'F') => self.open_selected_file_history(),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right
                if self.tab == Tab::Files && self.file_tree_focus =>
            {
                self.repository_key(true, key.code == KeyCode::Enter)
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right if self.tab == Tab::Files => {
                self.list_focus = false
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
                    self.list_focus = !self.list_focus;
                }
            }
            KeyCode::Char(' ') if self.tab == Tab::Files && self.file_tree_focus => {
                self.repository_key(true, true)
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right
                if self.list_focus && self.commit_list() =>
            {
                self.open_history()
            }
            KeyCode::Char('h') | KeyCode::Left => self.tree_key(false, false),
            KeyCode::Char('l') | KeyCode::Right if self.list_focus => self.tree_key(true, false),
            KeyCode::Char('l') | KeyCode::Right => {}
            KeyCode::Enter | KeyCode::Char(' ') if self.list_focus && !self.commit_list() => {
                self.tree_key(true, true)
            }
            KeyCode::Enter => self.list_focus = false,
            KeyCode::Tab | KeyCode::BackTab => self.list_focus = !self.list_focus,
            KeyCode::Char('g') if was_g => {
                if self.tab == Tab::Files && self.file_tree_focus {
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
                if self.tab == Tab::Files && self.file_tree_focus {
                    self.repository_selected = self.repository_rows().len().saturating_sub(1);
                    self.sync_repository_history();
                } else if self.list_focus {
                    self.selected = self.item_count().saturating_sub(1);
                    self.load_preview();
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
            KeyCode::Char('/') => {
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
            KeyCode::Esc | KeyCode::Backspace if self.history.is_some() => {
                self.leave_history();
            }
            KeyCode::Esc => {
                self.filter.clear();
                self.list_focus = true;
                self.load_preview();
            }
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('s' | 'u' | 'c' | 'e') if matches!(self.tab, Tab::Log | Tab::Files) => {
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
