//! A port of `@isaacs/nonrecursive-glob-match` 1.1.0, the single-portion glob
//! matcher that minimatch 11 compiles each magic path portion into.
//!
//! The pattern becomes a state machine (literal chars, `?`, `*`, classes, splits
//! for extglob alternatives, and negation nodes for `!(...)`), simulated one code
//! point at a time without backtracking. Only the options minimatch passes here
//! are ported: `dot` and `rangeMark`. `nocase`, `noext` and `normalize` are never
//! set by lq. Upstream's DFA cache of simulation states is left out: it only
//! memoizes [`Sim::step`], and a cached state holds the same threads as a fresh
//! one.

use lq_json::regex::JsRegex;
use std::rc::Rc;
use std::sync::Arc;

/// One char of a `[...]` class.
#[derive(Debug)]
enum Class {
    /// `/$./`, from an empty or invalid class: matches nothing.
    Never,
    /// `/^[a-b]$/` for the digit classes of a numeric range.
    Digits(char, char),
    /// The `u`-mode RegExp that `parseClass` builds.
    Re(Box<JsRegex>),
}

impl Class {
    fn test(&self, c: char) -> bool {
        match self {
            Class::Never => false,
            Class::Digits(a, b) => (*a..=*b).contains(&c),
            Class::Re(re) => re.is_match(c.encode_utf8(&mut [0; 4])),
        }
    }
}

/// A parsed pattern item.
#[derive(Debug, Clone)]
enum Item {
    Lit(char),
    Any,
    Star,
    Class(Arc<Class>),
    /// An extglob: its type (`!?+*@`) and alternatives.
    Ext(char, Vec<Vec<Item>>),
}

/// Extglob syntax becomes literal unless it is part of a closed extglob.
#[derive(Debug)]
enum Tok {
    Item(Item),
    Open(char),
    Bar,
    Close,
}

const EXT_TYPES: &[char] = &['!', '?', '+', '*', '@'];

// ---------------------------------------------------------------------------
// parse-class.ts
// ---------------------------------------------------------------------------

/// `{ <posix class>: [<translation>, negated] }`
const POSIX_CLASSES: &[(&str, &str, bool)] = &[
    ("[:alnum:]", "\\p{L}\\p{Nl}\\p{Nd}", false),
    ("[:alpha:]", "\\p{L}\\p{Nl}", false),
    ("[:ascii:]", "\\x00-\\x7f", false),
    ("[:blank:]", "\\p{Zs}\\t", false),
    ("[:cntrl:]", "\\p{Cc}", false),
    ("[:digit:]", "\\p{Nd}", false),
    ("[:graph:]", "\\p{Z}\\p{C}", true),
    ("[:lower:]", "\\p{Ll}", false),
    ("[:print:]", "\\p{C}", true),
    ("[:punct:]", "\\p{P}\\p{S}", false),
    ("[:space:]", "\\p{Z}\\t\\r\\n\\v\\f", false),
    ("[:upper:]", "\\p{Lu}", false),
    ("[:word:]", "\\p{L}\\p{Nl}\\p{Nd}\\p{Pc}", false),
    ("[:xdigit:]", "A-Fa-f0-9", false),
];

/// `braceEscape`: only `[ \ ] -` need escaping inside a class.
fn brace_escape_into(c: char, out: &mut String) {
    if matches!(c, '[' | ']' | '\\' | '-') {
        out.push('\\');
    }
    out.push(c);
}

fn starts_with_at(g: &[char], s: &str, i: usize) -> bool {
    s.chars().enumerate().all(|(k, c)| g.get(i + k) == Some(&c))
}

