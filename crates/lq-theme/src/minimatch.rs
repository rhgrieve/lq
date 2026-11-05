//! A port of `minimatch` 10.2.6 with default options on a POSIX platform.
//!
//! Compilation follows the upstream steps: comment and negation handling, brace
//! expansion (`brace-expansion` 5.0.12, `balanced-match` 4.0.4), slash splitting,
//! level-one optimisation, then one matcher per path segment. Literal segments
//! and upstream's own "fast tests" (`*`, `*.ext`, `?`, `*.*`, `.*`) never touch a
//! regex; everything else goes through the extglob AST port and [`JsRegex`].
//!
//! Known limit: `JsRegex` always matches on code points, while upstream's regexes
//! without the `u` flag match UTF-16 code units, so a magic segment that is not a
//! fast test sees an astral character as one `?` here and two upstream.

use lq_json::js::{is_js_whitespace, number_to_string, utf16_len};
use lq_json::regex::JsRegex;

#[derive(Debug)]
enum Fast {
    /// `/^\*+$/`
    Star,
    /// `/^\*+([^+@!?*[(]*)$/`
    StarExt(String),
    /// `/^\?+([^+@!?*[(]*)?$/`, length of the whole pattern.
    Qmarks(usize, String),
    /// `/^\*+\.\*+$/`
    StarDotStar,
    /// `/^\.\*+$/`
    DotStar,
}

#[derive(Debug)]
enum Seg {
    Lit(String),
    Globstar,
    Fast(Fast),
    Re(Box<JsRegex>),
}

impl Seg {
    fn test(&self, f: &str) -> bool {
        match self {
            Seg::Lit(s) => s == f,
            Seg::Globstar => false,
            Seg::Re(re) => re.is_match(f),
            Seg::Fast(fast) => match fast {
                Fast::Star => !f.is_empty() && !f.starts_with('.'),
                Fast::StarExt(ext) => !f.starts_with('.') && f.ends_with(ext.as_str()),
                Fast::Qmarks(len, ext) => {
                    utf16_len(f) == *len && !f.starts_with('.') && f.ends_with(ext.as_str())
                }
                Fast::StarDotStar => !f.starts_with('.') && f.contains('.'),
                Fast::DotStar => f != "." && f != ".." && f.starts_with('.'),
            },
        }
    }
}

/// A compiled pattern.
#[derive(Debug)]
pub struct Minimatch {
    comment: bool,
    empty: bool,
    negate: bool,
    set: Vec<Vec<Seg>>,
    error: Option<String>,
}

/// `minimatch(path, pattern)`. Compiles the pattern every call; keep a
/// [`Minimatch`] around for repeated use. A pattern upstream would throw on (see
/// [`Minimatch::error`]) matches nothing here.
pub fn minimatch(path: &str, pattern: &str) -> bool {
    Minimatch::new(pattern).matches(path)
}

impl Minimatch {
    pub fn new(pattern: &str) -> Minimatch {
        let mut mm = Minimatch { comment: false, empty: false, negate: false, set: Vec::new(), error: None };
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

        let mut seen: Vec<String> = Vec::new();
        for g in brace_expand(pattern) {
            if !seen.contains(&g) {
                seen.push(g);
            }
        }
        for g in &seen {
            let parts = level_one_optimize(slash_split(g));
            let mut segs = Vec::with_capacity(parts.len());
            for p in parts {
                match parse_segment(p) {
                    Ok(s) => segs.push(s),
                    Err(e) => {
                        mm.error = Some(e);
                        return mm;
                    }
                }
            }
            mm.set.push(segs);
        }
        mm
    }

