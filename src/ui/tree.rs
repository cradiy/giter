use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::theme::BORDER;

pub(super) fn fit_line(
    prefix: &str,
    marker: Span<'static>,
    mut name: Span<'static>,
    annotations: Vec<Span<'static>>,
    width: usize,
) -> Line<'static> {
    let minimum_prefix = if prefix.is_empty() { 0 } else { 2 };
    let name_width = width.saturating_sub(marker.width() + minimum_prefix);
    name.content = shorten(&name.content, name_width).into();
    let remaining = width.saturating_sub(marker.width() + name.width());
    let mut annotation_width = 0;
    let annotations: Vec<_> = annotations
        .into_iter()
        .take_while(|span| {
            let next = annotation_width + span.width();
            if next + minimum_prefix > remaining {
                return false;
            }
            annotation_width = next;
            true
        })
        .collect();
    let prefix = compact_prefix(prefix, remaining.saturating_sub(annotation_width));
    let mut spans = vec![
        Span::styled(prefix, Style::default().fg(BORDER)),
        marker,
        name,
    ];
    spans.extend(annotations);
    Line::from(spans)
}

fn compact_prefix(prefix: &str, width: usize) -> String {
    if prefix.width() <= width {
        return prefix.into();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".into();
    }
    if width < 5 {
        return "… ".into();
    }
    let suffix: String = prefix
        .chars()
        .skip(prefix.chars().count() - (width - 2))
        .collect();
    format!("… {suffix}")
}

fn shorten(name: &str, width: usize) -> String {
    if name.width() <= width {
        return name.into();
    }
    if width == 0 {
        return String::new();
    }
    let tail_width = (width - 1) / 2;
    let head_width = width - 1 - tail_width;
    let mut used = 0;
    let head: String = name
        .graphemes(true)
        .take_while(|part| {
            used += part.width();
            used <= head_width
        })
        .collect();
    used = 0;
    let mut tail: Vec<_> = name
        .graphemes(true)
        .rev()
        .take_while(|part| {
            used += part.width();
            used <= tail_width
        })
        .collect();
    tail.reverse();
    format!("{head}…{}", tail.concat())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_names_preserve_graphemes_and_file_extensions() {
        let name = "组件👨‍👩‍👧‍👦e\u{301}_very_long_module_name.rs";
        let graphemes: Vec<_> = name.graphemes(true).collect();
        for width in 0..=name.width() + 2 {
            let text = shorten(name, width);
            assert!(text.width() <= width);
            assert!(
                text.graphemes(true)
                    .all(|part| part == "…" || graphemes.contains(&part))
            );
            if width >= 7 {
                assert!(text.ends_with(".rs"));
            }
        }
        assert_eq!(shorten(name, name.width()), name);
    }

    #[test]
    fn deep_file_trees_keep_leaf_names_visible_in_both_panes_and_repository_view() {
        use crate::{
            app::{App, Tab},
            git::{File, Repo, Snapshot},
            theme::{AMBER, GREEN},
        };
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = App::new(Repo {
            root: "/nonexistent-giter-long-path-test".into(),
        });
        let path: std::path::PathBuf = (0..14)
            .map(|index| format!("segment{index}"))
            .chain(["app.rs".into()])
            .collect();
        let file = File {
            path: path.clone(),
            original: None,
            index: 'A',
            worktree: 'M',
        };
        app.snapshot = Some(Snapshot {
            branch: "main".into(),
            operation: String::new(),
            files: vec![file.clone()],
            committed_files: vec![file],
            head: None,
            stashes: vec![],
            branches: vec![],
        });
        app.selected = app.tree_rows().len() - 1;
        app.change_panes[1].selected = app.selected;
        for (width, height) in [(60, 24), (120, 30)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| super::super::draw(frame, &mut app))
                .unwrap();
            let buffer = terminal.backend().buffer();
            let sidebar = (width / 3).clamp(23, 42);
            let names: Vec<_> = (1..height - 2)
                .filter(|y| {
                    let row: String = (0..sidebar).map(|x| buffer[(x, *y)].symbol()).collect();
                    row.contains("app.rs")
                })
                .collect();
            assert_eq!(names.len(), 2);
            for (y, symbol, color) in [(names[0], "M", AMBER), (names[1], "A", GREEN)] {
                let x = (0..sidebar)
                    .find(|x| buffer[(*x, y)].symbol() == symbol)
                    .unwrap();
                assert_eq!(buffer[(x, y)].fg, color);
                assert!((0..sidebar).any(|x| buffer[(x, y)].symbol() == "…"));
            }
            assert!(
                matches!(app.selection(), Some(crate::git::Selection::File(file, false)) if file.path == path)
            );
        }
        app.tab = Tab::Files;
        app.file_tree_focus = true;
        app.repository_selected = app.repository_rows().len() - 1;
        let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
        terminal
            .draw(|frame| super::super::draw(frame, &mut app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let rows: Vec<String> = (1..22)
            .map(|y| (0..26).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        assert!(rows.iter().any(|row| row.contains("app.rs")));
        assert_eq!(app.repository_rows().last().unwrap().path, path);
    }
}
