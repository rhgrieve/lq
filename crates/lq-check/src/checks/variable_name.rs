//! `VariableName`, with the parts of lodash's `camelCase`, `kebabCase` and `snakeCase`
//! it depends on.

use super::shared_rules::type_error;
use crate::api::*;
use lq_json::regex::JsRegex;
use lq_syntax::{Markup, Node, Position};
use std::sync::OnceLock;

pub const IMPL: CheckImpl = CheckImpl::Liquid {
    create,
    enter: NodeMask::of(&[NodeKind::LiquidTag]),
    exit: NodeMask::NONE,
};

fn create<'a, 'f>(cx: &LiquidCx<'a, 'f>) -> Option<Box<dyn LiquidCheck<'a> + 'a>> {
    let format = match cx.settings().str("format") {
        Some("camelCase") => Some(Format::Camel),
        Some("PascalCase") => Some(Format::Pascal),
        Some("snake_case") => Some(Format::Snake),
        Some("kebab-case") => Some(Format::Kebab),
        _ => None,
    };
    Some(Box::new(VariableName { format }))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Camel,
    Pascal,
    Snake,
    Kebab,
}

struct VariableName {
    format: Option<Format>,
}

/// `deburrLetter`: Latin-1 Supplement and Latin Extended-A letters.
fn deburr_letter(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{c0}' | '\u{c1}' | '\u{c2}' | '\u{c3}' | '\u{c4}' | '\u{c5}' | '\u{100}' | '\u{102}' | '\u{104}' => "A",
        '\u{e0}' | '\u{e1}' | '\u{e2}' | '\u{e3}' | '\u{e4}' | '\u{e5}' | '\u{101}' | '\u{103}' | '\u{105}' => "a",
        '\u{c7}' | '\u{106}' | '\u{108}' | '\u{10a}' | '\u{10c}' => "C",
        '\u{e7}' | '\u{107}' | '\u{109}' | '\u{10b}' | '\u{10d}' => "c",
        '\u{d0}' | '\u{10e}' | '\u{110}' => "D",
        '\u{f0}' | '\u{10f}' | '\u{111}' => "d",
        '\u{c8}' | '\u{c9}' | '\u{ca}' | '\u{cb}' | '\u{112}' | '\u{114}' | '\u{116}' | '\u{118}' | '\u{11a}' => "E",
        '\u{e8}' | '\u{e9}' | '\u{ea}' | '\u{eb}' | '\u{113}' | '\u{115}' | '\u{117}' | '\u{119}' | '\u{11b}' => "e",
        '\u{cc}' | '\u{cd}' | '\u{ce}' | '\u{cf}' | '\u{128}' | '\u{12a}' | '\u{12c}' | '\u{12e}' | '\u{130}' => "I",
        '\u{ec}' | '\u{ed}' | '\u{ee}' | '\u{ef}' | '\u{129}' | '\u{12b}' | '\u{12d}' | '\u{12f}' | '\u{131}' => "i",
        '\u{d1}' | '\u{143}' | '\u{145}' | '\u{147}' | '\u{14a}' => "N",
        '\u{f1}' | '\u{144}' | '\u{146}' | '\u{148}' | '\u{14b}' => "n",
        '\u{d2}' | '\u{d3}' | '\u{d4}' | '\u{d5}' | '\u{d6}' | '\u{d8}' | '\u{14c}' | '\u{14e}' | '\u{150}' => "O",
        '\u{f2}' | '\u{f3}' | '\u{f4}' | '\u{f5}' | '\u{f6}' | '\u{f8}' | '\u{14d}' | '\u{14f}' | '\u{151}' => "o",
        '\u{d9}' | '\u{da}' | '\u{db}' | '\u{dc}' | '\u{168}' | '\u{16a}' | '\u{16c}' | '\u{16e}' | '\u{170}' | '\u{172}' => "U",
        '\u{f9}' | '\u{fa}' | '\u{fb}' | '\u{fc}' | '\u{169}' | '\u{16b}' | '\u{16d}' | '\u{16f}' | '\u{171}' | '\u{173}' => "u",
        '\u{dd}' | '\u{176}' | '\u{178}' => "Y",
        '\u{fd}' | '\u{ff}' | '\u{177}' => "y",
        '\u{c6}' => "Ae",
        '\u{e6}' => "ae",
        '\u{de}' => "Th",
        '\u{fe}' => "th",
        '\u{df}' => "ss",
        '\u{11c}' | '\u{11e}' | '\u{120}' | '\u{122}' => "G",
        '\u{11d}' | '\u{11f}' | '\u{121}' | '\u{123}' => "g",
        '\u{124}' | '\u{126}' => "H",
        '\u{125}' | '\u{127}' => "h",
        '\u{134}' => "J",
        '\u{135}' => "j",
        '\u{136}' => "K",
        '\u{137}' | '\u{138}' => "k",
        '\u{139}' | '\u{13b}' | '\u{13d}' | '\u{13f}' | '\u{141}' => "L",
        '\u{13a}' | '\u{13c}' | '\u{13e}' | '\u{140}' | '\u{142}' => "l",
        '\u{154}' | '\u{156}' | '\u{158}' => "R",
        '\u{155}' | '\u{157}' | '\u{159}' => "r",
        '\u{15a}' | '\u{15c}' | '\u{15e}' | '\u{160}' => "S",
        '\u{15b}' | '\u{15d}' | '\u{15f}' | '\u{161}' | '\u{17f}' => "s",
        '\u{162}' | '\u{164}' | '\u{166}' => "T",
        '\u{163}' | '\u{165}' | '\u{167}' => "t",
        '\u{174}' => "W",
        '\u{175}' => "w",
        '\u{179}' | '\u{17b}' | '\u{17d}' => "Z",
        '\u{17a}' | '\u{17c}' | '\u{17e}' => "z",
        '\u{132}' => "IJ",
        '\u{133}' => "ij",
        '\u{152}' => "Oe",
        '\u{153}' => "oe",
        '\u{149}' => "'n",
        _ => return None,
    })
}