    /// The message of the exception upstream throws while compiling this pattern,
    /// if any: an invalid `u`-mode regex from a POSIX class next to an escaped
    /// literal, or an over-long pattern. Such a pattern never matches here.
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
        for pattern in &self.set {
            let hit = if pattern.iter().any(|s| matches!(s, Seg::Globstar)) {
                match_globstar(file, pattern)
            } else {
                match_one(file, pattern, 0, 0)
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
                p.iter()
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

fn is_dot_part(f: &str) -> bool {
    f.starts_with('.')
}

fn match_one(file: &[&str], pattern: &[Seg], mut fi: usize, mut pi: usize) -> bool {
    let (fl, pl) = (file.len(), pattern.len());
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

fn match_globstar(file: &[&str], pattern: &[Seg]) -> bool {
    let is_gs = |s: &Seg| matches!(s, Seg::Globstar);
    let firstgs = pattern.iter().position(is_gs).expect("globstar");
    let lastgs = pattern.iter().rposition(is_gs).expect("globstar");
    let head = &pattern[..firstgs];
    let body = if firstgs < lastgs { &pattern[firstgs + 1..lastgs] } else { &pattern[0..0] };
    let tail = &pattern[lastgs + 1..];
    let mut file_index = 0;

    if !head.is_empty() {
        let end = head.len().min(file.len());
        if !match_one(&file[..end], head, 0, 0) {
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
        if match_one(file, tail, tail_start, 0) {
            tail_match = tail.len();
        } else {
            // a/**/* matching a/b/
            if !file[file.len() - 1].is_empty() || file_index + tail.len() == file.len() {
                return false;
            }
            if !match_one(file, tail, tail_start - 1, 0) {
                return false;
            }
            tail_match = tail.len() + 1;
        }
    }

    if body.is_empty() {
        let mut saw_some = tail_match != 0;
        let end = file.len() - tail_match;
        let mut i = file_index;
        while i < end {
            saw_some = true;
            if is_dot_part(file[i]) {
                return false;
            }
            i += 1;
        }
        return saw_some;
    }

    let mut sections: Vec<&[Seg]> = Vec::new();
    let mut sums = vec![0usize];
    let mut non_gs = 0;
    let mut start = 0;
    for (i, b) in body.iter().enumerate() {
        if is_gs(b) {
            sums.push(non_gs);
            sections.push(&body[start..i]);
            start = i + 1;
        } else {
            non_gs += 1;
        }
    }
    sections.push(&body[start..]);
    let file_length = file.len() as isize - tail_match as isize;
    let mut idx = sections.len();
    let segs: Vec<(&[Seg], isize)> = sections
        .iter()
        .map(|s| {
            idx -= 1;
            (*s, file_length - (sums[idx] + s.len()) as isize)
        })
        .collect();
    match_body_sections(file, &segs, file_index, 0, 0, tail_match != 0) == Some(true)
}

/// `Some(false)` is "not matching", `None` "not matching, stop trying".
fn match_body_sections(
    file: &[&str],
    segs: &[(&[Seg], isize)],
    mut file_index: usize,
    body_index: usize,
    depth: usize,
    mut saw_tail: bool,
) -> Option<bool> {
    let Some((body, after)) = segs.get(body_index) else {
        let mut i = file_index;
        while i < file.len() {
            saw_tail = true;
            if is_dot_part(file[i]) {
                return Some(false);
            }
            i += 1;
        }
        return Some(saw_tail);
    };
    while file_index as isize <= *after {
        let end = (file_index + body.len()).min(file.len());
        let m = match_one(&file[..end], body, file_index, 0);
        if m && depth < 200 {
            let sub = match_body_sections(file, segs, file_index + body.len(), body_index + 1, depth + 1, saw_tail);
            if sub != Some(false) {
                return sub;
            }
        }
        if file_index < file.len() && is_dot_part(file[file_index]) {
            return Some(false);
        }
        file_index += 1;
    }
    None
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
    expand(pattern)
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
// Segment parsing
// ---------------------------------------------------------------------------

const EXT_EXCLUDED: &[char] = &['+', '@', '!', '?', '*', '[', '('];

fn parse_segment(p: &str) -> Result<Seg, String> {
    if p == "**" {
        return Ok(Seg::Globstar);
    }
    if p.is_empty() {
        return Ok(Seg::Lit(String::new()));
    }
    let b = p.as_bytes();
    // A pure literal: nothing the AST or the class parser would treat specially,
    // and the regex-escape/unescape round trip is the identity.
    if !b.iter().any(|c| matches!(c, b'*' | b'?' | b'[' | b']' | b'\\' | b'(' | b')')) {
        return Ok(Seg::Lit(p.to_string()));
    }
    // Upstream's fast tests, checked in its order. Each of these shapes always has
    // magic, so the AST (built upstream but overridden) has no observable effect.
    let stars = b.iter().take_while(|c| **c == b'*').count();
    if stars > 0 {
        if stars == b.len() {
            return Ok(Seg::Fast(Fast::Star));
        }
        let rest = &p[stars..];
        if !rest.contains(EXT_EXCLUDED) {
            return Ok(Seg::Fast(Fast::StarExt(rest.to_string())));
        }
    }
    let qmarks = b.iter().take_while(|c| **c == b'?').count();
    if qmarks > 0 {
        let rest = &p[qmarks..];
        if !rest.contains(EXT_EXCLUDED) {
            return Ok(Seg::Fast(Fast::Qmarks(utf16_len(p), rest.to_string())));
        }
    }
    if stars > 0 {
        let rest = &b[stars..];
        if rest[0] == b'.' && rest.len() > 1 && rest[1..].iter().all(|c| *c == b'*') {
            return Ok(Seg::Fast(Fast::StarDotStar));
        }
    }
    if b[0] == b'.' && b.len() > 1 && b[1..].iter().all(|c| *c == b'*') {
        return Ok(Seg::Fast(Fast::DotStar));
    }

    let mut ast = Arena::from_glob(p);
    match ast.mm_pattern() {
        MmPattern::Lit(s) => Ok(Seg::Lit(s)),
        MmPattern::Re(src, uflag) => {
            let full = format!("^{src}$");
            if uflag {
                if let Some(msg) = u_mode_error(&full) {
                    return Err(format!("Invalid regular expression: /{full}/u: {msg}"));
                }
            }
            JsRegex::new(&full, if uflag { "u" } else { "" })
                .map(|re| Seg::Re(Box::new(re)))
                .map_err(|e| format!("Invalid regular expression: /{full}/: {e}"))
        }
    }
}

/// What `new RegExp(src, 'u')` rejects among the sources minimatch generates:
/// identity escapes of non-syntax characters.
fn u_mode_error(src: &str) -> Option<&'static str> {
    let mut in_class = false;
    let mut it = src.chars();
    while let Some(c) = it.next() {
        match c {
            '\\' => {
                let e = it.next()?;
                let ok = matches!(
                    e,
                    '^' | '$' | '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '/'
                ) || e.is_ascii_alphanumeric()
                    || (in_class && e == '-');
                if !ok {
                    return Some("Invalid escape");
                }
            }
            '[' if !in_class => in_class = true,
            ']' if in_class => in_class = false,
            _ => {}
        }
    }
    None
}

fn regexp_escape_into(c: char, out: &mut String) {
    if matches!(
        c,
        '-' | '[' | ']' | '{' | '}' | '(' | ')' | '*' | '+' | '?' | '.' | ',' | '\\' | '^' | '$' | '|' | '#'
    ) || is_js_whitespace(c)
    {
        out.push('\\');
    }
    out.push(c);
}

/// `unescape(s)` with default options.
fn unescape(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let n = c.len();
    let mut first = Vec::with_capacity(n);
    let mut i = 0;
    let inner = |x: char| x != '/' && x != '\\';
    while i < n {
        if c[i] != '\\'
            && !is_line_terminator(c[i])
            && i + 3 < n
            && c[i + 1] == '['
            && inner(c[i + 2])
            && c[i + 3] == ']'
        {
            first.push(c[i]);
            first.push(c[i + 2]);
            i += 4;
            continue;
        }
        if i == 0 && n >= 3 && c[0] == '[' && inner(c[1]) && c[2] == ']' {
            first.push(c[1]);
            i = 3;
            continue;
        }
        first.push(c[i]);
        i += 1;
    }
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < first.len() {
        if first[i] == '\\' && i + 1 < first.len() && first[i + 1] != '/' {
            out.push(first[i + 1]);
            i += 2;
        } else {
            out.push(first[i]);
            i += 1;
        }
    }
    out
}

const POSIX_CLASSES: &[(&str, &str, bool, bool)] = &[
    ("[:alnum:]", "\\p{L}\\p{Nl}\\p{Nd}", true, false),
    ("[:alpha:]", "\\p{L}\\p{Nl}", true, false),
    ("[:ascii:]", "\\x00-\\x7f", false, false),
    ("[:blank:]", "\\p{Zs}\\t", true, false),
    ("[:cntrl:]", "\\p{Cc}", true, false),
    ("[:digit:]", "\\p{Nd}", true, false),
    ("[:graph:]", "\\p{Z}\\p{C}", true, true),
    ("[:lower:]", "\\p{Ll}", true, false),
    ("[:print:]", "\\p{C}", true, false),
    ("[:punct:]", "\\p{P}", true, false),
    ("[:space:]", "\\p{Z}\\t\\r\\n\\v\\f", true, false),
    ("[:upper:]", "\\p{Lu}", true, false),
    ("[:word:]", "\\p{L}\\p{Nl}\\p{Nd}\\p{Pc}", true, false),
    ("[:xdigit:]", "A-Fa-f0-9", false, false),
];

fn brace_escape(s: &[char]) -> String {
    let mut out = String::new();
    for &c in s {
        if matches!(c, '[' | ']' | '\\' | '-') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn starts_with_at(g: &[char], s: &str, i: usize) -> bool {
    s.chars().enumerate().all(|(k, c)| g.get(i + k) == Some(&c))
}

/// `parseClass(glob, pos)`: `(src, uflag, consumed, magic)`.
fn parse_class(glob: &[char], pos: usize) -> (String, bool, usize, bool) {
    let mut ranges: Vec<String> = Vec::new();
    let mut negs: Vec<String> = Vec::new();
    let mut i = pos + 1;
    let (mut saw_start, mut uflag, mut escaping, mut negate) = (false, false, false, false);
    let mut end_pos = pos;
    let mut range_start: Option<char> = None;
    'WHILE: while i < glob.len() {
        let c = glob[i];
        if (c == '!' || c == '^') && i == pos + 1 {
            negate = true;
            i += 1;
            continue;
        }
        if c == ']' && saw_start && !escaping {
            end_pos = i + 1;
            break;
        }
        saw_start = true;
        if c == '\\' && !escaping {
            escaping = true;
            i += 1;
            continue;
        }
        if c == '[' && !escaping {
            for (cls, unip, u, neg) in POSIX_CLASSES {
                if starts_with_at(glob, cls, i) {
                    if range_start.is_some() {
                        return ("$.".into(), false, glob.len() - pos, true);
                    }
                    i += cls.len();
                    if *neg {
                        negs.push((*unip).into());
                    } else {
                        ranges.push((*unip).into());
                    }
                    uflag = uflag || *u;
                    continue 'WHILE;
                }
            }
        }
        escaping = false;
        if let Some(rs) = range_start {
            if c > rs {
                ranges.push(format!("{}-{}", brace_escape(&[rs]), brace_escape(&[c])));
            } else if c == rs {
                ranges.push(brace_escape(&[c]));
            }
            range_start = None;
            i += 1;
            continue;
        }
        if starts_with_at(glob, "-]", i + 1) {
            ranges.push(brace_escape(&[c, '-']));
            i += 2;
            continue;
        }
        if glob.get(i + 1) == Some(&'-') {
            range_start = Some(c);
            i += 2;
            continue;
        }
        ranges.push(brace_escape(&[c]));
        i += 1;
    }
    if end_pos < i {
        return (String::new(), false, 0, false);
    }
    if ranges.is_empty() && negs.is_empty() {
        return ("$.".into(), false, glob.len() - pos, true);
    }
    if negs.is_empty() && ranges.len() == 1 && !negate {
        let r: Vec<char> = ranges[0].chars().collect();
        let single = match r.as_slice() {
            [a] if !is_line_terminator(*a) => Some(*a),
            ['\\', a] if !is_line_terminator(*a) => Some(*a),
            _ => None,
        };
        if let Some(a) = single {
            let mut src = String::new();
            regexp_escape_into(a, &mut src);
            return (src, false, end_pos - pos, false);
        }
    }
    let sranges = format!("[{}{}]", if negate { "^" } else { "" }, ranges.concat());
    let snegs = format!("[{}{}]", if negate { "" } else { "^" }, negs.concat());
    let comb = if !ranges.is_empty() && !negs.is_empty() {
        format!("({sranges}|{snegs})")
    } else if !ranges.is_empty() {
        sranges
    } else {
        snegs
    };
    (comb, uflag, end_pos - pos, true)
}

/// `AST.#parseGlob(glob, hasMagic, noEmpty)`: `(re, hasMagic, uflag)`.
fn parse_glob(glob: &str, has_magic: bool, no_empty: bool) -> (String, bool, bool) {
    let g: Vec<char> = glob.chars().collect();
    let all_stars = g.iter().all(|c| *c == '*');
    let (mut escaping, mut uflag, mut in_star, mut has_magic) = (false, false, false, has_magic);
    let mut re = String::new();
    let mut i = 0;
    while i < g.len() {
        let c = g[i];
        if escaping {
            escaping = false;
            if matches!(
                c,
                '(' | ')' | '.' | '*' | '{' | '}' | '+' | '?' | '[' | ']' | '^' | '$' | '\\' | '!'
            ) {
                re.push('\\');
            }
            re.push(c);
            i += 1;
            continue;
        }
        if c == '*' {
            if !in_star {
                in_star = true;
                re.push_str(if no_empty && all_stars { "[^/]+?" } else { "[^/]*?" });
                has_magic = true;
            }
            i += 1;
            continue;
        }
        in_star = false;
        if c == '\\' {
            if i == g.len() - 1 {
                re.push_str("\\\\");
            } else {
                escaping = true;
            }
            i += 1;
            continue;
        }
        if c == '[' {
            let (src, need_u, consumed, magic) = parse_class(&g, i);
            if consumed > 0 {
                re.push_str(&src);
                uflag = uflag || need_u;
                i += consumed;
                has_magic = has_magic || magic;
                continue;
            }
        }
        if c == '?' {
            re.push_str("[^/]");
            has_magic = true;
            i += 1;
            continue;
        }
        regexp_escape_into(c, &mut re);
        i += 1;
    }
    (re, has_magic, uflag)
}

// ---------------------------------------------------------------------------
// The extglob AST, on an arena since upstream's nodes point at their parents.
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum Part {
    Str(String),
    Node(usize),
}

struct Ast {
    ty: Option<char>,
    parts: Vec<Part>,
    parent: Option<usize>,
    parent_index: usize,
    has_magic: Option<bool>,
    uflag: bool,
    empty_ext: bool,
    to_string: Option<String>,
}

struct Arena {
    nodes: Vec<Ast>,
    negs: Vec<usize>,
    filled_negs: bool,
}

enum MmPattern {
    Lit(String),
    Re(String, bool),
}

fn can_adopt_any(parent: char, c: char) -> bool {
    match parent {
        '!' => matches!(c, '?' | '@'),
        '?' => matches!(c, '?' | '@'),
        '@' => matches!(c, '?' | '@'),
        '*' => matches!(c, '*' | '+' | '?' | '@'),
        '+' => matches!(c, '+' | '@' | '?' | '*'),
        _ => false,
    }
}

fn can_adopt_plain(parent: char, c: char) -> bool {
    match parent {
        '!' => c == '@',
        '?' => matches!(c, '?' | '@'),
        '@' => c == '@',
        '*' => matches!(c, '*' | '+' | '?' | '@'),
        '+' => matches!(c, '+' | '@'),
        _ => false,
    }
}

fn can_adopt_space(parent: char, c: char) -> bool {
    match parent {
        '!' => c == '?',
        '@' => c == '?',
        '+' => matches!(c, '?' | '*'),
        _ => false,
    }
}

fn usurp_type(parent: char, c: char) -> Option<char> {
    match (parent, c) {
        ('!', '!') => Some('@'),
        ('?', '*') | ('?', '+') => Some('*'),
        ('@', '!') => Some('!'),
        ('@', '?') => Some('?'),
        ('@', '@') => Some('@'),
        ('@', '*') => Some('*'),
        ('@', '+') => Some('+'),
        ('+', '?') | ('+', '*') => Some('*'),
        _ => None,
    }
}

impl Arena {
    fn new_node(&mut self, ty: Option<char>, parent: Option<usize>) -> usize {
        let id = self.nodes.len();
        let parent_index = parent.map_or(0, |p| self.nodes[p].parts.len());
        self.nodes.push(Ast {
            ty,
            parts: Vec::new(),
            parent,
            parent_index,
            has_magic: ty.map(|_| true),
            uflag: false,
            empty_ext: false,
            to_string: None,
        });
        if ty == Some('!') && !self.filled_negs {
            self.negs.push(id);
        }
        id
    }

    fn from_glob(pattern: &str) -> Arena {
        let mut a = Arena { nodes: Vec::new(), negs: Vec::new(), filled_negs: false };
        let root = a.new_node(None, None);
        let chars: Vec<char> = pattern.chars().collect();
        a.parse_ast(&chars, root, 0, 0);
        a
    }

    fn push_str(&mut self, id: usize, s: String) {
        if !s.is_empty() {
            self.nodes[id].parts.push(Part::Str(s));
        }
    }

    fn parse_ast(&mut self, s: &[char], ast: usize, pos: usize, ext_depth: usize) -> usize {
        const MAX_DEPTH: usize = 2;
        let is_ext = |c: char| matches!(c, '!' | '?' | '+' | '*' | '@');
        let mut escaping = false;
        let mut in_brace = false;
        let mut brace_start = 0usize;
        let mut brace_neg = false;
        if self.nodes[ast].ty.is_none() {
            let mut i = pos;
            let mut acc = String::new();
            while i < s.len() {
                let c = s[i];
                i += 1;
                if escaping || c == '\\' {
                    escaping = !escaping;
                    acc.push(c);
                    continue;
                }
                if in_brace {
                    if i == brace_start + 1 {
                        if c == '^' || c == '!' {
                            brace_neg = true;
                        }
                    } else if c == ']' && !(i == brace_start + 2 && brace_neg) {
                        in_brace = false;
                    }
                    acc.push(c);
                    continue;
                } else if c == '[' {
                    in_brace = true;
                    brace_start = i;
                    brace_neg = false;
                    acc.push(c);
                    continue;
                }
                if is_ext(c) && s.get(i) == Some(&'(') && ext_depth <= MAX_DEPTH {
                    self.push_str(ast, std::mem::take(&mut acc));
                    let ext = self.new_node(Some(c), Some(ast));
                    i = self.parse_ast(s, ext, i, ext_depth + 1);
                    self.nodes[ast].parts.push(Part::Node(ext));
                    continue;
                }
                acc.push(c);
            }
            self.push_str(ast, acc);
            return i;
        }

        let mut i = pos + 1;
        let mut part = self.new_node(None, Some(ast));
        let mut parts: Vec<usize> = Vec::new();
        let mut acc = String::new();
        let ty = self.nodes[ast].ty.expect("extglob");
        while i < s.len() {
            let c = s[i];
            i += 1;
            if escaping || c == '\\' {
                escaping = !escaping;
                acc.push(c);
                continue;
            }
            if in_brace {
                if i == brace_start + 1 {
                    if c == '^' || c == '!' {
                        brace_neg = true;
                    }
                } else if c == ']' && !(i == brace_start + 2 && brace_neg) {
                    in_brace = false;
                }
                acc.push(c);
                continue;
            } else if c == '[' {
                in_brace = true;
                brace_start = i;
                brace_neg = false;
                acc.push(c);
                continue;
            }
            if is_ext(c) && s.get(i) == Some(&'(') && (ext_depth <= MAX_DEPTH || can_adopt_any(ty, c)) {
                let depth_add = if can_adopt_any(ty, c) { 0 } else { 1 };
                self.push_str(part, std::mem::take(&mut acc));
                let ext = self.new_node(Some(c), Some(part));
                self.nodes[part].parts.push(Part::Node(ext));
                i = self.parse_ast(s, ext, i, ext_depth + depth_add);
                continue;
            }
            if c == '|' {
                self.push_str(part, std::mem::take(&mut acc));
                parts.push(part);
                part = self.new_node(None, Some(ast));
                continue;
            }
            if c == ')' {
                if acc.is_empty() && self.nodes[ast].parts.is_empty() {
                    self.nodes[ast].empty_ext = true;
                }
                self.push_str(part, std::mem::take(&mut acc));
                parts.push(part);
                for p in parts {
                    self.nodes[ast].parts.push(Part::Node(p));
                }
                return i;
            }
            acc.push(c);
        }
        // An unfinished extglob: not an extglob at all.
        let n = &mut self.nodes[ast];
        n.ty = None;
        n.has_magic = None;
        n.parts = vec![Part::Str(s[pos - 1..].iter().collect())];
        i
    }

    fn ast_string(&mut self, id: usize) -> String {
        if let Some(s) = &self.nodes[id].to_string {
            return s.clone();
        }
        let parts = self.nodes[id].parts.clone();
        let strs: Vec<String> = parts
            .iter()
            .map(|p| match p {
                Part::Str(s) => s.clone(),
                Part::Node(n) => self.ast_string(*n),
            })
            .collect();
        let s = match self.nodes[id].ty {
            None => strs.concat(),
            Some(t) => format!("{t}({})", strs.join("|")),
        };
        self.nodes[id].to_string = Some(s.clone());
        s
    }

    fn copy_in(&mut self, this: usize, part: &Part) {
        match part {
            Part::Str(s) => self.push_str(this, s.clone()),
            Part::Node(n) => {
                let c = self.clone_node(*n, this);
                self.nodes[this].parts.push(Part::Node(c));
            }
        }
    }

    fn clone_node(&mut self, id: usize, parent: usize) -> usize {
        let ty = self.nodes[id].ty;
        let c = self.new_node(ty, Some(parent));
        for p in self.nodes[id].parts.clone() {
            self.copy_in(c, &p);
        }
        c
    }

    fn fill_negs(&mut self, root: usize) {
        if self.filled_negs {
            return;
        }
        self.ast_string(root);
        self.filled_negs = true;
        while let Some(n) = self.negs.pop() {
            if self.nodes[n].ty != Some('!') {
                continue;
            }
            let mut p = n;
            let mut pp = self.nodes[p].parent;
            while let Some(q) = pp {
                let mut i = self.nodes[p].parent_index + 1;
                while self.nodes[q].ty.is_none() && i < self.nodes[q].parts.len() {
                    for part in self.nodes[n].parts.clone() {
                        if let Part::Node(pn) = part {
                            let target = self.nodes[q].parts[i].clone();
                            self.copy_in(pn, &target);
                        }
                    }
                    i += 1;
                }
                p = q;
                pp = self.nodes[p].parent;
            }
        }
    }

    fn is_start(&self, id: usize) -> bool {
        let Some(parent) = self.nodes[id].parent else { return true };
        if !self.is_start(parent) {
            return false;
        }
        let pi = self.nodes[id].parent_index;
        if pi == 0 {
            return true;
        }
        for i in 0..pi {
            match self.nodes[parent].parts.get(i) {
                Some(Part::Node(n)) if self.nodes[*n].ty == Some('!') => {}
                _ => return false,
            }
        }
        true
    }

    fn is_end(&self, id: usize) -> bool {
        let Some(parent) = self.nodes[id].parent else { return true };
        if self.nodes[parent].ty == Some('!') {
            return true;
        }
        if !self.is_end(parent) {
            return false;
        }
        if self.nodes[id].ty.is_none() {
            return true;
        }
        self.nodes[id].parent_index + 1 == self.nodes[parent].parts.len()
    }

    fn only_child_ext(&self, child: &Part) -> Option<(usize, char)> {
        let Part::Node(c) = child else { return None };
        let cn = &self.nodes[*c];
        if cn.ty.is_some() || cn.parts.len() != 1 {
            return None;
        }
        let Part::Node(gc) = cn.parts[0] else { return None };
        self.nodes[gc].ty.map(|t| (gc, t))
    }

    fn adopt(&mut self, this: usize, gc: usize, index: usize) {
        let gc_parts = self.nodes[gc].parts.clone();
        for p in &gc_parts {
            if let Part::Node(n) = p {
                self.nodes[*n].parent = Some(this);
            }
        }
        self.nodes[this].parts.splice(index..=index, gc_parts);
        self.nodes[this].to_string = None;
    }

    fn flatten(&mut self, id: usize) {
        if self.nodes[id].ty.is_none() {
            for p in self.nodes[id].parts.clone() {
                if let Part::Node(n) = p {
                    self.flatten(n);
                }
            }
        } else {
            let mut iterations = 0;
            loop {
                let mut done = true;
                let mut i = 0;
                while i < self.nodes[id].parts.len() {
                    let c = self.nodes[id].parts[i].clone();
                    if let Part::Node(cn) = c {
                        self.flatten(cn);
                        let ty = self.nodes[id].ty.expect("extglob");
                        let only = self.only_child_ext(&Part::Node(cn));
                        if let Some((gc, gt)) = only {
                            if can_adopt_plain(ty, gt) {
                                done = false;
                                self.adopt(id, gc, i);
                            } else if can_adopt_space(ty, gt) {
                                done = false;
                                let blank = self.new_node(None, Some(gc));
                                self.nodes[blank].parts.push(Part::Str(String::new()));
                                self.nodes[gc].parts.push(Part::Node(blank));
                                self.adopt(id, gc, i);
                            } else if self.nodes[id].parts.len() == 1 {
                                if let Some(nt) = usurp_type(ty, gt) {
                                    done = false;
                                    let gc_parts = self.nodes[gc].parts.clone();
                                    for p in &gc_parts {
                                        if let Part::Node(n) = p {
                                            self.nodes[*n].parent = Some(id);
                                        }
                                    }
                                    let me = &mut self.nodes[id];
                                    me.parts = gc_parts;
                                    me.ty = Some(nt);
                                    me.to_string = None;
                                    me.empty_ext = false;
                                }
                            }
                        }
                    }
                    i += 1;
                }
                iterations += 1;
                if done || iterations >= 10 {
                    break;
                }
            }
        }
        self.nodes[id].to_string = None;
    }

    fn mm_pattern(&mut self) -> MmPattern {
        self.ast_string(0);
        let (re, body, has_magic, uflag) = self.regexp_source(0, None);
        if !(has_magic || self.nodes[0].has_magic == Some(true)) {
            return MmPattern::Lit(body);
        }
        MmPattern::Re(re, uflag)
    }

    fn parts_to_regexp(&mut self, id: usize, dot: bool) -> String {
        let keep_all = !(self.is_start(id) && self.is_end(id));
        let parts = self.nodes[id].parts.clone();
        let mut out: Vec<String> = Vec::new();
        for p in parts {
            let Part::Node(n) = p else { continue };
            let (re, _, _, u) = self.regexp_source(n, Some(dot));
            self.nodes[id].uflag = self.nodes[id].uflag || u;
            if keep_all || !re.is_empty() {
                out.push(re);
            }
        }
        out.join("|")
    }

    fn regexp_source(&mut self, id: usize, allow_dot: Option<bool>) -> (String, String, bool, bool) {
        let dot = allow_dot.unwrap_or(false);
        if id == 0 {
            self.flatten(0);
            self.fill_negs(0);
        }
        let Some(ty) = self.nodes[id].ty else {
            let parts = self.nodes[id].parts.clone();
            let no_empty = self.is_start(id)
                && self.is_end(id)
                && !parts.iter().any(|p| matches!(p, Part::Node(_)));
            let mut src = String::new();
            for p in &parts {
                let (re, magic, u) = match p {
                    Part::Str(s) => {
                        let hm = self.nodes[id].has_magic == Some(true);
                        parse_glob(s, hm, no_empty)
                    }
                    Part::Node(n) => {
                        let (re, _, m, u) = self.regexp_source(*n, allow_dot);
                        (re, m, u)
                    }
                };
                let me = &mut self.nodes[id];
                me.has_magic = Some(me.has_magic == Some(true) || magic);
                me.uflag = me.uflag || u;
                src.push_str(&re);
            }
            let mut start = "";
            if self.is_start(id) {
                if let Some(Part::Str(first)) = parts.first() {
                    let dot_trav_allowed = parts.len() == 1 && (first == "." || first == "..");
                    if !dot_trav_allowed {
                        let aps = |c: Option<u8>| matches!(c, Some(b'[') | Some(b'.'));
                        let b = src.as_bytes();
                        let need_no_trav = (dot && aps(b.first().copied()))
                            || (src.starts_with("\\.") && aps(b.get(2).copied()))
                            || (src.starts_with("\\.\\.") && aps(b.get(4).copied()));
                        let need_no_dot = !dot && allow_dot != Some(true) && aps(b.first().copied());
                        start = if need_no_trav {
                            "(?!(?:^|/)\\.\\.?(?:$|/))"
                        } else if need_no_dot {
                            "(?!\\.)"
                        } else {
                            ""
                        };
                    }
                }
            }
            let parent_neg = self.nodes[id].parent.is_some_and(|p| self.nodes[p].ty == Some('!'));
            let end = if self.is_end(id) && self.filled_negs && parent_neg { "(?:$|\\/)" } else { "" };
            let fin = format!("{start}{src}{end}");
            let me = &mut self.nodes[id];
            let magic = me.has_magic == Some(true);
            me.has_magic = Some(magic);
            return (fin, unescape(&src), magic, me.uflag);
        };

        let repeated = ty == '*' || ty == '+';
        let start = if ty == '!' { "(?:(?!(?:" } else { "(?:" };
        let mut body = self.parts_to_regexp(id, dot);
        if self.is_start(id) && self.is_end(id) && body.is_empty() && ty != '!' {
            let s = self.ast_string(id);
            let me = &mut self.nodes[id];
            me.parts = vec![Part::Str(s.clone())];
            me.ty = None;
            me.has_magic = None;
            let un = unescape(&self.ast_string(id));
            return (s, un, false, false);
        }
        let mut body_dot = if !repeated || allow_dot == Some(true) || dot {
            String::new()
        } else {
            self.parts_to_regexp(id, true)
        };
        if body_dot == body {
            body_dot.clear();
        }
        if !body_dot.is_empty() {
            body = format!("(?:{body})(?:{body_dot})*?");
        }
        let start_no_dot = self.is_start(id) && !dot;
        let fin = if ty == '!' && self.nodes[id].empty_ext {
            format!("{}[^/]+?", if start_no_dot { "(?!\\.)" } else { "" })
        } else {
            let close = if ty == '!' {
                let nd = if self.is_start(id) && !dot && allow_dot != Some(true) { "(?!\\.)" } else { "" };
                format!(")){nd}[^/]*?)")
            } else if ty == '@' {
                ")".to_string()
            } else if ty == '?' {
                ")?".to_string()
            } else if ty == '+' && !body_dot.is_empty() {
                ")".to_string()
            } else if ty == '*' && !body_dot.is_empty() {
                ")?".to_string()
            } else {
                format!("){ty}")
            };
            format!("{start}{body}{close}")
        };
        let me = &mut self.nodes[id];
        let magic = me.has_magic == Some(true);
        me.has_magic = Some(magic);
        let uflag = me.uflag;
        (fin, unescape(&body), magic, uflag)
    }
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
    }

    #[test]
    fn braces_and_extglobs() {
        assert_eq!(brace_expand("a{b,c}d"), ["abd", "acd"]);
        assert_eq!(brace_expand("x{1..3}"), ["x1", "x2", "x3"]);
        assert!(minimatch("foo.bar", "foo.@(bar|baz)"));
        assert!(!minimatch("foo.qux", "foo.@(bar|baz)"));
        assert!(minimatch("foo.qux", "foo.!(bar|baz)"));
    }

    #[test]
    fn classes() {
        assert!(minimatch("a1", "a[0-9]"));
        assert!(!minimatch("ab", "a[0-9]"));
        assert!(minimatch("a*", "a[*]"));
    }
}
