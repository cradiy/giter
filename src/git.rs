use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        OnceLock,
        mpsc::{self, Receiver, Sender},
    },
    thread,
};

use anyhow::{Context, Result, ensure};

use crate::conflict::{self, Chunk};

const MAX_BLOB: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct File {
    pub path: PathBuf,
    pub original: Option<PathBuf>,
    pub index: char,
    pub worktree: char,
}

impl File {
    pub fn status(&self, staged: bool) -> &'static str {
        if self.conflicted() {
            return "Conflict";
        }
        match if staged { self.index } else { self.worktree } {
            '?' | 'A' => "Added",
            'M' | 'T' => "Modified",
            'D' => "Deleted",
            'R' => "Renamed",
            'C' => "Copied",
            _ => "Changed",
        }
    }
    pub fn conflicted(&self) -> bool {
        matches!(
            (self.index, self.worktree),
            ('D', 'D')
                | ('A', 'U')
                | ('U', 'D')
                | ('U', 'A')
                | ('D', 'U')
                | ('A', 'A')
                | ('U', 'U')
        )
    }

    pub fn staged(&self) -> bool {
        !self.conflicted() && !matches!(self.index, ' ' | '?')
    }

    pub fn unstaged(&self) -> bool {
        self.conflicted() || self.worktree != ' '
    }

    pub fn label(&self) -> String {
        match &self.original {
            Some(original) => format!("{} → {}", original.display(), self.path.display()),
            None => self.path.display().to_string(),
        }
    }
}

#[derive(Clone)]
pub struct Commit {
    pub id: String,
    pub subject: String,
    pub author: String,
    pub age: String,
}

pub struct Snapshot {
    pub branch: String,
    pub operation: String,
    pub files: Vec<File>,
    pub commits: Vec<Commit>,
}

#[derive(Clone)]
pub enum Selection {
    File(File, bool),
    Directory(PathBuf, Vec<File>, bool),
    Commit(String),
}

pub struct ConflictView {
    pub base: String,
    pub ours: String,
    pub theirs: String,
    pub working: String,
    pub chunks: Vec<Chunk>,
}

pub struct Preview {
    pub title: String,
    pub text: String,
    pub conflict: Option<ConflictView>,
    pub code_path: Option<PathBuf>,
    pub styles: OnceLock<Box<crate::highlight::StyledPreview>>,
}

impl Preview {
    pub fn styled(&self) -> &crate::highlight::StyledPreview {
        self.styles
            .get_or_init(|| Box::new(crate::highlight::preview(self)))
    }
}

#[derive(Clone)]
pub struct Repo {
    pub root: PathBuf,
}

impl Repo {
    pub fn open(path: &Path) -> Result<Self> {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .context("Cannot run git")?;
        ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        let bytes = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
        Ok(Self {
            root: PathBuf::from(os_string(bytes)),
        })
    }

    fn command(&self) -> Command {
        let mut command = Command::new("git");
        command
            .arg("--no-pager")
            .arg("--literal-pathspecs")
            .arg("-C")
            .arg(&self.root)
            .args(["-c", "color.ui=false", "-c", "core.quotepath=false"])
            .env("GIT_TERMINAL_PROMPT", "0");
        command
    }

    fn run(&self, args: &[&OsStr]) -> Result<Vec<u8>> {
        let output = self
            .command()
            .args(args)
            .output()
            .context("Cannot run git")?;
        ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(output.stdout)
    }

    fn strings(&self, args: &[&str]) -> Result<Vec<u8>> {
        self.run(&args.iter().map(OsStr::new).collect::<Vec<_>>())
    }

