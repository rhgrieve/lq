//! A port of `minimatch` 11.0.9 with default options (and `dot`) on a POSIX platform.
//!
//! Compilation follows the upstream steps: comment and negation handling, brace
//! expansion (`brace-expansion` 5.0.12, `balanced-match` 4.0.4) that leaves large
//! numeric ranges for the matcher, slash splitting, level-one optimisation, then
//! one matcher per path portion. A portion without magic (upstream's `literal()`)
//! is compared as a string; the others compile to a [`matcher::Matcher`], the
//! port of `@isaacs/nonrecursive-glob-match` that replaced 10's regexes.
//!
//! Left out because they only make matching faster: `mergeRows` (rows that differ
//! in one portion merged into an `@(a|b)`), the `#canMatch` checks of the unsplit
//! path, the `#matchShape` shortcuts, and the caches of compiled patterns and
//! split paths.

mod matcher;

use lq_json::js::{number_to_string, utf16_len};
use matcher::Matcher;

#[derive(Debug)]
enum Seg {
    Lit(String),
    Globstar,
    Magic(Matcher),
}

impl Seg {
    /// `portionMatch(p, f)`: an empty path portion only matches an empty pattern
    /// portion.
    fn test(&self, f: &str) -> bool {
        match self {
            Seg::Lit(s) => s == f,
            Seg::Globstar => false,
            Seg::Magic(m) => !f.is_empty() && m.test(f),
        }
    }
}

/// `#getGlobstarParts`: a row with a `**`, split into the portions before the first
/// `**`, after the last one, and the sections between them. These only depend on
/// the row, so they are kept with it.
#[derive(Debug)]
struct GlobstarParts {
    head: usize,
    tail: usize,
    body: usize,
    /// Each section's range in the row, and the portions that it and the
    /// sections after it need.
    segments: Vec<(std::ops::Range<usize>, usize)>,
}

#[derive(Debug)]
struct Row {
    segs: Vec<Seg>,
    globstar: Option<GlobstarParts>,
}

impl Row {
    fn new(segs: Vec<Seg>) -> Row {
        let is_gs = |s: &Seg| matches!(s, Seg::Globstar);
        let globstar = segs.iter().position(is_gs).map(|firstgs| {
            let lastgs = segs.iter().rposition(is_gs).expect("globstar");
            let mut segments = vec![(firstgs + 1..firstgs + 1, 0)];
            let mut non_gs = 0;
            let mut sums = vec![0];
            for (i, s) in segs.iter().enumerate().take(lastgs).skip(firstgs + 1) {
                if is_gs(s) {
                    sums.push(non_gs);
                    segments.push((i + 1..i + 1, 0));
                } else {
                    segments.last_mut().expect("nonempty").0.end = i + 1;
                    non_gs += 1;
                }
            }
            // the portions in this section and the ones after it
            for (s, sum) in segments.iter_mut().zip(&sums) {
                s.1 = non_gs - sum;
            }
            GlobstarParts { head: firstgs, tail: segs.len() - lastgs - 1, body: lastgs.saturating_sub(firstgs + 1), segments }
        });
        Row { segs, globstar }
    }
}

/// A compiled pattern.
#[derive(Debug)]
pub struct Minimatch {
    /// `options.dot`.
    dot: bool,
    comment: bool,
    empty: bool,
    negate: bool,
    set: Vec<Row>,
    error: Option<String>,
}

/// `minimatch(path, pattern)`. Compiles the pattern every call; keep a
/// [`Minimatch`] around for repeated use. A pattern upstream would throw on (see
/// [`Minimatch::error`]) matches nothing here.
pub fn minimatch(path: &str, pattern: &str) -> bool {
    Minimatch::new(pattern).matches(path)
}

/// `MAX_EXTGLOBS`: portions with more extglob openers are rejected.
const MAX_EXTGLOBS: usize = 256;

impl Minimatch {
    pub fn new(pattern: &str) -> Minimatch {
        Minimatch::with_dot(pattern, false)
    }

    /// `new Minimatch(pattern, { dot })`: with `dot`, wildcards and `**` match names that
    /// start with a `.`, other than `.` and `..`.
    pub fn with_dot(pattern: &str, dot: bool) -> Minimatch {
        let mut mm = Minimatch { dot, comment: false, empty: false, negate: false, set: Vec::new(), error: None };
        if utf16_len(pattern) > 64 * 1024 {
            mm.error = Some("pattern is too long".into());
            return mm;
        }
        if pattern.starts_with('#') {
            mm.comment = true;
            return mm;
        }
        if pattern.is_empty() {
            mm.empty = true;
            return mm;
        }
        let bangs = pattern.bytes().take_while(|b| *b == b'!').count();
        mm.negate = bangs % 2 == 1;
        let pattern = &pattern[bangs..];

        let (globs, range_mark) = expand_braces(pattern);
        let mut seen: Vec<&str> = Vec::new();
        for g in &globs {
            if !seen.contains(&g.as_str()) {
                seen.push(g);
            }
        }
        for g in seen {
            let parts = level_one_optimize(slash_split(g));
            let mut segs = Vec::with_capacity(parts.len());
            for p in parts {
                match parse(p, dot, range_mark) {
                    Ok(s) => segs.push(s),
                    Err(e) => {
                        mm.error = Some(e);
                        return mm;
                    }
                }
            }
            mm.set.push(Row::new(segs));
        }
        mm
    }

