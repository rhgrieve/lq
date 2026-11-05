//! Port of Prettier's `printDocToString` and `fits`.

use super::utils::propagate_breaks;
use super::width::get_string_width;
use super::Doc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Break,
    Flat,
}

/// What a command prints: a doc, or the rest of a fill (`DOC_FILL_PRINTED_LENGTH`).
#[derive(Clone, Copy)]
enum R<'d, 'a> {
    D(&'d Doc<'a>),
    FillRest(&'d [Doc<'a>]),
}

#[derive(Clone, Copy)]
struct Cmd<'d, 'a> {
    /// Indent level. The root indent is always empty: `markAsRoot` is only used for
    /// the literal line in a block comment, which is its own doc.
    indent: u32,
    mode: Mode,
    doc: R<'d, 'a>,
}

pub struct PrintOptions {
    pub print_width: usize,
    pub tab_width: usize,
    pub use_tabs: bool,
    pub new_line: &'static str,
}

struct Printer<'p> {
    options: &'p PrintOptions,
    group_modes: Vec<Option<Mode>>,
    out: String,
    position: i64,
}

impl Printer<'_> {
    #[inline]
    fn indent_length(&self, level: u32) -> i64 {
        (level as usize * self.options.tab_width) as i64
    }

    fn write_indent(&mut self, level: u32) {
        if self.options.use_tabs {
            for _ in 0..level {
                self.out.push('\t');
            }
        } else {
            for _ in 0..level as usize * self.options.tab_width {
                self.out.push(' ');
            }
        }
    }

    #[inline]
    fn write_str(&mut self, s: &str) {
        if self.options.new_line == "\n" {
            self.out.push_str(s);
        } else {
            self.out.push_str(&s.replace('\n', self.options.new_line));
        }
        self.position += get_string_width(s) as i64;
    }

    fn trim(&mut self) {
        let trimmed = self.out.trim_end_matches([' ', '\t']).len();
        self.out.truncate(trimmed);
    }

    #[inline]
    fn group_mode(&self, id: u32) -> Option<Mode> {
        self.group_modes.get(id as usize).copied().flatten()
    }

    fn fits<'d, 'a>(
        &self,
        stack: &mut Vec<(Mode, R<'d, 'a>)>,
        rest: &[Cmd<'d, 'a>],
        mut remaining: i64,
        mut has_line_suffix: bool,
        must_be_flat: bool,
    ) -> bool {
        let mut rest_index = rest.len();
        let mut has_pending_space = false;
        while remaining >= 0 {
            let Some((mode, r)) = stack.pop() else {
                if rest_index == 0 {
                    return true;
                }
                rest_index -= 1;
                stack.push((rest[rest_index].mode, rest[rest_index].doc));
                continue;
            };
            let doc = match r {
                R::D(d) => d,
                R::FillRest(parts) => {
                    for p in parts.iter().rev() {
                        stack.push((mode, R::D(p)));
                    }
                    continue;
                }
            };
            match doc {
                Doc::Nil | Doc::BreakParent => {}
                Doc::Str(s) => {
                    if has_pending_space {
                        remaining -= 1;
                        has_pending_space = false;
                    }
                    remaining -= get_string_width(s) as i64;
                }
                Doc::String(s) => {
                    if has_pending_space {
                        remaining -= 1;
                        has_pending_space = false;
                    }
                    remaining -= get_string_width(s) as i64;
                }
                Doc::Concat(parts) | Doc::Fill(parts) => {
                    for p in parts.iter().rev() {
                        stack.push((mode, R::D(p)));
                    }
                }
                Doc::Indent(c) | Doc::DedentToRoot(c) => stack.push((mode, R::D(c))),
                Doc::Group(g) => {
                    if must_be_flat && g.break_ {
                        return false;
                    }
                    let group_mode = if g.break_ { Mode::Break } else { mode };
                    stack.push((group_mode, R::D(&g.contents)));
                }
                Doc::IfBreak(i) => {
                    let group_mode = if i.group_id != 0 {
                        self.group_mode(i.group_id).unwrap_or(Mode::Flat)
                    } else {
                        mode
                    };
                    let contents =
                        if group_mode == Mode::Break { &i.break_contents } else { &i.flat_contents };
                    if !matches!(contents, Doc::Nil) {
                        stack.push((mode, R::D(contents)));
                    }
                }
                Doc::Line => {
                    if mode == Mode::Break {
                        return true;
                    }
                    has_pending_space = true;
                }
                Doc::Softline => {
                    if mode == Mode::Break {
                        return true;
                    }
                }
                Doc::Hardline | Doc::Literalline | Doc::RootedLiteralline => return true,
                Doc::LineSuffix(_) => has_line_suffix = true,
                Doc::LineSuffixBoundary => {
                    if has_line_suffix {
                        return false;
                    }
                }
                // Embeds follow a hardline, which ends the measurement first.
                Doc::Embed(_) => return true,
            }
        }
        false
    }

    fn print<'d, 'a>(&mut self, doc: &'d Doc<'a>) {
        let width = self.options.print_width as i64;
        let mut commands: Vec<Cmd<'d, 'a>> = Vec::with_capacity(256);
        let mut fit_stack: Vec<(Mode, R<'d, 'a>)> = Vec::with_capacity(256);
        let mut line_suffixes: Vec<Cmd<'d, 'a>> = Vec::new();
        let mut should_remeasure = false;
        commands.push(Cmd { indent: 0, mode: Mode::Break, doc: R::D(doc) });

        while let Some(cmd) = commands.pop() {
            let Cmd { indent, mode, doc } = cmd;
            match doc {
                R::FillRest(parts) => {
                    self.print_fill(
                        parts,
                        indent,
                        mode,
                        width,
                        &mut commands,
                        &mut fit_stack,
                        !line_suffixes.is_empty(),
                    );
                }
                R::D(doc) => match doc {
                    Doc::Nil | Doc::BreakParent => {}
                    Doc::Str(s) => self.write_str(s),
                    Doc::String(s) => self.write_str(s),
                    Doc::Concat(parts) => {
                        for p in parts.iter().rev() {
                            commands.push(Cmd { indent, mode, doc: R::D(p) });
                        }
                    }
                    Doc::Indent(c) => {
                        commands.push(Cmd { indent: indent + 1, mode, doc: R::D(c) });
                    }
                    Doc::DedentToRoot(c) => {
                        commands.push(Cmd { indent: 0, mode, doc: R::D(c) });
                    }
                    Doc::Group(g) => {
                        let next_mode = if mode == Mode::Flat && !should_remeasure {
                            if g.break_ {
                                Mode::Break
                            } else {
                                Mode::Flat
                            }
                        } else {
                            should_remeasure = false;
                            if g.break_ {
                                Mode::Break
                            } else {
                                fit_stack.clear();
                                fit_stack.push((Mode::Flat, R::D(&g.contents)));
                                if self.fits(
                                    &mut fit_stack,
                                    &commands,
                                    width - self.position,
                                    !line_suffixes.is_empty(),
                                    false,
                                ) {
                                    Mode::Flat
                                } else {
                                    Mode::Break
                                }
                            }
                        };
                        commands.push(Cmd { indent, mode: next_mode, doc: R::D(&g.contents) });
                        if g.id != 0 {
                            let id = g.id as usize;
                            if id >= self.group_modes.len() {
                                self.group_modes.resize(id + 1, None);
                            }
                            self.group_modes[id] = Some(next_mode);
                        }
                    }
                    Doc::Fill(parts) => {
                        self.print_fill(
                            parts,
                            indent,
                            mode,
                            width,
                            &mut commands,
                            &mut fit_stack,
                            !line_suffixes.is_empty(),
                        );
                    }
                    Doc::IfBreak(i) => {
                        let group_mode =
                            if i.group_id != 0 { self.group_mode(i.group_id) } else { Some(mode) };
                        match group_mode {
                            Some(Mode::Break) => {
                                commands.push(Cmd { indent, mode, doc: R::D(&i.break_contents) });
                            }
                            Some(Mode::Flat) => {
                                commands.push(Cmd { indent, mode, doc: R::D(&i.flat_contents) });
                            }
                            None => {}
                        }
                    }
                    Doc::LineSuffix(c) => {
                        line_suffixes.push(Cmd { indent, mode, doc: R::D(c) });
                    }
                    Doc::LineSuffixBoundary => {
                        if !line_suffixes.is_empty() {
                            commands.push(Cmd { indent, mode, doc: R::D(&Doc::Hardline) });
                        }
                    }
                    Doc::Line | Doc::Softline | Doc::Hardline | Doc::Literalline
                    | Doc::RootedLiteralline => {
                        let hard = !matches!(doc, Doc::Line | Doc::Softline);
                        if mode == Mode::Flat && !hard {
                            if matches!(doc, Doc::Line) {
                                self.out.push(' ');
                                self.position += 1;
                            }
                        } else {
                            if mode == Mode::Flat {
                                should_remeasure = true;
                            }
                            if !line_suffixes.is_empty() {
                                commands.push(Cmd { indent, mode, doc: R::D(doc) });
                                commands.extend(line_suffixes.drain(..).rev());
                            } else {
                                match doc {
                                    Doc::Literalline => {
                                        self.out.push_str(self.options.new_line);
                                        self.position = 0;
                                    }
                                    Doc::RootedLiteralline => {
                                        self.out.push_str(self.options.new_line);
                                        self.write_indent(indent);
                                        self.position = self.indent_length(indent);
                                    }
                                    _ => {
                                        self.trim();
                                        self.out.push_str(self.options.new_line);
                                        self.write_indent(indent);
                                        self.position = self.indent_length(indent);
                                    }
                                }
                            }
                        }
                    }
                    Doc::Embed(e) => {
                        let body_indent = if e.indent_body { indent + 1 } else { 0 };
                        let available =
                            (self.options.print_width as i64 - self.indent_length(body_indent)).max(1);
                        match e.format(available as usize) {
                            Some(lines) => {
                                if mode == Mode::Flat {
                                    should_remeasure = true;
                                }
                                self.trim();
                                self.out.push_str(self.options.new_line);
                                self.write_indent(body_indent);
                                self.position = self.indent_length(body_indent);
                                self.write_embedded(&lines, body_indent);
                                self.trim();
                                self.out.push_str(self.options.new_line);
                                self.write_indent(indent);
                                self.position = self.indent_length(indent);
                            }
                            None => commands.push(Cmd { indent, mode, doc: R::D(&e.fallback) }),
                        }
                    }
                },
            }
            if commands.is_empty() && !line_suffixes.is_empty() {
                commands.extend(line_suffixes.drain(..).rev());
            }
        }
    }

    /// Writes formatted embedded code at the current indent. A literal line keeps its
    /// source column, as Prettier's `literalline` does for template literals.
    fn write_embedded(&mut self, lines: &[crate::embed::EmbeddedLine], indent: u32) {
        for (i, l) in lines.iter().enumerate() {
            if i > 0 {
                if !l.literal {
                    self.trim();
                }
                self.out.push_str(self.options.new_line);
                self.position = 0;
                if !l.literal && !l.text.is_empty() {
                    self.write_indent(indent);
                    self.position = self.indent_length(indent);
                }
            }
            self.write_str(&l.text);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn print_fill<'d, 'a>(
        &mut self,
        parts: &'d [Doc<'a>],
        indent: u32,
        mode: Mode,
        width: i64,
        commands: &mut Vec<Cmd<'d, 'a>>,
        fit_stack: &mut Vec<(Mode, R<'d, 'a>)>,
        has_line_suffix: bool,
    ) {
        let remaining = width - self.position;
        let length = parts.len();
        if length == 0 {
            return;
        }
        let content = &parts[0];
        let flat = |d: &'d Doc<'a>| Cmd { indent, mode: Mode::Flat, doc: R::D(d) };
        let brk = |d: &'d Doc<'a>| Cmd { indent, mode: Mode::Break, doc: R::D(d) };

        fit_stack.clear();
        fit_stack.push((Mode::Flat, R::D(content)));
        let content_fits = self.fits(fit_stack, &[], remaining, has_line_suffix, true);
        if length == 1 {
            commands.push(if content_fits { flat(content) } else { brk(content) });
            return;
        }
        let whitespace = &parts[1];
        if length == 2 {
            if content_fits {
                commands.push(flat(whitespace));
                commands.push(flat(content));
            } else {
                commands.push(brk(whitespace));
                commands.push(brk(content));
            }
            return;
        }
        let second = &parts[2];
        fit_stack.clear();
        fit_stack.push((Mode::Flat, R::D(second)));
        fit_stack.push((Mode::Flat, R::D(whitespace)));
        fit_stack.push((Mode::Flat, R::D(content)));
        let both_fit = self.fits(fit_stack, &[], remaining, has_line_suffix, true);
        commands.push(Cmd { indent, mode, doc: R::FillRest(&parts[2..]) });
        if both_fit {
            commands.push(flat(whitespace));
            commands.push(flat(content));
        } else if content_fits {
            commands.push(brk(whitespace));
            commands.push(flat(content));
        } else {
            commands.push(brk(whitespace));
            commands.push(brk(content));
        }
    }
}