/// `parseClass(glob, position)`: the class at `pos`, and the number of chars it
/// consumes. 0 means the class is not closed, so the `[` is a literal. A class
/// that can match nothing consumes the rest of the glob, so nothing matches.
fn parse_class(glob: &[char], pos: usize) -> (Class, usize) {
    let mut ranges = String::new();
    let mut negs = String::new();
    let (mut have_ranges, mut have_negs) = (false, false);
    let mut i = pos + 1;
    let (mut saw_start, mut escaping, mut negate) = (false, false, false);
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
            for (cls, unip, neg) in POSIX_CLASSES {
                if starts_with_at(glob, cls, i) {
                    // `[a-[]` is fine, but not `[a-[:alpha:]]`
                    if range_start.is_some() {
                        return (Class::Never, glob.len() - pos);
                    }
                    i += cls.len();
                    if *neg {
                        negs.push_str(unip);
                        have_negs = true;
                    } else {
                        ranges.push_str(unip);
                        have_ranges = true;
                    }
                    continue 'WHILE;
                }
            }
        }
        escaping = false;
        if let Some(rs) = range_start {
            // an out-of-order range is dropped, but the others can still match
            if c > rs {
                brace_escape_into(rs, &mut ranges);
                ranges.push('-');
                brace_escape_into(c, &mut ranges);
                have_ranges = true;
            } else if c == rs {
                brace_escape_into(c, &mut ranges);
                have_ranges = true;
            }
            range_start = None;
            i += 1;
            continue;
        }
        if starts_with_at(glob, "-]", i + 1) {
            brace_escape_into(c, &mut ranges);
            brace_escape_into('-', &mut ranges);
            have_ranges = true;
            i += 2;
            continue;
        }
        if glob.get(i + 1) == Some(&'-') {
            range_start = Some(c);
            i += 2;
            continue;
        }
        brace_escape_into(c, &mut ranges);
        have_ranges = true;
        i += 1;
    }
    if end_pos < i {
        return (Class::Never, 0);
    }
    if !have_ranges && !have_negs {
        return (Class::Never, glob.len() - pos);
    }
    let sranges = format!("[{}{ranges}]", if negate { "^" } else { "" });
    let snegs = format!("[{}{negs}]", if negate { "" } else { "^" });
    let comb = if have_ranges && have_negs {
        format!("^({sranges}|{snegs})")
    } else if have_ranges {
        format!("^{sranges}")
    } else {
        format!("^{snegs}")
    };
    let re = JsRegex::new(&comb, "u").unwrap_or_else(|e| panic!("class /{comb}/u: {e}"));
    (Class::Re(Box::new(re)), end_pos - pos)
}

// ---------------------------------------------------------------------------
// numeric-range.ts
// ---------------------------------------------------------------------------

/// `digit(a, b)`: one digit, or a class of digits from a to b.
fn digit(a: u8, b: u8) -> Item {
    let (ca, cb) = (char::from(b'0' + a), char::from(b'0' + b));
    if a == b {
        Item::Lit(ca)
    } else {
        Item::Class(Arc::new(Class::Digits(ca, cb)))
    }
}

fn prefix(d: u8, alts: Vec<Vec<Item>>) -> Vec<Vec<Item>> {
    alts.into_iter()
        .map(|s| {
            let mut v = vec![digit(d, d)];
            v.extend(s);
            v
        })
        .collect()
}

/// `fixed(a, b)`: the digit strings from a to b, which have the same length.
fn fixed(a: &[u8], b: &[u8]) -> Vec<Vec<Item>> {
    if a.is_empty() {
        return vec![Vec::new()];
    }
    let mut lo = a[0] - b'0';
    let mut hi = b[0] - b'0';
    let (ra, rb) = (&a[1..], &b[1..]);
    if lo == hi {
        return prefix(lo, fixed(ra, rb));
    }
    let zeros = vec![b'0'; ra.len()];
    let nines = vec![b'9'; ra.len()];
    let mut out = Vec::new();
    if ra != zeros.as_slice() {
        out.extend(prefix(lo, fixed(ra, &nines)));
        lo += 1;
    }
    let last = if rb == nines.as_slice() {
        Vec::new()
    } else {
        let l = prefix(hi, fixed(&zeros, rb));
        hi -= 1;
        l
    };
    if lo <= hi {
        let mut v = vec![digit(lo, hi)];
        v.extend(ra.iter().map(|_| digit(0, 9)));
        out.push(v);
    }
    out.extend(last);
    out
}

fn pad(n: i64, width: usize) -> Vec<u8> {
    format!("{n:0>width$}").into_bytes()
}

/// `nonneg(a, b, width)`: the numbers from a to b, where 0 <= a <= b, each with
/// leading zeros to `width` digits if it is set.
fn nonneg(a: i64, b: i64, width: usize) -> Vec<Vec<Item>> {
    if width > 0 {
        return fixed(&pad(a, width), &pad(b, width));
    }
    let mut out = Vec::new();
    let (la, lb) = (a.to_string().len(), b.to_string().len());
    for len in la..=lb {
        let lo = a.max(if len == 1 { 0 } else { 10i64.pow(len as u32 - 1) });
        let hi = b.min(10i64.pow(len as u32) - 1);
        out.extend(fixed(lo.to_string().as_bytes(), hi.to_string().as_bytes()));
    }
    out
}

