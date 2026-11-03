use std::{ffi::OsStr, path::PathBuf, sync::Arc};

use anyhow::{Result, ensure};
use ratatui::widgets::ListState;

use crate::git::{Commit, Repo, os_string};

pub const PAGE: usize = 100;

#[derive(Clone)]
pub struct Branch {
    pub reference: String,
    pub name: String,
    pub tip: String,
    pub current: bool,
    pub remote: bool,
    pub upstream: String,
    pub tracking: String,
    pub worktree: Option<PathBuf>,
}

pub struct Entry {
    pub commit: Commit,
    pub parents: Vec<String>,
}

pub struct Graph {
    pub entries: Vec<Entry>,
    pub more: bool,
    pub limit: usize,
    pub comparison: Option<(usize, usize)>,
    pub layout: Arc<crate::graph::Layout>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Local,
    Remote,
    Branch(usize),
}

#[derive(Default)]
pub struct Browser {
    pub viewing: bool,
    pub selected: usize,
    pub filter: String,
    pub state: ListState,
    pub remote_open: bool,
    pub reference: Option<String>,
    pub roots: Vec<String>,
    pub graph: Option<Graph>,
    pub loading: bool,
    pub token: u64,
    pub cursor: Option<String>,
    pub saved_state: ListState,
}

impl Browser {
    pub fn rows(&self, branches: &[Branch]) -> Vec<Row> {
        let query = self.filter.to_lowercase();
        let mut rows = Vec::new();
        for remote in [false, true] {
            let matching: Vec<_> = branches
                .iter()
                .enumerate()
                .filter(|(_, branch)| {
                    branch.remote == remote
                        && format!("{} {}", branch.name, branch.upstream)
                            .to_lowercase()
                            .contains(&query)
                })
                .map(|(index, _)| Row::Branch(index))
                .collect();
            if matching.is_empty() {
                continue;
            }
            rows.push(if remote { Row::Remote } else { Row::Local });
            if !remote || self.remote_open || !query.is_empty() {
                rows.extend(matching);
            }
        }
        rows
    }
}

impl Repo {
    pub fn branches(&self) -> Result<Vec<Branch>> {
        let bytes = self.strings(&["for-each-ref", "--sort=-committerdate",
            "--format=%(refname)%00%(objectname)%00%(HEAD)%00%(upstream)%00%(upstream:track,nobracket)%00%(worktreepath)%00%(symref)%00",
            "refs/heads", "refs/remotes"])?;
        let fields: Vec<_> = bytes.split(|byte| *byte == 0).collect();
        let mut branches = Vec::new();
        for fields in fields.as_chunks::<7>().0 {
            if !fields[6].is_empty() {
                continue;
            }
            let reference =
                String::from_utf8_lossy(fields[0].strip_prefix(b"\n").unwrap_or(fields[0]))
                    .into_owned();
            let remote = reference.starts_with("refs/remotes/");
            let name = short_ref(&reference).to_owned();
            branches.push(Branch {
                reference,
                name,
                remote,
                tip: String::from_utf8_lossy(fields[1]).into_owned(),
                current: fields[2] == b"*",
                upstream: String::from_utf8_lossy(fields[3]).into_owned(),
                tracking: String::from_utf8_lossy(fields[4]).into_owned(),
                worktree: (!fields[5].is_empty()).then(|| PathBuf::from(os_string(fields[5]))),
            });
        }
        if !branches.iter().any(|branch| branch.current)
            && let Ok(bytes) = self.strings(&["rev-parse", "--verify", "-q", "HEAD"])
        {
            branches.push(Branch {
                reference: "HEAD".into(),
                name: "detached HEAD".into(),
                tip: String::from_utf8_lossy(&bytes).trim().to_owned(),
                current: true,
                remote: false,
                upstream: String::new(),
                tracking: String::new(),
                worktree: None,
            });
        }
        branches.sort_by_key(|branch| (!branch.current, branch.remote));
        Ok(branches)
    }