/// `printDocToString`. Mutates `doc` through `propagateBreaks`.
pub fn print_doc_to_string(doc: &mut Doc<'_>, options: &PrintOptions, size_hint: usize) -> String {
    propagate_breaks(doc);
    let mut printer = Printer {
        options,
        group_modes: Vec::new(),
        out: String::with_capacity(size_hint + size_hint / 4),
        position: 0,
    };
    printer.print(doc);
    printer.out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::concat;
    use crate::doc::{group, if_break, indent, line_suffix};

    fn opts(w: usize) -> PrintOptions {
        PrintOptions { print_width: w, tab_width: 2, use_tabs: false, new_line: "\n" }
    }

    #[test]
    fn group_breaks_when_too_wide() {
        let mk = || group(concat!["[", indent(concat![Doc::Softline, "aaaa"]), Doc::Softline, "]"]);
        assert_eq!(print_doc_to_string(&mut mk(), &opts(80), 0), "[aaaa]");
        assert_eq!(print_doc_to_string(&mut mk(), &opts(4), 0), "[\n  aaaa\n]");
    }

    #[test]
    fn fill_wraps() {
        let mut d = Doc::Fill(vec!["aa".into(), Doc::Line, "bb".into(), Doc::Line, "cc".into()]);
        assert_eq!(print_doc_to_string(&mut d, &opts(5), 0), "aa bb\ncc");
    }

    #[test]
    fn if_break_on_unprinted_group_prints_nothing() {
        let mut d = concat![if_break("b", "f", 7), "x"];
        assert_eq!(print_doc_to_string(&mut d, &opts(80), 0), "x");
    }

    #[test]
    fn line_suffix_flushes_before_newline() {
        let mut d = concat!["a", line_suffix(" // c"), "b", Doc::Hardline, "d"];
        assert_eq!(print_doc_to_string(&mut d, &opts(80), 0), "ab // c\nd");
    }

    #[test]
    fn hardline_trims_trailing_whitespace() {
        let mut d = concat!["a  ", Doc::Hardline, "b  ", Doc::Literalline, "c"];
        assert_eq!(print_doc_to_string(&mut d, &opts(80), 0), "a\nb  \nc");
    }
}