    /// The message of the exception upstream throws while compiling this pattern,
    /// if any: an over-long pattern, or a portion with too many extglobs. Such a
    /// pattern never matches here.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn matches(&self, path: &str) -> bool {
        if self.comment || self.error.is_some() {
            return false;
        }
        if self.empty {
            return path.is_empty();
        }
        self.matches_parts(&slash_split(path))
    }

    /// As [`Self::matches`], with `path` already split by [`split_path`].
    pub fn matches_parts(&self, file: &[&str]) -> bool {
        if self.comment || self.error.is_some() {
            return false;
        }
        if self.empty {
            return file.len() == 1 && file[0].is_empty();
        }
        for row in &self.set {
            let hit = match &row.globstar {
                Some(gs) => match_globstar(file, row, gs, self.dot),
                None => match_one(file, &row.segs, 0, 0, file.len()),
            };
            if hit {
                return !self.negate;
            }
        }
        self.negate
    }

    /// The compiled set as `(is_globstar, literal, tester)` per segment, for the
    /// discovery walk over a root that contains glob magic.
    pub(crate) fn segments(&self) -> Vec<Vec<SegRef<'_>>> {
        self.set
            .iter()
            .map(|p| {
                p.segs
                    .iter()
                    .map(|s| match s {
                        Seg::Lit(l) => SegRef::Lit(l),
                        Seg::Globstar => SegRef::Globstar,
                        other => SegRef::Magic(MagicSeg(other)),
                    })
                    .collect()
            })
            .collect()
    }
}

pub(crate) enum SegRef<'a> {
    Lit(&'a str),
    Globstar,
    Magic(MagicSeg<'a>),
}

pub(crate) struct MagicSeg<'a>(&'a Seg);

impl MagicSeg<'_> {
    pub(crate) fn test(&self, f: &str) -> bool {
        self.0.test(f)
    }
}

/// `p.split(/\/+/)`.
pub fn split_path(p: &str) -> Vec<&str> {
    slash_split(p)
}

fn slash_split(p: &str) -> Vec<&str> {
    let mut out = Vec::with_capacity(8);
    let b = p.as_bytes();
    let (mut start, mut i) = (0, 0);
    while i < b.len() {
        if b[i] == b'/' {
            out.push(&p[start..i]);
            while i < b.len() && b[i] == b'/' {
                i += 1;
            }
            start = i;
        } else {
            i += 1;
        }
    }
    out.push(&p[start..]);
    out
}

fn level_one_optimize(parts: Vec<&str>) -> Vec<&str> {
    let mut set: Vec<&str> = Vec::with_capacity(parts.len());
    for part in parts {
        let prev = set.last().copied();
        if part == "**" && prev == Some("**") {
            continue;
        }
        if part == ".." {
            if let Some(p) = prev {
                if !p.is_empty() && p != ".." && p != "." && p != "**" {
                    set.pop();
                    continue;
                }
            }
        }
        set.push(part);
    }
    if set.is_empty() {
        set.push("");
    }
    set
}

/// What `**` doesn't walk into: `.` and `..`, and without `dot` any name starting with `.`.
fn is_dot_part(f: &str, dot: bool) -> bool {
    f == "." || f == ".." || (!dot && f.starts_with('.'))
}

/// `#matchOne(file, pattern, partial, fileIndex, patternIndex, fileEnd)`, without
/// `partial`: only the file portions before `file_end` are matched.
fn match_one(file: &[&str], pattern: &[Seg], mut fi: usize, mut pi: usize, file_end: usize) -> bool {
    let (fl, pl) = (file_end.min(file.len()), pattern.len());
    while fi < fl && pi < pl {
        let p = &pattern[pi];
        if matches!(p, Seg::Globstar) || !p.test(file[fi]) {
            return false;
        }
        fi += 1;
        pi += 1;
    }
    if fi == fl && pi == pl {
        true
    } else if fi == fl {
        false
    } else {
        // Ran out of pattern: only acceptable on the empty segment of a trailing slash.
        fi == fl - 1 && file[fi].is_empty()
    }
}

