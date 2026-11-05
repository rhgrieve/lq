//! A panic in Biome falls back to printing the block verbatim instead of ending the run.
//! `scripts/parity/fmt-biome-guard.mjs` proves the same on the release binary.

use lq_fmt::embed::biome::inject_panic;
use lq_fmt::{format_source, EmbeddedLanguageFormatting, Options};

#[test]
fn a_biome_panic_prints_the_block_verbatim() {
    let source = "<style>\na { color:   red }\n</style>\n<script>\nlet   a=1\n</script>\n";
    let run = || format_source(source, &Options::default()).unwrap();
    let verbatim = format_source(
        source,
        &Options { embedded_language_formatting: EmbeddedLanguageFormatting::Off, ..Options::default() },
    )
    .unwrap();

    let formatted = run();
    assert_ne!(formatted, verbatim, "Biome should change this input");

    for step in ["parse", "format"] {
        inject_panic(step);
        assert_eq!(run(), verbatim, "panic in {step}");
    }
    inject_panic("none");
    assert_eq!(run(), formatted);
}
