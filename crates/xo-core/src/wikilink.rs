use regex::Regex;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Wikilink {
    pub id: String,
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub start_column: usize,
    pub end_column: usize,
}

#[must_use]
pub fn parse(text: &str) -> Vec<Wikilink> {
    let expression = Regex::new(r"\[\[([^\]]*)\]\]").expect("wikilink regex is valid");
    let line_offsets = line_offsets(text);
    expression
        .captures_iter(text)
        .filter_map(|captures| {
            let whole = captures.get(0)?;
            let id = captures.get(1)?;
            let (line, start_column) = offset_to_line_column(&line_offsets, whole.start());
            let (_, end_column) = offset_to_line_column(&line_offsets, whole.end());
            Some(Wikilink {
                id: id.as_str().to_owned(),
                start: whole.start(),
                end: whole.end(),
                line,
                start_column,
                end_column,
            })
        })
        .collect()
}

fn line_offsets(text: &str) -> Vec<usize> {
    let mut offsets = vec![0];
    offsets.extend(text.match_indices('\n').map(|(offset, _)| offset + 1));
    offsets
}

fn offset_to_line_column(offsets: &[usize], offset: usize) -> (usize, usize) {
    let line = offsets
        .partition_point(|line_offset| *line_offset <= offset)
        .saturating_sub(1);
    (line, offset - offsets[line])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_legacy_byte_ranges_and_lines() {
        let links = parse("first [[abc]]\n[[second]]");
        assert_eq!(links[0].id, "abc");
        assert_eq!((links[0].line, links[0].start_column), (0, 6));
        assert_eq!((links[1].line, links[1].start_column), (1, 0));
    }
}