/// `#matchGlobstar(file, pattern, false, 0, 0)`.
fn match_globstar(file: &[&str], row: &Row, gs: &GlobstarParts, dot: bool) -> bool {
    let segs = &row.segs;
    let head = &segs[..gs.head];
    let tail = &segs[segs.len() - gs.tail..];
    let mut file_index = 0;

    if !head.is_empty() {
        if !match_one(file, head, 0, 0, head.len()) {
            return false;
        }
        file_index += head.len();
    }

    let mut tail_match = 0;
    if !tail.is_empty() {
        if tail.len() + file_index > file.len() {
            return false;
        }
        let tail_start = file.len() - tail.len();
        if match_one(file, tail, tail_start, 0, file.len()) {
            tail_match = tail.len();
        } else {
            // a/**/* matching a/b/
            if !file[file.len() - 1].is_empty() || file_index + tail.len() == file.len() {
                return false;
            }
            if !match_one(file, tail, tail_start - 1, 0, file.len()) {
                return false;
            }
            tail_match = tail.len() + 1;
        }
    }

    if gs.body == 0 {
        let mut saw_some = tail_match != 0;
        let end = file.len() - tail_match;
        let mut i = file_index;
        while i < end {
            saw_some = true;
            if is_dot_part(file[i], dot) {
                return false;
            }
            i += 1;
        }
        return saw_some;
    }

    let file_length = file.len() - tail_match;
    let mut tried = vec![false; (gs.segments.len() + 1) * (file.len() + 1)];
    let mut walk = BodyWalk { file, segs, segments: &gs.segments, file_length, dot, tried: &mut tried };
    walk.sections(file_index, 0, 0, tail_match != 0) == Some(true)
}

/// The arguments of `#matchGlobStarBodySections` that stay the same in a walk.
struct BodyWalk<'a> {
    file: &'a [&'a str],
    segs: &'a [Seg],
    segments: &'a [(std::ops::Range<usize>, usize)],
    /// The end of the file portions that the body can match.
    file_length: usize,
    dot: bool,
    /// The places that a walk has already tried, as
    /// `body_index * (file.len() + 1) + file_index`. A walk from a place does the
    /// same no matter how it got there, and it only comes back if it found no
    /// match, so a walk that gets to one stops.
    tried: &'a mut [bool],
}

