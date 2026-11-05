//! `lq explain <check>`: what a check does and how to configure it, offline.

use crate::report::term::{style, ColorChoice, Term};
use lq_check::check_utils::{closest, did_you_mean};
use lq_check::registry::REGISTRY;
use lq_check::{CheckDef, SourceCodeType};

const USAGE: &str = "usage: lq explain <check>";

fn find(name: &str) -> Option<&'static CheckDef> {
    REGISTRY.iter().find(|d| {
        d.meta.code.eq_ignore_ascii_case(name) || d.meta.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))
    })
}


pub fn explain(args: &[String]) -> i32 {
    let [name] = args else {
        eprintln!("{USAGE}");
        return 2;
    };
    if name == "--help" || name == "-h" {
        println!("{USAGE}");
        return 0;
    }
    let Some(def) = find(name) else {
        eprintln!("[lq] error: there's no check named {}", crate::report::sanitize(name));
        if let Some(found) = closest(name, REGISTRY.iter().map(|d| d.meta.code)) {
            eprintln!("{}", did_you_mean(found));
        }
        return 2;
    };
    print!("{}", render(def, &Term::detect_stdout(ColorChoice::Auto)));
    0
}

fn render(def: &CheckDef, term: &Term) -> String {
    let meta = &def.meta;
    let heading = |text: &str| term.paint(style::BOLD, text);
    let mut out = format!("{}\n{}\n\n", term.paint(style::BOLD, meta.code), meta.description);
    out.push_str(&format!("{}  {}\n", heading("severity   "), meta.severity.as_str()));
    out.push_str(&format!("{}  {}\n", heading("recommended"), if meta.recommended { "yes" } else { "no" }));
    let files = match meta.kind {
        SourceCodeType::LiquidHtml => "Liquid",
        SourceCodeType::Json => "JSON",
    };
    out.push_str(&format!("{}  {files}\n", heading("files      ")));
    if meta.deprecated {
        out.push_str(&format!("{}  yes\n", heading("deprecated ")));
    }
    if !meta.aliases.is_empty() {
        out.push_str(&format!("{}  {}\n", heading("aliases    "), meta.aliases.join(", ")));
    }
    if let Some(url) = meta.url {
        out.push_str(&format!("{}  {}\n", heading("docs       "), term.link(url, url)));
    }
    if !meta.schema.is_empty() {
        out.push_str(&format!("\n{}\n", heading("Settings, with their defaults:")));
        for (key, prop) in meta.schema {
            let default = prop.default.map_or_else(|| "(none)".to_string(), |d| d.to_value().to_json());
            out.push_str(&format!("  {key}: {default}\n"));
        }
    }
    out.push_str(&format!("\n{}\n", heading("To turn it off in .theme-check.yml:")));
    out.push_str(&format!("  {}:\n    enabled: false\n", meta.code));
    if meta.kind == SourceCodeType::LiquidHtml {
        out.push_str(&format!("\n{}\n", heading("To turn it off in a Liquid file:")));
        out.push_str(&format!("  {{% # theme-check-disable-next-line {} %}}\n", meta.code));
        out.push_str(&format!("  {{% # theme-check-disable {} %}}\n", meta.code));
        out.push_str(&format!("  {{% # theme-check-enable {} %}}\n", meta.code));
    }
    out
}
