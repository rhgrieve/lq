//! Where `lq` deliberately differs from upstream, because upstream has a demonstrated
//! bug. `DIVERGENCES.md` has the reproduction and proof of each; `scripts/parity/divergences.mjs`
//! runs them against the real oracle.
//!
//! Fixes are on by default. `lq check --upstream-bugs` turns them all off, which is the
//! mode every parity gate runs in. A check asks `cx.divergence(Divergence::X)`: true means
//! apply the fix.

macro_rules! divergences {
    ($($variant:ident: $check:literal, $summary:literal;)*) => {
        /// One difference from upstream. Discriminants index the bits of [`Divergences`].
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(u8)]
        pub enum Divergence { $($variant),* }

        impl Divergence {
            /// Every divergence, in `DIVERGENCES.md` order.
            pub const ALL: &'static [Divergence] = &[$(Divergence::$variant),*];

            /// The stable name used in `DIVERGENCES.md` and on the debug command line.
            pub const fn id(self) -> &'static str {
                match self { $(Divergence::$variant => stringify!($variant)),* }
            }

            /// The check, or engine component, it changes.
            pub const fn check(self) -> &'static str {
                match self { $(Divergence::$variant => $check),* }
            }

            pub const fn summary(self) -> &'static str {
                match self { $(Divergence::$variant => $summary),* }
            }

            pub fn from_id(id: &str) -> Option<Divergence> {
                Divergence::ALL.iter().copied().find(|d| d.id() == id)
            }
        }
    };
}

divergences! {
    GraphNoType: "theme-graph", "A JSON template section or block without `type` is skipped instead of failing the whole graph.";
    GraphFailure: "OrphanedSnippet, ValidScopedCSSClass", "Graph-based conclusions are not drawn when the graph failed to build.";
    UnclosedHtmlIdentifier: "UnclosedHTMLElement", "Condition names print `a['b']['c']`, not `a['b'],['c']`.";
    HtmlSyntaxQuotedLiteral: "LiquidHTMLSyntaxError", "A quoted literal may contain the other kind of quote.";
    LiquidSyntaxUnclosedLine: "LiquidSyntaxError", "An unclosed `block` or `partial` in `{% liquid %}` is reported on its statement.";
    ContentForPrototype: "ValidContentForArguments", "A `content_for` type named like an `Object.prototype` member doesn't stop the check.";
    AddArgumentSuggestion: "Missing*Arguments", "The add-argument suggestion emits no double comma and no empty value.";
    PresetsBlockOrderPath: "SchemaPresetsBlockOrder", "The AST path is restored after recursing into nested blocks.";
    VisibleIfMatchIndex: "ValidVisibleIf", "Positions account for text before `{{` in a `visible_if`.";
    PrototypeLookup: "ValidVisibleIf, ValidSchemaTranslations, ValidSchemaName", "`validateLookup` and `deepGet` see own properties only.";
    SchemaDollarCrash: "engine", "A string `$schema` in `{% schema %}` doesn't crash the run. Not reproducible: see `is_reproducible`.";
    NestedBlockPath: "JSONMissingBlock", "A section or block keyed `blocks` doesn't count as nesting.";
    LinkToNoArgument: "HardcodedRoutes", "`link_to` without an argument doesn't stop the check.";
    EmptySchema: "TranslationKeyExists, PaginationSize, AssetSizeAppBlock*", "An empty or null `{% schema %}` is no schema.";
    NullInPath: "TranslationKeyExists, PaginationSize", "A null along a translation key or in settings counts as not found.";
    EmptyAssetString: "AssetSizeCSS, AssetSizeJavaScript", "An empty string before `stylesheet_tag` or `script_tag` doesn't stop the check.";
    CdnPreconnectHost: "CdnPreconnect", "Compares the URL's hostname instead of an unanchored, unescaped pattern.";
    RouteBoundary: "HardcodedRoutes", "A route matches only up to a segment boundary, `?` or `#`.";
    ImgUrlPrototype: "DeprecatedFilter", "An `img_url` size named like an `Object.prototype` member isn't a named size.";
    UnclosedTagHint: "LiquidHTMLSyntaxError", "A parse error caused by an unclosed tag points at that tag, not at the later markup that failed.";
    AutofixAbort: "engine", "A JSON file that `--auto-correct` can't rewrite, such as a locale with a comment header, is skipped with a warning instead of aborting every other fix.";
}

impl Divergence {
    /// Whether `--upstream-bugs` can reproduce it. Upstream's behaviour for
    /// [`Divergence::SchemaDollarCrash`] is a dead process, so that mode continues too.
    pub const fn is_reproducible(self) -> bool {
        !matches!(self, Divergence::SchemaDollarCrash)
    }
}

/// The set of fixes on for a run. Default: all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Divergences(u32);

impl Divergences {
    /// `lq check --upstream-bugs`.
    pub const NONE: Divergences = Divergences(0);
    /// The default.
    pub const ALL: Divergences = Divergences(u32::MAX);

    /// Only this fix on: how `lint.mjs --default-mode` attributes a difference.
    pub const fn only(d: Divergence) -> Divergences {
        Divergences(1 << d as u8)
    }

    #[inline]
    pub const fn contains(self, d: Divergence) -> bool {
        self.0 & (1 << d as u8) != 0
    }
}

impl Default for Divergences {
    fn default() -> Self {
        Divergences::ALL
    }
}

/// Parses the debug flags shared by `lq check` and the `debug-*` commands.
/// `--upstream-bugs` is one flag; `--only-divergence <ID>` is upstream plus one fix.
pub fn parse_divergence_flag(flag: &str, value: Option<&str>) -> Option<Result<Divergences, String>> {
    match flag {
        "--upstream-bugs" => Some(Ok(Divergences::NONE)),
        "--only-divergence" => Some(match value.and_then(Divergence::from_id) {
            Some(d) => Ok(Divergences::only(d)),
            None => Err(format!("--only-divergence: unknown id {}", value.unwrap_or("(missing)"))),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerable_and_within_the_mask() {
        assert!(Divergence::ALL.len() <= 32);
        for (i, d) in Divergence::ALL.iter().enumerate() {
            assert_eq!(*d as usize, i);
            assert_eq!(Divergence::from_id(d.id()), Some(*d));
            assert!(Divergences::ALL.contains(*d));
            assert!(!Divergences::NONE.contains(*d));
            assert!(Divergences::only(*d).contains(*d));
        }
    }

    #[test]
    fn only_is_exclusive() {
        let only = Divergences::only(Divergence::RouteBoundary);
        assert!(!only.contains(Divergence::GraphNoType));
    }
}
