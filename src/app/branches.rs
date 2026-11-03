use super::*;
use crate::branches::{Branch, Graph, PAGE, Row};

impl App {
    pub fn branch_list(&self) -> bool {
        self.tab == Tab::Branches && !self.branches.viewing && self.history.is_none()
    }

    pub fn branch_graph_view(&self) -> bool {
        self.tab == Tab::Branches && self.branches.viewing && self.history.is_none()
    }

    pub fn branch_rows(&self) -> Vec<Row> {
        self.branches.rows(
            self.snapshot
                .as_ref()
                .map_or(&[], |snapshot| &snapshot.branches),
        )
    }

    pub fn selected_branch(&self) -> Option<&Branch> {
        let Row::Branch(index) = *self.branch_rows().get(self.branches.selected)? else {
            return None;
        };
        self.snapshot.as_ref()?.branches.get(index)
    }

    pub(super) fn restore_branch(&mut self, reference: Option<String>) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let rows = self.branch_rows();
        self.branches.selected = reference
            .and_then(|reference| {
                rows.iter().position(|row| {
            matches!(row, Row::Branch(index) if snapshot.branches[*index].reference == reference)
        })
            })
            .or_else(|| {
                rows.iter().position(
                    |row| matches!(row, Row::Branch(index) if snapshot.branches[*index].current),
                )
            })
            .or_else(|| rows.iter().position(|row| matches!(row, Row::Branch(_))))
            .unwrap_or(0);
    }

    pub(super) fn sync_branches(&mut self, force: bool, more: bool) {
        if self.tab != Tab::Branches || self.history.is_some() {
            return;
        }
        let Some(branch) = self.selected_branch().cloned() else {
            return;
        };
        let snapshot = self.snapshot.as_ref().unwrap();
        let mut roots = vec![branch.tip.clone()];
        if let Some(head) = snapshot.commits.first()
            && !roots.contains(&head.id)
        {
            roots.push(head.id.clone());
        }
        let same = self.branches.reference.as_deref() == Some(&branch.reference);
        if !force
            && !more
            && same
            && self.branches.roots == roots
            && (self.branches.graph.is_some() || self.branches.loading)
        {
            return;
        }
        if self.branches.loading && more {
            return;
        }
        let limit = self
            .branches
            .graph
            .as_ref()
            .filter(|_| same)
            .map_or(PAGE, |graph| graph.limit + if more { PAGE } else { 0 });
        if !same {
            self.branches.graph = None;
            self.selected = 0;
            self.filter.clear();
            self.list_state = ListState::default();
            self.branches.cursor = None;
            self.branches.saved_state = ListState::default();
        }
        self.history_generation += 1;
        self.branches.token = self.history_generation;
        self.branches.reference = Some(branch.reference);
        self.branches.roots = roots.clone();
        self.branches.loading = true;
        let _ = self
            .sender
            .send(Request::BranchGraph(self.branches.token, roots, limit));
    }

    pub(super) fn apply_graph(&mut self, result: Result<Graph, String>) {
        self.branches.loading = false;
        match result {
            Ok(graph) => {
                let selected_id = if self.branch_graph_view() {
                    self.items()
                        .get(self.selected)
                        .and_then(|index| self.commit_at(*index))
                        .map(|commit| commit.id.clone())
                } else {
                    None
                };
                let target =
                    selected_id.or_else(|| self.selected_branch().map(|branch| branch.tip.clone()));
                self.branches.graph = Some(graph);
                if self.branch_graph_view() {
                    self.selected = target
                        .and_then(|id| {
                            self.items().iter().position(|index| {
                                self.commit_at(*index).is_some_and(|commit| commit.id == id)
                            })
                        })
                        .unwrap_or(0);
                    self.reload_preview();
                    self.more_branches();
                }
            }
            Err(error) => self.notice(error, true),
        }
    }

    pub(super) fn branch_key(&mut self, open: bool, toggle: bool) {
        match self.branch_rows().get(self.branches.selected).copied() {
            Some(Row::Remote) => {
                self.branches.remote_open = if toggle {
                    !self.branches.remote_open
                } else {
                    open
                };
            }
            Some(Row::Local) if open => self.move_by(1),
            Some(Row::Branch(index)) => {
                if open {
                    self.sync_branches(false, false);
                    self.branches.viewing = true;
                    self.list_focus = true;
                    let target = self
                        .branches
                        .cursor
                        .as_ref()
                        .unwrap_or(&self.snapshot.as_ref().unwrap().branches[index].tip);
                    self.selected = self
                        .items()
                        .iter()
                        .position(|index| {
                            self.commit_at(*index)
                                .is_some_and(|commit| &commit.id == target)
                        })
                        .unwrap_or(0);
                    self.list_state = std::mem::take(&mut self.branches.saved_state);
                    self.load_preview();
                } else if self.snapshot.as_ref().unwrap().branches[index].remote {
                    self.branches.selected = self
                        .branch_rows()
                        .iter()
                        .position(|row| matches!(row, Row::Remote))
                        .unwrap_or(0);
                    self.branches.remote_open = false;
                } else {
                    self.branches.selected = 0;
                }
            }
            _ => {}
        }
    }

    pub(super) fn leave_branches_graph(&mut self) {
        self.branches.cursor = self
            .items()
            .get(self.selected)
            .and_then(|index| self.commit_at(*index))
            .map(|commit| commit.id.clone());
        self.branches.saved_state = std::mem::take(&mut self.list_state);
        self.branches.viewing = false;
        self.list_focus = true;
        self.generation += 1;
        self.preview = None;
        self.preview_loading = false;
        self.sync_branches(false, false);
    }

    pub(super) fn more_branches(&mut self) {
        if self.branch_graph_view()
            && self.filter.is_empty()
            && self.branches.graph.as_ref().is_some_and(|graph| {
                graph.more
                    && self.selected.saturating_add(self.page_height.max(4)) >= graph.entries.len()
            })
        {
            self.sync_branches(false, true);
        }
    }

    pub(super) fn switch_selected_branch(&mut self) {
        if self.busy {
            return;
        }
        if let Some(branch) = self.selected_branch() {
            let _ = self
                .sender
                .send(Request::SwitchBranch(branch.reference.clone()));
            self.busy = true;
            self.notice("Switching branch…", false);
        }
    }
}