fn is_combo_mark(c: char) -> bool {
    matches!(c, '\u{300}'..='\u{36f}' | '\u{fe20}'..='\u{fe2f}' | '\u{20d0}'..='\u{20ff}')
}

/// `deburr(string).replace(reApos, '')`.
fn prepare(string: &str) -> String {
    let mut out = String::with_capacity(string.len());
    for c in string.chars() {
        if is_combo_mark(c) || c == '\'' || c == '\u{2019}' {
            continue;
        }
        match deburr_letter(c) {
            // A deburred apostrophe (`\u{149}` is `'n`) is removed by `reApos` too.
            Some(letters) => out.extend(letters.chars().filter(|&l| l != '\'')),
            None => out.push(c),
        }
    }
    out
}

fn compile(cell: &'static OnceLock<JsRegex>, pattern: impl FnOnce() -> String) -> &'static JsRegex {
    cell.get_or_init(|| JsRegex::new(&pattern(), "g").expect("valid regex"))
}

/// `reUnicodeWord`. Upstream runs it on UTF-16 units, `JsRegex` on code points, so
/// lodash's surrogate classes are written here as the astral code points they match.
fn unicode_word_regex() -> &'static JsRegex {
    static RE: OnceLock<JsRegex> = OnceLock::new();
    compile(&RE, || {
        let astral = "\u{10000}-\u{10ffff}";
        let combo = r"\u0300-\u036f\ufe20-\ufe2f\u20d0-\u20ff";
        let dingbat_range = r"\u2700-\u27bf";
        let lower_range = r"a-z\xdf-\xf6\xf8-\xff";
        let upper_range = r"A-Z\xc0-\xd6\xd8-\xde";
        let break_range = concat!(
            r"\xac\xb1\xd7\xf7",
            r"\x00-\x2f\x3a-\x40\x5b-\x60\x7b-\xbf",
            r"\u2000-\u206f",
            r" \t\x0b\f\xa0\ufeff\n\r\u2028\u2029\u1680\u180e\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u202f\u205f\u3000",
        );
        let brk = format!("[{break_range}]");
        let lower = format!("[{lower_range}]");
        let upper = format!("[{upper_range}]");
        let misc = format!(r"[^{astral}{break_range}\d+{dingbat_range}{lower_range}{upper_range}]");
        let fitz = "[\u{1f3fb}-\u{1f3ff}]";
        let modifier = format!("(?:[{combo}]|{fitz})");
        let non_astral = format!("[^{astral}]");
        let regional = "[\u{1f1e6}-\u{1f1ff}]{2}";
        let surr_pair = format!("[{astral}]");
        let misc_lower = format!("(?:{lower}|{misc})");
        let misc_upper = format!("(?:{upper}|{misc})");
        let contr_lower = r"(?:['\u2019](?:d|ll|m|re|s|t|ve))?";
        let contr_upper = r"(?:['\u2019](?:D|LL|M|RE|S|T|VE))?";
        let opt_mod = format!("{modifier}?");
        let opt_var = r"[\ufe0e\ufe0f]?";
        let opt_join = format!(r"(?:\u200d(?:{non_astral}|{regional}|{surr_pair}){opt_var}{opt_mod})*");
        let ord_lower = r"\d*(?:1st|2nd|3rd|(?![123])\dth)(?=\b|[A-Z_])";
        let ord_upper = r"\d*(?:1ST|2ND|3RD|(?![123])\dTH)(?=\b|[a-z_])";
        let seq = format!("{opt_var}{opt_mod}{opt_join}");
        let emoji = format!("(?:[{dingbat_range}]|{regional}|{surr_pair}){seq}");
        [
            format!("{upper}?{lower}+{contr_lower}(?={brk}|{upper}|$)"),
            format!("{misc_upper}+{contr_upper}(?={brk}|{upper}{misc_lower}|$)"),
            format!("{upper}?{misc_lower}+{contr_lower}"),
            format!("{upper}+{contr_upper}"),
            ord_upper.to_string(),
            ord_lower.to_string(),
            r"\d+".to_string(),
            emoji,
        ]
        .join("|")
    })
}