impl BodyWalk<'_> {
    /// `#matchGlobStarBodySections`: `Some(false)` is "not matching", `None` "not
    /// matching, stop trying".
    fn sections(&mut self, mut file_index: usize, body_index: usize, depth: usize, mut saw_tail: bool) -> Option<bool> {
        let file = self.file;
        let Some((range, need)) = self.segments.get(body_index).cloned() else {
            // just make sure that there's no bad dots before the tail
            for f in &file[file_index.min(self.file_length)..self.file_length] {
                saw_tail = true;
                if is_dot_part(f, self.dot) {
                    return Some(false);
                }
            }
            return Some(saw_tail);
        };
        let body = &self.segs[range];
        // the last index where the section can start
        let after = self.file_length.checked_sub(need)?;
        while file_index <= after {
            let key = body_index * (file.len() + 1) + file_index;
            if self.tried[key] {
                return Some(false);
            }
            self.tried[key] = true;
            let m = match_one(file, body, file_index, 0, file_index + body.len());
            // if limit exceeded, no match: upstream's intentional false negative
            if m && depth < 200 {
                let sub = self.sections(file_index + body.len(), body_index + 1, depth + 1, saw_tail);
                if sub != Some(false) {
                    return sub;
                }
            }
            // the ** does not walk past the last place the body can start
            if file_index == after {
                break;
            }
            if is_dot_part(file[file_index], self.dot) {
                return Some(false);
            }
            file_index += 1;
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Brace expansion
// ---------------------------------------------------------------------------

const ESC_SLASH: &str = "\0SLASH\0";
const ESC_OPEN: &str = "\0OPEN\0";
const ESC_CLOSE: &str = "\0CLOSE\0";
const ESC_COMMA: &str = "\0COMMA\0";
const ESC_PERIOD: &str = "\0PERIOD\0";
const EXPANSION_MAX: usize = 100_000;
const EXPANSION_MAX_LENGTH: usize = 4_000_000;
const EXPANSION_MAX_DEPTH: usize = 1_000;
const EXPANSION_MAX_REWRITES: usize = 1_000;

fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// `/\{(?:(?!\{).)*\}/.test(pattern)`
fn has_brace_set(p: &str) -> bool {
    let mut open = false;
    for c in p.chars() {
        match c {
            '{' => open = true,
            '}' if open => return true,
            c if is_line_terminator(c) => open = false,
            _ => {}
        }
    }
    false
}

/// `braceExpand(pattern)`.
pub fn brace_expand(pattern: &str) -> Vec<String> {
    if !has_brace_set(pattern) {
        return vec![pattern.to_string()];
    }
    // brace-expansion turns each escaped backslash `\\` into a single `\`, which
    // then escapes the next character instead, and can turn an escaped magic
    // character (`\\\*`) into a live one. Doubling every pair brings an escaped
    // backslash back out still escaped.
    expand(&pattern.replace("\\\\", "\\\\\\\\"))
}

fn expand(s: &str) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    let owned;
    let s = if let Some(rest) = s.strip_prefix("{}") {
        owned = format!("\\{{\\}}{rest}");
        owned.as_str()
    } else {
        s
    };
    let escaped = s
        .replace("\\\\", ESC_SLASH)
        .replace("\\{", ESC_OPEN)
        .replace("\\}", ESC_CLOSE)
        .replace("\\,", ESC_COMMA)
        .replace("\\.", ESC_PERIOD);
    expand_(escaped, 0, true)
        .into_iter()
        .map(|e| {
            if e.contains('\0') {
                e.replace(ESC_SLASH, "\\")
                    .replace(ESC_OPEN, "{")
                    .replace(ESC_CLOSE, "}")
                    .replace(ESC_COMMA, ",")
                    .replace(ESC_PERIOD, ".")
            } else {
                e
            }
        })
        .collect()
}

struct Balanced {
    pre: String,
    body: String,
    post: String,
}

/// `balanced('{', '}', str)`.
fn balanced(s: &str) -> Option<Balanced> {
    let b = s.as_bytes();
    let index_of = |c: u8, from: isize| -> isize {
        let from = from.max(0) as usize;
        if from >= b.len() {
            return -1;
        }
        b[from..].iter().position(|x| *x == c).map_or(-1, |p| (p + from) as isize)
    };
    let mut ai = index_of(b'{', 0);
    let mut bi = index_of(b'}', ai + 1);
    let mut i = ai;
    let mut result = None;
    if ai >= 0 && bi > 0 {
        let mut begs: Vec<isize> = Vec::new();
        let mut left = b.len() as isize;
        let mut right: Option<isize> = None;
        while i >= 0 && result.is_none() {
            if i == ai {
                begs.push(i);
                ai = index_of(b'{', i + 1);
            } else if begs.len() == 1 {
                let r = begs.pop().expect("one");
                result = Some((r, bi));
            } else {
                let beg = begs.pop().expect("nonempty");
                if beg < left {
                    left = beg;
                    right = Some(bi);
                }
                bi = index_of(b'}', i + 1);
            }
            i = if ai < bi && ai >= 0 { ai } else { bi };
        }
        if !begs.is_empty() {
            if let Some(r) = right {
                result = Some((left, r));
            }
        }
    }
    let (start, end) = result?;
    let (start, end) = (start as usize, end as usize);
    Some(Balanced {
        pre: s[..start].to_string(),
        body: s[start + 1..end].to_string(),
        post: s[end + 1..].to_string(),
    })
}

fn parse_comma_parts(s: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut carry = String::new();
    let mut s = s.to_string();
    loop {
        let Some(m) = balanced(&s) else {
            let mut tail: Vec<String> = s.split(',').map(str::to_string).collect();
            tail[0] = format!("{carry}{}", tail[0]);
            parts.append(&mut tail);
            return parts;
        };
        let mut p: Vec<String> = m.pre.split(',').map(str::to_string).collect();
        p[0] = format!("{carry}{}", p[0]);
        let last = p.len() - 1;
        p[last].push('{');
        p[last].push_str(&m.body);
        p[last].push('}');
        if m.post.is_empty() {
            parts.append(&mut p);
            return parts;
        }
        carry = p.pop().expect("nonempty");
        parts.append(&mut p);
        s = m.post;
    }
}

fn combine(acc: &[String], pre: &str, values: &[String], drop_empties: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut length = 0;
    for a in acc {
        for v in values {
            if out.len() >= EXPANSION_MAX {
                return out;
            }
            let expansion = format!("{a}{pre}{v}");
            if drop_empties && expansion.is_empty() {
                continue;
            }
            if length + expansion.len() > EXPANSION_MAX_LENGTH {
                return out;
            }
            length += expansion.len();
            out.push(expansion);
        }
    }
    out
}

fn numeric(s: &str) -> f64 {
    // `!isNaN(str) ? parseInt(str, 10) : str.charCodeAt(0)`; bodies are pre-validated.
    match s.parse::<f64>() {
        Ok(n) if s.bytes().all(|b| b.is_ascii_digit() || b == b'-') => n,
        _ => s.bytes().next().map_or(f64::NAN, f64::from),
    }
}

fn is_padded(el: &str) -> bool {
    let b = el.as_bytes();
    let b = if b.first() == Some(&b'-') { &b[1..] } else { b };
    b.len() >= 2 && b[0] == b'0' && b[1].is_ascii_digit()
}

fn expand_sequence(body: &str, is_alpha: bool) -> Vec<String> {
    let n: Vec<&str> = body.split("..").collect();
    let mut out = Vec::new();
    let x = numeric(n[0]);
    let y = numeric(n[1]);
    let width = utf16_len(n[0]).max(utf16_len(n[1]));
    let mut incr = if n.len() == 3 { numeric(n[2]).abs().max(1.0) } else { 1.0 };
    let reverse = y < x;
    if reverse {
        incr = -incr;
    }
    let pad = n.iter().any(|e| is_padded(e));
    let mut length = 0;
    let mut i = x;
    while (if reverse { i >= y } else { i <= y }) && out.len() < EXPANSION_MAX {
        let c = if is_alpha {
            let ch = char::from_u32(i as u32).unwrap_or('\u{FFFD}');
            if ch == '\\' {
                String::new()
            } else {
                ch.to_string()
            }
        } else {
            let mut c = number_to_string(i);
            if pad && width > c.len() {
                let z = "0".repeat(width - c.len());
                c = if i < 0.0 { format!("-{z}{}", &c[1..]) } else { format!("{z}{c}") };
            }
            c
        };
        if length + c.len() > EXPANSION_MAX_LENGTH {
            break;
        }
        length += c.len();
        out.push(c);
        i += incr;
    }
    out
}

fn digits(b: &[u8], mut i: usize) -> Option<usize> {
    let st = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    (i > st).then_some(i)
}

fn signed_digits(b: &[u8], i: usize) -> Option<usize> {
    digits(b, if b.get(i) == Some(&b'-') { i + 1 } else { i })
}

/// `/^-?\d+\.\.-?\d+(?:\.\.-?\d+)?$/`
fn is_numeric_seq(body: &str) -> bool {
    let b = body.as_bytes();
    let check = || -> Option<bool> {
        let i = signed_digits(b, 0)?;
        if !body[i..].starts_with("..") {
            return None;
        }
        let i = signed_digits(b, i + 2)?;
        if i == b.len() {
            return Some(true);
        }
        if !body[i..].starts_with("..") {
            return None;
        }
        Some(signed_digits(b, i + 2)? == b.len())
    };
    check().unwrap_or(false)
}

/// `/^[a-zA-Z]\.\.[a-zA-Z](?:\.\.-?\d+)?$/`
fn is_alpha_seq(body: &str) -> bool {
    let b = body.as_bytes();
    if b.len() < 4 || !b[0].is_ascii_alphabetic() || &b[1..3] != b".." || !b[3].is_ascii_alphabetic() {
        return false;
    }
    if b.len() == 4 {
        return true;
    }
    if b.len() < 7 || &b[4..6] != b".." {
        return false;
    }
    signed_digits(b, 6) == Some(b.len())
}

/// `m.post.match(/,(?!,).*\}/)`
fn rewrite_quirk(post: &str) -> bool {
    let b = post.as_bytes();
    let mut i = 0;
    while let Some(p) = b[i..].iter().position(|x| *x == b',') {
        let at = i + p;
        if b.get(at + 1) != Some(&b',') {
            for c in post[at + 1..].chars() {
                if c == '}' {
                    return true;
                }
                if is_line_terminator(c) {
                    break;
                }
            }
        }
        i = at + 1;
    }
    false
}

fn expand_(s: String, depth: usize, mut is_top: bool) -> Vec<String> {
    if depth > EXPANSION_MAX_DEPTH {
        return vec![s];
    }
    let mut s = s;
    let mut acc = vec![String::new()];
    let mut rewrites = 0;
    let mut drop_empties = false;
    let mut first_group = true;
    let empty = [String::new()];
    loop {
        let Some(m) = balanced(&s) else {
            return combine(&acc, &s, &empty, drop_empties);
        };
        if m.pre.ends_with('$') {
            let pre = format!("{}{{{}}}", m.pre, m.body);
            acc = combine(&acc, &pre, &empty, drop_empties && m.post.is_empty());
            first_group = false;
            if m.post.is_empty() {
                break;
            }
            s = m.post;
            continue;
        }
        let numeric_seq = is_numeric_seq(&m.body);
        let alpha_seq = is_alpha_seq(&m.body);
        let is_seq = numeric_seq || alpha_seq;
        let is_options = m.body.contains(',');
        if !is_seq && !is_options {
            if rewrites < EXPANSION_MAX_REWRITES && rewrite_quirk(&m.post) {
                rewrites += 1;
                s = format!("{}{{{}{ESC_CLOSE}{}", m.pre, m.body, m.post);
                is_top = true;
                continue;
            }
            let lit = format!("{}{{{}}}{}", m.pre, m.body, m.post);
            return combine(&acc, &lit, &empty, drop_empties);
        }
        if first_group {
            drop_empties = is_top && !is_seq;
            first_group = false;
        }
        let values = if is_seq {
            expand_sequence(&m.body, alpha_seq)
        } else {
            let mut n = parse_comma_parts(&m.body);
            if n.len() == 1 {
                n = expand_(n[0].clone(), depth + 1, false)
                    .into_iter()
                    .map(|e| format!("{{{e}}}"))
                    .collect();
                if n.len() == 1 {
                    let pre = format!("{}{}", m.pre, n[0]);
                    acc = combine(&acc, &pre, &empty, drop_empties && m.post.is_empty());
                    if m.post.is_empty() {
                        break;
                    }
                    s = m.post;
                    continue;
                }
            }
            let drops = drop_empties
                && m.post.is_empty()
                && m.pre.is_empty()
                && acc.iter().all(String::is_empty);
            let mut vals = Vec::new();
            let mut vals_len = 0;
            'outer: for part in n {
                for v in expand_(part, depth + 1, false) {
                    if drops && v.is_empty() {
                        continue;
                    }
                    if vals.len() >= EXPANSION_MAX || vals_len + v.len() > EXPANSION_MAX_LENGTH {
                        break 'outer;
                    }
                    vals_len += v.len();
                    vals.push(v);
                }
            }
            vals
        };
        acc = combine(&acc, &m.pre, &values, drop_empties && m.post.is_empty());
        if m.post.is_empty() {
            break;
        }
        s = m.post;
    }
    acc
}

