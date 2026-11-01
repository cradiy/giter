use std::{
    collections::HashSet,
    path::PathBuf,
    sync::mpsc::{Receiver, Sender},
};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;

use crate::{
    git::{self, Preview, Repo, Request, Response, Selection, Snapshot},
    input::Input,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Changes,
    Staged,
    Log,
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
    collapsed: [HashSet<PathBuf>; 2],
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
            collapsed: [HashSet::new(), HashSet::new()],
            sender,
            receiver,
            generation: 0,
            pending_g: false,
            committing: false,
        };
        let _ = app.sender.send(Request::Refresh);
        app
    }

    pub fn items(&self) -> Vec<usize> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        let query = self.filter.to_lowercase();
        match self.tab {
            Tab::Log => snapshot
                .commits
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    format!("{} {} {}", c.id, c.subject, c.author)
                        .to_lowercase()
                        .contains(&query)
                })
                .map(|(i, _)| i)
                .collect(),
            tab => snapshot
                .files
                .iter()
                .enumerate()
                .filter(|(_, f)| {
                    (if tab == Tab::Changes {
                        f.unstaged()
                    } else {
                        f.staged()
                    }) && f.label().to_lowercase().contains(&query)
                })
                .map(|(i, _)| i)
                .collect(),
        }
    }

    pub fn selection(&self) -> Option<Selection> {
        let snapshot = self.snapshot.as_ref()?;
        if self.tab == Tab::Log {
            return self
                .items()
                .get(self.selected)
                .map(|i| Selection::Commit(snapshot.commits[*i].id.clone()));
        }
        let rows = self.tree_rows();
        let row = rows.get(self.selected)?;
        Some(if row.directory {
            Selection::Directory(
                row.path.clone(),
                row.files
                    .iter()
                    .map(|i| snapshot.files[*i].clone())
                    .collect(),
                self.tab == Tab::Staged,
            )
        } else {
            Selection::File(
                snapshot.files[row.files[0]].clone(),
                self.tab == Tab::Staged,
            )
        })
    }

    pub fn tree_rows(&self) -> Vec<crate::tree::Row> {
        if self.tab == Tab::Log {
            return Vec::new();
        }
        self.snapshot
            .as_ref()
            .map(|snapshot| {
                crate::tree::rows(
                    &snapshot.files,
                    &self.items(),
                    &self.collapsed[(self.tab == Tab::Staged) as usize],
                )
            })
            .unwrap_or_default()
    }

    fn item_count(&self) -> usize {
        if self.tab == Tab::Log {
            self.items().len()
        } else {
            self.tree_rows().len()
        }
    }

    fn switch_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.selected = 0;
        self.filter.clear();
        self.list_focus = true;
        self.list_state = ListState::default();
        self.load_preview();
    }

    fn reveal_matches(&mut self) {
        if self.filter.is_empty() || self.tab == Tab::Log {
            return;
        }
        if let Some(snapshot) = &self.snapshot {
            let indices = self.items();
            let collapsed = &mut self.collapsed[(self.tab == Tab::Staged) as usize];
            for index in indices {
                for parent in snapshot.files[index].path.ancestors().skip(1) {
                    collapsed.remove(parent);
                }
            }
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
        let collapsed = &mut self.collapsed[(self.tab == Tab::Staged) as usize];
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
        self.selected = self.selected.min(self.item_count().saturating_sub(1));
        self.generation += 1;
        self.preview = None;
        self.scroll = 0;
        self.horizontal = 0;
        self.chunk = 0;
        self.whole_sources = false;
        self.preview_loading = false;
        if let Some(selection) = self.selection() {
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
                                            self.selected = position;
                                        }
                                    }
                                    selection => {
                                        let (path, directory) = match selection {
                                            Selection::File(file, _) => (file.path, false),
                                            Selection::Directory(path, ..) => (path, true),
                                            _ => unreachable!(),
                                        };
                                        let rows = self.tree_rows();
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
                .text
                .lines()
                .count()
                .saturating_sub(1)
                .min(u16::MAX as usize)
        }
    }

    pub fn refresh(&mut self) {
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
                        self.filter = input.text.clone();
                        self.selected = 0;
                        self.reveal_matches();
                        self.load_preview();
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
                        self.filter.clear();
                        self.selected = 0;
                        self.load_preview();
                    }
                    KeyCode::Enter => self.mode = Mode::Normal,
                    _ => {
                        input.key(key, false);
                        let filter = input.text.clone();
                        if filter != self.filter {
                            self.filter = filter;
                            self.selected = 0;
                            self.reveal_matches();
                            self.load_preview();
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
                    let next = match (self.tab, key.code) {
                        (Tab::Changes, KeyCode::Char('l')) | (Tab::Log, KeyCode::Char('h')) => {
                            Tab::Staged
                        }
                        (Tab::Staged, KeyCode::Char('l')) | (Tab::Changes, KeyCode::Char('h')) => {
                            Tab::Log
                        }
                        _ => Tab::Changes,
                    };
                    self.switch_tab(next);
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
            KeyCode::Char('1' | '2' | '3') => {
                let tab = match key.code {
                    KeyCode::Char('1') => Tab::Changes,
                    KeyCode::Char('2') => Tab::Staged,
                    _ => Tab::Log,
                };
                self.switch_tab(tab);
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_by(-1),
            KeyCode::Char('h') | KeyCode::Left => self.tree_key(false, false),
            KeyCode::Char('l') | KeyCode::Right if self.list_focus => self.tree_key(true, false),
            KeyCode::Char('l') | KeyCode::Right => {}
            KeyCode::Enter | KeyCode::Char(' ') if self.list_focus && self.tab != Tab::Log => {
                self.tree_key(true, true)
            }
            KeyCode::Enter => self.list_focus = false,
            KeyCode::Tab | KeyCode::BackTab => self.list_focus = !self.list_focus,
            KeyCode::Char('g') if was_g => {
                if self.list_focus {
                    self.selected = 0;
                    self.load_preview();
                } else {
                    self.scroll = 0;
                    self.horizontal = 0;
                }
            }
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => {
                if self.list_focus {
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
                input.insert(&self.filter, false);
                self.mode = Mode::Search(input);
            }
            KeyCode::Esc => {
                self.filter.clear();
                self.list_focus = true;
                self.load_preview();
            }
            KeyCode::Char('r') => self.refresh(),
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