/// `hasUnicodeWord(string)`.
fn has_unicode_word(string: &str) -> bool {
    static RE: OnceLock<JsRegex> = OnceLock::new();
    compile(&RE, || r"[a-z][A-Z]|[A-Z]{2}[a-z]|[0-9][a-zA-Z]|[a-zA-Z][0-9]|[^a-zA-Z0-9 ]".to_string()).is_match(string)
}

/// `words(string)`.
fn words(string: &str) -> Vec<&str> {
    static ASCII: OnceLock<JsRegex> = OnceLock::new();
    let regex = if has_unicode_word(string) {
        unicode_word_regex()
    } else {
        compile(&ASCII, || r"[^\x00-\x2f\x3a-\x40\x5b-\x60\x7b-\x7f]+".to_string())
    };
    regex.find_iter(string).into_iter().map(|m| m.as_str(string)).collect()
}

/// `upperFirst(word)`.
fn upper_first(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn camel_case(string: &str) -> String {
    let prepared = prepare(string);
    let mut out = String::new();
    for (i, word) in words(&prepared).into_iter().enumerate() {
        let lower = word.to_lowercase();
        if i == 0 {
            out.push_str(&lower);
        } else {
            out.push_str(&upper_first(&lower));
        }
    }
    out
}

/// `pascalCase`: `charAt(0)` leaves an astral first character alone.
fn pascal_case(string: &str) -> String {
    let camel = camel_case(string);
    match camel.chars().next() {
        Some(first) if first.len_utf16() == 1 => upper_first(&camel),
        _ => camel,
    }
}

fn joined_lower(string: &str, separator: &str) -> String {
    let prepared = prepare(string);
    words(&prepared).into_iter().map(str::to_lowercase).collect::<Vec<_>>().join(separator)
}

/// `varName.replace(/[-_]?\d[-_]?/g, '')`.
fn collapse_number_spacing(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = String::with_capacity(name.len());
    let is_separator = |i: usize| matches!(bytes.get(i), Some(b'-' | b'_'));
    let is_digit = |i: usize| bytes.get(i).is_some_and(u8::is_ascii_digit);
    let mut i = 0;
    let mut kept_from = 0;
    while i < bytes.len() {
        let digit_at = if is_separator(i) && is_digit(i + 1) {
            i + 1
        } else if is_digit(i) {
            i
        } else {
            i += 1;
            continue;
        };
        out.push_str(&name[kept_from..i]);
        i = digit_at + 1;
        if is_separator(i) {
            i += 1;
        }
        kept_from = i;
    }
    out.push_str(&name[kept_from..]);
    out
}

/// A name `snakeCase` leaves alone: lowercase ASCII words joined by single underscores.
fn is_plain_snake_case(name: &str) -> bool {
    !name.is_empty() && name.split('_').all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_lowercase()))
}