// ---------------------------------------------------------------------------
// Numeric ranges
// ---------------------------------------------------------------------------

/// `RANGE_MIN`: a numeric range with at least this many values is matched as one
/// pattern portion, rather than expanded.
const RANGE_MIN: f64 = 100.0;

/// `pattern.replace(rangeRe, ...)` in `#expandBraces`, where `rangeRe` is
/// `/(?<![\\$])\{(-?\d+)\.\.(-?\d+)\}/g`: each range of at least [`RANGE_MIN`]
/// values becomes `<mark>lo..hi<mark>`. Also returns whether there was one.
fn mark_ranges(pattern: &str, mark: char) -> (String, bool) {
    let b = pattern.as_bytes();
    let mut out = String::with_capacity(pattern.len());
    let mut found = false;
    let (mut copied, mut i) = (0, 0);
    while i < b.len() {
        if b[i] != b'{' || (i > 0 && matches!(b[i - 1], b'\\' | b'$')) {
            i += 1;
            continue;
        }
        let m = (|| {
            let lo_end = signed_digits(b, i + 1)?;
            if !pattern[lo_end..].starts_with("..") {
                return None;
            }
            let hi_end = signed_digits(b, lo_end + 2)?;
            (b.get(hi_end) == Some(&b'}')).then_some((lo_end, hi_end))
        })();
        let Some((lo_end, hi_end)) = m else {
            i += 1;
            continue;
        };
        let (lo, hi) = (&pattern[i + 1..lo_end], &pattern[lo_end + 2..hi_end]);
        let safe = |s: &str| s.parse::<f64>().ok().filter(|n| n.abs() <= 9_007_199_254_740_991.0);
        if let (Some(a), Some(z)) = (safe(lo), safe(hi)) {
            if (a - z).abs() + 1.0 >= RANGE_MIN {
                found = true;
                out.push_str(&pattern[copied..i]);
                out.push(mark);
                out.push_str(lo);
                out.push_str("..");
                out.push_str(hi);
                out.push(mark);
                copied = hi_end + 1;
            }
        }
        i = hi_end + 1;
    }
    out.push_str(&pattern[copied..]);
    (out, found)
}

