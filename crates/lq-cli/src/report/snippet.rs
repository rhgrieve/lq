//! Lays out the source around a span: line numbers, context, the underline. Works in terminal
//! cells from byte offsets, so tabs, wide characters and long lines stay aligned.

use super::clean_char;
use super::highlight::Highlights;
use super::term::{style, Term};
use super::upstream::cell_width;

const TAB_WIDTH: usize = 4;
/// A span over more lines than this has its middle elided.
const MAX_SPAN_LINES: usize = 6;
const EDGE_LINES: usize = 2;
/// What the gutter and the rail take from the width, beyond the line number.
const GUTTER_EXTRA: usize = 3;
const MIN_CODE_WIDTH: usize = 20;

/// Where each line of a file starts.
pub struct LineIndex(Vec<usize>);

impl LineIndex {
    pub fn new(source: &str) -> LineIndex {
        let mut starts = vec![0];
        starts.extend(source.bytes().enumerate().filter(|(_, b)| *b == b'\n').map(|(i, _)| i + 1));
        if starts.len() > 1 && starts.last() == Some(&source.len()) {
            starts.pop();
        }
        LineIndex(starts)
    }

    pub fn line_of(&self, byte: usize) -> usize {
        self.0.partition_point(|start| *start <= byte) - 1
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    fn line<'s>(&self, source: &'s str, line: usize) -> (usize, &'s str) {
        let start = self.0[line];
        let end = self.0.get(line + 1).copied().unwrap_or(source.len());
        let text = source[start..end].trim_end_matches('\n').trim_end_matches('\r');
        (start, text)
    }
}

/// The largest char boundary at or before `byte`, in range.
pub fn floor_boundary(source: &str, byte: usize) -> usize {
    let mut b = byte.min(source.len());
    while !source.is_char_boundary(b) {
        b -= 1;
    }
    b
}

#[derive(Clone, Copy)]
struct Cell {
    byte: usize,
    ch: char,
    width: usize,
}

fn cells(line: &str, base: usize) -> Vec<Cell> {
    let mut out = Vec::with_capacity(line.len());
    for (i, c) in line.char_indices() {
        let byte = base + i;
        if c == '\t' {
            out.extend((0..TAB_WIDTH).map(|_| Cell { byte, ch: ' ', width: 1 }));
        } else {
            let ch = clean_char(c);
            out.push(Cell { byte, ch, width: cell_width(ch) });
        }
    }
    out
}

fn indent(cells: &[Cell]) -> usize {
    cells.iter().take_while(|c| c.ch == ' ').count()
}

/// The columns before `byte`.
fn column(cells: &[Cell], byte: usize) -> usize {
    cells.iter().take_while(|c| c.byte < byte).map(|c| c.width).sum()
}

fn line_width(cells: &[Cell]) -> usize {
    cells.iter().map(|c| c.width).sum()
}

/// One row: a line number, if it's a line of code, and the styled text.
pub struct Row {
    pub number: Option<usize>,
    pub text: String,
}

pub struct Snippet {
    pub rows: Vec<Row>,
    /// The width of the line numbers.
    pub gutter: usize,
}

pub struct Span<'a> {
    pub start: usize,
    pub end: usize,
    pub label: Option<&'a str>,
    /// The SGR code of the underline.
    pub code: &'a str,
    /// The problem is with the whole file, so its span means nothing.
    pub file_level: bool,
}

/// The visible columns of one line.
#[derive(Clone, Copy)]
struct Window {
    start: usize,
    width: usize,
}

fn render_cells(line: &[Cell], window: Window, term: &Term, highlights: Option<&Highlights>, dim: bool) -> String {
    let Window { start, width } = window;
    let total = line_width(line);
    let clip_left = start > 0;
    let clip_right = total > start + width;
    let first = start + usize::from(clip_left);
    let last = (start + width).saturating_sub(usize::from(clip_right));
    let mut out = String::new();
    let mut current: Option<String> = None;
    let set = |out: &mut String, current: &mut Option<String>, code: Option<String>| {
        if term.color && *current != code {
            if current.is_some() {
                out.push_str("\x1b[0m");
            }
            if let Some(c) = &code {
                out.push_str(&format!("\x1b[{c}m"));
            }
            *current = code;
        }
    };
    let base = dim.then(|| style::DIM.to_string());
    if clip_left {
        set(&mut out, &mut current, base.clone());
        out.push('…');
    }
    let mut col = 0;
    for cell in line {
        let next = col + cell.width;
        if col >= first && next <= last {
            let token = highlights.and_then(|h| h.at(cell.byte).code());
            let code = match (dim, token) {
                (true, Some(t)) => Some(format!("{};{t}", style::DIM)),
                (true, None) => base.clone(),
                (false, t) => t.map(String::from),
            };
            set(&mut out, &mut current, code);
            out.push(cell.ch);
        }
        col = next;
    }
    if clip_right {
        set(&mut out, &mut current, base);
        out.push('…');
    }
    set(&mut out, &mut current, None);
    out
}