    fn head_exists(&self) -> bool {
        self.strings(&["rev-parse", "--verify", "-q", "HEAD"])
            .is_ok()
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let status = self.strings(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
        let mut files = parse_status(&status)?;
        files.sort_by_key(|f| (!f.conflicted(), f.path.clone()));
        let branch = self
            .strings(&["symbolic-ref", "--short", "HEAD"])
            .map(|s| String::from_utf8_lossy(&s).trim().to_owned())
            .unwrap_or_else(|_| "detached HEAD".into());
        let mut operation = String::new();
        for (path, label) in [
            ("MERGE_HEAD", "MERGING"),
            ("rebase-merge", "REBASING"),
            ("rebase-apply", "REBASING"),
            ("CHERRY_PICK_HEAD", "CHERRY-PICK"),
            ("REVERT_HEAD", "REVERTING"),
        ] {
            let path = self.strings(&["rev-parse", "--git-path", path])?;
            if self
                .root
                .join(PathBuf::from(os_string(
                    path.strip_suffix(b"\n").unwrap_or(&path),
                )))
                .exists()
            {
                operation = label.into();
                break;
            }
        }
        let mut commits = Vec::new();
        if self.head_exists() {
            let log = self.strings(&["log", "-150", "-z", "--format=%H%x00%s%x00%an%x00%ar"])?;
            let fields: Vec<_> = log.split(|b| *b == 0).collect();
            for fields in fields.as_chunks::<4>().0 {
                commits.push(Commit {
                    id: String::from_utf8_lossy(fields[0]).into(),
                    subject: String::from_utf8_lossy(fields[1]).into(),
                    author: String::from_utf8_lossy(fields[2]).into(),
                    age: String::from_utf8_lossy(fields[3]).into(),
                });
            }
        }
        Ok(Snapshot {
            branch,
            operation,
            files,
            commits,
        })
    }

    fn blob(&self, id: &str) -> Result<String> {
        let size = self.strings(&["cat-file", "-s", id])?;
        let size: usize = String::from_utf8_lossy(&size).trim().parse()?;
        if size > MAX_BLOB {
            return Ok(format!(
                "[Content exceeds {} MiB; open in editor]",
                MAX_BLOB / 1024 / 1024
            ));
        }
        Ok(display_content(self.strings(&["cat-file", "blob", id])?))
    }

    fn working(&self, path: &Path) -> Result<String> {
        let path = self.root.join(path);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok("[Deleted in working tree]".into());
            }
            Err(error) => return Err(error.into()),
        };
        if metadata.is_symlink() {
            return Ok(format!(
                "[Symbolic link → {}]",
                fs::read_link(path)?.display()
            ));
        }
        if !metadata.is_file() {
            return Ok("[Directory / submodule]".into());
        }
        if metadata.len() > MAX_BLOB as u64 {
            return Ok("[File exceeds 2 MiB; open in editor]".into());
        }
        Ok(display_content(fs::read(path)?))
    }

    pub fn preview(&self, selection: &Selection) -> Result<Preview> {
        match selection {
            Selection::Directory(path, files, staged) => {
                let name = if path.as_os_str().is_empty() {
                    self.root
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                } else {
                    path.display().to_string()
                };
                let mut text = format!("Directory {name}/\n\n{} changed files\n", files.len());
                for status in [
                    "Added", "Modified", "Deleted", "Renamed", "Copied", "Conflict",
                ] {
                    let count = files.iter().filter(|f| f.status(*staged) == status).count();
                    if count > 0 {
                        text.push_str(&format!("{status}: {count}\n"));
                    }
                }
                text.push_str(&format!(
                    "\n{} affects all {} listed files, including collapsed descendants.\n\n",
                    if *staged { "u: unstage" } else { "s: stage" },
                    files.len()
                ));
                for file in files {
                    text.push_str(&format!("{:<9} {}\n", file.status(*staged), file.label()));
                }
                Ok(Preview {
                    title: format!("{name}/"),
                    text,
                    conflict: None,
                    code_path: None,
                    styles: OnceLock::new(),
                })
            }
            Selection::Commit(id) => {
                let text = self.strings(&[
                    "show",
                    "--format=fuller",
                    "--stat",
                    "--patch",
                    "--no-ext-diff",
                    "--no-textconv",
                    id,
                ])?;
                Ok(Preview {
                    title: format!("Commit {}", &id[..8]),
                    text: String::from_utf8_lossy(&text).into(),
                    conflict: None,
                    code_path: None,
                    styles: OnceLock::new(),
                })
            }
            Selection::File(file, staged) => {
                if file.conflicted() {
                    let mut sources = [
                        "[Absent in base]".to_owned(),
                        "[Absent on ours side]".into(),
                        "[Absent on theirs side]".into(),
                    ];
                    let entries = self.run(&[
                        OsStr::new("ls-files"),
                        OsStr::new("-u"),
                        OsStr::new("-z"),
                        OsStr::new("--"),
                        file.path.as_os_str(),
                    ])?;
                    for record in entries.split(|b| *b == 0).filter(|s| !s.is_empty()) {
                        let header = record
                            .split(|b| *b == b'\t')
                            .next()
                            .context("Invalid index entry")?;
                        let fields: Vec<_> = header.split(|b| *b == b' ').collect();
                        ensure!(fields.len() == 3, "Invalid index entry");
                        let stage: usize = std::str::from_utf8(fields[2])?.parse()?;
                        ensure!((1..=3).contains(&stage), "Invalid conflict stage");
                        sources[stage - 1] = if fields[0] == b"160000" {
                            format!("[Submodule commit {}]", String::from_utf8_lossy(fields[1]))
                        } else {
                            self.blob(std::str::from_utf8(fields[1])?)?
                        };
                    }
                    let working = self.working(&file.path)?;
                    let chunks = conflict::parse(&working);
                    let [base, ours, theirs] = sources;
                    return Ok(Preview {
                        title: file.label(),
                        text: String::new(),
                        conflict: Some(ConflictView {
                            base,
                            ours,
                            theirs,
                            working,
                            chunks,
                        }),
                        code_path: Some(file.path.clone()),
                        styles: OnceLock::new(),
                    });
                }
                let text = if file.index == '?' {
                    self.working(&file.path)?
                } else {
                    let mut args = vec![
                        OsStr::new("diff"),
                        OsStr::new("--no-ext-diff"),
                        OsStr::new("--no-textconv"),
                        OsStr::new("--no-color"),
                    ];
                    if *staged {
                        args.push(OsStr::new("--cached"));
                    }
                    args.push(OsStr::new("--"));
                    args.push(file.path.as_os_str());
                    if let Some(original) = &file.original {
                        args.push(original.as_os_str());
                    }
                    String::from_utf8_lossy(&self.run(&args)?).into()
                };
                Ok(Preview {
                    title: file.label(),
                    text,
                    conflict: None,
                    code_path: (file.index == '?').then(|| file.path.clone()),
                    styles: OnceLock::new(),
                })
            }
        }
    }

    fn selected_paths(files: &[File]) -> BTreeSet<PathBuf> {
        files
            .iter()
            .flat_map(|file| std::iter::once(file.path.clone()).chain(file.original.clone()))
            .collect()
    }

    fn run_paths(&self, args: &[&str], paths: &BTreeSet<PathBuf>) -> Result<()> {
        ensure!(!paths.is_empty(), "No files selected");
        let mut child = self
            .command()
            .args(args)
            .args(["--pathspec-from-file=-", "--pathspec-file-nul"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut input = child.stdin.take().context("Cannot open git stdin")?;
        for path in paths {
            input.write_all(path.as_os_str().as_encoded_bytes())?;
            input.write_all(&[0])?;
        }
        drop(input);
        let output = child.wait_with_output()?;
        ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(())
    }

    pub fn stage(&self, files: &[File]) -> Result<()> {
        let paths = Self::selected_paths(files);
        let conflicts = self.strings(&["ls-files", "-u", "-z"])?;
        let mut checked = BTreeSet::new();
        for record in conflicts.split(|b| *b == 0).filter(|r| !r.is_empty()) {
            let separator = record
                .iter()
                .position(|b| *b == b'\t')
                .context("Invalid index entry")?;
            let path = PathBuf::from(os_string(&record[separator + 1..]));
            if paths.contains(&path) && checked.insert(path.clone()) {
                let full_path = self.root.join(&path);
                if let Ok(metadata) = fs::symlink_metadata(&full_path)
                    && metadata.is_file()
                {
                    let bytes = fs::read(&full_path)?;
                    ensure!(
                        !conflict::has_markers(&String::from_utf8_lossy(&bytes)),
                        "Conflict markers remain in {}. Edit the file before staging.",
                        path.display()
                    );
                }
            }
        }
        self.run_paths(&["add", "-A"], &paths)
    }

    pub fn unstage(&self, files: &[File]) -> Result<()> {
        let paths = Self::selected_paths(files);
        if self.head_exists() {
            self.run_paths(&["restore", "--staged"], &paths)
        } else {
            self.run_paths(&["rm", "--cached", "-r", "-q"], &paths)
        }
    }

    pub fn commit(&self, message: &str) -> Result<()> {
        ensure!(!message.trim().is_empty(), "Commit message is empty");
        let mut child = self
            .command()
            .args(["commit", "--file=-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .context("Cannot open git stdin")?
            .write_all(message.as_bytes())?;
        let output = child.wait_with_output()?;
        ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(())
    }

    pub fn editor(&self, path: &Path) -> Result<Command> {
        ensure!(
            fs::symlink_metadata(self.root.join(path))?.is_file(),
            "Only regular working-tree files can be opened in the editor"
        );
        let editor = self.strings(&["var", "GIT_EDITOR"])?;
        let editor = String::from_utf8(editor)?.trim().to_owned();
        let mut command = {
            let mut command = Command::new("sh");
            command
                .arg("-c")
                .arg(format!("{editor} \"$1\""))
                .arg("giter-editor")
                .arg(self.root.join(path));
            command
        };
        command.current_dir(&self.root);
        Ok(command)
    }
}

fn display_content(bytes: Vec<u8>) -> String {
    if bytes.contains(&0) {
        return "[Binary content; open in an external tool]".into();
    }
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => "[Non-UTF-8 content; open in an external editor]".into(),
    }
}

#[cfg(unix)]
fn os_string(bytes: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(bytes.to_vec())
}

#[cfg(not(unix))]
fn os_string(bytes: &[u8]) -> OsString {
    OsString::from(String::from_utf8_lossy(bytes).as_ref())
}

fn parse_status(bytes: &[u8]) -> Result<Vec<File>> {
    let mut records = bytes.split(|b| *b == 0).filter(|r| !r.is_empty());
    let mut files = Vec::new();
    while let Some(record) = records.next() {
        ensure!(
            record.len() >= 4 && record[2] == b' ',
            "Invalid git status record"
        );
        let (index, worktree) = (record[0] as char, record[1] as char);
        let original = if matches!(index, 'R' | 'C') || matches!(worktree, 'R' | 'C') {
            Some(PathBuf::from(os_string(
                records.next().context("Missing rename source")?,
            )))
        } else {
            None
        };
        files.push(File {
            path: PathBuf::from(os_string(&record[3..])),
            original,
            index,
            worktree,
        });
    }
    Ok(files)
}

pub enum Request {
    Refresh,
    Preview(u64, Selection),
    Stage(Vec<File>),
    Unstage(Vec<File>),
    Commit(String),
}

pub enum Response {
    Snapshot(Result<Snapshot, String>, Option<String>),
    Preview(u64, Result<Preview, String>),
    Failed(String),
}

pub fn worker(repo: Repo) -> (Sender<Request>, Receiver<Response>) {
    let (send, receive) = mpsc::channel();
    let (results, updates) = mpsc::channel();
    thread::spawn(move || {
        let mut pending = None;
        while let Some(mut request) = pending.take().or_else(|| receive.recv().ok()) {
            if matches!(request, Request::Preview(..)) {
                while let Ok(next) = receive.try_recv() {
                    if matches!(next, Request::Preview(..)) {
                        request = next;
                    } else {
                        pending = Some(next);
                        break;
                    }
                }
            }
            let response = match request {
                Request::Refresh => {
                    Response::Snapshot(repo.snapshot().map_err(|e| format!("{e:#}")), None)
                }
                Request::Preview(id, selection) => Response::Preview(
                    id,
                    repo.preview(&selection)
                        .inspect(|preview| {
                            preview.styled();
                        })
                        .map_err(|e| format!("{e:#}")),
                ),
                request => {
                    let (result, message) = match request {
                        Request::Stage(files) => (repo.stage(&files), "Staged"),
                        Request::Unstage(files) => (repo.unstage(&files), "Unstaged"),
                        Request::Commit(message) => (repo.commit(&message), "Commit created"),
                        _ => unreachable!(),
                    };
                    match result {
                        Ok(()) => Response::Snapshot(
                            repo.snapshot().map_err(|e| format!("{e:#}")),
                            Some(message.into()),
                        ),
                        Err(error) => Response::Failed(format!("{error:#}")),
                    }
                }
            };
            if results.send(response).is_err() {
                break;
            }
        }
    });
    (send, updates)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        repo.strings(&["config", "user.email", "test@example.com"])
            .unwrap();
        repo.strings(&["config", "user.name", "Test"]).unwrap();
        (dir, repo)
    }

    #[test]
    fn real_merge_exposes_three_stages_and_blocks_unresolved_staging() {
        let (_dir, repo) = fixture();
        let path = repo.root.join("冲突.txt");
        fs::write(&path, "base\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("initial").unwrap();
        repo.strings(&["checkout", "-b", "topic"]).unwrap();
        fs::write(&path, "theirs\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("incoming").unwrap();
        repo.strings(&["checkout", "main"]).unwrap();
        fs::write(&path, "ours\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("current").unwrap();
        assert!(repo.strings(&["merge", "topic"]).is_err());
        let snapshot = repo.snapshot().unwrap();
        assert_eq!(snapshot.operation, "MERGING");
        assert_eq!(snapshot.commits[0].subject, "current");
        let file = &snapshot.files[0];
        assert!(file.conflicted());
        let conflict = repo
            .preview(&Selection::File(file.clone(), false))
            .unwrap()
            .conflict
            .unwrap();
        assert_eq!(conflict.base, "base\n");
        assert_eq!(conflict.ours, "ours\n");
        assert_eq!(conflict.theirs, "theirs\n");
        assert_eq!(conflict.chunks.len(), 1);
        assert!(repo.stage(std::slice::from_ref(file)).is_err());
        fs::write(repo.root.join("clean.txt"), "ready\n").unwrap();
        let batch = repo.snapshot().unwrap().files;
        assert!(repo.stage(&batch).is_err());
        assert!(
            repo.snapshot()
                .unwrap()
                .files
                .iter()
                .all(|file| !file.staged())
        );
        fs::write(&path, "合并完成\n").unwrap();
        repo.stage(std::slice::from_ref(file)).unwrap();
        assert!(!repo.snapshot().unwrap().files[0].conflicted());
        repo.commit("修复冲突\n\n中文提交内容").unwrap();
        assert_eq!(repo.snapshot().unwrap().commits[0].subject, "修复冲突");
    }

    #[test]
    fn literal_paths_unborn_unstage_and_rename_roundtrip() {
        let (_dir, repo) = fixture();
        fs::write(repo.root.join(":(glob)*.txt"), "literal").unwrap();
        fs::write(repo.root.join("other.txt"), "other").unwrap();
        let file = repo
            .snapshot()
            .unwrap()
            .files
            .into_iter()
            .find(|f| f.path == Path::new(":(glob)*.txt"))
            .unwrap();
        repo.stage(std::slice::from_ref(&file)).unwrap();
        let snapshot = repo.snapshot().unwrap();
        assert_eq!(snapshot.files.iter().filter(|f| f.staged()).count(), 1);
        repo.unstage(std::slice::from_ref(&snapshot.files[0]))
            .unwrap();
        assert!(repo.root.join(&file.path).exists());
        repo.stage(std::slice::from_ref(&file)).unwrap();
        repo.commit("initial").unwrap();
        repo.strings(&["mv", ":(glob)*.txt", "新\n名称.txt"])
            .unwrap();
        let renamed = repo
            .snapshot()
            .unwrap()
            .files
            .into_iter()
            .find(|f| f.index == 'R')
            .unwrap();
        assert_eq!(renamed.original.as_deref(), Some(Path::new(":(glob)*.txt")));
        repo.unstage(std::slice::from_ref(&renamed)).unwrap();
        assert!(repo.root.join("新\n名称.txt").exists());
        assert!(repo.snapshot().unwrap().files.iter().all(|f| !f.staged()));
    }

    #[test]
    fn modify_delete_conflict_keeps_missing_side_visible() {
        let (_dir, repo) = fixture();
        fs::write(repo.root.join("file"), "base\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("initial").unwrap();
        repo.strings(&["checkout", "-b", "topic"]).unwrap();
        repo.strings(&["rm", "file"]).unwrap();
        repo.commit("delete").unwrap();
        repo.strings(&["checkout", "main"]).unwrap();
        fs::write(repo.root.join("file"), "modified\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("modify").unwrap();
        assert!(repo.strings(&["merge", "topic"]).is_err());
        let file = repo.snapshot().unwrap().files.remove(0);
        let conflict = repo
            .preview(&Selection::File(file, false))
            .unwrap()
            .conflict
            .unwrap();
        assert_eq!(conflict.base, "base\n");
        assert_eq!(conflict.ours, "modified\n");
        assert!(conflict.theirs.contains("Absent"));
        assert!(conflict.chunks.is_empty());
    }

    #[test]
    fn directory_batch_preserves_siblings_and_handles_added_modified_deleted_paths() {
        let (_dir, repo) = fixture();
        fs::create_dir_all(repo.root.join("src/nested")).unwrap();
        fs::create_dir_all(repo.root.join("src-other")).unwrap();
        for path in [
            "src/keep.rs",
            "src/nested/remove.rs",
            "src-other/outside.rs",
        ] {
            fs::write(repo.root.join(path), "initial\n").unwrap();
        }
        repo.stage(&repo.snapshot().unwrap().files).unwrap();
        repo.unstage(&repo.snapshot().unwrap().files).unwrap();
        assert!(repo.snapshot().unwrap().files.iter().all(|f| !f.staged()));
        assert!(repo.root.join("src/keep.rs").exists());
        repo.stage(&repo.snapshot().unwrap().files).unwrap();
        repo.commit("initial").unwrap();
        fs::write(repo.root.join("src/keep.rs"), "modified\n").unwrap();
        fs::remove_file(repo.root.join("src/nested/remove.rs")).unwrap();
        fs::write(repo.root.join("src/nested/新\n文件.rs"), "added\n").unwrap();
        fs::write(repo.root.join("src-other/outside.rs"), "outside\n").unwrap();
        let files: Vec<_> = repo
            .snapshot()
            .unwrap()
            .files
            .into_iter()
            .filter(|f| f.path.starts_with("src"))
            .collect();
        repo.stage(&files).unwrap();
        let snapshot = repo.snapshot().unwrap();
        let staged: Vec<_> = snapshot
            .files
            .iter()
            .filter(|f| f.staged())
            .cloned()
            .collect();
        assert_eq!(staged.len(), 3);
        assert!(staged.iter().any(|f| f.status(true) == "Added"));
        assert!(staged.iter().any(|f| f.status(true) == "Modified"));
        assert!(staged.iter().any(|f| f.status(true) == "Deleted"));
        assert!(
            !snapshot
                .files
                .iter()
                .find(|f| f.path.starts_with("src-other"))
                .unwrap()
                .staged()
        );
        repo.unstage(&staged).unwrap();
        assert!(repo.snapshot().unwrap().files.iter().all(|f| !f.staged()));
        assert_eq!(
            fs::read_to_string(repo.root.join("src/keep.rs")).unwrap(),
            "modified\n"
        );
        assert!(!repo.root.join("src/nested/remove.rs").exists());
        assert!(repo.root.join("src/nested/新\n文件.rs").exists());
    }
}