/// `inRepeatOrNegExt(portion, mark)`: true if a range mark in this portion is
/// inside a `!(...)`, `*(...)` or `+(...)` extglob. Those repeat or negate their
/// contents, so one matcher for the whole range does not match the same strings as
/// the union of one pattern per value. `@(...)` and `?(...)` pick one value at most.
fn in_repeat_or_neg_ext(portion: &str, mark: char) -> bool {
    let c: Vec<char> = portion.chars().collect();
    let mut stack: Vec<char> = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch == '\\' {
            i += 1;
        } else if ch == mark {
            if stack.iter().any(|t| *t != '@' && *t != '?') {
                return true;
            }
        } else if c.get(i + 1) == Some(&'(') && matches!(ch, '!' | '?' | '+' | '*' | '@') {
            stack.push(ch);
            i += 1;
        } else if ch == ')' {
            stack.pop();
        }
        i += 1;
    }
    false
}

/// `#expandBraces()`: brace expansion, except numeric ranges such as `{0..999}`
/// with at least [`RANGE_MIN`] values. Each of those stays in its pattern between
/// two marks, a private-use char that the pattern does not contain, for the matcher
/// to test. If a range might be inside a `[...]` class, or a repeating or negated
/// extglob, all of them are expanded instead. Returns the mark, if one is used.
fn expand_braces(pattern: &str) -> (Vec<String>, Option<char>) {
    let mut code = 0xE000;
    let mark = loop {
        let m = char::from_u32(code).expect("private use");
        if !pattern.contains(m) {
            break m;
        }
        code += 1;
        if code > 0xF8FF {
            return (brace_expand(pattern), None);
        }
    };
    let (marked, found) = mark_ranges(pattern, mark);
    if !found {
        return (brace_expand(pattern), None);
    }
    let set = brace_expand(&marked);
    for p in &set {
        for portion in p.split('/') {
            if let (Some(c), Some(m)) = (portion.find('['), portion.rfind(mark)) {
                if c < m {
                    return (brace_expand(pattern), None);
                }
            }
            if in_repeat_or_neg_ext(portion, mark) {
                return (brace_expand(pattern), None);
            }
        }
    }
    (set, Some(mark))
}

