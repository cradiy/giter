# giter

A Rust + Ratatui Git TUI with Vim navigation, syntax-colored code previews, explicit conflict inspection, and Unicode text entry.

```sh
cargo run --release -- /path/to/repository
```

Requires Git, an interactive terminal, and a working tree. The default path is the current directory. Minimum terminal size: 60 × 14; a wider terminal gives conflict sources more room.

## Workflow

Use `1` for working-tree changes, `2` for staged changes, and `3` for the latest 150 commits. `Ctrl+h` / `Ctrl+l` cycle to the previous / next tab. Changes and Staged show a collapsible tree rooted at the repository directory, with colored status letters: `A` added, `M` modified, `D` deleted, `R` renamed, `C` copied, and `U` conflicted. Added files, including untracked files, appear in green. Directories summarize these letters with counts. Conflict entries and directories containing conflicts sort first.

Select a file to inspect its diff. Select a directory, including the root, to see its change summary. `s` / `u` stage / unstage all files listed under that directory, including collapsed descendants. With an active search, the scope includes only matching files; matching ancestors expand automatically. Directory staging checks all selected conflicts before updating the index. Git reads and writes run on a worker thread. Press `r` to refresh after external changes.

| Key | Action |
| --- | --- |
| `j/k`, arrows | Move through files or scroll the preview |
| `h/l` | Collapse / expand directory; move to parent / enter child; `l` on a file focuses preview |
| Enter / Space | Toggle selected directory; Enter on a file opens preview |
| `Tab` | Switch focus between tree and preview |
| `Ctrl+h/l` | Previous / next tab, wrapping around |
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

This version supports file and directory staging/unstaging, commit creation, recent commit inspection, conflict inspection, and external editing. It does not provide hunk staging, branch switching, fetch/push, or an embedded merge editor. Repository changes are refreshed explicitly. Commit hooks and signing use your Git configuration; Git subprocesses do not have an interactive terminal.

```sh
cargo test --offline
cargo clippy --offline --all-targets -- -D warnings
```