    pub fn branch_graph(&self, roots: &[String], limit: usize) -> Result<Graph> {
        let Some(selected) = roots.first() else {
            return Ok(Graph {
                entries: Vec::new(),
                more: false,
                limit,
                comparison: None,
                layout: Arc::new(crate::graph::Layout::default()),
            });
        };
        let comparison = roots
            .get(1)
            .map(|current| -> Result<_> {
                let range = format!("{selected}...{current}");
                let bytes = self.strings(&["rev-list", "--left-right", "--count", &range])?;
                let counts: Vec<usize> = std::str::from_utf8(&bytes)?
                    .split_whitespace()
                    .map(str::parse)
                    .collect::<Result<_, _>>()?;
                ensure!(counts.len() == 2, "Invalid branch comparison");
                Ok((counts[0], counts[1]))
            })
            .transpose()?;
        let limit_arg = format!("--max-count={}", limit.saturating_add(1));
        let args = [
            "log",
            "-z",
            "--topo-order",
            "--no-color",
            "--no-decorate",
            "--no-show-signature",
            "--format=%H%x00%P%x00%s%x00%an%x00%ar",
            &limit_arg,
            selected,
            "--",
        ];
        let bytes = self.strings(&args)?;
        let fields: Vec<_> = bytes.split(|byte| *byte == 0).collect();
        let mut entries: Vec<Entry> = Vec::new();
        for fields in fields.as_chunks::<5>().0 {
            entries.push(Entry {
                commit: Commit {
                    id: String::from_utf8_lossy(fields[0]).into_owned(),
                    subject: String::from_utf8_lossy(fields[2]).into_owned(),
                    author: String::from_utf8_lossy(fields[3]).into_owned(),
                    age: String::from_utf8_lossy(fields[4]).into_owned(),
                },
                parents: String::from_utf8_lossy(fields[1])
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect(),
            });
        }
        let more = entries.len() > limit;
        entries.truncate(limit);
        let layout = Arc::new(crate::graph::Layout::new(&entries));
        Ok(Graph {
            entries,
            more,
            limit,
            comparison,
            layout,
        })
    }

    pub fn switch_branch(&self, reference: &str) -> Result<()> {
        let branches = self.branches()?;
        let branch = branches.iter().find(|branch| branch.reference == reference);
        let branch = branch.ok_or_else(|| anyhow::anyhow!("Branch no longer exists"))?;
        ensure!(
            branch.current || branch.worktree.is_none(),
            "Branch is used by worktree: {}",
            branch
                .worktree
                .as_deref()
                .unwrap_or_else(|| std::path::Path::new(""))
                .display()
        );
        if branch.remote {
            let remotes = self.strings(&["remote"])?;
            let remotes = std::str::from_utf8(&remotes)?;
            let prefix = remotes
                .lines()
                .filter(|remote| branch.name.starts_with(&format!("{remote}/")))
                .max_by_key(|remote| remote.len());
            let prefix =
                prefix.ok_or_else(|| anyhow::anyhow!("Remote configuration is missing"))?;
            let name = &branch.name[prefix.len() + 1..];
            if let Some(local) = branches
                .iter()
                .find(|local| !local.remote && local.name == name)
            {
                ensure!(
                    local.upstream == reference,
                    "Local branch {name} already exists with a different upstream"
                );
                return self.switch_branch(&local.reference);
            }
            self.run(&[
                OsStr::new("switch"),
                OsStr::new("--track"),
                OsStr::new("-c"),
                OsStr::new(name),
                OsStr::new(reference),
            ])?;
        } else if !branch.current {
            self.run(&[
                OsStr::new("switch"),
                OsStr::new("--no-guess"),
                OsStr::new("--"),
                OsStr::new(&branch.name),
            ])?;
        }
        Ok(())
    }
}

