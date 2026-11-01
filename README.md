# giter

A Rust + Ratatui Git TUI with Vim navigation, syntax-colored code previews, explicit conflict inspection, and Unicode text entry.

```sh
cargo run --release -- /path/to/repository
```

Requires Git, an interactive terminal, and a working tree. The default path is the current directory. Minimum terminal size: 60 × 14; a wider terminal gives conflict sources more room.

## Workflow

Use `1` for working-tree changes, `2` for staged changes, `3` for the latest 150 commits, and `4` for all repository files. `Ctrl+h` / `Ctrl+l` cycle to the previous / next tab. Changes, Staged, and commit file trees show colored status letters: `A` added, `M` modified, `D` deleted, `R` renamed, `C` copied, and `U` conflicted. Added files, including untracked files, appear in green. The Files tree shows committed filenames without status letters or change counts. Other trees summarize changed statuses with counts. Conflict entries and directories containing conflicts sort first.

Select a file to inspect its diff. Select a directory, including the root, to see its change summary. `s` / `u` stage / unstage all files listed under that directory, including collapsed descendants. With an active search, the scope includes only matching files; matching ancestors expand automatically. Directory staging checks all selected conflicts before updating the index. Git reads and writes run on a worker thread. Press `r` to refresh after external changes.

In History, selecting a commit shows its metadata and a list of changed files with `A/M/D/R` statuses. Press Enter or `l` to browse that commit's changed files in the same collapsible tree. Select a file to see only its patch, with the same status colors and navigation. `/` filters historical paths. Esc or Backspace returns to the commit list and restores its selection and search. Historical views are read-only and use committed content, independent of the current index and working tree. Merge commits compare against their first parent; root commits compare against the empty tree.

## File history

The Files tab has two columns: committed file tree on the left, the selected file's commits on the right. Moving to a file loads its commit history. Enter or `l` opens the next view: the commit list moves to the left and that commit's file changes appear on the right. Esc or `h` from the commit list returns to the file tree, preserving the selected file and scroll position.

The tree is read from HEAD and lists committed files only, without working-tree or index statuses. Untracked files, staged additions, and uncommitted renames are excluded. `/` in the tree filters file paths. `f` also opens the selected file's history from Changes, Staged, or a historical commit's file tree.

In the commit/change view, `j/k` select commits and immediately display their file patches. Enter, `l`, or Tab focuses the patch for scrolling; `h` returns to the commit list, and Esc returns to the file tree. `/` filters commit IDs, subjects, or authors; `r` reloads history. History follows Git-detected renames using each revision's filename, without the main History list's 150-commit cap. From a historical file it starts at the selected commit; otherwise it starts at HEAD. When opened from another tab, Esc from the file tree returns to that tab.

| Key | Action |
| --- | --- |
| `j/k`, arrows | Move through files or scroll the preview |
| `h/l` | Collapse / expand directory; move to parent / enter child; `l` on a file focuses preview |
| Enter / Space | Toggle selected directory; Enter on a file opens preview |
| `Tab` | Switch focus between tree and preview |
| `Ctrl+h/l` | Previous / next tab, wrapping around |
| Enter / `l` on a commit | Open its changed-file tree |
| Esc / Backspace in commit files | Return to the commit list |
| Enter / `l` on a file in Files | Move to the commit list and file changes |
| `f` | Show the selected file's historical commits and patches |
| `gg`, `G` | First / last item or line |
| `Ctrl+d/u`, Page Down/Up | Scroll half / full page |
| `H/L` | Scroll content horizontally |
| `/` | Filter current list by path, or commit ID/subject/author |
| `s` | Stage selected file or directory, including resolved conflicts |
| `u` | Unstage selected staged file or directory |
| `c` | Compose a commit; Enter inserts a newline, `Ctrl+s` submits |
| `e` | Edit the selected working-tree file |
| `r` | Refresh |
| `?` | Keyboard help |
| `q`, `Ctrl+c` | Quit |

## Conflicts

The conflict view shows ours and theirs above the working file, with a base pane when the preview is wide enough. On narrower terminals, `b` switches the left pane between ours and base.

- `[` / `]` select the previous / next conflict block and reveal it in the working file.
- Marker blocks display their exact ours/theirs sections. Diff3/zdiff3 base sections and custom marker widths of at least seven characters are recognized.
- `v` switches between marker blocks and complete Git index sources: stage `:1` is base, `:2` is ours, and `:3` is theirs.
- Files without marker blocks still show index sources. Missing sides, binary files, non-UTF-8 content, symbolic links, and submodule commits have explicit labels. Blob and working-file previews are limited to 2 MiB.
- `e` uses `git var GIT_EDITOR`, honoring `GIT_EDITOR`, `core.editor`, `VISUAL`, and `EDITOR`. Terminal state is restored while the editor runs. A POSIX `sh` is required for editor commands.
- Edit and save the file, leave the editor, then press `s` to mark it resolved. Remaining conflict marker lines block staging; the complete file is checked, including content beyond the preview limit. For a deletion resolution, remove the file externally, refresh, then stage it.

Ours/theirs follow Git's index terminology. During a rebase, ours is the upstream state and theirs is the commit being replayed. Review the content before resolving.

## Chinese and IME input

Search and commit fields accept Unicode characters and bracketed paste. Vim shortcuts are inactive while typing. Cursor placement uses terminal cell widths; Left/Right, Backspace, and Delete operate on grapheme clusters, including Chinese, combining marks, and emoji. Up/Down navigate commit message lines.

Configure the input method in your terminal emulator. The terminal handles IME preedit and candidate windows and sends committed text to giter. Candidate positioning and composition behavior depend on the terminal, desktop session, and IME. Automated tests cover Unicode text, paste, and cursor placement; a live IME composition session has not been verified.

## Scope

This version supports file and directory staging/unstaging, commit creation, recent commit inspection, repository file browsing, file history across renames, conflict inspection, and external editing. It does not provide hunk staging, branch switching, fetch/push, or an embedded merge editor. Repository changes are refreshed explicitly. Commit hooks and signing use your Git configuration; Git subprocesses do not have an interactive terminal.

```sh
cargo test --offline
cargo clippy --offline --all-targets -- -D warnings
```
