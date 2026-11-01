use std::ops::Range;

#[derive(Clone, Debug)]
pub struct Chunk {
    pub lines: Range<usize>,
    pub ours: String,
    pub base: Option<String>,
    pub theirs: String,
}

fn marker(line: &str, symbol: char, width: usize) -> bool {
    let line = line.trim_end_matches(['\r', '\n']);
    let count = line.chars().take_while(|c| *c == symbol).count();
    count == width && (line.len() == count || line.as_bytes()[count] == b' ')
}

pub fn has_markers(text: &str) -> bool {
    text.lines().any(|line| {
        ['<', '|', '=', '>'].into_iter().any(|symbol| {
            let count = line.chars().take_while(|c| *c == symbol).count();
            count >= 7 && marker(line, symbol, count)
        })
    })
}

pub fn parse(text: &str) -> Vec<Chunk> {
    let lines: Vec<_> = text.split_inclusive('\n').collect();
    let mut chunks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let width = lines[i].chars().take_while(|c| *c == '<').count();
        if width < 7 || !marker(lines[i], '<', width) {
            i += 1;
            continue;
        }
        let start = i;
        let mut base = None;
        let mut separator = None;
        let mut end = None;
        for (j, line) in lines.iter().enumerate().skip(start + 1) {
            if marker(line, '<', width) {
                break;
            }
            if separator.is_none() && marker(line, '|', width) {
                base = Some(j);
            } else if marker(line, '=', width) {
                separator = Some(j);
            } else if marker(line, '>', width) {
                end = Some(j);
                break;
            }
        }
        if let (Some(separator), Some(end)) = (separator, end) {
            chunks.push(Chunk {
                lines: start..end + 1,
                ours: lines[start + 1..base.unwrap_or(separator)].concat(),
                base: base.map(|base| lines[base + 1..separator].concat()),
                theirs: lines[separator + 1..end].concat(),
            });
            i = end + 1;
        } else {
            i += 1;
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_diff3_custom_width_crlf_and_missing_final_newline() {
        let text = "context\r\n<<<<<<<<< HEAD\r\n当前\r\n||||||||| base\r\n共同\r\n=========\r\n传入\r\n>>>>>>>>> branch";
        let chunks = parse(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].lines, 1..8);
        assert_eq!(chunks[0].ours, "当前\r\n");
        assert_eq!(chunks[0].base.as_deref(), Some("共同\r\n"));
        assert_eq!(chunks[0].theirs, "传入\r\n");
    }

    #[test]
    fn separates_multiple_conflicts_and_detects_incomplete_markers() {
        let text = "<<<<<<< HEAD\na\n=======\nb\n>>>>>>> topic\ncontext\n<<<<<<< HEAD\nc\n=======\nd\n>>>>>>> topic\n";
        let chunks = parse(text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[1].lines, 6..11);
        assert!(has_markers("<<<<<<< HEAD\nunfinished"));
        assert!(!has_markers("a <<<<<<< b\n========heading\n"));
    }
}