pub fn short_ref(reference: &str) -> &str {
    reference
        .strip_prefix("refs/heads/")
        .or_else(|| reference.strip_prefix("refs/remotes/"))
        .unwrap_or(reference)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command};
    use tempfile::TempDir;

    fn fixture() -> (TempDir, Repo) {
        let dir = TempDir::new().unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q", "--initial-branch=main"])
                .arg(dir.path())
                .status()
                .unwrap()
                .success()
        );
        let repo = Repo::open(dir.path()).unwrap();
        repo.strings(&["config", "user.name", "Branch Test"])
            .unwrap();
        repo.strings(&["config", "user.email", "branches@example.com"])
            .unwrap();
        repo.strings(&["config", "commit.gpgsign", "false"])
            .unwrap();
        fs::write(repo.root.join("file"), "base\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("base").unwrap();
        (dir, repo)
    }

    fn head(repo: &Repo) -> String {
        String::from_utf8(repo.strings(&["rev-parse", "HEAD"]).unwrap())
            .unwrap()
            .trim()
            .to_owned()
    }

    #[test]
    fn branch_inventory_preserves_tracking_divergence_symrefs_and_worktree_occupancy() {
        let (_dir, repo) = fixture();
        repo.strings(&["remote", "add", "origin", "."]).unwrap();
        repo.strings(&["switch", "-c", "feature"]).unwrap();
        fs::write(repo.root.join("file"), "feature\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("feature").unwrap();
        repo.strings(&["update-ref", "refs/remotes/origin/main", &head(&repo)])
            .unwrap();
        repo.strings(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ])
        .unwrap();
        repo.strings(&["switch", "main"]).unwrap();
        fs::write(repo.root.join("file"), "main\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("main").unwrap();
        repo.strings(&["branch", "--set-upstream-to=origin/main"])
            .unwrap();
        let linked = TempDir::new().unwrap();
        let path = linked.path().join("linked\nworktree");
        repo.run(&[
            OsStr::new("worktree"),
            OsStr::new("add"),
            path.as_os_str(),
            OsStr::new("feature"),
        ])
        .unwrap();
        let branches = repo.branches().unwrap();
        assert!(branches[0].current);
        assert_eq!(branches[0].upstream, "refs/remotes/origin/main");
        assert!(
            branches[0].tracking.contains("ahead 1") && branches[0].tracking.contains("behind 1")
        );
        assert!(!branches.iter().any(|branch| branch.name == "origin/HEAD"));
        assert!(
            branches
                .iter()
                .any(|branch| branch.remote && branch.name == "origin/main")
        );
        assert_eq!(
            branches
                .iter()
                .find(|branch| branch.name == "feature")
                .unwrap()
                .worktree
                .as_ref(),
            Some(&path)
        );
        let before = head(&repo);
        assert!(
            repo.switch_branch("refs/heads/feature")
                .unwrap_err()
                .to_string()
                .contains("worktree")
        );
        assert_eq!(head(&repo), before);
    }

    #[test]
    fn detached_head_keeps_its_own_history_available_without_named_branches() {
        let (_dir, repo) = fixture();
        repo.strings(&["switch", "--detach"]).unwrap();
        fs::write(repo.root.join("file"), "detached work\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("detached commit").unwrap();
        let tip = head(&repo);
        let refs = repo.strings(&["show-ref"]).unwrap();
        let branches = repo.branches().unwrap();
        assert!(branches[0].current);
        assert_eq!(branches[0].reference, "HEAD");
        assert_eq!(branches[0].tip, tip);
        let graph = repo.branch_graph(&[branches[0].tip.clone()], PAGE).unwrap();
        assert_eq!(graph.entries[0].commit.subject, "detached commit");
        assert_eq!(graph.entries.len(), 2);
        repo.switch_branch("HEAD").unwrap();
        assert_eq!(repo.strings(&["show-ref"]).unwrap(), refs);
        assert_eq!(repo.snapshot().unwrap().head.as_deref(), Some(tip.as_str()));
        repo.strings(&["update-ref", "-d", "refs/heads/main"])
            .unwrap();
        let branches = repo.branches().unwrap();
        assert_eq!(branches.len(), 1);
        assert_eq!(branches[0].reference, "HEAD");
        assert_eq!(
            repo.branch_graph(&[branches[0].tip.clone()], PAGE)
                .unwrap()
                .entries
                .len(),
            2
        );
        repo.strings(&["switch", "-c", "restored"]).unwrap();
        assert!(
            repo.branches()
                .unwrap()
                .iter()
                .all(|branch| branch.reference != "HEAD")
        );
    }

    #[test]
    fn linear_history_has_continuous_edges_and_stops_at_the_root() {
        let (_dir, repo) = fixture();
        for message in ["middle", "latest"] {
            fs::write(repo.root.join("file"), message).unwrap();
            repo.strings(&["add", "."]).unwrap();
            repo.commit(message).unwrap();
        }
        let roots = vec![head(&repo)];
        let graph = repo.branch_graph(&roots, 100).unwrap();
        assert_eq!(graph.entries.len(), 3);
        assert_eq!(graph.layout.edges.len(), 2);
        assert!(
            graph
                .layout
                .edges
                .iter()
                .all(|edge| edge.to == edge.from + 1)
        );
        assert!(graph.entries[2].parents.is_empty());
        let page = repo.branch_graph(&roots, 1).unwrap();
        assert!(page.more);
        assert_eq!(page.layout.edges[0].to, 1);
        assert_eq!(page.layout.nodes[0].lane, graph.layout.nodes[0].lane);
    }

    #[test]
    fn branch_graph_keeps_merge_parents_and_paginates_pinned_roots_without_stash_nodes() {
        let (_dir, repo) = fixture();
        let base = head(&repo);
        repo.strings(&["switch", "-c", "feature"]).unwrap();
        fs::write(repo.root.join("feature"), "feature\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("feature").unwrap();
        let feature = head(&repo);
        repo.strings(&["switch", "main"]).unwrap();
        fs::write(repo.root.join("main"), "main\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("main").unwrap();
        let main = head(&repo);
        repo.strings(&["merge", "--no-ff", "feature", "-m", "merge feature"])
            .unwrap();
        let tip = head(&repo);
        let roots = vec![tip.clone(), feature];
        let page = repo.branch_graph(&roots, 2).unwrap();
        assert!(page.more);
        assert_eq!(
            page.entries[0].parents,
            vec![main.clone(), roots[1].clone()]
        );
        let graph = repo.branch_graph(&roots, 100).unwrap();
        assert!(!graph.more);
        assert_eq!(graph.entries.len(), 4);
        assert_eq!(
            page.entries
                .iter()
                .map(|entry| &entry.commit.id)
                .collect::<Vec<_>>(),
            graph.entries[..2]
                .iter()
                .map(|entry| &entry.commit.id)
                .collect::<Vec<_>>()
        );
        assert!(
            graph
                .entries
                .iter()
                .any(|entry| entry.commit.id == base && entry.parents.is_empty())
        );
        assert_eq!(
            graph
                .layout
                .edges
                .iter()
                .filter(|edge| edge.from == 0)
                .count(),
            2
        );
        assert!(graph.layout.lanes >= 2);
        let selected_only = repo
            .branch_graph(&[roots[1].clone(), tip.clone()], 100)
            .unwrap();
        assert_eq!(selected_only.entries.len(), 2);
        assert!(
            selected_only
                .entries
                .iter()
                .all(|entry| entry.commit.id != tip && entry.commit.id != main)
        );
        assert_eq!(selected_only.comparison, Some((0, 2)));
        fs::write(repo.root.join("file"), "stash only\n").unwrap();
        repo.strings(&["stash", "push", "-m", "hidden from graph"])
            .unwrap();
        let status = repo.strings(&["status", "--porcelain=v1", "-z"]).unwrap();
        let pinned = repo.branch_graph(&roots, 100).unwrap();
        assert_eq!(
            pinned
                .entries
                .iter()
                .map(|entry| &entry.commit.id)
                .collect::<Vec<_>>(),
            graph
                .entries
                .iter()
                .map(|entry| &entry.commit.id)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            repo.strings(&["status", "--porcelain=v1", "-z"]).unwrap(),
            status
        );
        assert_eq!(head(&repo), tip);
    }

    #[test]
    fn switching_preserves_dirty_files_and_creates_remote_tracking_branch_explicitly() {
        let (_dir, repo) = fixture();
        repo.strings(&["switch", "-c", "feature"]).unwrap();
        fs::write(repo.root.join("file"), "feature\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("feature").unwrap();
        let feature = head(&repo);
        repo.strings(&["switch", "main"]).unwrap();
        let main = head(&repo);
        fs::write(repo.root.join("file"), "local work\n").unwrap();
        let status = repo.strings(&["status", "--porcelain=v1", "-z"]).unwrap();
        assert!(repo.switch_branch("refs/heads/feature").is_err());
        assert_eq!(head(&repo), main);
        assert_eq!(
            fs::read_to_string(repo.root.join("file")).unwrap(),
            "local work\n"
        );
        assert_eq!(
            repo.strings(&["status", "--porcelain=v1", "-z"]).unwrap(),
            status
        );
        fs::write(repo.root.join("file"), "base\n").unwrap();
        repo.switch_branch("refs/heads/feature").unwrap();
        assert_eq!(head(&repo), feature);
        repo.strings(&["remote", "add", "team/origin", "."])
            .unwrap();
        repo.strings(&["update-ref", "refs/remotes/team/origin/task", &feature])
            .unwrap();
        repo.switch_branch("refs/remotes/team/origin/task").unwrap();
        let branches = repo.branches().unwrap();
        assert_eq!(branches[0].name, "task");
        assert_eq!(branches[0].upstream, "refs/remotes/team/origin/task");
        repo.switch_branch("refs/remotes/team/origin/task").unwrap();
        assert!(repo.switch_branch("refs/heads/missing").is_err());
    }
}
