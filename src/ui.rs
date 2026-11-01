use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::{
    app::{App, Mode, Tab},
    input::Input,
    theme::*,
};

fn block(title: impl Into<Line<'static>>, focused: bool) -> Block<'static> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused { BLUE } else { BORDER }))
        .style(Style::default().bg(BG).fg(FG))
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG).fg(FG)), area);
    if area.width < 60 || area.height < 14 {
        frame.render_widget(
            Paragraph::new("giter · resize terminal to at least 60 × 14\nq / Ctrl+C to exit")
                .style(Style::default().fg(AMBER)),
            area,
        );
        return;
    }
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Min(4),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .split(area);
    let snapshot = app.snapshot.as_ref();
    let branch = snapshot.map_or("…", |s| s.branch.as_str());
    let operation = snapshot.map_or("", |s| s.operation.as_str());
    let conflicts = snapshot.map_or(0, |s| s.files.iter().filter(|f| f.conflicted()).count());
    let mut header = vec![
        Span::styled(
            " giter ",
            Style::default()
                .bg(PURPLE)
                .fg(BG)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {}", safe(branch)), Style::default().fg(GREEN)),
        Span::styled(format!("  {}", safe(operation)), Style::default().fg(AMBER)),
        Span::styled(
            format!("  {}", safe(&app.repo.root.display().to_string())),
            Style::default().fg(MUTED),
        ),
    ];
    if let Some(history) = &app.file_history {
        header.push(Span::styled(
            format!(
                "  File history / {}",
                safe(&history.path.display().to_string())
            ),
            Style::default().fg(CYAN),
        ));
    } else if let Some(stash) = &app.stash {
        header.push(Span::styled(
            format!(
                "  Stash / {} · {}",
                safe(&stash.item.reference),
                &stash.item.id[..8]
            ),
            Style::default().fg(PEACH),
        ));
    } else if let Some(history) = &app.history {
        header.push(Span::styled(
            format!(
                "  History / {} · {}",
                &history.commit.id[..8],
                safe(&history.commit.subject)
            ),
            Style::default().fg(PURPLE),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(header)), rows[0]);
    let counts = snapshot
        .map(|s| {
            [
                s.files.iter().filter(|f| f.unstaged()).count(),
                s.files.iter().filter(|f| f.staged()).count(),
                s.commits.len(),
                s.committed_files.len(),
                s.stashes.len(),
            ]
        })
        .unwrap_or([0; 5]);
    let mut tabs = Vec::new();
    for (i, (tab, name)) in [
        (Tab::Changes, "Changes"),
        (Tab::Staged, "Staged"),
        (Tab::Log, "History"),
        (Tab::Files, "Files"),
        (Tab::Stash, "Stash"),
    ]
    .into_iter()
    .enumerate()
    {
        let accent = match tab {
            Tab::Changes => BLUE,
            Tab::Staged => GREEN,
            Tab::Log => PURPLE,
            Tab::Files => CYAN,
            Tab::Stash => PEACH,
        };
        tabs.push(Span::styled(
            if area.width < 85 {
                format!(" {} {name} ", i + 1)
            } else {
                format!(" {} {name} {} ", i + 1, counts[i])
            },
            Style::default()
                .fg(if app.tab == tab { BG } else { accent })
                .bg(if app.tab == tab { accent } else { PANEL })
                .add_modifier(if app.tab == tab {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ));
        tabs.push(Span::raw("  "));
    }
    if conflicts > 0 {
        tabs.push(Span::styled(
            format!(
                "{conflicts} CONFLICT{}",
                if conflicts == 1 { "" } else { "S" }
            ),
            Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(tabs)), rows[1]);
    if app.tab == Tab::Files {
        draw_files(frame, rows[2], app);
    } else {
        let sidebar = (area.width / 3).clamp(23, 42);
        let columns =
            Layout::horizontal([Constraint::Length(sidebar), Constraint::Min(20)]).split(rows[2]);
        app.page_height = if app.list_focus {
            columns[0].height.saturating_sub(2) as usize
        } else {
            columns[1].height.saturating_sub(2) as usize
        };
        render_list(frame, columns[0], app);
        render_preview(frame, columns[1], app);
    }
    frame.render_widget(
        Paragraph::new(format!(
            " {}{}",
            if app.busy { "Working…  " } else { "" },
            safe(&app.message)
        ))
        .style(Style::default().fg(if app.error { RED } else { MUTED }))
        .wrap(Wrap { trim: false }),
        rows[3],
    );
    let hints = if app.tab == Tab::Files {
        if app.file_tree_focus {
            let mut hints = vec![
                ("j/k", "files", BLUE),
                ("h/l", "folders", BLUE),
                ("Enter/l", "open history", CYAN),
                ("/", "find file", PEACH),
                ("?", "help", AMBER),
            ];
            if app.file_history_has_origin() {
                hints.push(("Esc", "back", PURPLE));
            }
            hints
        } else if app.list_focus {
            vec![
                ("j/k", "commits", BLUE),
                ("l/Enter", "changes", GREEN),
                ("h/Esc", "files", PURPLE),
                ("/", "find commit", CYAN),
                ("?", "help", AMBER),
            ]
        } else {
            vec![
                ("j/k", "scroll", BLUE),
                ("h", "commits", PURPLE),
                ("Esc", "files", CYAN),
                ("H/L", "horizontal", PEACH),
                ("?", "help", AMBER),
            ]
        }
    } else if app.stash_list() {
        vec![
            ("j/k", "stashes", BLUE),
            ("Enter/l", "files", PEACH),
            ("/", "search", CYAN),
            ("r", "refresh", TEAL),
            ("?", "help", AMBER),
        ]
    } else if app.stash.is_some() {
        vec![
            ("hjkl", "tree", BLUE),
            ("Enter", "preview", GREEN),
            ("Esc", "stashes", PEACH),
            ("/", "search", CYAN),
            ("?", "help", AMBER),
        ]
    } else if app.commit_list() {
        vec![
            ("j/k", "commits", BLUE),
            ("Enter/l", "files", PURPLE),
            ("/", "search", CYAN),
            ("Ctrl+h/l", "tabs", PEACH),
            ("?", "help", AMBER),
            ("q", "quit", RED),
        ]
    } else if app.history.is_some() {
        vec![
            ("hjkl", "tree", BLUE),
            ("Enter", "preview", GREEN),
            ("f", "file history", CYAN),
            ("Esc", "commits", PURPLE),
            ("/", "search", CYAN),
            ("Ctrl+h/l", "tabs", PEACH),
            ("?", "help", AMBER),
        ]
    } else if app.preview.as_ref().is_some_and(|p| p.conflict.is_some()) {
        vec![
            ("hjkl", "move", BLUE),
            ("[ ]", "conflict", AMBER),
            ("b", "base", PURPLE),
            ("v", "sources", CYAN),
            ("e", "edit", PEACH),
            ("s", "resolved", GREEN),
            ("?", "help", PURPLE),
        ]
    } else {
        vec![
            ("hjkl", "move", BLUE),
            ("f", "history", CYAN),
            ("s", "stage", GREEN),
            ("u", "unstage", PEACH),
            ("c", "commit", PURPLE),
            ("/", "search", CYAN),
            ("r", "refresh", TEAL),
            ("?", "help", AMBER),
            ("q", "quit", RED),
        ]
    };
    let hints = Line::from(
        hints
            .into_iter()
            .flat_map(|(key, label, color)| {
                [
                    Span::styled(
                        format!(" {key} "),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("{label} "), Style::default().fg(MUTED)),
                ]
            })
            .collect::<Vec<_>>(),
    );
    frame.render_widget(
        Paragraph::new(hints).style(Style::default().bg(PANEL).fg(MUTED)),
        rows[4],
    );
    match &app.mode {
        Mode::Help => render_help(frame),
        Mode::Commit(input) => {
            render_input(frame, input, " Commit · Ctrl+S submit · Esc cancel ", true)
        }
        Mode::Search(input) => {
            render_input(frame, input, " Search · Enter keep · Esc clear ", false)
        }
        Mode::Normal => {}
    }
}

fn change_color(status: &str) -> Color {
    match status {
        "Clean" => FG,
        "Added" => GREEN,
        "Modified" => AMBER,
        "Deleted" => RED,
        "Renamed" | "Copied" => PURPLE,
        "Conflict" => PEACH,
        _ => FG,
    }
}

fn change_symbol(status: &str) -> char {
    match status {
        "Clean" => '·',
        "Added" => 'A',
        "Modified" => 'M',
        "Deleted" => 'D',
        "Renamed" => 'R',
        "Copied" => 'C',
        "Conflict" => 'U',
        _ => '?',
    }
}

fn file_items(
    app: &App,
    files: &[crate::git::File],
    rows: Vec<crate::tree::Row>,
) -> Vec<ListItem<'static>> {
    let mut items = Vec::new();
    for row in rows {
        let mut spans = vec![Span::styled(
            row.prefix.clone(),
            Style::default().fg(BORDER),
        )];
        if row.directory {
            let root = row.path.as_os_str().is_empty();
            let name = if root {
                app.repo.root.file_name().unwrap_or_default()
            } else {
                row.path.file_name().unwrap_or_default()
            };
            spans.push(Span::styled(
                format!(
                    "{} {}/",
                    if row.expanded { "▾" } else { "▸" },
                    safe(&name.to_string_lossy())
                ),
                Style::default()
                    .fg(if root { PURPLE } else { BLUE })
                    .add_modifier(Modifier::BOLD),
            ));
            if app.tab != Tab::Files {
                for status in [
                    "Added", "Modified", "Deleted", "Renamed", "Copied", "Conflict",
                ] {
                    let count = row
                        .files
                        .iter()
                        .filter(|index| app.file_status(&files[**index]) == status)
                        .count();
                    if count > 0 {
                        spans.push(Span::styled(
                            format!(" {}{count}", change_symbol(status)),
                            Style::default().fg(change_color(status)),
                        ));
                    }
                }
            }
        } else {
            let file = &files[row.files[0]];
            let color = if app.tab == Tab::Files {
                FG
            } else {
                let status = app.file_status(file);
                let color = change_color(status);
                spans.push(Span::styled(
                    format!("{} ", change_symbol(status)),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ));
                color
            };
            spans.push(Span::styled(
                safe(&file.path.file_name().unwrap_or_default().to_string_lossy()),
                Style::default().fg(color),
            ));
        }
        items.push(ListItem::new(Line::from(spans)));
    }
    items
}

fn render_repository_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let rows = app.repository_rows();
    let count = rows.first().map_or(0, |row| row.files.len());
    app.repository_selected = app.repository_selected.min(rows.len().saturating_sub(1));
    let files = app.repository_files();
    let items = file_items(app, &files, rows);
    app.repository_state.select(if items.is_empty() {
        None
    } else {
        Some(app.repository_selected)
    });
    let title = if app.repository_filter.is_empty() {
        format!(" Files · {count} ")
    } else {
        format!(" / {} · {count} ", safe(&app.repository_filter))
    };
    frame.render_stateful_widget(
        List::new(items)
            .block(block(title, app.file_tree_focus))
            .highlight_style(
                Style::default()
                    .bg(Color::Rgb(49, 50, 74))
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› "),
        area,
        &mut app.repository_state,
    );
}

fn draw_files(frame: &mut Frame, area: Rect, app: &mut App) {
    let columns = Layout::horizontal([
        Constraint::Length((area.width / 3).clamp(26, 48)),
        Constraint::Min(26),
    ])
    .split(area);
    app.page_height = area.height.saturating_sub(2) as usize;
    if app.file_tree_focus {
        render_repository_list(frame, columns[0], app);
        if app.file_history.is_some() {
            render_list(frame, columns[1], app);
        } else {
            frame.render_widget(
                Paragraph::new(
                    "Select a file to see its commits.\n\nEnter opens the commit and change view.",
                )
                .block(block(" Commits ", false))
                .style(Style::default().fg(MUTED)),
                columns[1],
            );
        }
    } else {
        render_list(frame, columns[0], app);
        render_preview(frame, columns[1], app);
    }
}

fn render_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let indices = app.items();
    let mut items = Vec::new();
    if app.snapshot.is_some() {
        if app.stash_list() {
            if let Some(snapshot) = &app.snapshot {
                for &index in &indices {
                    let stash = &snapshot.stashes[index];
                    items.push(ListItem::new(vec![
                        Line::from(vec![
                            Span::styled(
                                format!("{} ", safe(&stash.reference)),
                                Style::default().fg(PEACH).add_modifier(Modifier::BOLD),
                            ),
                            Span::raw(safe(&stash.subject)),
                        ]),
                        Line::from(vec![
                            Span::styled(
                                format!("  {}", safe(&stash.author)),
                                Style::default().fg(TEAL),
                            ),
                            Span::styled(
                                format!(" · {}", safe(&stash.age)),
                                Style::default().fg(MUTED),
                            ),
                        ]),
                    ]));
                }
            }
        } else if app.commit_list() {
            for &index in &indices {
                let Some(commit) = app.commit_at(index) else {
                    continue;
                };
                let mut heading = Vec::new();
                if let Some(entry) = app
                    .file_history
                    .as_ref()
                    .and_then(|view| view.entries.as_ref())
                    .and_then(|entries| entries.get(index))
                {
                    heading.push(Span::styled(
                        format!("{} ", change_symbol(entry.file.status(true))),
                        Style::default().fg(change_color(entry.file.status(true))),
                    ));
                }
                heading.extend([
                    Span::styled(format!("{} ", &commit.id[..8]), Style::default().fg(PEACH)),
                    Span::raw(safe(&commit.subject)),
                ]);
                items.push(ListItem::new(vec![
                    Line::from(heading),
                    Line::from(vec![
                        Span::styled(
                            format!("  {}", safe(&commit.author)),
                            Style::default().fg(TEAL),
                        ),
                        Span::styled(
                            format!(" · {}", safe(&commit.age)),
                            Style::default().fg(MUTED),
                        ),
                    ]),
                ]));
            }
        } else {
            let files = app.files();
            items = file_items(app, files, app.tree_rows());
        }
    }
    let title = if app.filter.is_empty() {
        if app.stash_list() {
            format!(" Stashes · {} ", indices.len())
        } else if app.file_history.is_some() {
            format!(
                " Commits · {} · {} ",
                app.file_history
                    .as_ref()
                    .and_then(|view| view.path.file_name())
                    .map(|name| safe(&name.to_string_lossy()))
                    .unwrap_or_default(),
                indices.len()
            )
        } else if app.commit_list() {
            format!(" Commits · {} ", indices.len())
        } else if let Some(stash) = &app.stash {
            format!(
                " Files · {} · {} ",
                indices.len(),
                safe(&stash.item.reference)
            )
        } else if let Some(history) = &app.history {
            format!(" Files · {} · {} ", indices.len(), &history.commit.id[..8])
        } else {
            format!(" Files · {} ", indices.len())
        }
    } else {
        format!(" / {} · {} ", safe(&app.filter), indices.len())
    };
    app.list_state.select(if items.is_empty() {
        None
    } else {
        Some(app.selected)
    });
    let empty = indices.is_empty();
    frame.render_stateful_widget(
        List::new(items)
            .block(block(
                title,
                app.list_focus && !(app.tab == Tab::Files && app.file_tree_focus),
            ))
            .highlight_style(
                Style::default()
                    .bg(Color::Rgb(49, 50, 74))
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› "),
        area,
        &mut app.list_state,
    );
    if empty {
        let inner = area.inner(ratatui::layout::Margin::new(2, 2));
        frame.render_widget(
            Paragraph::new(if !app.filter.is_empty() {
                "No matches"
            } else if app.stash_list() {
                "No stashes"
            } else if let Some(stash) = &app.stash {
                if stash.details.is_none() {
                    "Loading stash files…"
                } else {
                    "No files changed"
                }
            } else if let Some(history) = &app.file_history {
                if history.entries.is_none() {
                    "Loading file history…"
                } else {
                    "No commits for this file"
                }
            } else if app.commit_list() {
                "No commits yet"
            } else if let Some(history) = &app.history {
                if history.details.is_none() {
                    "Loading commit files…"
                } else {
                    "No files changed"
                }
            } else {
                "Nothing here"
            })
            .style(Style::default().fg(MUTED)),
            inner,
        );
    }
}

fn render_preview(frame: &mut Frame, area: Rect, app: &App) {
    let Some(preview) = &app.preview else {
        frame.render_widget(
            Paragraph::new(if app.preview_loading {
                "Loading…"
            } else if app.file_history.is_some() {
                "This file has no committed changes yet."
            } else if app.tab == Tab::Files {
                "Select a file, then a commit.\n\nThe changes appear here."
            } else {
                "Select a file or commit\n\n? opens keyboard help."
            })
            .block(block(" Preview ", !app.list_focus))
            .style(Style::default().fg(MUTED)),
            area,
        );
        return;
    };
    if let Some(conflict) = &preview.conflict {
        let rows = Layout::vertical([
            Constraint::Length(2),
            Constraint::Percentage(45),
            Constraint::Min(4),
        ])
        .split(area);
        let chunk = conflict.chunks.get(app.chunk);
        let mode = if app.whole_sources || chunk.is_none() {
            "index sources".to_owned()
        } else {
            format!("block {}/{}", app.chunk + 1, conflict.chunks.len())
        };
        frame.render_widget(
            Paragraph::new(format!(" CONFLICT · {} · {}", safe(&preview.title), mode))
                .style(Style::default().fg(AMBER).add_modifier(Modifier::BOLD)),
            rows[0],
        );
        let styled = preview.styled();
        let (sources, ours_title, base_title, theirs_title) = if !app.whole_sources
            && let Some(sources) = styled.chunks.get(app.chunk)
        {
            (
                sources,
                " Ours · marker block ",
                " Base · marker block ",
                " Theirs · marker block ",
            )
        } else {
            (
                styled
                    .sources
                    .as_ref()
                    .expect("Conflict sources are prepared"),
                " Ours · index :2 ",
                " Base · index :1 ",
                " Theirs · index :3 ",
            )
        };
        if area.width >= 100 {
            let columns = Layout::horizontal([Constraint::Ratio(1, 3); 3]).split(rows[1]);
            source(frame, columns[0], ours_title, &sources.ours, GREEN, app);
            source(frame, columns[1], base_title, &sources.base, PURPLE, app);
            source(frame, columns[2], theirs_title, &sources.theirs, BLUE, app);
        } else {
            let columns = Layout::horizontal([Constraint::Ratio(1, 2); 2]).split(rows[1]);
            source(
                frame,
                columns[0],
                if app.base { base_title } else { ours_title },
                if app.base {
                    &sources.base
                } else {
                    &sources.ours
                },
                if app.base { PURPLE } else { GREEN },
                app,
            );
            source(frame, columns[1], theirs_title, &sources.theirs, BLUE, app);
        }
        let offset = chunk
            .map_or(0, |chunk| chunk.lines.start)
            .saturating_add(app.scroll);
        let lines = &styled.lines;
        let title = format!(
            " Working tree · {} lines · e edit · s mark resolved ",
            lines.len()
        );
        frame.render_widget(
            Paragraph::new(lines.clone())
                .block(block(title, !app.list_focus))
                .scroll((offset.min(u16::MAX as usize) as u16, app.horizontal)),
            rows[2],
        );
    } else {
        let lines = &preview.styled().lines;
        let title = format!(" {} · {} lines ", safe(&preview.title), lines.len());
        frame.render_widget(
            Paragraph::new(lines.clone())
                .block(block(title, !app.list_focus))
                .scroll((app.scroll as u16, app.horizontal)),
            area,
        );
    }
}

fn source(
    frame: &mut Frame,
    area: Rect,
    title: &'static str,
    lines: &[Line<'static>],
    color: Color,
    app: &App,
) {
    frame.render_widget(
        Paragraph::new(lines.to_vec())
            .block(block(
                Line::styled(title, Style::default().fg(color)),
                !app.list_focus,
            ))
            .scroll((app.scroll as u16, app.horizontal)),
        area,
    );
}

fn popup(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(4));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn render_input(frame: &mut Frame, input: &Input, title: &'static str, multiline: bool) {
    let area = popup(frame.area(), 90, if multiline { 16 } else { 3 });
    frame.render_widget(Clear, area);
    let border = block(title, true);
    let inner = border.inner(area);
    frame.render_widget(border, area);
    let (x, y) = input.cursor_cell();
    let horizontal = x.saturating_sub(inner.width.saturating_sub(1) as usize);
    let vertical = y.saturating_sub(inner.height.saturating_sub(1) as usize);
    let lines: Vec<_> = input
        .text
        .split('\n')
        .map(|line| Line::raw(safe(line)))
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(FG).bg(BG))
            .scroll((
                vertical.min(u16::MAX as usize) as u16,
                horizontal.min(u16::MAX as usize) as u16,
            )),
        inner,
    );
    frame.set_cursor_position((
        inner.x + x.saturating_sub(horizontal) as u16,
        inner.y + y.saturating_sub(vertical) as u16,
    ));
}

fn render_help(frame: &mut Frame) {
    let area = popup(frame.area(), 84, 35);
    frame.render_widget(Clear, area);
    let text = vec![
        Line::styled(
            " NAVIGATION",
            Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
        ),
        Line::raw(" j/k             Move selection or scroll preview"),
        Line::raw(" h/l             Collapse / expand; h parent, l enter"),
        Line::raw(" Enter / Space   Toggle directory; Enter file preview"),
        Line::raw(" Ctrl+h / Ctrl+l Previous / next tab"),
        Line::raw(" Tab             Switch visible pane; open file history"),
        Line::raw(" gg / G          First / last item or line"),
        Line::raw(" Ctrl+d / Ctrl+u Half page down / up"),
        Line::raw(" H / L           Scroll content horizontally"),
        Line::raw(" 1 / 2 / 3 / 4 / 5  Changes / staged / history / files / stash"),
        Line::raw(" f               Show selected file history"),
        Line::raw(" /               Search current list (Unicode input)"),
        Line::raw(" r               Refresh repository and preview"),
        Line::raw(" Enter / l       Open selected commit or stash file tree"),
        Line::raw(" Esc / Backspace Return from commit / stash files to list"),
        Line::raw(" History files are read-only; merge diff uses first parent."),
        Line::raw(""),
        Line::styled(
            " GIT",
            Style::default().fg(GREEN).add_modifier(Modifier::BOLD),
        ),
        Line::raw(" s / u           Stage / unstage selected file or directory"),
        Line::raw(" c               Compose commit; Ctrl+S submits"),
        Line::raw(" e               Open file using Git's configured editor"),
        Line::raw(""),
        Line::styled(
            " CONFLICTS",
            Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
        ),
        Line::raw(" [ / ]           Previous / next conflict block"),
        Line::raw(" v               Marker blocks / full Git index sources"),
        Line::raw(" b               Toggle ours / base on narrow terminals"),
        Line::raw(" e → edit → s    Resolve in editor, then stage result"),
        Line::raw(" :1 base · :2 ours · :3 theirs (Git index terminology)"),
        Line::raw(" During rebase, ours is upstream; theirs is replayed work."),
        Line::raw(""),
        Line::styled(
            " Esc / ? close help · q quit · Ctrl+C quit anywhere",
            Style::default().fg(MUTED),
        ),
    ];
    frame.render_widget(
        Paragraph::new(text).block(block(" Keyboard help ", true)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        conflict,
        git::{ConflictView, Preview, Repo},
    };
    use ratatui::{Terminal, backend::TestBackend};
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn renders_conflict_sides_and_places_cjk_input_cursor() {
        let mut app = App::new(Repo {
            root: "/nonexistent-giter-test".into(),
        });
        let text = "<<<<<<< HEAD\n当前内容\n=======\n传入内容\n>>>>>>> topic\n";
        app.preview = Some(Preview {
            title: "冲突.rs".into(),
            code_path: Some("冲突.rs".into()),
            styles: std::sync::OnceLock::new(),
            text: String::new(),
            conflict: Some(ConflictView {
                ours: "当前内容\n".into(),
                base: "共同内容\n".into(),
                theirs: "传入内容\n".into(),
                working: text.into(),
                chunks: conflict::parse(text),
            }),
        });
        let mut terminal = Terminal::new(TestBackend::new(160, 45)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut content = String::new();
        for y in 0..45 {
            let mut x = 0;
            while x < 160 {
                let symbol = buffer[(x, y)].symbol();
                content.push_str(symbol);
                x += symbol.width().max(1) as u16;
            }
            content.push('\n');
        }
        assert!(content.contains("Ours · marker block"));
        assert!(content.contains("Theirs · marker block"));
        assert!(content.contains("当前内容"));
        assert!(content.contains("传入内容"));
        let mut input = Input::default();
        input.insert("修复冲突", true);
        app.mode = Mode::Commit(input);
        let completed = terminal.draw(|f| draw(f, &mut app)).unwrap();
        let popup = popup(completed.area, 90, 16);
        let cursor = terminal.get_cursor_position().unwrap();
        assert_eq!(cursor.x, popup.x + 1 + 8);
        assert_eq!(cursor.y, popup.y + 1);
    }
}
