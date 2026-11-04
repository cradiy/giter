# giter

A Rust + Ratatui Git TUI with Vim navigation, syntax-colored code previews, explicit conflict inspection, and Unicode text entry.

```sh
cargo run --release -- /path/to/repository
```

Requires Git, an interactive terminal, and a working tree. The default path is the current directory. Minimum terminal size: 60 × 14; a wider terminal gives conflict sources more room.

## Workflow

Two-column views split the available width equally.

Use `1` for changes, `2` for branches and commit history, `3` for committed repository files, and `4` for stashes. The Changes view shows unstaged files in the upper-left tree and staged files in the lower-left tree, with a shared diff preview on the right. `Ctrl+j` focuses Staged and `Ctrl+k` focuses Changes. Moving down past the last Changes row or up past the first Staged row with `j/k` or the arrow keys crosses to the adjacent tree. Each tree retains its own selection, search, collapsed directories, and scroll position. `Ctrl+h` / `Ctrl+l` cycle to the previous / next tab. Changes, Staged, and commit file trees show colored status letters: `A` added, `M` modified, `D` deleted, `R` renamed, `C` copied, and `U` conflicted. Added files, including untracked files, appear in green. The Files tree shows committed filenames without status letters or change counts. Other trees summarize changed statuses with counts. Changes and Staged sort sibling files and directories together by name. Other file trees list directories first.

Select a file to preview its diff. Enter, `l`, or Tab opens the file in a full-screen diff view. Esc, `h`, or Tab returns to the same file list, preserving selection, filters, and preview scroll. Full-screen diffs hide tabs and the repository bar, retaining the file title and navigation shortcuts. Historical and stash files use the same view. Select a directory, including the root, to see its change summary. `s` / `u` stage / unstage all files listed under that directory, including collapsed descendants. With an active search, the scope includes only matching files; matching ancestors expand automatically. Directory staging checks all selected conflicts before updating the index. Git reads and writes run on a worker thread.

Narrow file trees prioritize names over indentation. Hidden leading tree branches appear as `…`; names that still exceed the available width use a middle ellipsis. Status letters stay visible, and Git operations use the full original paths.

Filesystem notifications refresh working files, the index, branches, history, and stashes after external changes. Batches use a 180 ms quiet period and a one-second debounce limit. Watching respects Git ignore rules, includes tracked paths inside ignored directories, and supports linked worktrees and shared Git refs. Git object storage is excluded. Automatic refresh preserves selection, searches, and preview scroll; refresh waits while a search or commit message is being entered. Historical commit and stash contents remain pinned. Press `r` for a manual refresh.

In the Branches commit graph, selecting a commit shows its metadata and a list of changed files with `A/M/D/R` statuses. Press Enter or `l` to browse that commit's changed files in the same collapsible tree. Select a file to see only its patch, with the same status colors and navigation. `/` filters historical paths. Esc or Backspace returns to the commit list and restores its selection and search. Historical views are read-only and use committed content, independent of the current index and working tree. Merge commits compare against their first parent; root commits compare against the empty tree.

## File history

The Files tab has two columns: committed file tree on the left, the selected file's commits on the right. Moving to a file loads its commit history. Enter or `l` opens the next view: the commit list moves to the left and that commit's file changes appear on the right. Esc or `h` from the commit list returns to the file tree, preserving the selected file and scroll position.

The tree is read from HEAD and lists committed files only, without working-tree or index statuses. Untracked files, staged additions, and uncommitted renames are excluded. `/` in the tree filters file paths. `f` also opens the selected file's history from Changes, Staged, or a historical commit's file tree.

In the commit/change view, `j/k` select commits and immediately display their file patches. Enter, `l`, or Tab opens the full-screen patch. Esc, `h`, or Tab returns to the commit list; Esc from that list returns to the file tree. `/` filters commit IDs, subjects, or authors; `r` reloads history. History follows Git-detected renames using each revision's filename, with no fixed commit limit. From a historical file it starts at the selected commit; otherwise it starts at HEAD. When opened from another tab, Esc from the file tree returns to that tab.

