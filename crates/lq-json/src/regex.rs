//! A small backtracking regex engine with ECMAScript semantics.
//!
//! Use it where JS regex semantics are load-bearing and the pattern is simple or
//! rarely run: `\w`, `\d`, `\b` are ASCII-only, `\s` and `.` use JS's sets, `$`
//! does not match before a final newline, captures reset per quantifier
//! iteration, and lookaround and backreferences work.
//!
//! Matching works on code points (as with the `u` flag, also without it), so
//! `.` consumes a whole astral character. All offsets are byte offsets. It is
//! not fast: prefer hand-written scans in hot paths.

use crate::js::is_js_whitespace;

#[path = "regex_tables.rs"]
mod tables;

#[derive(Clone, Debug)]
enum Node {
    Empty,
    Char(char),
    Any,
    Class { negated: bool, items: Vec<ClassItem> },
    Start,
    End,
    WordBoundary(bool),
    Group(Box<Node>, Option<usize>),
    Backref(usize),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat { node: Box<Node>, min: u32, max: u32, greedy: bool, first_cap: usize, last_cap: usize },
    Look { node: Box<Node>, ahead: bool, negated: bool },
}

#[derive(Clone, Debug)]
enum ClassItem {
    Range(char, char),
    Digit(bool),
    Word(bool),
    Space(bool),
    Prop(UnicodeProp, bool),
}

#[derive(Clone, Copy, Debug)]
enum UnicodeProp {
    Letter,
    Number,
    Punctuation,
    Symbol,
    Separator,
    Control,
    Lowercase,
    Uppercase,
    /// An exact general category, from `regex_tables`.
    Table(&'static [(u32, u32)]),
}

#[derive(Clone, Debug)]
pub struct JsRegex {
    root: Node,
    groups: usize,
    names: Vec<(String, usize)>,
    ignore_case: bool,
    multiline: bool,
    dot_all: bool,
}

/// One match: `groups[0]` is the whole match, as `(start, end)` byte offsets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub groups: Vec<Option<(usize, usize)>>,
}

impl Match {
    pub fn start(&self) -> usize {
        self.groups[0].map_or(0, |g| g.0)
    }
    pub fn end(&self) -> usize {
        self.groups[0].map_or(0, |g| g.1)
    }
    pub fn as_str<'s>(&self, s: &'s str) -> &'s str {
        &s[self.start()..self.end()]
    }
    pub fn group<'s>(&self, s: &'s str, i: usize) -> Option<&'s str> {
        self.groups.get(i).copied().flatten().map(|(a, b)| &s[a..b])
    }
}

struct Parser<'p> {
    chars: Vec<char>,
    i: usize,
    groups: usize,
    names: Vec<(String, usize)>,
    pattern: &'p str,
}

