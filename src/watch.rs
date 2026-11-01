use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher, event::ModifyKind};

use crate::git::Repo;

const CHANGED: u8 = 1;
const DIRECTORIES: u8 = 2;
const QUIET: Duration = Duration::from_millis(180);
const MAX_DELAY: Duration = Duration::from_secs(1);

pub struct RepoWatch {
    stop: Arc<AtomicBool>,
    wake: SyncSender<()>,
    updates: Receiver<Result<(), String>>,
    thread: Option<JoinHandle<()>>,
}

impl RepoWatch {
    pub fn new(repo: Repo) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let (wake, events) = mpsc::sync_channel(1);
        let notify = wake.clone();
        let (send, updates) = mpsc::sync_channel(1);
        let (ready, started) = mpsc::sync_channel(1);
        let thread = thread::spawn(move || {
            let flags = Arc::new(AtomicU8::new(0));
            let pending = flags.clone();
            let errors = Arc::new(std::sync::Mutex::new(None));
            let reported = errors.clone();
            let setup = (|| -> Result<_> {
                let git_dirs = repo.watch_git_dirs()?;
                let metadata = git_dirs.clone();
                let mut watcher =
                    notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                        match event {
                            Ok(event) => {
                                if matches!(event.kind, EventKind::Access(_)) {
                                    return;
                                }
                                if !event.paths.is_empty()
                                    && event.paths.iter().all(|path| {
                                        metadata.iter().any(|dir| path.starts_with(dir))
                                            && path.extension().is_some_and(|ext| ext == "lock")
                                    })
                                {
                                    return;
                                }
                                let rebuild = event.need_rescan()
                                    || !matches!(
                                        event.kind,
                                        EventKind::Modify(
                                            ModifyKind::Data(_) | ModifyKind::Metadata(_)
                                        )
                                    )
                                    || event.paths.iter().any(|path| {
                                        path.file_name().is_some_and(|name| {
                                            name == ".gitignore"
                                                || name == "exclude"
                                                || name == "index"
                                                || name == "config"
                                        })
                                    });
                                pending.fetch_or(
                                    CHANGED | if rebuild { DIRECTORIES } else { 0 },
                                    Ordering::Relaxed,
                                );
                            }
                            Err(error) => *reported.lock().unwrap() = Some(error.to_string()),
                        }
                        let _ = notify.try_send(());
                    })?;
                let mut watched = BTreeSet::new();
                reconcile(&mut watcher, &mut watched, &repo, &git_dirs)?;
                Ok((watcher, watched, git_dirs))
            })();
            let (mut watcher, mut watched, git_dirs) = match setup {
                Ok(state) => {
                    let _ = ready.send(Ok(()));
                    state
                }
                Err(error) => {
                    let _ = ready.send(Err(format!("{error:#}")));
                    return;
                }
            };
            let mut first = None;
            let mut last = Instant::now();
            let mut rebuild = false;
            while !stopped.load(Ordering::Relaxed) {
                let _ = events.recv_timeout(Duration::from_millis(40));
                if let Some(error) = errors.lock().unwrap().take() {
                    let _ = send.try_send(Err(error));
                }
                let changes = flags.swap(0, Ordering::Relaxed);
                if changes != 0 {
                    last = Instant::now();
                    first.get_or_insert(last);
                    rebuild |= changes & DIRECTORIES != 0;
                }
                if first.is_some_and(|start: Instant| {
                    last.elapsed() >= QUIET || start.elapsed() >= MAX_DELAY
                }) {
                    if rebuild {
                        if let Err(error) = reconcile(&mut watcher, &mut watched, &repo, &git_dirs)
                        {
                            let _ = send.try_send(Err(format!("{error:#}")));
                        }
                        rebuild = false;
                    }
                    let _ = send.try_send(Ok(()));
                    first = None;
                }
            }
        });
        started
            .recv()
            .context("Repository watcher stopped during startup")?
            .map_err(anyhow::Error::msg)?;
        Ok(Self {
            stop,
            wake,
            updates,
            thread: Some(thread),
        })
    }

    pub fn poll(&self) -> Option<Result<(), String>> {
        self.updates.try_recv().ok()
    }
}

impl Drop for RepoWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.wake.try_send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn reconcile(
    watcher: &mut RecommendedWatcher,
    watched: &mut BTreeSet<PathBuf>,
    repo: &Repo,
    git_dirs: &BTreeSet<PathBuf>,
) -> Result<()> {
    let mut directories = BTreeSet::new();
    let mut walk = ignore::WalkBuilder::new(&repo.root);
    walk.hidden(false)
        .ignore(false)
        .filter_entry(|entry| entry.file_name() != ".git");
    for entry in walk.build() {
        let entry = entry?;
        if entry.file_type().is_some_and(|kind| kind.is_dir()) {
            directories.insert(entry.into_path());
        }
    }
    for file in repo.tracked_paths()? {
        for parent in file
            .ancestors()
            .skip(1)
            .take_while(|path| path.starts_with(&repo.root))
        {
            if parent.is_dir() {
                directories.insert(parent.to_path_buf());
            }
        }
    }
    for dir in git_dirs {
        metadata_directories(dir, &mut directories, true)?;
    }
    for path in watched.difference(&directories) {
        let _ = watcher.unwatch(path);
    }
    // Re-register surviving paths too: an editor or checkout can replace a
    // directory at the same pathname after its native watch was removed.
    for path in &directories {
        if let Err(error) = watcher.watch(path, RecursiveMode::NonRecursive)
            && path.is_dir()
        {
            return Err(error).with_context(|| format!("Cannot watch {}", path.display()));
        }
    }
    *watched = directories;
    Ok(())
}

