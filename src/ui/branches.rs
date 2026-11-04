use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Paragraph},
};

use super::{block, render_preview};
use crate::{
    app::App,
    branches::{Row, short_ref},
    theme::*,
};

pub(super) fn draw(frame: &mut Frame, area: Rect, app: &mut App) {
    let columns = Layout::horizontal([Constraint::Ratio(1, 2); 2]).split(area);
    app.page_height = if app.list_focus {
        columns[0].height
    } else {
        columns[1].height
    }
    .saturating_sub(2) as usize;
    if app.branch_list() {
        branch_list(frame, columns[0], app);
        graph(frame, columns[1], app, false);
    } else {
        graph(frame, columns[0], app, true);
        render_preview(frame, columns[1], app);
    }
}

fn branch_list(frame: &mut Frame, area: Rect, app: &mut App) {
    let rows = app.branch_rows();
    let mut items = Vec::new();
    if let Some(snapshot) = &app.snapshot {
        for row in &rows {
            items.push(match *row {
                Row::Local => ListItem::new(Line::styled(
                    "▾ Local",
                    Style::default().fg(TEAL).add_modifier(Modifier::BOLD),
                )),
                Row::Remote => ListItem::new(Line::styled(
                    format!(
                        "{} Remote",
                        if app.branches.remote_open || !app.branches.filter.is_empty() {
                            "▾"
                        } else {
                            "▸"
                        }
                    ),
                    Style::default().fg(PURPLE).add_modifier(Modifier::BOLD),
                )),
                Row::Branch(index) => {
                    let branch = &snapshot.branches[index];
                    let mut name = vec![Span::styled(
                        format!(
                            "{}{}",
                            if branch.current { "● " } else { "  " },
                            safe(&branch.name)
                        ),
                        Style::default().fg(if branch.current {
                            GREEN
                        } else if branch.remote {
                            PURPLE
                        } else {
                            BLUE
                        }),
                    )];
                    if branch.worktree.is_some() && !branch.current {
                        name.push(Span::styled(" [worktree]", Style::default().fg(PEACH)));
                    }
                    let mut lines = vec![Line::from(name)];
                    if !branch.upstream.is_empty() {
                        let tracking = if branch.tracking.is_empty() {
                            "=".into()
                        } else {
                            branch
                                .tracking
                                .replace("ahead ", "↑")
                                .replace("behind ", "↓")
                                .replace(", ", " ")
                        };
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!("  ↳ {} ", safe(short_ref(&branch.upstream))),
                                Style::default().fg(MUTED),
                            ),
                            Span::styled(
                                tracking,
                                Style::default().fg(if branch.tracking == "gone" {
                                    RED
                                } else {
                                    AMBER
                                }),
                            ),
                        ]));
                    } else if let Some(path) = &branch.worktree
                        && !branch.current
                    {
                        lines.push(Line::styled(
                            format!("  {}", safe(&path.display().to_string())),
                            Style::default().fg(MUTED),
                        ));
                    }
                    ListItem::new(lines)
                }
            });
        }
    }
    let count = rows
        .iter()
        .filter(|row| matches!(row, Row::Branch(_)))
        .count();
    let title = if app.branches.filter.is_empty() {
        format!(" Branches · {count} ")
    } else {
        format!(" / {} · {count} ", safe(&app.branches.filter))
    };
    let border = block(title, true);
    let inner = border.inner(area);
    app.branches
        .state
        .select((!rows.is_empty()).then_some(app.branches.selected));
    frame.render_stateful_widget(
        List::new(items)
            .block(border)
            .highlight_symbol("› ")
            .highlight_style(Style::default().bg(BORDER).add_modifier(Modifier::BOLD)),
        area,
        &mut app.branches.state,
    );
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(if app.branches.filter.is_empty() {
                "No branches yet"
            } else {
                "No matching branches"
            })
            .style(Style::default().fg(MUTED)),
            inner,
        );
    }
}