impl Parser<'_> {
    fn err<T>(&self, msg: &str) -> Result<T, String> {
        Err(format!("Invalid regular expression: /{}/: {msg}", self.pattern))
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn looking_at(&self, s: &str) -> bool {
        s.chars().enumerate().all(|(k, c)| self.chars.get(self.i + k) == Some(&c))
    }

    fn alternation(&mut self) -> Result<Node, String> {
        let mut alts = vec![self.concat()?];
        while self.eat('|') {
            alts.push(self.concat()?);
        }
        Ok(if alts.len() == 1 { alts.pop().unwrap() } else { Node::Alt(alts) })
    }

    fn concat(&mut self) -> Result<Node, String> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            items.push(self.quantified()?);
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.pop().unwrap(),
            _ => Node::Concat(items),
        })
    }

    fn quantified(&mut self) -> Result<Node, String> {
        let first_cap = self.groups + 1;
        let atom = self.atom()?;
        let (min, max) = match self.peek() {
            Some('*') => {
                self.i += 1;
                (0, u32::MAX)
            }
            Some('+') => {
                self.i += 1;
                (1, u32::MAX)
            }
            Some('?') => {
                self.i += 1;
                (0, 1)
            }
            Some('{') => match self.braces() {
                Some(b) => b,
                None => return Ok(atom), // Annex B: a literal `{`, read next.
            },
            _ => return Ok(atom),
        };
        let greedy = !self.eat('?');
        Ok(Node::Repeat {
            node: Box::new(atom),
            min,
            max,
            greedy,
            first_cap,
            last_cap: self.groups,
        })
    }

    fn braces(&mut self) -> Option<(u32, u32)> {
        let save = self.i;
        self.i += 1;
        let num = |p: &mut Self| {
            let s = p.i;
            let mut v = 0u32;
            while let Some(d) = p.peek().and_then(|c| c.to_digit(10)) {
                v = v.saturating_mul(10).saturating_add(d);
                p.i += 1;
            }
            (p.i > s).then_some(v)
        };
        let result = (|| {
            let min = num(self)?;
            let max = if self.eat(',') {
                if self.peek() == Some('}') {
                    u32::MAX
                } else {
                    num(self)?
                }
            } else {
                min
            };
            self.eat('}').then_some((min, max))
        })();
        if result.is_none() {
            self.i = save;
        }
        result
    }

    fn atom(&mut self) -> Result<Node, String> {
        let c = self.peek().unwrap();
        match c {
            '(' => self.group(),
            '[' => self.class(),
            '.' => {
                self.i += 1;
                Ok(Node::Any)
            }
            '^' => {
                self.i += 1;
                Ok(Node::Start)
            }
            '$' => {
                self.i += 1;
                Ok(Node::End)
            }
            '\\' => self.escape(),
            '*' | '+' | '?' => self.err("Nothing to repeat"),
            '{' => {
                if self.braces().is_some() {
                    return self.err("Nothing to repeat");
                }
                self.i += 1;
                Ok(Node::Char('{'))
            }
            _ => {
                self.i += 1;
                Ok(Node::Char(c))
            }
        }
    }

    fn group(&mut self) -> Result<Node, String> {
        self.i += 1;
        let node = if self.looking_at("?:") {
            self.i += 2;
            Node::Group(Box::new(self.alternation()?), None)
        } else if self.looking_at("?=") || self.looking_at("?!") {
            let negated = self.chars[self.i + 1] == '!';
            self.i += 2;
            Node::Look { node: Box::new(self.alternation()?), ahead: true, negated }
        } else if self.looking_at("?<=") || self.looking_at("?<!") {
            let negated = self.chars[self.i + 2] == '!';
            self.i += 3;
            Node::Look { node: Box::new(self.alternation()?), ahead: false, negated }
        } else {
            let mut name = None;
            if self.looking_at("?<") {
                self.i += 2;
                let mut n = String::new();
                while let Some(c) = self.peek() {
                    self.i += 1;
                    if c == '>' {
                        break;
                    }
                    n.push(c);
                }
                name = Some(n);
            }
            self.groups += 1;
            let idx = self.groups;
            if let Some(n) = name {
                self.names.push((n, idx));
            }
            Node::Group(Box::new(self.alternation()?), Some(idx))
        };
        if !self.eat(')') {
            return self.err("Unterminated group");
        }
        Ok(node)
    }

    fn hex(&mut self, n: usize) -> Option<u32> {
        let mut v = 0;
        for k in 0..n {
            v = v * 16 + self.chars.get(self.i + k)?.to_digit(16)?;
        }
        self.i += n;
        Some(v)
    }

    /// The char of a `\x`, `\u` or control escape, after the backslash and letter.
    fn char_escape(&mut self, c: char) -> char {
        match c {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'f' => '\u{c}',
            'v' => '\u{b}',
            '0' => '\0',
            'x' => self.hex(2).and_then(char::from_u32).unwrap_or('x'),
            'u' => {
                if self.eat('{') {
                    let mut v = 0u32;
                    while let Some(d) = self.peek().and_then(|c| c.to_digit(16)) {
                        v = v * 16 + d;
                        self.i += 1;
                    }
                    self.eat('}');
                    char::from_u32(v).unwrap_or('\u{FFFD}')
                } else {
                    let v = self.hex(4);
                    match v {
                        Some(high @ 0xD800..=0xDBFF) if self.looking_at("\\u") => {
                            let save = self.i;
                            self.i += 2;
                            match self.hex(4) {
                                Some(low @ 0xDC00..=0xDFFF) => char::from_u32(
                                    0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00),
                                )
                                .unwrap_or('\u{FFFD}'),
                                _ => {
                                    self.i = save;
                                    '\u{FFFD}'
                                }
                            }
                        }
                        Some(v) => char::from_u32(v).unwrap_or('\u{FFFD}'),
                        None => 'u',
                    }
                }
            }
            other => other,
        }
    }

    fn unicode_prop(&mut self) -> Result<UnicodeProp, String> {
        if !self.eat('{') {
            return self.err("Invalid property name");
        }
        let mut name = String::new();
        while let Some(c) = self.peek() {
            self.i += 1;
            if c == '}' {
                break;
            }
            name.push(c);
        }
        Ok(match name.as_str() {
            "L" => UnicodeProp::Table(tables::L),
            "Lu" => UnicodeProp::Table(tables::LU),
            "Ll" => UnicodeProp::Table(tables::LL),
            "Nl" => UnicodeProp::Table(tables::NL),
            "Nd" => UnicodeProp::Table(tables::ND),
            "P" => UnicodeProp::Table(tables::P),
            "Pc" => UnicodeProp::Table(tables::PC),
            "Z" => UnicodeProp::Table(tables::Z),
            "Zs" => UnicodeProp::Table(tables::ZS),
            "Cc" => UnicodeProp::Table(tables::CC),
            "C" => UnicodeProp::Table(tables::C),
            "Letter" | "Alphabetic" => UnicodeProp::Letter,
            "N" | "Number" => UnicodeProp::Number,
            "Punctuation" => UnicodeProp::Punctuation,
            "S" | "Symbol" => UnicodeProp::Symbol,
            "Separator" => UnicodeProp::Separator,
            "Control" => UnicodeProp::Control,
            "Lowercase" => UnicodeProp::Lowercase,
            "Uppercase" => UnicodeProp::Uppercase,
            _ => return self.err("Invalid property name"),
        })
    }

    fn escape(&mut self) -> Result<Node, String> {
        self.i += 1;
        let Some(c) = self.peek() else { return self.err("\\ at end of pattern") };
        self.i += 1;
        Ok(match c {
            'd' | 'D' => Node::Class { negated: false, items: vec![ClassItem::Digit(c == 'D')] },
            'w' | 'W' => Node::Class { negated: false, items: vec![ClassItem::Word(c == 'W')] },
            's' | 'S' => Node::Class { negated: false, items: vec![ClassItem::Space(c == 'S')] },
            'p' | 'P' => {
                let prop = self.unicode_prop()?;
                Node::Class { negated: false, items: vec![ClassItem::Prop(prop, c == 'P')] }
            }
            'b' => Node::WordBoundary(false),
            'B' => Node::WordBoundary(true),
            '1'..='9' => {
                let mut v = c.to_digit(10).unwrap() as usize;
                while let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
                    v = v * 10 + d as usize;
                    self.i += 1;
                }
                Node::Backref(v)
            }
            'k' if self.peek() == Some('<') => {
                let end = self.chars[self.i..].iter().position(|&c| c == '>');
                let Some(end) = end else { return self.err("Invalid named reference") };
                let name: String = self.chars[self.i + 1..self.i + end].iter().collect();
                self.i += end + 1;
                // A reference ahead of its group always matches the empty string.
                match self.names.iter().find(|(n, _)| *n == name) {
                    Some((_, index)) => Node::Backref(*index),
                    None => Node::Empty,
                }
            }
            other => Node::Char(self.char_escape(other)),
        })
    }

    fn class_atom(&mut self) -> Result<Result<char, ClassItem>, String> {
        let Some(c) = self.peek() else { return self.err("Unterminated character class") };
        self.i += 1;
        if c != '\\' {
            return Ok(Ok(c));
        }
        let Some(e) = self.peek() else { return self.err("\\ at end of pattern") };
        self.i += 1;
        Ok(match e {
            'd' | 'D' => Err(ClassItem::Digit(e == 'D')),
            'w' | 'W' => Err(ClassItem::Word(e == 'W')),
            's' | 'S' => Err(ClassItem::Space(e == 'S')),
            'p' | 'P' => Err(ClassItem::Prop(self.unicode_prop()?, e == 'P')),
            'b' => Ok('\u{8}'),
            other => Ok(self.char_escape(other)),
        })
    }

    fn class(&mut self) -> Result<Node, String> {
        self.i += 1;
        let negated = self.eat('^');
        let mut items = Vec::new();
        loop {
            match self.peek() {
                None => return self.err("Unterminated character class"),
                Some(']') => {
                    self.i += 1;
                    break;
                }
                _ => {}
            }
            match self.class_atom()? {
                Err(item) => items.push(item),
                Ok(lo) => {
                    if self.peek() == Some('-') && self.chars.get(self.i + 1) != Some(&']')
                        && self.i + 1 < self.chars.len()
                    {
                        self.i += 1;
                        match self.class_atom()? {
                            Ok(hi) => {
                                if hi < lo {
                                    return self.err("Range out of order in character class");
                                }
                                items.push(ClassItem::Range(lo, hi));
                            }
                            Err(item) => {
                                items.push(ClassItem::Range(lo, lo));
                                items.push(ClassItem::Range('-', '-'));
                                items.push(item);
                            }
                        }
                    } else {
                        items.push(ClassItem::Range(lo, lo));
                    }
                }
            }
        }
        Ok(Node::Class { negated, items })
    }
}

