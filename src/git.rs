use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, OnceLock,
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
            ' ' => "Clean",
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

#[derive(Clone)]
pub struct Stash {
    pub id: String,
    pub reference: String,
    pub subject: String,
    pub author: String,
    pub age: String,
}

pub struct StashDetails {
    pub id: String,
    pub worktree: CommitDetails,
    pub index_id: String,
    pub index: CommitDetails,
    pub untracked_id: Option<String>,
    pub untracked: Option<CommitDetails>,
    pub files: Vec<File>,
}

pub struct Snapshot {
    pub branch: String,
    pub operation: String,
    pub files: Vec<File>,
    pub committed_files: Vec<File>,
    pub commits: Vec<Commit>,
    pub stashes: Vec<Stash>,
}

pub struct CommitDetails {
    pub parent: Option<String>,
    pub files: Vec<File>,
}

pub struct FileCommit {
    pub commit: Commit,
    pub parent: Option<String>,
    pub file: File,
}

#[derive(Clone)]
pub enum Selection {
    File(File, bool),
    Directory(PathBuf, Vec<File>, bool),
    RepositoryFile(File),
    RepositoryDirectory(PathBuf, Vec<File>),
    Commit(String),
    Stash(Stash),
    StashFile {
        details: Arc<StashDetails>,
        file: File,
    },
    StashDirectory {
        details: Arc<StashDetails>,
        path: PathBuf,
        files: Vec<File>,
    },
    CommitFile {
        id: String,
        parent: Option<String>,
        file: File,
    },
    CommitDirectory {
        id: String,
        path: PathBuf,
        files: Vec<File>,
    },
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
            .env("GIT_OPTIONAL_LOCKS", "0")
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

    pub fn watch_git_dirs(&self) -> Result<BTreeSet<PathBuf>> {
        ["--absolute-git-dir", "--git-common-dir"]
            .into_iter()
            .map(|option| {
                let bytes = self.strings(&["rev-parse", "--path-format=absolute", option])?;
                Ok(PathBuf::from(os_string(
                    bytes.strip_suffix(b"\n").unwrap_or(&bytes),
                )))
            })
            .collect()
    }

