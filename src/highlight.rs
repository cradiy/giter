use std::{path::Path, sync::OnceLock};

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use syntect::{
    easy::HighlightLines,
    highlighting::{
        Color as SyntaxColor, FontStyle, StyleModifier, Theme, ThemeItem, ThemeSettings,
    },
    parsing::SyntaxSet,
};

use crate::{git::Preview, theme::*};

pub struct StyledSources {
    pub ours: Vec<Line<'static>>,
    pub base: Vec<Line<'static>>,
    pub theirs: Vec<Line<'static>>,
}

pub struct StyledPreview {
    pub lines: Vec<Line<'static>>,
    pub sources: Option<StyledSources>,
    pub chunks: Vec<StyledSources>,
}

fn syntax_color(color: Color) -> SyntaxColor {
    let Color::Rgb(r, g, b) = color else {
        unreachable!("Syntax palette uses RGB")
    };
    SyntaxColor { r, g, b, a: 255 }
}

fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| Theme {
        settings: ThemeSettings {
            foreground: Some(syntax_color(FG)),
            background: Some(syntax_color(BG)),
            ..ThemeSettings::default()
        },
        scopes: [
            ("comment", MUTED, FontStyle::ITALIC),
            ("keyword, storage", PURPLE, FontStyle::empty()),
            ("keyword.operator", CYAN, FontStyle::empty()),
            ("string", GREEN, FontStyle::empty()),
            (
                "constant.numeric, constant.language",
                PEACH,
                FontStyle::empty(),
            ),
            (
                "entity.name.type, entity.name.class, support.type, support.class",
                AMBER,
                FontStyle::empty(),
            ),
            ("entity.name.function", BLUE, FontStyle::empty()),
            ("support.function", TEAL, FontStyle::empty()),
            ("variable.parameter", RED, FontStyle::empty()),
            ("variable.language", PEACH, FontStyle::empty()),
            (
                "entity.name.tag, entity.other.attribute-name",
                RED,
                FontStyle::empty(),
            ),
            ("punctuation", MUTED, FontStyle::empty()),
            ("punctuation.definition.string", GREEN, FontStyle::empty()),
        ]
        .into_iter()
        .map(|(scope, color, font_style)| ThemeItem {
            scope: scope.parse().expect("Valid built-in syntax selector"),
            style: StyleModifier {
                foreground: Some(syntax_color(color)),
                font_style: Some(font_style),
                ..StyleModifier::default()
            },
        })
        .collect(),
        ..Theme::default()
    })
}

fn code(text: &str, path: &Path) -> Vec<Line<'static>> {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    let syntaxes = SYNTAXES.get_or_init(SyntaxSet::load_defaults_nonewlines);
    let syntax = path
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(|ext| syntaxes.find_syntax_by_extension(ext))
        .or_else(|| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| syntaxes.find_syntax_by_extension(name))
        })
        .or_else(|| {
            text.lines()
                .next()
                .and_then(|line| syntaxes.find_syntax_by_first_line(line))
        })
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text());
    let mut highlighter = HighlightLines::new(syntax, theme());
    let width = text.lines().count().max(1).to_string().len().max(3);
    text.lines()
        .enumerate()
        .map(|(i, text)| {
            let mut spans = vec![gutter(i, width, MUTED)];
            match highlighter.highlight_line(text, syntaxes) {
                Ok(tokens) => spans.extend(tokens.into_iter().map(|(token, text)| {
                    let mut style = Style::default().fg(Color::Rgb(
                        token.foreground.r,
                        token.foreground.g,
                        token.foreground.b,
                    ));
                    if token.font_style.contains(FontStyle::ITALIC) {
                        style = style.add_modifier(Modifier::ITALIC);
                    }
                    Span::styled(safe(text), style)
                })),
                Err(_) => spans.push(Span::styled(safe(text), Style::default().fg(FG))),
            }
            Line::from(spans)
        })
        .collect()
}

fn gutter(index: usize, width: usize, color: Color) -> Span<'static> {
    Span::styled(
        format!("{:>width$} │ ", index + 1),
        Style::default().fg(color),
    )
}