## Branches

Press `2` for the branch list and the selected branch's own commit history. Selecting another branch changes the displayed history; commits reachable only from HEAD are excluded. The summary shows how many commits the selected branch is ahead of or behind the current branch. Curves and commit nodes are antialiased images displayed through the [Kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/); branch graphs require a terminal supporting that protocol. Commit text, navigation, and search remain terminal widgets. Images update with scrolling and resizing, and clear when leaving the graph or opening an input popup.

Local and remote refs appear as labels on their commits; green `HEAD` labels identify the checked-out branch. Local branches appear first, with the current branch at the top. Remote branches start collapsed; `h/l`, Enter, or Space operates their group. `/` searches branch names and upstreams, including collapsed remote branches. Upstream indicators show `↑` ahead, `↓` behind, `=` synchronized, or `gone`; `[worktree]` marks branches occupied by another worktree. Remote information uses locally stored refs.

When HEAD is detached, a `detached HEAD` entry provides access to the checked-out commit and its history, including commits not reachable from a named branch.

Enter, `l`, or Tab moves the graph to the left and displays the selected commit's message and changed-file summary on the right. `j/k` selects commits; Enter or `l` opens that commit's file tree and individual patches. Esc returns from files to the graph, then from the graph to the branch list. `h` returns from the graph to branches; when the patch pane is focused it first returns focus to the graph. Returning preserves the selected commit, search, and list position.

Graphs load in batches of 100 commits, extending as you approach the end. `G` moves to the end of the loaded history and requests the next batch when available; `+` beside the count indicates more commits. Graph search filters the loaded commits and shows isolated nodes while filtered. Clearing the search restores the full graph. Browsing keeps the working tree unchanged and historical contents pinned by commit ID. Files-tab history remains scoped to its selected file.

Press `s` on a branch in the branch list to switch explicitly. A remote ref opens its matching local tracking branch, creating one when needed. Git checks for conflicting local changes and worktree occupancy. `s` does not switch branches while viewing commits or historical files. Branch refs and graphs refresh automatically after external changes; `r` also refreshes manually.

## Stashes

Press `4` to browse the stash list. Selecting a stash shows its message and changed-file summary. Enter or `l` opens its collapsible file tree; selecting a file shows only that file's saved changes. `/` filters stashes or paths, `r` refreshes, and Esc / Backspace returns to the stash list with its selection and search retained.

Stash previews include saved working-tree changes, the saved index, and untracked or ignored files included when the stash was created. Index-only changes remain visible even if the saved working file matches the base. When working-tree and index patches are identical, they share one section. Stashes are inspected through their commit IDs, so content stays pinned if stash numbers change. This view provides inspection only.

| Key | Action |
| --- | --- |
| `j/k`, arrows | Move through files or scroll the preview |
| `h/l` | Collapse / expand directory; move to parent / enter child; `l` on a file opens full-screen diff |
| Enter / Space | Toggle selected directory; Enter on a file opens full-screen diff |
| `Tab` | Open a selected file diff; return from full-screen diff; otherwise switch pane focus |
| `Ctrl+h/l` | Previous / next tab, wrapping around |
| `Ctrl+j/k` | Focus Staged / Changes in the combined Changes view |
| `j/k`, up/down at adjacent tree edges | Move between Changes and Staged |
| Enter / `l` on a commit or stash | Open its changed-file tree |
| Esc / Backspace in commit or stash files | Return to its list |
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

This version supports file and directory staging/unstaging, commit creation, recent commit inspection, repository file browsing, file history across renames, conflict inspection, and external editing. It does not provide hunk staging, branch deletion, merge/rebase commands, fetch/push, or an embedded merge editor. Repository changes refresh automatically. Commit hooks and signing use your Git configuration; Git subprocesses do not have an interactive terminal.

```sh
cargo test --offline
cargo clippy --offline --all-targets -- -D warnings
```