    pub fn tracked_paths(&self) -> Result<Vec<PathBuf>> {
        Ok(self
            .strings(&["ls-files", "--cached", "-z"])?
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| self.root.join(os_string(path)))
            .collect())
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let status = self.strings(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
        let mut files = parse_status(&status)?;
        files.sort_by_key(|f| (!f.conflicted(), f.path.clone()));
        let has_head = self.head_exists();
        let committed_files = if has_head {
            self.strings(&["ls-tree", "-r", "--name-only", "-z", "HEAD"])?
                .split(|byte| *byte == 0)
                .filter(|path| !path.is_empty())
                .map(|path| File {
                    path: PathBuf::from(os_string(path)),
                    original: None,
                    index: ' ',
                    worktree: ' ',
                })
                .collect()
        } else {
            Vec::new()
        };
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
        if has_head {
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
            committed_files,
            commits,
            stashes: self.stashes()?,
        })
    }

    fn stashes(&self) -> Result<Vec<Stash>> {
        let bytes = self.strings(&[
            "stash",
            "list",
            "-z",
            "--format=%H%x00%gd%x00%gs%x00%an%x00%ar",
        ])?;
        let fields: Vec<_> = bytes.split(|b| *b == 0).collect();
        Ok(fields
            .as_chunks::<5>()
            .0
            .iter()
            .map(|fields| Stash {
                id: String::from_utf8_lossy(fields[0]).into(),
                reference: String::from_utf8_lossy(fields[1]).into(),
                subject: String::from_utf8_lossy(fields[2]).into(),
                author: String::from_utf8_lossy(fields[3]).into(),
                age: String::from_utf8_lossy(fields[4]).into(),
            })
            .collect())
    }

    pub fn stash_files(&self, id: &str) -> Result<Arc<StashDetails>> {
        let parents = self.strings(&["rev-list", "--parents", "-n", "1", id])?;
        let parents: Vec<_> = std::str::from_utf8(&parents)?.split_whitespace().collect();
        ensure!(matches!(parents.len(), 3 | 4), "Invalid stash snapshot");
        let worktree = self.commit_files(id)?;
        let index_id = parents[2].to_owned();
        let index = self.commit_files(&index_id)?;
        let untracked_id = parents.get(3).map(|id| id.to_string());
        let untracked = untracked_id
            .as_deref()
            .map(|id| self.commit_files(id))
            .transpose()?;
        let mut files = std::collections::BTreeMap::new();
        for file in worktree
            .files
            .iter()
            .chain(&index.files)
            .chain(untracked.iter().flat_map(|details| &details.files))
        {
            files
                .entry(file.path.clone())
                .or_insert_with(|| file.clone());
        }
        Ok(Arc::new(StashDetails {
            id: id.to_owned(),
            worktree,
            index_id,
            index,
            untracked_id,
            untracked,
            files: files.into_values().collect(),
        }))
    }

    fn stash_preview(&self, details: &StashDetails, file: &File) -> Result<Preview> {
        let patch = |id: &str, comparison: &CommitDetails| -> Result<Option<String>> {
            comparison
                .files
                .iter()
                .find(|entry| entry.path == file.path)
                .map(|entry| {
                    self.preview(&Selection::CommitFile {
                        id: id.to_owned(),
                        parent: comparison.parent.clone(),
                        file: entry.clone(),
                    })
                    .map(|preview| preview.text)
                })
                .transpose()
        };
        let worktree = patch(&details.id, &details.worktree)?;
        let index = patch(&details.index_id, &details.index)?;
        let untracked = details
            .untracked_id
            .as_deref()
            .zip(details.untracked.as_ref())
            .map(|(id, comparison)| patch(id, comparison))
            .transpose()?
            .flatten();
        let mut text = String::new();
        if let Some(worktree) = &worktree {
            text.push_str(if index.as_ref() == Some(worktree) {
                "Saved working tree and index\n\n"
            } else {
                "Saved working tree\n\n"
            });
            text.push_str(worktree);
        }
        if let Some(index) = &index
            && worktree.as_ref() != Some(index)
        {
            text.push_str("\nSaved index\n\n");
            text.push_str(index);
        }
        if let Some(untracked) = untracked {
            text.push_str("\nSaved untracked files\n\n");
            text.push_str(&untracked);
        }
        Ok(Preview {
            title: format!("{} · stash {}", file.label(), &details.id[..8]),
            text,
            conflict: None,
            code_path: None,
            styles: OnceLock::new(),
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

    pub fn commit_files(&self, id: &str) -> Result<CommitDetails> {
        let parents = self.strings(&["rev-list", "--parents", "-n", "1", id])?;
        let parent = std::str::from_utf8(&parents)?
            .split_whitespace()
            .nth(1)
            .map(str::to_owned);
        let mut args = vec![
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-status",
            "-r",
            "-z",
            "--find-renames",
        ];
        if let Some(parent) = &parent {
            args.push(parent);
        }
        args.push(id);
        args.push("--");
        let changes = self.strings(&args)?;
        let mut records = changes.split(|b| *b == 0).filter(|s| !s.is_empty());
        let mut files = Vec::new();
        while let Some(status) = records.next() {
            let status = *status.first().context("Invalid commit file status")? as char;
            let path = PathBuf::from(os_string(
                records.next().context("Missing commit file path")?,
            ));
            let (path, original) = if matches!(status, 'R' | 'C') {
                (
                    PathBuf::from(os_string(
                        records.next().context("Missing rename destination")?,
                    )),
                    Some(path),
                )
            } else {
                (path, None)
            };
            files.push(File {
                path,
                original,
                index: status,
                worktree: ' ',
            });
        }
        Ok(CommitDetails { parent, files })
    }

    pub fn file_history(&self, path: &Path, start: Option<&str>) -> Result<Vec<FileCommit>> {
        let path = if path.is_absolute() {
            path.strip_prefix(&self.root)
                .context("File must be inside this repository")?
        } else {
            path
        };
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                Component::Normal(part) => normalized.push(part),
                Component::CurDir => {}
                Component::ParentDir => {
                    ensure!(normalized.pop(), "File must be inside this repository")
                }
                _ => anyhow::bail!("File must be inside this repository"),
            }
        }
        ensure!(
            !normalized.as_os_str().is_empty(),
            "Enter a repository-relative file path"
        );
        if start.is_none() && !self.head_exists() {
            return Ok(Vec::new());
        }
        let revision = self.strings(&[
            "rev-parse",
            "--verify",
            "--end-of-options",
            start.unwrap_or("HEAD"),
        ])?;
        let revision = std::str::from_utf8(&revision)?.trim();
        let mut object = OsString::from(format!("{revision}:"));
        object.push(normalized.as_os_str());
        if let Ok(kind) = self.run(&[OsStr::new("cat-file"), OsStr::new("-t"), &object]) {
            ensure!(kind != b"tree\n", "Choose a file rather than a directory");
        }
        let log = self.run(&[
            OsStr::new("log"),
            OsStr::new("--follow"),
            OsStr::new("--full-history"),
            OsStr::new("--diff-merges=first-parent"),
            OsStr::new("--find-renames"),
            OsStr::new("-z"),
            OsStr::new("--name-status"),
            OsStr::new("--format=%H%x00%s%x00%an%x00%ar%x00%P"),
            OsStr::new(revision),
            OsStr::new("--"),
            normalized.as_os_str(),
        ])?;
        parse_file_history(&log)
    }

    fn directory_preview(
        &self,
        path: &Path,
        files: &[File],
        staged: bool,
        commit: Option<&str>,
        browse: bool,
    ) -> Preview {
        let name = if path.as_os_str().is_empty() {
            self.root
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            path.display().to_string()
        };
        let mut text = format!(
            "Directory {name}/\n\n{} {}\n",
            files.len(),
            if browse { "files" } else { "changed files" }
        );
        for status in [
            "Added", "Modified", "Deleted", "Renamed", "Copied", "Conflict",
        ] {
            let count = files
                .iter()
                .filter(|f| f.status(if browse { f.worktree == ' ' } else { staged }) == status)
                .count();
            if count > 0 {
                text.push_str(&format!("{status}: {count}\n"));
            }
        }
        if browse {
            text.push_str("\nSelect a file and press Enter to view its history.\n\n");
        } else if let Some(id) = commit {
            text.push_str(&format!(
                "\nCommit {} · select a file to inspect its changes.\n\n",
                &id[..8]
            ));
        } else {
            text.push_str(&format!(
                "\n{} affects all {} listed files, including collapsed descendants.\n\n",
                if staged { "u: unstage" } else { "s: stage" },
                files.len()
            ));
        }
        for file in files {
            text.push_str(&format!(
                "{:<9} {}\n",
                file.status(if browse { file.worktree == ' ' } else { staged }),
                file.label()
            ));
        }
        Preview {
            title: format!("{name}/"),
            text,
            conflict: None,
            code_path: None,
            styles: OnceLock::new(),
        }
    }

    pub fn preview(&self, selection: &Selection) -> Result<Preview> {
        match selection {
            Selection::Stash(stash) => {
                let details = self.stash_files(&stash.id)?;
                let text = format!(
                    "{}\n{}\n{} · {}\nSnapshot {}\n\n{}",
                    stash.reference,
                    stash.subject,
                    stash.author,
                    stash.age,
                    stash.id,
                    stash_file_list(&details, &details.files)
                );
                Ok(Preview {
                    title: format!("{} · {}", stash.reference, &stash.id[..8]),
                    text,
                    conflict: None,
                    code_path: None,
                    styles: OnceLock::new(),
                })
            }
            Selection::StashDirectory {
                details,
                path,
                files,
            } => {
                let name = if path.as_os_str().is_empty() {
                    self.root
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                } else {
                    path.display().to_string()
                };
                Ok(Preview {
                    title: format!("{name}/ · stash {}", &details.id[..8]),
                    text: stash_file_list(details, files),
                    conflict: None,
                    code_path: None,
                    styles: OnceLock::new(),
                })
            }
            Selection::StashFile { details, file } => self.stash_preview(details, file),
            Selection::Directory(path, files, staged) => {
                Ok(self.directory_preview(path, files, *staged, None, false))
            }
            Selection::CommitDirectory { id, path, files } => {
                Ok(self.directory_preview(path, files, true, Some(id), false))
            }
            Selection::RepositoryDirectory(path, files) => {
                Ok(self.directory_preview(path, files, false, None, true))
            }
            Selection::RepositoryFile(file) => {
                let mut object = OsString::from("HEAD:");
                object.push(file.path.as_os_str());
                let id = self.run(&[OsStr::new("rev-parse"), OsStr::new("--verify"), &object])?;
                let id = std::str::from_utf8(&id)?.trim();
                let kind = self.strings(&["cat-file", "-t", id])?;
                let text = if kind == b"blob\n" {
                    self.blob(id)?
                } else {
                    format!("[Submodule commit {id}]")
                };
                Ok(Preview {
                    title: file.label(),
                    text,
                    conflict: None,
                    code_path: Some(file.path.clone()),
                    styles: OnceLock::new(),
                })
            }
            Selection::CommitFile { id, parent, file } => {
                let mut args = vec![
                    OsStr::new(if parent.is_some() { "diff" } else { "show" }),
                    OsStr::new("--no-ext-diff"),
                    OsStr::new("--no-textconv"),
                    OsStr::new("--no-color"),
                    OsStr::new("--find-renames"),
                ];
                if let Some(parent) = parent {
                    args.push(OsStr::new(parent));
                } else {
                    args.push(OsStr::new("--format="));
                    args.push(OsStr::new("--root"));
                }
                args.push(OsStr::new(id));
                args.push(OsStr::new("--"));
                args.push(file.path.as_os_str());
                if let Some(original) = &file.original {
                    args.push(original.as_os_str());
                }
                let text = String::from_utf8_lossy(&self.run(&args)?).into_owned();
                Ok(Preview {
                    title: format!("{} · {}", file.label(), &id[..8]),
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
                    "--no-patch",
                    "--no-ext-diff",
                    "--no-textconv",
                    id,
                ])?;
                let mut text = String::from_utf8_lossy(&text).into_owned();
                let details = self.commit_files(id)?;
                text.push_str(&format!("\nChanged files · {}\n", details.files.len()));
                for file in details.files {
                    text.push_str(&format!(" {}  {}\n", file.index, file.label()));
                }
                Ok(Preview {
                    title: format!("Commit {}", &id[..8]),
                    text,
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

fn stash_file_list(details: &StashDetails, files: &[File]) -> String {
    let count = |entries: &[File]| {
        entries
            .iter()
            .filter(|entry| files.iter().any(|file| file.path == entry.path))
            .count()
    };
    let mut text = format!(
        "Saved working tree: {} files\nSaved index: {} files\nSaved untracked files: {} files\n\nChanged files · {}\n",
        count(&details.worktree.files),
        count(&details.index.files),
        details
            .untracked
            .as_ref()
            .map_or(0, |comparison| count(&comparison.files)),
        files.len()
    );
    for file in files {
        text.push_str(&format!(" {}  {}\n", file.index, file.label()));
    }
    text
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

fn parse_file_history(bytes: &[u8]) -> Result<Vec<FileCommit>> {
    let mut records = bytes.split(|b| *b == 0).peekable();
    let mut entries = Vec::new();
    while let Some(id) = records.next() {
        let id = id.trim_ascii_start();
        if id.is_empty() {
            continue;
        }
        ensure!(
            matches!(id.len(), 40 | 64) && id.iter().all(u8::is_ascii_hexdigit),
            "Invalid file history commit"
        );
        let subject = records.next().context("Missing commit subject")?;
        let author = records.next().context("Missing commit author")?;
        let age = records.next().context("Missing commit age")?;
        let parents = records.next().context("Missing commit parents")?;
        let mut file = None;
        while let Some(status) = records.peek() {
            let status = status.trim_ascii_start();
            if status.is_empty() {
                records.next();
                continue;
            }
            if matches!(status.len(), 40 | 64) && status.iter().all(u8::is_ascii_hexdigit) {
                break;
            }
            let status = *status.first().context("Missing file status")? as char;
            ensure!(
                matches!(status, 'A' | 'M' | 'D' | 'R' | 'C' | 'T'),
                "Invalid file history status"
            );
            records.next();
            let path = PathBuf::from(os_string(
                records.next().context("Missing historical path")?,
            ));
            let (path, original) = if matches!(status, 'R' | 'C') {
                (
                    PathBuf::from(os_string(
                        records
                            .next()
                            .context("Missing historical rename destination")?,
                    )),
                    Some(path),
                )
            } else {
                (path, None)
            };
            ensure!(file.is_none(), "Expected a single file in followed history");
            file = Some(File {
                path,
                original,
                index: status,
                worktree: ' ',
            });
        }
        if let Some(file) = file {
            entries.push(FileCommit {
                commit: Commit {
                    id: String::from_utf8_lossy(id).into(),
                    subject: String::from_utf8_lossy(subject).into(),
                    author: String::from_utf8_lossy(author).into(),
                    age: String::from_utf8_lossy(age).into(),
                },
                parent: std::str::from_utf8(parents)?
                    .split_whitespace()
                    .next()
                    .map(str::to_owned),
                file,
            });
        }
    }
    Ok(entries)
}

pub enum Request {
    Refresh,
    AutoRefresh,
    Preview(u64, Selection),
    CommitFiles(u64, String),
    FileHistory(u64, PathBuf, Option<String>),
    StashFiles(u64, String),
    Stage(Vec<File>),
    Unstage(Vec<File>),
    Commit(String),
}

pub enum Response {
    Snapshot(Result<Snapshot, String>, Option<String>, bool),
    Preview(u64, Result<Preview, String>),
    CommitFiles(u64, Result<CommitDetails, String>),
    FileHistory(u64, Result<Vec<FileCommit>, String>),
    StashFiles(u64, Result<Arc<StashDetails>, String>),
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
                Request::Refresh | Request::AutoRefresh => Response::Snapshot(
                    repo.snapshot().map_err(|e| format!("{e:#}")),
                    None,
                    matches!(request, Request::AutoRefresh),
                ),
                Request::Preview(id, selection) => Response::Preview(
                    id,
                    repo.preview(&selection)
                        .inspect(|preview| {
                            preview.styled();
                        })
                        .map_err(|e| format!("{e:#}")),
                ),
                Request::CommitFiles(token, id) => Response::CommitFiles(
                    token,
                    repo.commit_files(&id).map_err(|e| format!("{e:#}")),
                ),
                Request::FileHistory(token, path, start) => Response::FileHistory(
                    token,
                    repo.file_history(&path, start.as_deref())
                        .map_err(|e| format!("{e:#}")),
                ),
                Request::StashFiles(token, id) => Response::StashFiles(
                    token,
                    repo.stash_files(&id).map_err(|error| format!("{error:#}")),
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
                            false,
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

    #[test]
    fn historical_files_and_patches_ignore_current_index_and_worktree() {
        let (_dir, repo) = fixture();
        fs::create_dir(repo.root.join("src")).unwrap();
        fs::write(repo.root.join("src/旧\n名字.rs"), "fn original() {}\n").unwrap();
        fs::write(repo.root.join("gone.txt"), "deleted content\n").unwrap();
        fs::write(repo.root.join("other.txt"), "before\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("root").unwrap();
        let root = String::from_utf8(repo.strings(&["rev-parse", "HEAD"]).unwrap())
            .unwrap()
            .trim()
            .to_owned();
        let details = repo.commit_files(&root).unwrap();
        assert!(details.parent.is_none());
        assert_eq!(details.files.len(), 3);
        assert!(details.files.iter().all(|file| file.index == 'A'));
        let file = details
            .files
            .iter()
            .find(|file| file.path.starts_with("src"))
            .unwrap()
            .clone();
        let preview = repo
            .preview(&Selection::CommitFile {
                id: root.clone(),
                parent: None,
                file,
            })
            .unwrap();
        assert!(preview.text.contains("+fn original() {}"));
        assert!(!preview.text.contains("gone.txt"));

        repo.strings(&["mv", "src/旧\n名字.rs", "src/新名字.rs"])
            .unwrap();
        fs::remove_file(repo.root.join("gone.txt")).unwrap();
        fs::write(repo.root.join("other.txt"), "historical change\n").unwrap();
        fs::write(repo.root.join("added.txt"), "new historical file\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("second").unwrap();
        let id = String::from_utf8(repo.strings(&["rev-parse", "HEAD"]).unwrap())
            .unwrap()
            .trim()
            .to_owned();
        let details = repo.commit_files(&id).unwrap();
        assert_eq!(details.parent.as_deref(), Some(root.as_str()));
        assert_eq!(details.files.len(), 4);
        let renamed = details.files.iter().find(|file| file.index == 'R').unwrap();
        assert_eq!(
            renamed.original.as_deref(),
            Some(Path::new("src/旧\n名字.rs"))
        );
        fs::write(repo.root.join("other.txt"), "INDEX ONLY\n").unwrap();
        repo.strings(&["add", "other.txt"]).unwrap();
        fs::write(repo.root.join("other.txt"), "WORKTREE ONLY\n").unwrap();
        fs::remove_file(repo.root.join("src/新名字.rs")).unwrap();
        let before = repo.strings(&["status", "--porcelain=v1", "-z"]).unwrap();
        for file in details.files {
            let kind = file.index;
            let preview = repo
                .preview(&Selection::CommitFile {
                    id: id.clone(),
                    parent: details.parent.clone(),
                    file,
                })
                .unwrap();
            assert_eq!(preview.text.matches("diff --git ").count(), 1);
            assert!(!preview.text.contains("INDEX ONLY"));
            assert!(!preview.text.contains("WORKTREE ONLY"));
            match kind {
                'R' => assert!(preview.text.contains("rename from ")),
                'D' => assert!(preview.text.contains("-deleted content")),
                'M' => assert!(preview.text.contains("+historical change")),
                'A' => assert!(preview.text.contains("+new historical file")),
                _ => panic!("Unexpected historical status"),
            }
        }
        let summary = repo.preview(&Selection::Commit(id)).unwrap();
        assert!(!summary.text.contains("diff --git"));
        assert!(summary.text.contains("Changed files · 4"));
        assert!(summary.text.contains(" M  other.txt"));
        assert!(summary.text.contains(" D  gone.txt"));
        assert_eq!(
            repo.strings(&["status", "--porcelain=v1", "-z"]).unwrap(),
            before
        );
    }

    #[test]
    fn merge_history_compares_first_parent_and_empty_commits_have_no_files() {
        let (_dir, repo) = fixture();
        fs::write(repo.root.join("base.txt"), "base\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("base").unwrap();
        repo.strings(&["checkout", "-b", "topic"]).unwrap();
        fs::write(repo.root.join("incoming.txt"), "incoming\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("incoming").unwrap();
        repo.strings(&["checkout", "main"]).unwrap();
        fs::write(repo.root.join("own.txt"), "own\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("own").unwrap();
        repo.strings(&["merge", "--no-ff", "-m", "merge topic", "topic"])
            .unwrap();
        let id = String::from_utf8(repo.strings(&["rev-parse", "HEAD"]).unwrap())
            .unwrap()
            .trim()
            .to_owned();
        let details = repo.commit_files(&id).unwrap();
        assert_eq!(details.files.len(), 1);
        assert_eq!(details.files[0].path, Path::new("incoming.txt"));
        let parent = String::from_utf8(repo.strings(&["rev-parse", "HEAD^1"]).unwrap()).unwrap();
        assert_eq!(details.parent.as_deref(), Some(parent.trim()));
        let preview = repo
            .preview(&Selection::CommitFile {
                id,
                parent: details.parent,
                file: details.files[0].clone(),
            })
            .unwrap();
        assert!(preview.text.contains("+incoming"));
        assert!(!preview.text.contains("own.txt"));
        let history = repo.file_history(Path::new("incoming.txt"), None).unwrap();
        assert!(
            history
                .iter()
                .any(|entry| entry.commit.subject == "merge topic")
        );
        assert!(
            history
                .iter()
                .any(|entry| entry.commit.subject == "incoming")
        );
        let merge = history
            .iter()
            .find(|entry| entry.commit.subject == "merge topic")
            .unwrap();
        assert_eq!(merge.parent.as_deref(), Some(parent.trim()));
        repo.strings(&["commit", "--allow-empty", "-m", "empty"])
            .unwrap();
        assert!(repo.commit_files("HEAD").unwrap().files.is_empty());
    }

    #[test]
    fn followed_file_history_tracks_old_paths_deletion_and_single_file_patches() {
        let (_dir, repo) = fixture();
        fs::create_dir(repo.root.join("src")).unwrap();
        let old = "src/:(glob)旧\n名字.rs";
        let new = "src/新名字.rs";
        fs::write(repo.root.join(old), "original\n").unwrap();
        fs::write(repo.root.join("other.txt"), "other\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("create file").unwrap();
        fs::write(repo.root.join(old), "first edit\n").unwrap();
        fs::write(repo.root.join("other.txt"), "other edit\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("first edit").unwrap();
        repo.strings(&["mv", old, new]).unwrap();
        repo.commit("rename file").unwrap();
        let rename = String::from_utf8(repo.strings(&["rev-parse", "HEAD"]).unwrap())
            .unwrap()
            .trim()
            .to_owned();
        fs::write(repo.root.join(new), "second edit\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("second edit").unwrap();
        repo.strings(&["rm", new]).unwrap();
        repo.commit("delete file").unwrap();
        fs::write(repo.root.join("other.txt"), "current working content\n").unwrap();
        let before = repo.strings(&["status", "--porcelain=v1", "-z"]).unwrap();
        let history = repo.file_history(Path::new(new), None).unwrap();
        assert_eq!(
            history
                .iter()
                .map(|entry| entry.commit.subject.as_str())
                .collect::<Vec<_>>(),
            [
                "delete file",
                "second edit",
                "rename file",
                "first edit",
                "create file"
            ]
        );
        assert_eq!(
            history
                .iter()
                .map(|entry| entry.file.index)
                .collect::<Vec<_>>(),
            ['D', 'M', 'R', 'M', 'A']
        );
        assert_eq!(history[3].file.path, Path::new(old));
        assert_eq!(history[2].file.original.as_deref(), Some(Path::new(old)));
        for entry in &history {
            let preview = repo
                .preview(&Selection::CommitFile {
                    id: entry.commit.id.clone(),
                    parent: entry.parent.clone(),
                    file: entry.file.clone(),
                })
                .unwrap();
            assert_eq!(preview.text.matches("diff --git ").count(), 1);
            assert!(!preview.text.contains("other.txt"));
            assert!(!preview.text.contains("current working content"));
        }
        let anchored = repo.file_history(Path::new(new), Some(&rename)).unwrap();
        assert_eq!(anchored.len(), 3);
        assert_eq!(anchored[0].commit.subject, "rename file");
        assert!(
            repo.file_history(Path::new("does-not-exist"), None)
                .unwrap()
                .is_empty()
        );
        assert!(repo.file_history(Path::new("src"), Some(&rename)).is_err());
        assert!(repo.file_history(Path::new("../outside"), None).is_err());
        assert_eq!(
            repo.strings(&["status", "--porcelain=v1", "-z"]).unwrap(),
            before
        );
    }

    #[test]
    fn repository_browser_uses_head_and_ignores_all_uncommitted_path_and_content_changes() {
        let (_dir, repo) = fixture();
        fs::write(repo.root.join("clean.txt"), "clean\n").unwrap();
        fs::write(repo.root.join("old.txt"), "old\n").unwrap();
        fs::write(repo.root.join("deleted.txt"), "deleted\n").unwrap();
        assert!(repo.snapshot().unwrap().committed_files.is_empty());
        repo.strings(&["add", "."]).unwrap();
        repo.commit("initial").unwrap();
        fs::write(repo.root.join("clean.txt"), "uncommitted content\n").unwrap();
        repo.strings(&["mv", "old.txt", "renamed.txt"]).unwrap();
        fs::write(repo.root.join("new.txt"), "staged addition\n").unwrap();
        repo.strings(&["add", "new.txt"]).unwrap();
        fs::write(repo.root.join("untracked.txt"), "untracked\n").unwrap();
        fs::remove_file(repo.root.join("deleted.txt")).unwrap();
        let snapshot = repo.snapshot().unwrap();
        assert_eq!(
            snapshot
                .committed_files
                .iter()
                .map(|file| file.path.as_path())
                .collect::<Vec<_>>(),
            [
                Path::new("clean.txt"),
                Path::new("deleted.txt"),
                Path::new("old.txt")
            ]
        );
        assert!(
            snapshot
                .committed_files
                .iter()
                .all(|file| file.status(true) == "Clean" && file.status(false) == "Clean")
        );
        let clean = snapshot
            .committed_files
            .iter()
            .find(|file| file.path == Path::new("clean.txt"))
            .unwrap();
        assert_eq!(
            repo.preview(&Selection::RepositoryFile(clean.clone()))
                .unwrap()
                .text,
            "clean\n"
        );
        let deleted = snapshot
            .committed_files
            .iter()
            .find(|file| file.path == Path::new("deleted.txt"))
            .unwrap();
        assert_eq!(
            repo.preview(&Selection::RepositoryFile(deleted.clone()))
                .unwrap()
                .text,
            "deleted\n"
        );
    }

    #[test]
    fn stash_inspection_includes_worktree_index_only_and_untracked_changes() {
        let (_dir, repo) = fixture();
        for name in ["tracked.txt", "index-only.txt", "deleted.txt", "old.txt"] {
            fs::write(repo.root.join(name), format!("base {name}\n")).unwrap();
        }
        repo.strings(&["add", "."]).unwrap();
        repo.commit("base").unwrap();
        fs::write(repo.root.join("tracked.txt"), "STAGED VERSION\n").unwrap();
        fs::write(repo.root.join("index-only.txt"), "INDEX ONLY\n").unwrap();
        repo.strings(&["add", "tracked.txt", "index-only.txt"])
            .unwrap();
        fs::write(repo.root.join("tracked.txt"), "WORKTREE VERSION\n").unwrap();
        fs::write(repo.root.join("index-only.txt"), "base index-only.txt\n").unwrap();
        fs::remove_file(repo.root.join("deleted.txt")).unwrap();
        repo.strings(&["mv", "old.txt", "renamed.txt"]).unwrap();
        fs::write(repo.root.join("临时\n文件.txt"), "UNTRACKED CONTENT\n").unwrap();
        repo.strings(&["stash", "push", "--include-untracked", "-m", "all layers"])
            .unwrap();
        let item = repo.snapshot().unwrap().stashes.remove(0);
        assert_eq!(item.reference, "stash@{0}");
        let details = repo.stash_files(&item.id).unwrap();
        assert_eq!(details.files.len(), 5);
        assert!(
            details
                .worktree
                .files
                .iter()
                .all(|file| file.path != Path::new("index-only.txt"))
        );
        assert!(
            details
                .index
                .files
                .iter()
                .any(|file| file.path == Path::new("index-only.txt"))
        );
        assert_eq!(details.untracked.as_ref().unwrap().files.len(), 1);
        fs::write(repo.root.join("tracked.txt"), "CURRENT CONTENT\n").unwrap();
        let before = repo.strings(&["status", "--porcelain=v1", "-z"]).unwrap();
        let mut previews = std::collections::BTreeMap::new();
        for file in &details.files {
            let preview = repo
                .preview(&Selection::StashFile {
                    details: details.clone(),
                    file: file.clone(),
                })
                .unwrap();
            assert!(!preview.text.contains("CURRENT CONTENT"));
            previews.insert(file.path.clone(), preview.text);
        }
        assert!(previews[Path::new("tracked.txt")].contains("+STAGED VERSION"));
        assert!(previews[Path::new("tracked.txt")].contains("+WORKTREE VERSION"));
        assert!(previews[Path::new("index-only.txt")].contains("Saved index"));
        assert!(previews[Path::new("index-only.txt")].contains("+INDEX ONLY"));
        assert!(previews[Path::new("临时\n文件.txt")].contains("+UNTRACKED CONTENT"));
        assert!(previews[Path::new("renamed.txt")].contains("rename from old.txt"));
        assert!(previews[Path::new("deleted.txt")].contains("-base deleted.txt"));
        let summary = repo.preview(&Selection::Stash(item)).unwrap();
        assert!(summary.text.contains("Changed files · 5"));
        assert!(summary.text.contains("临时\n文件.txt"));
        assert_eq!(
            repo.strings(&["status", "--porcelain=v1", "-z"]).unwrap(),
            before
        );
        assert_eq!(repo.snapshot().unwrap().stashes.len(), 1);
    }

    #[test]
    fn stash_contents_remain_pinned_when_reflog_indices_change() {
        let (_dir, repo) = fixture();
        assert!(repo.snapshot().unwrap().stashes.is_empty());
        fs::write(repo.root.join("file"), "base\n").unwrap();
        repo.strings(&["add", "."]).unwrap();
        repo.commit("base").unwrap();
        fs::write(repo.root.join("file"), "FIRST STASH\n").unwrap();
        repo.strings(&["stash", "push", "-m", "first"]).unwrap();
        let first = repo.snapshot().unwrap().stashes.remove(0);
        fs::write(repo.root.join("file"), "SECOND STASH\n").unwrap();
        repo.strings(&["stash", "push", "-m", "second"]).unwrap();
        let items = repo.snapshot().unwrap().stashes;
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].id, first.id);
        assert_eq!(items[1].reference, "stash@{1}");
        let details = repo.stash_files(&first.id).unwrap();
        assert!(details.untracked.is_none());
        let preview = repo
            .preview(&Selection::StashFile {
                details: details.clone(),
                file: details.files[0].clone(),
            })
            .unwrap();
        assert!(preview.text.contains("+FIRST STASH"));
        assert!(!preview.text.contains("SECOND STASH"));
        assert_eq!(
            repo.preview(&Selection::Stash(first))
                .unwrap()
                .text
                .matches("Changed files")
                .count(),
            1
        );
    }
}