fn suggestion(name: &str, format: Format) -> String {
    let (leading, rest) = match name.strip_prefix('_') {
        Some(rest) => ("_", rest),
        None => ("", name),
    };
    let formatted = match format {
        Format::Camel => camel_case(rest),
        Format::Pascal => pascal_case(rest),
        Format::Snake => joined_lower(rest, "_"),
        Format::Kebab => joined_lower(rest, "-"),
    };
    format!("{leading}{formatted}")
}

fn check_name(cx: &LiquidCx<'_, '_>, format: Option<Format>, name: &str, position: Position) {
    let Some(format) = format else {
        // `formatTypes[format]` is undefined, and calling it throws.
        cx.fail(type_error("formatter is not a function"));
        return;
    };
    let rest = name.strip_prefix('_').unwrap_or(name);
    if format == Format::Snake && is_plain_snake_case(rest) {
        return;
    }
    let suggested = suggestion(name, format);
    if !rest.is_empty() && collapse_number_spacing(name) == collapse_number_spacing(&suggested) {
        return;
    }
    let replacement = lq_syntax::js::slice(cx.source(), position.start, position.end).replacen(name, &suggested, 1);
    cx.report(
        LiquidProblem::new(format!("The variable '{name}' uses wrong naming format"), position.start, position.end)
            .suggest(format!("Change variable '{name}' to '{suggested}'"), |c| {
                c.replace(position.start, position.end, replacement);
            }),
    );
}