// ---------------------------------------------------------------------------
// Portion parsing
// ---------------------------------------------------------------------------

/// `classEnd(pattern, start)`: the index of the `]` that closes the class at
/// `start`, if it is closed. POSIX classes are not skipped, so this can find a `]`
/// before the real end, which only makes a class look less like a literal.
fn class_end(c: &[char], start: usize) -> Option<usize> {
    let mut i = start + 1;
    if matches!(c.get(i), Some('!' | '^')) {
        i += 1;
    }
    let mut saw_start = false;
    while i < c.len() {
        if c[i] == ']' && saw_start {
            return Some(i);
        }
        saw_start = true;
        if c[i] == '\\' {
            i += 1;
        }
        i += 1;
    }
    None
}

/// `literal(pattern)`: the string a portion matches, if it has no magic. This can
/// report magic for a portion that is in fact literal (`[]`), but never the reverse.
fn literal(pattern: &str) -> Option<String> {
    if pattern == "+()" || pattern == "@()" {
        return Some(pattern.to_string());
    }
    let c: Vec<char> = pattern.chars().collect();
    let mut lit = String::with_capacity(pattern.len());
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch == '\\' {
            // a trailing \ is a literal \
            i += 1;
            lit.push(c.get(i).copied().unwrap_or(ch));
        } else if ch == '*' || ch == '?' {
            return None;
        } else if ch == '[' {
            let Some(close) = class_end(&c, i) else {
                // not closed, so the [ is a literal
                lit.push(ch);
                i += 1;
                continue;
            };
            // a class of one char, such as `[*]` or `[\b]`, is a literal
            let mut body = &c[i + 1..close];
            if body.first() == Some(&'\\') {
                body = &body[1..];
            }
            if body.len() != 1 || matches!(c[i + 1], '!' | '^') {
                return None;
            }
            lit.push(body[0]);
            i = close;
        } else if matches!(ch, '+' | '@' | '!') && c.get(i + 1) == Some(&'(') && c[i + 2..].contains(&')') {
            return None;
        } else {
            lit.push(ch);
        }
        i += 1;
    }
    Some(lit)
}

/// `extglobCount(pattern)`: the extglob openers, such as `*(`, in a portion. Chars
/// in a class count too, so this can be more than the extglobs, but never less.
fn extglob_count(pattern: &str) -> usize {
    let c: Vec<char> = pattern.chars().collect();
    let mut n = 0;
    let mut i = 0;
    while i + 1 < c.len() {
        if c[i] == '\\' {
            i += 1;
        } else if matches!(c[i], '+' | '@' | '!' | '?' | '*') && c[i + 1] == '(' {
            n += 1;
            i += 1;
        }
        i += 1;
    }
    n
}