fn metadata_directories(
    path: &Path,
    directories: &mut BTreeSet<PathBuf>,
    root: bool,
) -> Result<()> {
    directories.insert(path.to_path_buf());
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && !(root && (entry.file_name() == "objects" || entry.file_name() == "modules"))
        {
            metadata_directories(&entry.path(), directories, false)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn fixture() -> TempDir {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["init", "-q", "--initial-branch=main"]);
        git(dir.path(), &["config", "user.name", "Watch Test"]);
        git(dir.path(), &["config", "user.email", "watch@example.com"]);
        git(dir.path(), &["config", "commit.gpgsign", "false"]);
        fs::write(dir.path().join("file.txt"), "base\n").unwrap();
        fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        fs::create_dir(dir.path().join("target")).unwrap();
        fs::write(
            dir.path().join("target/tracked.txt"),
            "tracked despite ignore\n",
        )
        .unwrap();
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["add", "-f", "target/tracked.txt"]);
        git(dir.path(), &["commit", "-qm", "base"]);
        dir
    }

    fn changed(watch: &RepoWatch) {
        let start = Instant::now();
        loop {
            if let Some(update) = watch.poll() {
                update.unwrap();
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "No filesystem update received"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn quiet(watch: &RepoWatch) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(550) {
            assert!(watch.poll().is_none(), "Unexpected filesystem update");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn watches_new_replaced_and_ignored_tracked_directories_without_read_feedback() {
        let dir = fixture();
        let root = dir.path();
        fs::create_dir_all(root.join("target/build/deep")).unwrap();
        let repo = Repo::open(root).unwrap();
        let watch = RepoWatch::new(repo.clone()).unwrap();
        quiet(&watch);
        // Object writes and ignored build output must not trigger Git reads.
        fs::write(root.join("target/build/deep/output"), "ignored").unwrap();
        fs::write(root.join(".git/objects/probe"), "ignored object").unwrap();
        fs::write(root.join(".git/index.lock"), "lock").unwrap();
        fs::remove_file(root.join(".git/index.lock")).unwrap();
        repo.snapshot().unwrap();
        quiet(&watch);
        fs::write(root.join("target/tracked.txt"), "tracked change").unwrap();
        changed(&watch);
        quiet(&watch);
        fs::create_dir_all(root.join("new/nested")).unwrap();
        changed(&watch);
        fs::write(root.join("new/nested/file"), "first").unwrap();
        changed(&watch);
        fs::remove_dir_all(root.join("new")).unwrap();
        fs::create_dir_all(root.join("new/nested")).unwrap();
        changed(&watch);
        fs::write(root.join("new/nested/file"), "replacement").unwrap();
        changed(&watch);
        // Atomic editor saves and bursts converge to one bounded notification.
        for value in 0..50 {
            fs::write(root.join("temporary"), value.to_string()).unwrap();
            fs::rename(root.join("temporary"), root.join("file.txt")).unwrap();
        }
        changed(&watch);
        quiet(&watch);
        fs::write(root.join(".gitignore"), "target/\nnew/\n").unwrap();
        changed(&watch);
        fs::write(root.join("new/nested/file"), "now ignored").unwrap();
        quiet(&watch);
    }

    #[test]
    fn watches_linked_worktree_index_and_shared_refs_and_stash_reflog() {
        let dir = fixture();
        let linked = TempDir::new().unwrap();
        let path = linked.path().join("worktree");
        git(
            dir.path(),
            &["worktree", "add", "-qb", "linked", path.to_str().unwrap()],
        );
        let watch = RepoWatch::new(Repo::open(&path).unwrap()).unwrap();
        fs::write(path.join("file.txt"), "linked worktree change").unwrap();
        changed(&watch);
        git(&path, &["add", "file.txt"]);
        changed(&watch);
        git(&path, &["commit", "-qm", "linked commit"]);
        changed(&watch);
        // The main worktree's stash lives in the common Git directory.
        fs::write(dir.path().join("file.txt"), "first stash").unwrap();
        git(dir.path(), &["stash", "push", "-qm", "first"]);
        changed(&watch);
        fs::write(dir.path().join("file.txt"), "second stash").unwrap();
        git(dir.path(), &["stash", "push", "-qm", "second"]);
        changed(&watch);
        // Dropping a non-head stash changes the reflog without moving refs/stash.
        git(dir.path(), &["stash", "drop", "stash@{1}"]);
        changed(&watch);
        quiet(&watch);
    }
}