fn graph(frame: &mut Frame, area: Rect, app: &mut App, active: bool) {
    use unicode_width::UnicodeWidthStr;
    let branch = app.selected_branch();
    let missing_branch = branch.is_none();
    let title = branch.map_or_else(
        || " Select a branch ".into(),
        |branch| format!(" {} · history ", safe(&branch.name)),
    );
    let filtered = active && !app.filter.is_empty();
    let graph = app
        .branches
        .graph
        .as_ref()
        .filter(|_| !missing_branch || active);
    let border = block(title, active && app.list_focus).title_style(Style::default().fg(TEAL));
    let inner = border.inner(area);
    let regions = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(inner);
    let list_area = regions[1];
    let indices: Vec<_> = if active {
        app.items()
    } else {
        (0..graph.map_or(0, |graph| graph.entries.len())).collect()
    };
    let selection = if active {
        app.selected
    } else {
        branch
            .and_then(|branch| {
                graph.and_then(|graph| {
                    graph
                        .entries
                        .iter()
                        .position(|entry| entry.commit.id == branch.tip)
                })
            })
            .unwrap_or(0)
    };
    let gutter = graph.map_or(5, |graph| {
        (graph.layout.lanes as u16 * 3 + 2)
            .min(list_area.width.saturating_sub(2) / 3)
            .max(3)
    });
    let text_width = list_area.width.saturating_sub(gutter + 3) as usize;
    let padding = " ".repeat(gutter as usize);
    let mut items = Vec::new();
    if let Some(graph) = graph {
        for (row, &index) in indices.iter().enumerate() {
            let entry = &graph.entries[index];
            let mut tags = Vec::new();
            let mut tag_width = 0;
            if let Some(snapshot) = &app.snapshot {
                for branch in snapshot
                    .branches
                    .iter()
                    .filter(|branch| branch.tip == entry.commit.id)
                {
                    let name = if branch.reference == "HEAD" {
                        "HEAD (detached)".into()
                    } else if branch.current {
                        format!("HEAD → {}", branch.name)
                    } else {
                        branch.name.clone()
                    };
                    let label = format!(" {} ", safe(&name));
                    if tag_width + label.width() > text_width.saturating_sub(8) {
                        continue;
                    }
                    let color = if branch.current {
                        GREEN
                    } else if branch.reference.as_str()
                        == app.branches.reference.as_deref().unwrap_or("")
                    {
                        TEAL
                    } else if branch.remote {
                        PURPLE
                    } else {
                        BLUE
                    };
                    tag_width += label.width() + 1;
                    tags.push(Span::styled(
                        label,
                        Style::default()
                            .fg(BG)
                            .bg(color)
                            .add_modifier(Modifier::BOLD),
                    ));
                    tags.push(Span::raw(" "));
                }
            }
            let subject = safe(&entry.commit.subject);
            let available = text_width.saturating_sub(tag_width);
            let mut clipped = String::new();
            for ch in subject.chars() {
                if clipped.width() + unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0)
                    > available
                {
                    break;
                }
                clipped.push(ch);
            }
            let gap = text_width.saturating_sub(clipped.width() + tag_width);
            let mut first = vec![
                Span::raw(padding.clone()),
                Span::styled(clipped, Style::default().fg(FG)),
                Span::raw(" ".repeat(gap)),
            ];
            first.extend(tags);
            let second = vec![
                Span::raw(padding.clone()),
                Span::styled(entry.commit.id[..8].to_owned(), Style::default().fg(PEACH)),
                Span::styled(
                    format!(
                        " · {} · {}",
                        safe(&entry.commit.author),
                        safe(&entry.commit.age)
                    ),
                    Style::default().fg(MUTED),
                ),
            ];
            items.push(
                ListItem::new(vec![Line::from(first), Line::from(second)])
                    .style(Style::default().bg(if row == selection { PANEL } else { BG })),
            );
        }
    }
    let info = if filtered {
        format!(" {} matches in loaded history", indices.len())
    } else if let Some(graph) = graph {
        let count = format!(
            "{}{} commits",
            graph.entries.len(),
            if graph.more { "+" } else { "" }
        );
        if let Some((ahead, behind)) = graph.comparison {
            format!(
                " {count}  ·  {ahead} ahead / {behind} behind {}",
                app.snapshot
                    .as_ref()
                    .map_or("HEAD", |snapshot| snapshot.branch.as_str())
            )
        } else {
            format!(
                " {count}{}",
                if branch.is_some_and(|branch| branch.reference == "HEAD") {
                    " · detached HEAD"
                } else if branch.is_some_and(|branch| branch.current) {
                    " · current branch"
                } else {
                    " · same tip as HEAD"
                }
            )
        }
    } else if app.branches.loading {
        " Loading history…".into()
    } else {
        String::new()
    };
    frame.render_widget(border, area);
    frame.render_widget(
        Paragraph::new(info).style(Style::default().fg(MUTED)),
        regions[0],
    );
    app.list_state
        .select((!items.is_empty()).then_some(selection));
    frame.render_stateful_widget(
        List::new(items)
            .highlight_symbol(if active { "› " } else { "  " })
            .highlight_style(Style::default().add_modifier(Modifier::BOLD)),
        list_area,
        &mut app.list_state,
    );
    if indices.is_empty() {
        frame.render_widget(
            Paragraph::new(if app.branches.loading {
                "Loading branch history…"
            } else if missing_branch {
                "Choose a branch. Enter opens its history."
            } else if filtered {
                "No matching commits in loaded history"
            } else {
                "No commits"
            })
            .style(Style::default().fg(MUTED)),
            list_area,
        );
    } else if matches!(app.mode, crate::app::Mode::Normal)
        && let Some(graph) = graph
    {
        let offset = app.list_state.offset();
        app.graphics.scene = Some(crate::graphics::Scene {
            area: Rect::new(list_area.x + 2, list_area.y, gutter, list_area.height),
            layout: graph.layout.clone(),
            visible: indices
                .iter()
                .skip(offset)
                .take(usize::from(list_area.height).div_ceil(2))
                .copied()
                .collect(),
            selected: indices.get(selection).copied(),
            filtered,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::Tab,
        branches::{Branch, Entry, Graph},
        git::{Commit, Repo, Snapshot},
    };
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn selected_graph_ref_badges_keep_their_colors_and_four_tabs_fit_minimum_width() {
        let mut app = App::new(Repo {
            root: "/nonexistent-giter-ui-test".into(),
        });
        let id = "1".repeat(40);
        let commit = Commit {
            id: id.clone(),
            subject: "feature subject".into(),
            author: "Test".into(),
            age: "now".into(),
        };
        app.snapshot = Some(Snapshot {
            branch: "feature/ime".into(),
            operation: String::new(),
            files: vec![],
            committed_files: std::sync::Arc::default(),
            head: Some(commit.id.clone()),
            stashes: vec![],
            branches: vec![Branch {
                reference: "refs/heads/feature/ime".into(),
                name: "feature/ime".into(),
                tip: id,
                current: true,
                remote: false,
                upstream: String::new(),
                tracking: String::new(),
                worktree: None,
            }],
        });
        app.tab = Tab::Branches;
        app.branches.selected = 1;
        app.branches.graph = Some(Graph {
            entries: vec![Entry {
                commit,
                parents: vec![],
            }],
            more: false,
            limit: 100,
            comparison: None,
            layout: std::sync::Arc::new(crate::graph::Layout {
                nodes: vec![crate::graph::Node {
                    lane: 0,
                    merge: false,
                }],
                edges: vec![],
                lanes: 1,
            }),
        });
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|frame| super::super::draw(frame, &mut app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let head = (60..115)
            .find(|x| buffer[(*x, 3)].symbol() == "H" && buffer[(*x + 1, 3)].symbol() == "E")
            .unwrap();
        assert_eq!(buffer[(head, 3)].fg, BG);
        assert_eq!(buffer[(head, 3)].bg, GREEN);
        let mut narrow = Terminal::new(TestBackend::new(60, 14)).unwrap();
        narrow
            .draw(|frame| super::super::draw(frame, &mut app))
            .unwrap();
        let tabs: String = (0..60)
            .map(|x| narrow.backend().buffer()[(x, 0)].symbol())
            .collect();
        assert!(tabs.contains("1 Chg") && tabs.contains("2 Br"));
    }
}