#[inline]
fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

#[inline]
fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn fold(c: char) -> char {
    // Canonicalize: a simple uppercase mapping.
    let mut up = c.to_uppercase();
    match (up.next(), up.next()) {
        (Some(u), None) if !(c as u32 >= 128 && (u as u32) < 128) => u,
        _ => c,
    }
}

fn prop_matches(p: UnicodeProp, c: char) -> bool {
    match p {
        UnicodeProp::Letter => c.is_alphabetic(),
        UnicodeProp::Number => c.is_numeric(),
        UnicodeProp::Punctuation => {
            c.is_ascii_punctuation() && !matches!(c, '$' | '+' | '<' | '=' | '>' | '^' | '`' | '|' | '~')
        }
        UnicodeProp::Symbol => matches!(c, '$' | '+' | '<' | '=' | '>' | '^' | '`' | '|' | '~'),
        UnicodeProp::Separator => c == ' ' || (c != '\u{FEFF}' && !c.is_ascii() && is_js_whitespace(c)),
        UnicodeProp::Control => c.is_control(),
        UnicodeProp::Lowercase => c.is_lowercase(),
        UnicodeProp::Uppercase => c.is_uppercase(),
        UnicodeProp::Table(ranges) => {
            let cp = c as u32;
            ranges
                .binary_search_by(|&(lo, hi)| {
                    if hi < cp {
                        std::cmp::Ordering::Less
                    } else if lo > cp {
                        std::cmp::Ordering::Greater
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .is_ok()
        }
    }
}

type Caps = Vec<Option<(usize, usize)>>;

struct Ctx<'s> {
    s: &'s str,
    re: &'s JsRegex,
}

impl Ctx<'_> {
    fn next_char(&self, pos: usize) -> Option<char> {
        self.s[pos..].chars().next()
    }

    fn prev_char(&self, pos: usize) -> Option<char> {
        self.s[..pos].chars().next_back()
    }

    fn class_item(&self, item: &ClassItem, c: char) -> bool {
        match item {
            ClassItem::Range(lo, hi) => {
                if self.re.ignore_case {
                    if *lo == *hi {
                        return fold(*lo) == fold(c);
                    }
                    let f = fold(c);
                    (*lo <= c && c <= *hi)
                        || (*lo <= f && f <= *hi)
                        || c.to_lowercase().next().is_some_and(|l| *lo <= l && l <= *hi)
                } else {
                    *lo <= c && c <= *hi
                }
            }
            ClassItem::Digit(neg) => c.is_ascii_digit() != *neg,
            ClassItem::Word(neg) => is_word(c) != *neg,
            ClassItem::Space(neg) => is_js_whitespace(c) != *neg,
            ClassItem::Prop(p, neg) => prop_matches(*p, c) != *neg,
        }
    }

    fn m(
        &self,
        node: &Node,
        pos: usize,
        caps: &mut Caps,
        k: &mut dyn FnMut(usize, &mut Caps) -> bool,
    ) -> bool {
        match node {
            Node::Empty => k(pos, caps),
            Node::Char(ch) => match self.next_char(pos) {
                Some(c)
                    if c == *ch || (self.re.ignore_case && fold(c) == fold(*ch)) =>
                {
                    k(pos + c.len_utf8(), caps)
                }
                _ => false,
            },
            Node::Any => match self.next_char(pos) {
                Some(c) if self.re.dot_all || !is_line_terminator(c) => k(pos + c.len_utf8(), caps),
                _ => false,
            },
            Node::Class { negated, items } => match self.next_char(pos) {
                Some(c) if items.iter().any(|i| self.class_item(i, c)) != *negated => {
                    k(pos + c.len_utf8(), caps)
                }
                _ => false,
            },
            Node::Start => {
                (pos == 0
                    || (self.re.multiline && self.prev_char(pos).is_some_and(is_line_terminator)))
                    && k(pos, caps)
            }
            Node::End => {
                (pos == self.s.len()
                    || (self.re.multiline && self.next_char(pos).is_some_and(is_line_terminator)))
                    && k(pos, caps)
            }
            Node::WordBoundary(negated) => {
                let a = self.prev_char(pos).is_some_and(is_word);
                let b = self.next_char(pos).is_some_and(is_word);
                ((a != b) != *negated) && k(pos, caps)
            }
            Node::Group(inner, None) => self.m(inner, pos, caps, k),
            Node::Group(inner, Some(i)) => {
                let i = *i;
                let saved = caps[i];
                let matched = self.m(inner, pos, caps, &mut |end, caps| {
                    let prev = caps[i];
                    caps[i] = Some((pos, end));
                    if k(end, caps) {
                        return true;
                    }
                    caps[i] = prev;
                    false
                });
                if !matched {
                    caps[i] = saved;
                }
                matched
            }
            Node::Backref(i) => {
                let cap = caps.get(*i).copied().flatten();
                match cap {
                    None => k(pos, caps),
                    Some((a, b)) => {
                        let want = &self.s[a..b];
                        let rest = &self.s[pos..];
                        let matched = if self.re.ignore_case {
                            let mut wi = want.chars();
                            let mut ri = rest.char_indices();
                            let mut len = 0;
                            let mut ok = true;
                            for w in wi.by_ref() {
                                match ri.next() {
                                    Some((_, r)) if fold(r) == fold(w) => len += r.len_utf8(),
                                    _ => {
                                        ok = false;
                                        break;
                                    }
                                }
                            }
                            ok.then_some(len)
                        } else {
                            rest.starts_with(want).then_some(want.len())
                        };
                        match matched {
                            Some(len) => k(pos + len, caps),
                            None => false,
                        }
                    }
                }
            }
            Node::Concat(items) => self.concat(items, pos, caps, k),
            Node::Alt(alts) => alts.iter().any(|a| self.m(a, pos, caps, k)),
            Node::Repeat { node, min, max, greedy, first_cap, last_cap } => {
                self.repeat(node, *min, *max, *greedy, (*first_cap, *last_cap), 0, pos, caps, k)
            }
            Node::Look { node, ahead, negated } => {
                let mut sub = caps.clone();
                let found = if *ahead {
                    self.m(node, pos, &mut sub, &mut |_, _| true)
                } else {
                    let mut starts: Vec<usize> =
                        self.s[..pos].char_indices().map(|(i, _)| i).collect();
                    starts.push(pos);
                    starts.iter().rev().any(|&st| {
                        self.m(node, st, &mut sub, &mut |end, _| end == pos)
                    })
                };
                if found == *negated {
                    return false;
                }
                if *negated {
                    k(pos, caps)
                } else {
                    let saved = std::mem::replace(caps, sub);
                    if k(pos, caps) {
                        return true;
                    }
                    *caps = saved;
                    false
                }
            }
        }
    }

    fn concat(
        &self,
        items: &[Node],
        pos: usize,
        caps: &mut Caps,
        k: &mut dyn FnMut(usize, &mut Caps) -> bool,
    ) -> bool {
        match items.split_first() {
            None => k(pos, caps),
            Some((first, rest)) => {
                self.m(first, pos, caps, &mut |p, caps| self.concat(rest, p, caps, k))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn repeat(
        &self,
        node: &Node,
        min: u32,
        max: u32,
        greedy: bool,
        cap_range: (usize, usize),
        count: u32,
        pos: usize,
        caps: &mut Caps,
        k: &mut dyn FnMut(usize, &mut Caps) -> bool,
    ) -> bool {
        let try_more = |this: &Self, caps: &mut Caps, k: &mut dyn FnMut(usize, &mut Caps) -> bool| {
            if count >= max {
                return false;
            }
            let saved: Vec<_> = (cap_range.0..=cap_range.1).map(|i| caps[i]).collect();
            caps[cap_range.0..=cap_range.1].fill(None);
            let matched = this.m(node, pos, caps, &mut |p, caps| {
                // An iteration past `min` that consumed nothing fails, as in JS.
                if p == pos && count >= min {
                    return false;
                }
                this.repeat(node, min, max, greedy, cap_range, count + 1, p, caps, k)
            });
            if !matched {
                for (o, i) in (cap_range.0..=cap_range.1).enumerate() {
                    caps[i] = saved[o];
                }
            }
            matched
        };
        if count < min {
            return try_more(self, caps, k);
        }
        // The order of the operands is the difference between greedy and lazy.
        #[allow(clippy::if_same_then_else)]
        if greedy {
            try_more(self, caps, k) || k(pos, caps)
        } else {
            k(pos, caps) || try_more(self, caps, k)
        }
    }
}

impl JsRegex {
    /// `new RegExp(pattern, flags)`. `g`, `y`, `u`, `d` and `v` are accepted and do
    /// not change matching; use [`Self::find_iter`] for `g`.
    pub fn new(pattern: &str, flags: &str) -> Result<JsRegex, String> {
        let mut p =
            Parser { chars: pattern.chars().collect(), i: 0, groups: 0, names: Vec::new(), pattern };
        let root = p.alternation()?;
        if p.peek().is_some() {
            return p.err("Unmatched ')'");
        }
        Ok(JsRegex {
            root,
            groups: p.groups,
            names: p.names,
            ignore_case: flags.contains('i'),
            multiline: flags.contains('m'),
            dot_all: flags.contains('s'),
        })
    }

    /// `regex.test(s)`.
    pub fn is_match(&self, s: &str) -> bool {
        self.exec_at(s, 0).is_some()
    }

    /// The match at exactly `start`, if any (the `y` flag).
    pub fn match_at(&self, s: &str, start: usize) -> Option<Match> {
        let ctx = Ctx { s, re: self };
        let mut caps: Caps = vec![None; self.groups + 1];
        let mut end = None;
        if ctx.m(&self.root, start, &mut caps, &mut |e, _| {
            end = Some(e);
            true
        }) {
            caps[0] = Some((start, end.unwrap()));
            Some(Match { groups: caps })
        } else {
            None
        }
    }

    /// `regex.exec(s)` with `lastIndex = start` (a char boundary), scanning forward.
    pub fn exec_at(&self, s: &str, start: usize) -> Option<Match> {
        let mut pos = start;
        loop {
            if let Some(m) = self.match_at(s, pos) {
                return Some(m);
            }
            if pos >= s.len() {
                return None;
            }
            pos += s[pos..].chars().next().map_or(1, char::len_utf8);
        }
    }

    /// All matches, as `s.matchAll(g)` (an empty match advances one char).
    pub fn find_iter(&self, s: &str) -> Vec<Match> {
        let mut out = Vec::new();
        let mut pos = 0;
        while pos <= s.len() {
            let Some(m) = self.exec_at(s, pos) else { break };
            pos = if m.end() == m.start() {
                m.end() + s[m.end()..].chars().next().map_or(1, char::len_utf8)
            } else {
                m.end()
            };
            out.push(m);
        }
        out
    }

    /// The index of a named group.
    pub fn group_index(&self, name: &str) -> Option<usize> {
        self.names.iter().find(|(n, _)| n == name).map(|(_, i)| *i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(p: &str, s: &str) -> bool {
        JsRegex::new(p, "u").unwrap().is_match(s)
    }

    #[test]
    fn schema_patterns() {
        assert!(t("^(-?[0-9]+(px|%) ?){1,4}$", "1px 2% -3px 4%"));
        assert!(!t("^(-?[0-9]+(px|%) ?){1,4}$", "1px 2% 3px 4% 5px"));
        assert!(t("^[a-zA-Z]\\w*$", "a_1"));
        assert!(!t("^[a-zA-Z]\\w*$", "aé"));
        assert!(t("^t:", "t:foo"));
        assert!(t(".*_html$", "foo_html"));
        assert!(!t(".*_html$", "foo_html\n"));
        assert!(!t("^[0-9]+$", ""));
    }

    #[test]
    fn js_semantics() {
        let re = JsRegex::new("(a)|(b)", "").unwrap();
        let m = re.exec_at("b", 0).unwrap();
        assert_eq!(m.groups[1], None);
        assert_eq!(m.groups[2], Some((0, 1)));
        // Captures reset per iteration: /(?:(a)|b)*/ on "ab" leaves group 1 undefined.
        let re = JsRegex::new("(?:(a)|b)*", "").unwrap();
        assert_eq!(re.exec_at("ab", 0).unwrap().groups[1], None);
        assert!(JsRegex::new("^\\s+$", "").unwrap().is_match("\u{FEFF}\u{A0}"));
        assert!(JsRegex::new("(?<!x)y", "").unwrap().is_match("ay"));
        assert!(!JsRegex::new("(?<!x)y", "").unwrap().is_match("xy"));
        assert!(JsRegex::new("^(?!\\.)a", "").unwrap().is_match("a"));
        assert!(JsRegex::new("HELLO", "i").unwrap().is_match("hello"));
        assert!(JsRegex::new("(a)\\1", "").unwrap().is_match("aa"));
        assert!(JsRegex::new("[", "").is_err());
        assert_eq!(JsRegex::new("a*", "").unwrap().find_iter("baa").len(), 3);
        assert_eq!(JsRegex::new("x{2,3}", "").unwrap().exec_at("xxxx", 0).unwrap().end(), 3);
        assert_eq!(JsRegex::new("x{2,3}?", "").unwrap().exec_at("xxxx", 0).unwrap().end(), 2);
    }
}