/// `parse(pattern)` for one portion: `**`, a literal, or a [`Matcher`].
fn parse(p: &str, dot: bool, range_mark: Option<char>) -> Result<Seg, String> {
    if p == "**" {
        return Ok(Seg::Globstar);
    }
    if p.is_empty() {
        return Ok(Seg::Lit(String::new()));
    }
    let has_mark = range_mark.is_some_and(|m| p.contains(m));
    // Most portions have no syntax at all, and so are their own literal.
    if !has_mark && !p.bytes().any(|c| matches!(c, b'*' | b'?' | b'[' | b'\\' | b'(')) {
        return Ok(Seg::Lit(p.to_string()));
    }
    let lit = if has_mark { None } else { literal(p) };
    if let Some(l) = lit {
        return Ok(Seg::Lit(l));
    }
    // `assertValidExtglobs`: the matcher compiles and walks extglobs recursively, so
    // upstream rejects portions with very many of them.
    if extglob_count(p) > MAX_EXTGLOBS {
        return Err("pattern has too many extglobs".into());
    }
    Ok(Seg::Magic(Matcher::new(p, dot, range_mark)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_star_and_globstar() {
        assert!(minimatch("a/b.js", "a/*.js"));
        assert!(!minimatch("a/.b.js", "a/*.js"));
        assert!(minimatch("x/y/z.js", "**/*.js"));
        assert!(!minimatch("x/.y/z.js", "**/*.js"));
        assert!(minimatch("/root/a", "/root/**"));
        assert!(!minimatch("a", "#a"));
        assert!(minimatch("b", "!a"));
        assert!(minimatch("a/b/", "a/*"));
        assert!(!minimatch("a/", "a/*"));
    }

    #[test]
    fn braces_and_extglobs() {
        assert_eq!(brace_expand("a{b,c}d"), ["abd", "acd"]);
        assert_eq!(brace_expand("x{1..3}"), ["x1", "x2", "x3"]);
        assert!(minimatch("foo.bar", "foo.@(bar|baz)"));
        assert!(!minimatch("foo.qux", "foo.@(bar|baz)"));
        assert!(minimatch("foo.qux", "foo.!(bar|baz)"));
        assert!(minimatch("ab", "!(a)b"));
        assert!(minimatch("b", "!(a)b"));
        assert!(minimatch("aab", "+(a)b"));
    }

    #[test]
    fn classes() {
        assert!(minimatch("a1", "a[0-9]"));
        assert!(!minimatch("ab", "a[0-9]"));
        assert!(minimatch("a*", "a[*]"));
    }

    // 10 threw on these (an invalid `u`-mode regex); 11's matcher has no regex to
    // reject. `[:punct:]` now has the symbols, and `[:print:]` is "not \p{C}".
    #[test]
    fn posix_classes() {
        assert!(Minimatch::new("[[:alpha:]]").error().is_none());
        assert!(minimatch("é", "[[:alpha:]]"));
        assert!(minimatch("$", "[[:punct:]]"));
        assert!(minimatch("a", "[[:print:]]"));
        assert!(!minimatch("a", "[[:punct:]]"));
    }

    // A magic portion never matches an empty path portion, and matches code points.
    #[test]
    fn matcher_portions() {
        assert!(!minimatch("", "*(a)"));
        assert!(!minimatch("", "?(a)"));
        assert!(minimatch("😀", "?"));
        assert!(!minimatch("😀", "??"));
        // a portion that is only an empty extglob: `+()` and `@()` are literals,
        // `?()` and `*()` keep their wildcard
        assert!(minimatch("+()", "+()"));
        assert!(!minimatch("a", "+()"));
        assert!(minimatch("x()", "?()"));
        assert!(minimatch("x()", "*()"));
        // a class can not match a leading dot, and `.` and `..` only match literal dots
        assert!(!minimatch(".a", "[.]*"));
        assert!(minimatch(".a", "[.]a"));
        assert!(!minimatch("..", ".!(a)"));
        assert!(minimatch(".a", "!(a)"));
    }

    #[test]
    fn too_many_extglobs() {
        assert_eq!(Minimatch::new(&"?(a)".repeat(257)).error(), Some("pattern has too many extglobs"));
        assert!(Minimatch::new(&"@(a)".repeat(256)).error().is_none());
        // a literal portion is never counted
        assert!(Minimatch::new(&"\\?(a)".repeat(257)).error().is_none());
    }

    // An escaped backslash before a brace set stays escaped.
    #[test]
    fn escaped_backslash_braces() {
        assert!(minimatch("\\a", "\\\\{a,b}"));
        assert!(!minimatch("a", "\\\\{a,b}"));
        assert!(!minimatch("a\\x", "{a,b}\\\\\\*"));
    }

    // Ranges of 100 values or more are matched, not expanded, so they are not cut
    // off at brace-expansion's 100,000 values.
    #[test]
    fn numeric_ranges() {
        assert!(minimatch("150000", "{1..200000}"));
        assert!(minimatch("x/1000", "x/{1..1000}"));
        assert!(!minimatch("x/1001", "x/{1..1000}"));
        assert!(!minimatch("x/0", "x/{1..1000}"));
        assert!(minimatch("001", "{001..150}"));
        assert!(!minimatch("1", "{001..150}"));
        assert!(minimatch("-05", "{-05..120}"));
        assert!(minimatch("007", "{-05..120}"));
        assert!(!minimatch("7", "{-05..120}"));
        assert!(minimatch("a120b", "a{150..1}b"));
        // in a repeating extglob, each value is its own pattern
        assert!(!minimatch("510", "*({1..150})"));
    }

    // The ** before a body section can walk up to where the section and the ones
    // after it still fit, and the dot check stops before the tail.
    #[test]
    fn globstar_body_sections() {
        assert!(minimatch("a/b/c/d", "**/a/b/**/c/**/d"));
        assert!(minimatch("x/q/a/b/c/d/e", "x/**/a/b/c/**/d/**/e"));
        assert!(minimatch("a/.x", "**/a/**/.x"));
        assert!(minimatch("x/q/.y/z", "**/x/**/.y/z"));
        assert!(!minimatch("a/.q/b/c", "a/**/b/**/c"));
    }

    #[test]
    fn dot_option() {
        let dot = |p: &str, g: &str| Minimatch::with_dot(g, true).matches(p);
        assert!(dot(".a", "*"));
        assert!(!dot("..", "*"));
        assert!(dot(".a/x", "**/x"));
        assert!(!dot("./x", "**/x"));
        assert!(dot(".b", "@(*|.x)"));
        assert!(!minimatch(".b", "@(*|.x)"));
        assert!(dot("a/.b/c", "a/**/c"));
        assert!(!dot(".a", "*(a)"));
        assert!(dot(".aa", ".*(a)"));
    }
}