fn patch(text: &str) -> Vec<Line<'static>> {
    let mut in_diff = false;
    let mut in_hunk = false;
    let lines: Vec<_> = text
        .lines()
        .filter(|line| {
            if line.starts_with("diff --git ") {
                in_diff = true;
                in_hunk = false;
                return false;
            }
            if in_diff && line.starts_with("@@") {
                in_hunk = true;
            }
            !(in_diff
                && !in_hunk
                && ["index ", "--- ", "+++ "]
                    .iter()
                    .any(|prefix| line.starts_with(prefix)))
        })
        .collect();
    let width = lines.len().max(1).to_string().len().max(3);
    lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| {
            let (fg, bg) = if line.starts_with(" A  ") {
                (GREEN, BG)
            } else if line.starts_with(" M  ") || line.starts_with(" T  ") {
                (AMBER, BG)
            } else if line.starts_with(" D  ") {
                (RED, BG)
            } else if line.starts_with(" R  ") || line.starts_with(" C  ") {
                (PURPLE, BG)
            } else if line.starts_with('+') {
                (GREEN, ADD_BG)
            } else if line.starts_with('-') {
                (RED, DELETE_BG)
            } else if line.starts_with("@@") {
                (CYAN, INCOMING_BG)
            } else if line.starts_with("diff ") || line.starts_with("commit ") {
                (PURPLE, PANEL)
            } else if line.starts_with("index ")
                || line.starts_with("Author:")
                || line.starts_with("Date:")
            {
                (PEACH, BG)
            } else {
                (FG, BG)
            };
            Line::from(vec![
                gutter(i, width, if bg == BG { MUTED } else { fg }),
                Span::styled(safe(line), Style::default().fg(fg)),
            ])
            .style(Style::default().bg(bg))
        })
        .collect()
}

fn sources(ours: &str, base: &str, theirs: &str, path: &Path) -> StyledSources {
    let paint = |text: &str| {
        code(
            if text.is_empty() {
                "[Empty content]"
            } else {
                text
            },
            path,
        )
    };
    StyledSources {
        ours: paint(ours),
        base: paint(base),
        theirs: paint(theirs),
    }
}

pub fn preview(preview: &Preview) -> StyledPreview {
    let path = preview
        .code_path
        .as_deref()
        .unwrap_or_else(|| Path::new(""));
    if let Some(conflict) = &preview.conflict {
        let mut lines = code(&conflict.working, path);
        for chunk in &conflict.chunks {
            let mut background = ADD_BG;
            for (index, text) in conflict
                .working
                .lines()
                .enumerate()
                .skip(chunk.lines.start)
                .take(chunk.lines.len())
            {
                let marker = text.starts_with("<<<<<<<")
                    || text.starts_with("|||||||")
                    || text.starts_with("=======")
                    || text.starts_with(">>>>>>>");
                if text.starts_with("|||||||") {
                    background = PANEL;
                }
                if text.starts_with("=======") {
                    background = INCOMING_BG;
                }
                lines[index].style =
                    Style::default().bg(if marker { DELETE_BG } else { background });
                if marker {
                    lines[index].spans.truncate(1);
                    lines[index].spans.push(Span::styled(
                        safe(text),
                        Style::default().fg(AMBER).add_modifier(Modifier::BOLD),
                    ));
                }
            }
        }
        StyledPreview {
            lines,
            sources: Some(sources(
                &conflict.ours,
                &conflict.base,
                &conflict.theirs,
                path,
            )),
            chunks: conflict
                .chunks
                .iter()
                .map(|chunk| {
                    sources(
                        &chunk.ours,
                        chunk
                            .base
                            .as_deref()
                            .unwrap_or("[No base section; press v]"),
                        &chunk.theirs,
                        path,
                    )
                })
                .collect(),
        }
    } else {
        StyledPreview {
            lines: if preview.code_path.is_some() {
                code(&preview.text, path)
            } else {
                patch(&preview.text)
            },
            sources: None,
            chunks: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_file_headers_but_keeps_header_like_source_lines_and_binary_summaries() {
        let text = "diff --git a/code b/code\nindex 123..456 100644\n--- a/code\n+++ b/code\n@@ -1 +1 @@\n--- a/source text\n+++ b/source text\ndiff --git a/image b/image\nindex abc..def 100644\nBinary files a/image and b/image differ\n";
        let lines = patch(text);
        let visible: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .skip(1)
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();
        assert_eq!(
            visible,
            [
                "@@ -1 +1 @@",
                "--- a/source text",
                "+++ b/source text",
                "Binary files a/image and b/image differ"
            ]
        );
    }
}