/// `Number(s)` for a `-?\d+` string, if it is a safe integer.
fn safe_integer(s: &str) -> Option<i64> {
    let n: f64 = s.parse().ok()?;
    (n.abs() <= 9_007_199_254_740_991.0).then_some(n as i64)
}

/// `numericRange(lo, hi)`: the strings of the bash brace expansion `{lo..hi}`, as
/// the alternatives of an `@(...)` extglob of digits and digit classes.
fn numeric_range(lo_str: &str, hi_str: &str) -> Option<Vec<Vec<Item>>> {
    let mut lo = safe_integer(lo_str)?;
    let mut hi = safe_integer(hi_str)?;
    if lo > hi {
        std::mem::swap(&mut lo, &mut hi);
    }
    // `/^-?0\d/`
    let padded = |s: &str| {
        let b = s.strip_prefix('-').unwrap_or(s).as_bytes();
        b.len() >= 2 && b[0] == b'0' && b[1].is_ascii_digit()
    };
    let width = if padded(lo_str) || padded(hi_str) { lo_str.len().max(hi_str.len()) } else { 0 };
    let mut out = Vec::new();
    if lo < 0 {
        for s in nonneg((-hi).max(1), -lo, width.saturating_sub(1)) {
            let mut v = vec![Item::Lit('-')];
            v.extend(s);
            out.push(v);
        }
    }
    if hi >= 0 {
        out.extend(nonneg(lo.max(0), hi, width));
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// index.ts: parsing
// ---------------------------------------------------------------------------

/// `/^(-?\d+)\.\.(-?\d+)$/`
fn range_body(s: &str) -> Option<(&str, &str)> {
    let (a, b) = s.split_once("..")?;
    let int = |x: &str| {
        let d = x.strip_prefix('-').unwrap_or(x);
        !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit())
    };
    (int(a) && int(b)).then_some((a, b))
}

fn tokenize(p: &[char], noext: bool, mark: Option<char>) -> Vec<Tok> {
    let mut toks = Vec::with_capacity(p.len());
    let mut i = 0;
    while i < p.len() {
        let c = p[i];
        if let Some(mark) = mark {
            // a \ before a range would only escape its first char
            let m = usize::from(c == '\\' && p.get(i + 1) == Some(&mark));
            if m == 1 || c == mark {
                let start = i + m + 1;
                if let Some(end) = p[start.min(p.len())..].iter().position(|x| *x == mark).map(|e| e + start) {
                    let body: String = p[start..end].iter().collect();
                    if let Some(alts) = range_body(&body).and_then(|(a, b)| numeric_range(a, b)) {
                        toks.push(Tok::Item(Item::Ext('@', alts)));
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        if c == '\\' {
            // a trailing \ is a literal \
            toks.push(Tok::Item(Item::Lit(p.get(i + 1).copied().unwrap_or(c))));
            i += 2;
            continue;
        }
        if c == '[' {
            let (class, consumed) = parse_class(p, i);
            if consumed > 0 {
                toks.push(Tok::Item(Item::Class(Arc::new(class))));
                i += consumed;
                continue;
            }
        }
        if !noext && EXT_TYPES.contains(&c) && p.get(i + 1) == Some(&'(') {
            toks.push(Tok::Open(c));
            i += 2;
            continue;
        }
        toks.push(match c {
            '|' if !noext => Tok::Bar,
            ')' if !noext => Tok::Close,
            '*' => Tok::Item(Item::Star),
            '?' => Tok::Item(Item::Any),
            c => Tok::Item(Item::Lit(c)),
        });
        i += 1;
    }
    toks
}

/// `pair(toks)`: each open token index to the index of its close token.
fn pair(toks: &[Tok]) -> Vec<Option<usize>> {
    let mut pairs = vec![None; toks.len()];
    let mut stack = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        match t {
            Tok::Open(_) => stack.push(i),
            Tok::Close => {
                if let Some(o) = stack.pop() {
                    pairs[o] = Some(i);
                }
            }
            _ => {}
        }
    }
    pairs
}

/// `build(...)`: the alternatives between `start` and `end`. An open token that
/// is not closed is never inside a closed extglob, so bar and close tokens are
/// only literal at the top level.
fn build(toks: &[Tok], pairs: &[Option<usize>], start: usize, end: usize, top: bool) -> Vec<Vec<Item>> {
    let mut alts = vec![Vec::new()];
    let mut i = start;
    while i < end {
        if matches!(toks[i], Tok::Bar) && !top {
            alts.push(Vec::new());
            i += 1;
            continue;
        }
        let seq = alts.last_mut().expect("nonempty");
        match (&toks[i], pairs[i]) {
            (Tok::Open(t), Some(close)) => {
                let sub = build(toks, pairs, i + 1, close, false);
                seq.push(Item::Ext(*t, sub));
                i = close;
            }
            (Tok::Open(t), None) => {
                seq.push(Item::Lit(*t));
                seq.push(Item::Lit('('));
            }
            (Tok::Bar, _) => seq.push(Item::Lit('|')),
            (Tok::Close, _) => seq.push(Item::Lit(')')),
            (Tok::Item(it), _) => seq.push(it.clone()),
        }
        i += 1;
    }
    alts
}

// ---------------------------------------------------------------------------
// index.ts: the state machine
// ---------------------------------------------------------------------------

/// The match node, at id 0: the end of the pattern, or of a negated sub-pattern.
const MATCH: usize = 0;

#[derive(Debug)]
enum Node {
    Match,
    Lit(char, usize),
    Any(usize),
    Star(usize),
    Class(Arc<Class>, usize),
    Split(Vec<usize>),
    Neg { sub: usize, next: usize },
}

/// `compile(items)`: the nodes, by id, and the start node.
fn compile(items: &[Item]) -> (Vec<Node>, usize) {
    fn push(nodes: &mut Vec<Node>, n: Node) -> usize {
        nodes.push(n);
        nodes.len() - 1
    }
    fn seq(nodes: &mut Vec<Node>, items: &[Item], next: usize) -> usize {
        items.iter().rev().fold(next, |next, it| item(nodes, it, next))
    }
    fn alts(nodes: &mut Vec<Node>, alts: &[Vec<Item>], next: usize) -> Vec<usize> {
        alts.iter().map(|a| seq(nodes, a, next)).collect()
    }
    fn item(nodes: &mut Vec<Node>, it: &Item, next: usize) -> usize {
        match it {
            Item::Lit(c) => push(nodes, Node::Lit(*c, next)),
            Item::Any => push(nodes, Node::Any(next)),
            Item::Star => push(nodes, Node::Star(next)),
            Item::Class(c) => push(nodes, Node::Class(c.clone(), next)),
            Item::Ext('@', a) => {
                let out = alts(nodes, a, next);
                push(nodes, Node::Split(out))
            }
            Item::Ext('?', a) => {
                let mut out = vec![next];
                out.extend(alts(nodes, a, next));
                push(nodes, Node::Split(out))
            }
            Item::Ext('!', a) => {
                let out = alts(nodes, a, MATCH);
                let sub = push(nodes, Node::Split(out));
                push(nodes, Node::Neg { sub, next })
            }
            // * and +, loop back after each match
            Item::Ext(t, a) => {
                let after = push(nodes, Node::Split(Vec::new()));
                let out = alts(nodes, a, after);
                let entry = push(nodes, Node::Split(out));
                if let Node::Split(o) = &mut nodes[after] {
                    o.push(next);
                    o.push(entry);
                }
                if *t == '*' {
                    after
                } else {
                    entry
                }
            }
        }
    }
    let mut nodes = vec![Node::Match];
    let start = seq(&mut nodes, items, MATCH);
    (nodes, start)
}

/// A thread inside a `!(...)` negation: the threads of the negated sub-pattern,
/// started where the negation started. The negation matches the chars so far if
/// the sub-pattern does not.
#[derive(Debug, Clone)]
struct Run {
    neg: usize,
    state: Rc<State>,
}

/// The threads at one position in the string. Each node thread is kept once, and
/// runs with the same state are kept once, so that identical threads merge. The
/// match node is not a thread: it sets `is_match` instead.
#[derive(Debug)]
struct State {
    gen: u64,
    threads: Vec<usize>,
    is_match: bool,
    runs: Vec<Run>,
    /// The sorted ids of the threads, with 0 for a match, once the state is done.
    ids: Vec<usize>,
}

impl State {
    /// No threads, so nothing more can match.
    fn dead(&self) -> bool {
        self.threads.is_empty() && !self.is_match && self.runs.is_empty()
    }
}

/// `sameState(a, b)`: true if the two states hold the same threads.
fn same_state(a: &State, b: &State) -> bool {
    a.ids == b.ids
        && a.runs.len() == b.runs.len()
        && a.runs.iter().all(|r| b.runs.iter().any(|o| o.neg == r.neg && same_state(&o.state, &r.state)))
}

/// `Sim`, with its `Marks`: for each node id, the gen of the last state that has
/// the node as a thread, and of the last state that walked through it. A state is
/// built to the end before the states around it go on, and the nodes of a negated
/// sub-pattern are only in its states, so one mark per node is enough.
struct Sim<'a> {
    nodes: &'a [Node],
    /// `.` and `..` only match literal dots, even with `dot:true`.
    dots: bool,
    /// The string starts with `.`, and `dot:true` is not set.
    dot_start: bool,
    has: Vec<u64>,
    seen: Vec<u64>,
    gen: u64,
}

impl Sim<'_> {
    fn state(&mut self) -> State {
        self.gen += 1;
        State { gen: self.gen, threads: Vec::new(), is_match: false, runs: Vec::new(), ids: Vec::new() }
    }

    fn done(state: &mut State) {
        let mut ids = state.threads.clone();
        ids.sort_unstable();
        if state.is_match {
            ids.insert(0, MATCH);
        }
        state.ids = ids;
    }

    /// True the first time that the state walks through the node, so that loops in
    /// the graph stop.
    fn visit(&mut self, state: &State, node: usize) -> bool {
        if self.seen[node] == state.gen {
            return false;
        }
        self.seen[node] = state.gen;
        true
    }

    fn add_thread(&mut self, state: &mut State, node: usize) -> bool {
        if self.has[node] == state.gen {
            return false;
        }
        self.has[node] = state.gen;
        state.threads.push(node);
        true
    }

    /// Wildcards only match a leading dot if `dot:true`. In a negated sub-pattern
    /// this does not apply, because the negation itself can not match a leading dot.
    fn restricted(&self, i: usize, top: bool) -> bool {
        top && (self.dots || (self.dot_start && i == 0))
    }

    fn add(&mut self, state: &mut State, node: usize, i: usize, top: bool) {
        let nodes = self.nodes;
        match &nodes[node] {
            Node::Split(out) => {
                if !self.visit(state, node) {
                    return;
                }
                for &o in out {
                    self.add(state, o, i, top);
                }
            }
            &Node::Neg { sub, .. } => {
                if !self.visit(state, node) {
                    return;
                }
                let mut s = self.state();
                self.add(&mut s, sub, i, false);
                Self::done(&mut s);
                self.add_run(state, Run { neg: node, state: Rc::new(s) }, i, top);
            }
            &Node::Star(next) => {
                // a * can not start a dotfile, even if it matches nothing
                if self.restricted(i, top) {
                    return;
                }
                if self.add_thread(state, node) {
                    self.add(state, next, i, top);
                }
            }
            Node::Match => state.is_match = true,
            _ => {
                self.add_thread(state, node);
            }
        }
    }

    fn add_run(&mut self, state: &mut State, run: Run, i: usize, top: bool) {
        if state.runs.iter().any(|r| r.neg == run.neg && same_state(&r.state, &run.state)) {
            return;
        }
        let matched = run.state.is_match;
        let Node::Neg { next, .. } = self.nodes[run.neg] else { unreachable!("run of a non-negation") };
        state.runs.push(run);
        if !matched {
            self.add(state, next, i, top);
        }
    }

    /// The threads after char `c`, at index `i`.
    fn step(&mut self, state: &State, c: char, i: usize, top: bool) -> State {
        let mut next = self.state();
        let restricted = self.restricted(i, top);
        let nodes = self.nodes;
        for &th in &state.threads {
            match &nodes[th] {
                &Node::Lit(ch, n) if ch == c => self.add(&mut next, n, i + 1, top),
                &Node::Any(n) if !restricted => self.add(&mut next, n, i + 1, top),
                &Node::Class(ref cls, n) if !restricted && cls.test(c) => self.add(&mut next, n, i + 1, top),
                Node::Star(_) if !restricted => self.add(&mut next, th, i + 1, top),
                _ => {}
            }
        }
        if !restricted {
            for run in &state.runs {
                // a sub-pattern with no threads can not match again, so the run
                // stays the same
                if run.state.dead() {
                    self.add_run(&mut next, run.clone(), i + 1, top);
                    continue;
                }
                let sub = self.step(&run.state, c, i, false);
                self.add_run(&mut next, Run { neg: run.neg, state: Rc::new(sub) }, i + 1, top);
            }
        }
        Self::done(&mut next);
        next
    }
}

// ---------------------------------------------------------------------------
// index.ts: Matcher
// ---------------------------------------------------------------------------

/// A bound of `lengths`: `None` is Infinity.
type Len = Option<usize>;

fn add_len(a: Len, b: Len) -> Len {
    a.zip(b).map(|(a, b)| a + b)
}

/// `lengths(items)`: bounds on the chars that the items can match. A negation can
/// match a string of any length, so it counts as 0 to Infinity.
fn lengths(items: &[Item]) -> (usize, Len) {
    let mut min = 0;
    let mut max = Some(0);
    for it in items {
        let (lo, hi) = item_lengths(it);
        min += lo;
        max = add_len(max, hi);
    }
    (min, max)
}

fn item_lengths(it: &Item) -> (usize, Len) {
    let Item::Ext(t, alts) = it else {
        return if matches!(it, Item::Star) { (0, None) } else { (1, Some(1)) };
    };
    if *t == '!' {
        return (0, None);
    }
    let alts: Vec<(usize, Len)> = alts.iter().map(|a| lengths(a)).collect();
    let min = alts.iter().map(|a| a.0).min().unwrap_or(0);
    let max = alts.iter().map(|a| a.1).try_fold(0, |m, a| a.map(|a| m.max(a)));
    // a loop of alternatives that only match the empty string is empty
    let lp = if max == Some(0) { Some(0) } else { None };
    match t {
        '@' => (min, max),
        '?' => (0, max),
        '*' => (0, lp),
        _ => (min, lp),
    }
}

/// A node of a graph with no splits or negations.
#[derive(Debug)]
enum Linear {
    Lit(char),
    Any,
    Star,
    Class(Arc<Class>),
}

/// `testLinear(...)`: match a chain of nodes. Each node other than `*` matches
/// exactly one char, so on a failure it is enough to go back to the last `*` and
/// let it match one more char.
fn test_linear(nodes: &[Linear], chars: &[char], start: usize, end: usize) -> bool {
    let (mut ni, mut ci) = (0, start);
    let mut star_ni: Option<usize> = None;
    let mut star_ci = 0;
    while ci < end {
        let n = nodes.get(ni);
        if let Some(Linear::Star) = n {
            star_ni = Some(ni);
            ni += 1;
            star_ci = ci;
            continue;
        }
        let c = chars[ci];
        let hit = match n {
            Some(Linear::Any) => true,
            Some(Linear::Lit(l)) => *l == c,
            Some(Linear::Class(cls)) => cls.test(c),
            _ => false,
        };
        if hit {
            ni += 1;
            ci += 1;
            continue;
        }
        let Some(s) = star_ni else { return false };
        ni = s + 1;
        star_ci += 1;
        ci = star_ci;
    }
    while let Some(Linear::Star) = nodes.get(ni) {
        ni += 1;
    }
    ni == nodes.len()
}

/// `new Matcher(pattern, { dot, rangeMark })` for one path portion.
#[derive(Debug)]
pub(super) struct Matcher {
    dot: bool,
    nodes: Vec<Node>,
    start: usize,
    /// The string to compare, if the pattern has no magic.
    literal: Option<String>,
    /// The nodes between the prefix and suffix, if the graph is a single chain.
    linear: Option<Vec<Linear>>,
    /// The linear nodes are only `*` and `?`, so the length bounds and the
    /// prefix and suffix decide the match.
    wild: bool,
    min: usize,
    max: Len,
    /// The literals that a match must start and end with.
    prefix: String,
    prefix_len: usize,
    suffix: String,
    suffix_len: usize,
}

impl Matcher {
    pub(super) fn new(pattern: &str, dot: bool, range_mark: Option<char>) -> Matcher {
        let p: Vec<char> = pattern.chars().collect();
        // a pattern that is only an empty extglob is a literal string, except for
        // `!()`, which matches any non-empty string
        let literal = p.len() == 3 && matches!(p[0], '?' | '*' | '+' | '@') && p[1] == '(' && p[2] == ')';
        let toks = tokenize(&p, literal, range_mark);
        let pairs = pair(&toks);
        let items = build(&toks, &pairs, 0, toks.len(), true).swap_remove(0);
        let (nodes, start) = compile(&items);
        let mut m = Matcher {
            dot,
            nodes,
            start,
            literal: None,
            linear: None,
            wild: false,
            min: 0,
            max: None,
            prefix: String::new(),
            prefix_len: 0,
            suffix: String::new(),
            suffix_len: 0,
        };
        if items.iter().all(|it| matches!(it, Item::Lit(_))) {
            m.literal = Some(items.iter().map(|it| if let Item::Lit(c) = it { *c } else { unreachable!() }).collect());
            return m;
        }
        let lit = |it: &Item| if let Item::Lit(c) = it { Some(*c) } else { None };
        m.prefix = items.iter().map_while(lit).collect();
        m.prefix_len = m.prefix.chars().count();
        let mut suffix: Vec<char> = items.iter().rev().map_while(lit).collect();
        suffix.reverse();
        m.suffix_len = suffix.len();
        m.suffix = suffix.into_iter().collect();
        let middle = &items[m.prefix_len..items.len() - m.suffix_len];
        m.linear = middle
            .iter()
            .map(|it| match it {
                Item::Lit(c) => Some(Linear::Lit(*c)),
                Item::Any => Some(Linear::Any),
                Item::Star => Some(Linear::Star),
                Item::Class(c) => Some(Linear::Class(c.clone())),
                Item::Ext(..) => None,
            })
            .collect();
        m.wild = m.linear.as_ref().is_some_and(|l| l.iter().all(|n| matches!(n, Linear::Star | Linear::Any)));
        (m.min, m.max) = lengths(&items);
        m
    }

    /// `testUnchecked(s)`: `s` is one path portion, with no `/`.
    pub(super) fn test(&self, s: &str) -> bool {
        if let Some(l) = &self.literal {
            return s == l;
        }
        // The length in code points is from a quarter of the UTF-8 length to all of
        // it, which is often enough to know without counting.
        let (min, max) = (self.min, self.max);
        let most = s.len();
        let least = most.div_ceil(4);
        if most < min || max.is_some_and(|m| least > m) {
            return false;
        }
        let mut chars: Option<Vec<char>> = None;
        if least < min || max.is_some_and(|m| most > m) {
            let c: Vec<char> = s.chars().collect();
            if c.len() < min || max.is_some_and(|m| c.len() > m) {
                return false;
            }
            chars = Some(c);
        }
        if !s.starts_with(self.prefix.as_str()) || !s.ends_with(self.suffix.as_str()) {
            return false;
        }
        // the length check means that the prefix and suffix do not overlap
        let dot_start = !self.dot && s.starts_with('.');
        if let Some(linear) = &self.linear {
            // `.` and `..` only match literal dots, even with `dot:true`
            if s == "." || s == ".." {
                return false;
            }
            // only a literal can match the leading dot. A `*` can not start a
            // dotfile, even if it matches nothing.
            if dot_start && self.prefix_len == 0 {
                return false;
            }
            if self.wild {
                return true;
            }
            let chars = chars.unwrap_or_else(|| s.chars().collect());
            let end = chars.len() - self.suffix_len;
            return test_linear(linear, &chars, self.prefix_len, end);
        }
        let dots = s == "." || s == "..";
        let mut sim = Sim {
            nodes: &self.nodes,
            dots,
            dot_start: !dots && dot_start,
            has: vec![0; self.nodes.len()],
            seen: vec![0; self.nodes.len()],
            gen: 0,
        };
        let mut state = sim.state();
        sim.add(&mut state, self.start, 0, true);
        Sim::done(&mut state);
        for (i, c) in s.chars().enumerate() {
            if state.dead() {
                return false;
            }
            state = sim.step(&state, c, i, true);
        }
        state.is_match
    }
}
