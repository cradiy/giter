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
    let header = Line::from(vec![
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
    ]);
    frame.render_widget(Paragraph::new(header), rows[0]);
    let counts = snapshot
        .map(|s| {
            [
                s.files.iter().filter(|f| f.unstaged()).count(),
                s.files.iter().filter(|f| f.staged()).count(),
                s.commits.len(),
            ]
        })
        .unwrap_or([0; 3]);
    let mut tabs = Vec::new();
    for (i, (tab, name)) in [
        (Tab::Changes, "Changes"),
        (Tab::Staged, "Staged"),
        (Tab::Log, "History"),
    ]
    .into_iter()
    .enumerate()
    {
        let accent = match tab {
            Tab::Changes => BLUE,
            Tab::Staged => GREEN,
            Tab::Log => PURPLE,
        };
        tabs.push(Span::styled(
            format!(" {} {name} {} ", i + 1, counts[i]),
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
    let hints = if app.preview.as_ref().is_some_and(|p| p.conflict.is_some()) {
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
        "Added" => 'A',
        "Modified" => 'M',
        "Deleted" => 'D',
        "Renamed" => 'R',
        "Copied" => 'C',
        "Conflict" => 'U',
        _ => '?',
    }
}

fn render_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let indices = app.items();
    let mut items = Vec::new();
    if let Some(snapshot) = &app.snapshot {
        if app.tab == Tab::Log {
            for &index in &indices {
                let commit = &snapshot.commits[index];
                items.push(ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(format!("{} ", &commit.id[..8]), Style::default().fg(PEACH)),
                        Span::raw(safe(&commit.subject)),
                    ]),
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
            for row in app.tree_rows() {
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
                    for status in [
                        "Added", "Modified", "Deleted", "Renamed", "Copied", "Conflict",
                    ] {
                        let count = row
                            .files
                            .iter()
                            .filter(|index| {
                                snapshot.files[**index].status(app.tab == Tab::Staged) == status
                            })
                            .count();
                        if count > 0 {
                            spans.push(Span::styled(
                                format!(" {}{count}", change_symbol(status)),
                                Style::default().fg(change_color(status)),
                            ));
                        }
                    }
                } else {
                    let file = &snapshot.files[row.files[0]];
                    let status = file.status(app.tab == Tab::Staged);
                    let color = change_color(status);
                    spans.push(Span::styled(
                        format!("{} ", change_symbol(status)),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::styled(
                        safe(&file.path.file_name().unwrap_or_default().to_string_lossy()),
                        Style::default().fg(color),
                    ));
                }
                items.push(ListItem::new(Line::from(spans)));
            }
        }
    }
    let title = if app.filter.is_empty() {
        format!(" Files · {} ", indices.len())
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
            .block(block(title, app.list_focus))
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
            } else if app.tab == Tab::Log {
                "No commits yet"
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
        frame.render_widget(Paragraph::new(if app.preview_loading { "Loading…" } else { "Select a file or commit\n\n1 Changes   2 Staged   3 History\n\nConflict files appear first.\n? opens keyboard help." }).block(block(" Preview ", !app.list_focus)).style(Style::default().fg(MUTED)), area);
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
    let area = popup(frame.area(), 84, 30);
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
        Line::raw(" Tab             Switch focused pane"),
        Line::raw(" gg / G          First / last item or line"),
        Line::raw(" Ctrl+d / Ctrl+u Half page down / up"),
        Line::raw(" H / L           Scroll content horizontally"),
        Line::raw(" 1 / 2 / 3       Changes / staged / recent history"),
        Line::raw(" /               Search current list (Unicode input)"),
        Line::raw(" r               Refresh repository and preview"),
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