/// The source around `span`, or `None` if the span is the whole file, so that there's nothing
/// to point at.
pub fn render(source: &str, index: &LineIndex, span: &Span<'_>, term: &Term, highlights: Option<&Highlights>) -> Option<Snippet> {
    let start = floor_boundary(source, span.start);
    let mut end = floor_boundary(source, span.end.max(span.start));
    if span.file_level || (start == 0 && end >= source.trim_end().len() && index.len() > 2) {
        return None;
    }
    let first = index.line_of(start);
    // A span ending right after a newline ends on the line before.
    if end > start && index.line_of(end) > first && index.0[index.line_of(end)] == end {
        end -= 1;
    }
    let last = index.line_of(end);

    let mut shown: Vec<usize> = Vec::new();
    let is_blank = |l: usize| index.line(source, l).1.trim().is_empty();
    let before = first.checked_sub(1).filter(|l| !is_blank(*l));
    let after = Some(last + 1).filter(|l| *l < index.len() && !is_blank(*l));
    shown.extend(before);
    let span_lines = last - first + 1;
    let elided = span_lines > MAX_SPAN_LINES;
    if elided {
        shown.extend(first..first + EDGE_LINES);
        shown.extend(last + 1 - EDGE_LINES..=last);
    } else {
        shown.extend(first..=last);
    }
    shown.extend(after);

    let number_width = shown.iter().map(|l| l + 1).max().unwrap_or(1).to_string().len();
    let lines: Vec<(usize, Vec<Cell>)> = shown
        .iter()
        .map(|&l| {
            let (base, text) = index.line(source, l);
            (l, cells(text, base))
        })
        .collect();
    let dedent = lines.iter().filter(|(_, c)| c.iter().any(|x| x.ch != ' ')).map(|(_, c)| indent(c)).min().unwrap_or(0);
    let multi = first != last;
    let marker = if multi { 2 } else { 0 };
    let avail = term.width.saturating_sub(number_width + GUTTER_EXTRA + marker).max(MIN_CODE_WIDTH);

    // Everything below is in columns after dedenting.
    let target = lines.iter().find(|(l, _)| *l == first).map(|(_, c)| c.as_slice()).unwrap_or(&[]);
    let start_col = column(target, start).saturating_sub(dedent);
    let end_col = if multi { start_col } else { column(target, end).saturating_sub(dedent) };
    let span_width = end_col.saturating_sub(start_col).max(1);
    let widest = lines.iter().map(|(_, c)| line_width(c).saturating_sub(dedent)).max().unwrap_or(0);
    let window_start = if multi || widest <= avail || start_col + span_width <= avail {
        0
    } else {
        start_col.saturating_sub((avail.saturating_sub(span_width) / 2).max(4))
    };
    let window = Window { start: window_start, width: avail };

    let mut rows = Vec::new();
    let mut previous: Option<usize> = None;
    for (l, line) in &lines {
        if elided && previous.is_some_and(|p| p + 1 != *l) {
            let hidden = l - previous.unwrap_or(0) - 1;
            let bar = term.paint(span.code, "┆");
            rows.push(Row { number: None, text: format!("{bar} {}", term.paint(style::DIM, &format!("{hidden} more lines"))) });
        }
        previous = Some(*l);
        let in_span = (first..=last).contains(l);
        let body: Vec<Cell> = {
            let skip = indent(line).min(dedent);
            line[skip..].to_vec()
        };
        // Only the span's line scrolls: a context line scrolled with it is often just blank.
        let own = if in_span { window } else { Window { start: 0, width: avail } };
        let code = render_cells(&body, own, term, highlights, !in_span);
        let text = if multi {
            let bar = if in_span { term.paint(span.code, "┃") } else { " ".to_string() };
            format!("{bar} {code}")
        } else {
            code
        };
        rows.push(Row { number: Some(l + 1), text });
        if *l == last {
            mark_span(&mut rows, span, term, multi, start_col.saturating_sub(window_start), span_width, avail);
        }
    }
    Some(Snippet { rows, gutter: number_width })
}

/// The row or two under the span.
fn mark_span(rows: &mut Vec<Row>, span: &Span<'_>, term: &Term, multi: bool, offset: usize, span_width: usize, avail: usize) {
    if multi {
        if let Some(label) = span.label {
            rows.push(Row { number: None, text: term.paint(span.code, &format!("┗━ {label}")) });
        }
        return;
    }
    let width = span_width.min(avail.saturating_sub(offset)).max(1);
    let pad = " ".repeat(offset);
    let line = "━".repeat(width);
    let label = span.label.unwrap_or("");
    if !label.is_empty() && offset + width + 1 + label.chars().count() <= avail {
        rows.push(Row { number: None, text: format!("{pad}{}", term.paint(span.code, &format!("{line} {label}"))) });
        return;
    }
    rows.push(Row { number: None, text: format!("{pad}{}", term.paint(span.code, &line)) });
    if !label.is_empty() {
        rows.push(Row { number: None, text: term.paint(span.code, label) });
    }
}