impl<'a> LiquidCheck<'a> for VariableName {
    fn enter(&mut self, node: &'a Node<'a>, _ancestors: &[&'a Node<'a>], cx: &LiquidCx<'a, '_>) {
        let Node::LiquidTag(tag) = node else { return };
        if !matches!(tag.name, "assign" | "capture") {
            return;
        }
        let Markup::Node(markup) = &tag.markup else { return };
        match (tag.name, &**markup) {
            ("assign", Node::AssignMarkup(m)) => check_name(cx, self.format, m.name, markup.position()),
            ("capture", Node::VariableLookup(l)) => {
                if let Some(name) = l.name.filter(|n| !n.is_empty()) {
                    check_name(cx, self.format, name, markup.position());
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapse() {
        assert_eq!(collapse_number_spacing("a_1_b"), "ab");
        assert_eq!(collapse_number_spacing("a1b"), "ab");
        assert_eq!(collapse_number_spacing("a--1--b"), "a--b");
        assert_eq!(collapse_number_spacing("x_"), "x_");
        assert_eq!(collapse_number_spacing("12"), "");
    }

    /// Expected values were generated by running lodash itself on these inputs.
    #[test]
    fn lodash_cases() {
        let cases: &[(&str, &str, &str, &str)] = &[
            ("fooBar", "fooBar", "foo_bar", "foo-bar"),
            ("foo_bar", "fooBar", "foo_bar", "foo-bar"),
            ("foo-bar", "fooBar", "foo_bar", "foo-bar"),
            ("FooBar", "fooBar", "foo_bar", "foo-bar"),
            ("FOO_BAR", "fooBar", "foo_bar", "foo-bar"),
            ("foo2bar", "foo2Bar", "foo_2_bar", "foo-2-bar"),
            ("foo_2_bar", "foo2Bar", "foo_2_bar", "foo-2-bar"),
            ("foo-2", "foo2", "foo_2", "foo-2"),
            ("Foo2Bar", "foo2Bar", "foo_2_bar", "foo-2-bar"),
            ("HTMLParser", "htmlParser", "html_parser", "html-parser"),
            ("parseHTML", "parseHtml", "parse_html", "parse-html"),
            ("x", "x", "x", "x"),
            ("_", "", "", ""),
            ("a__b", "aB", "a_b", "a-b"),
            ("a--b", "aB", "a_b", "a-b"),
            ("foo1st", "foo1st", "foo_1st", "foo-1st"),
            ("foo2ND", "foo2nd", "foo_2nd", "foo-2nd"),
            ("1st", "1st", "1st", "1st"),
            ("3rdPlace", "3rdPlace", "3rd_place", "3rd-place"),
            ("4th_item", "4thItem", "4th_item", "4th-item"),
            ("éa_b", "eaB", "ea_b", "ea-b"),
            ("Éclair_foo", "eclairFoo", "eclair_foo", "eclair-foo"),
            ("naïve", "naive", "naive", "naive"),
            ("straße", "strasse", "strasse", "strasse"),
            ("ĲK", "ijk", "ijk", "ijk"),
            ("aŉb", "anb", "anb", "anb"),
            ("日本語", "日本語", "日本語", "日本語"),
            ("日本_語", "日本語", "日本_語", "日本-語"),
            ("fooBar日本", "fooBar日本", "foo_bar日本", "foo-bar日本"),
            ("a😀b", "a😀B", "a_😀_b", "a-😀-b"),
            ("😀", "😀", "😀", "😀"),
            ("x_😀", "x😀", "x_😀", "x-😀"),
            ("☃snow", "☃snow", "☃snow", "☃snow"),
            ("foóbar", "foobar", "foobar", "foobar"),
            ("ÀB", "ab", "ab", "ab"),
            ("aÀb", "aAb", "a_ab", "a-ab"),
            ("10x10", "10X10", "10_x_10", "10-x-10"),
            ("x10", "x10", "x_10", "x-10"),
            ("X10Y", "x10Y", "x_10_y", "x-10-y"),
            ("A1B2", "a1B2", "a_1_b_2", "a-1-b-2"),
            ("aB1C", "aB1C", "a_b_1_c", "a-b-1-c"),
            ("price_min_max", "priceMinMax", "price_min_max", "price-min-max"),
            ("priceMin_max", "priceMinMax", "price_min_max", "price-min-max"),
            ("pascal-Case", "pascalCase", "pascal_case", "pascal-case"),
            ("__x", "x", "x", "x"),
            ("ﬁ_a", "ﬁA", "ﬁ_a", "ﬁ-a"),
            ("ǅa", "ǆa", "ǆa", "ǆa"),
            ("σς", "σς", "σς", "σς"),
            ("ΑΣ", "ας", "ας", "ας"),
            ("a.b", "aB", "a_b", "a-b"),
            ("a?b", "aB", "a_b", "a-b"),
            ("don't", "dont", "dont", "dont"),
            ("it’s_ok", "itsOk", "its_ok", "its-ok"),
            ("ABc", "aBc", "a_bc", "a-bc"),
            ("ABC", "abc", "abc", "abc"),
            ("abC", "abC", "ab_c", "ab-c"),
            ("aBC", "aBc", "a_bc", "a-bc"),
        ];
        for &(input, camel, snake, kebab) in cases {
            assert_eq!(camel_case(input), camel, "camelCase({input:?})");
            assert_eq!(joined_lower(input, "_"), snake, "snakeCase({input:?})");
            assert_eq!(joined_lower(input, "-"), kebab, "kebabCase({input:?})");
        }
    }
}
