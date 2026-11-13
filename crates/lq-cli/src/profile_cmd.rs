//! `lq profile`: estimate a theme's Liquid render time offline, against a mock shop.

use lq_render::calibrate::{self, Sample};
use lq_render::{report, speedscope, CostModel, Knobs, Limits, PageRequest, Shop, Theme};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const HELP: &str = "\
Usage: lq profile [options]
       lq profile init [--path <theme>] [--set <name>=<value>]... [--force]
       lq profile calibrate [--path <theme>] <page>=<speedscope.json>...
       lq profile analyze <speedscope.json>

Estimates how long Shopify's renderer spends executing a page's Liquid, without a store or a
network. lq renders the page against a mock shop and counts what each tag and output does:
lookups, filters, loop iterations, snippet renders, and reads Shopify serves from its database
(metafields, collection products, all_products[...], ...). A cost model turns the counts into
time. The profile has the shape of `shopify theme profile`: one frame per rendered tag or output.

The built-in costs are estimates. Calibrate them against a few real profiles to get closer to
your store: rankings and before/after differences are reliable well before absolute times are.

Options:
  --path <dir>          Theme root (default: .)
  --page <type>         Page to render: index, product, collection, list-collections, cart,
                        search, page, blog, article, 404, or a suffixed template such as
                        product.alternate (default: index)
  --all                 Profile every page type the theme has a template for
  --handle <handle>     The product, collection, page or article to render (default: the first)
  --shop <file>         Mock shop (default: <path>/.lq/shop.json, else a generated one)
  --costs <file>        Cost model (default: <path>/.lq/profile-costs.json over the built-in one)
  --speedscope <file>   Also write the profile in Speedscope format (opens in speedscope.app)
  --html <file>         Also write the rendered page
  --compare <rev|dir>   Compare with the theme at a git revision or in another directory
  --output <format>     text (default) or json
  --top <n>             Rows per table (default: 15)
  --max-nodes <n>       Stop rendering after n nodes (default: 2000000)
  -h, --help            Print this help

init writes <path>/.lq/shop.json, a mock shop to edit or resize. Sizes (with --set):
  products, options, values_per_option, images_per_product, collections, collection_size,
  metafields_per_product, tags_per_product, cart_items, articles, countries, languages,
  menu_links, menu_children, filters, logged_in

calibrate reads real profiles, captured once with
  shopify theme profile --url <path> --json > real/product.json
and writes <path>/.lq/profile-costs.json and <path>/.lq/profile-shapes.json (loop counts seen on the
real store). Keep real profiles out of version control if they come from a client's store.
";

struct Args {
    path: PathBuf,
    pages: Vec<String>,
    all: bool,
    handle: Option<String>,
    shop: Option<PathBuf>,
    costs: Option<PathBuf>,
    speedscope: Option<PathBuf>,
    html: Option<PathBuf>,
    compare: Option<String>,
    json: bool,
    top: usize,
    max_nodes: u64,
    sets: Vec<String>,
    force: bool,
    positional: Vec<String>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args {
        path: PathBuf::from("."),
        pages: Vec::new(),
        all: false,
        handle: None,
        shop: None,
        costs: None,
        speedscope: None,
        html: None,
        compare: None,
        json: false,
        top: 15,
        max_nodes: Limits::default().max_nodes,
        sets: Vec::new(),
        force: false,
        positional: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        let mut value = |name: &str| -> Result<String, String> {
            inline.clone().or_else(|| it.next().cloned()).ok_or_else(|| format!("{name} needs a value"))
        };
        match flag {
            "--path" => a.path = PathBuf::from(value("--path")?),
            "--page" => a.pages.push(value("--page")?),
            "--all" => a.all = true,
            "--handle" => a.handle = Some(value("--handle")?),
            "--shop" => a.shop = Some(PathBuf::from(value("--shop")?)),
            "--costs" => a.costs = Some(PathBuf::from(value("--costs")?)),
            "--speedscope" => a.speedscope = Some(PathBuf::from(value("--speedscope")?)),
            "--html" => a.html = Some(PathBuf::from(value("--html")?)),
            "--compare" => a.compare = Some(value("--compare")?),
            "--output" => match value("--output")?.as_str() {
                "json" => a.json = true,
                "text" => a.json = false,
                other => return Err(format!("unknown --output '{other}': use text or json")),
            },
            "--top" => a.top = value("--top")?.parse().map_err(|_| "--top needs a number")?,
            "--max-nodes" => a.max_nodes = value("--max-nodes")?.parse().map_err(|_| "--max-nodes needs a number")?,
            "--set" => a.sets.push(value("--set")?),
            "--force" => a.force = true,
            f if f.starts_with('-') => return Err(format!("unknown option {f}")),
            _ => a.positional.push(arg.clone()),
        }
    }
    Ok(a)
}

pub fn profile(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{HELP}");
        return 0;
    }
    let (sub, rest) = match args.first().map(String::as_str) {
        Some(s @ ("init" | "calibrate" | "analyze")) => (s, &args[1..]),
        _ => ("run", args),
    };
    let a = match parse(rest) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("lq profile: {e}");
            return 2;
        }
    };
    let result = match sub {
        "init" => init(&a),
        "calibrate" => run_calibrate(&a),
        "analyze" => analyze(&a),
        _ => run(&a),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("lq profile: {e}");
            2
        }
    }
}

fn lq_dir(a: &Args) -> PathBuf {
    a.path.join(".lq")
}

fn load_shop(a: &Args) -> Result<(Shop, String), String> {
    let path = a.shop.clone().unwrap_or_else(|| lq_dir(a).join("shop.json"));
    if path.exists() {
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        return Ok((Shop::load(&text)?, path.display().to_string()));
    }
    if a.shop.is_some() {
        return Err(format!("{} doesn't exist", path.display()));
    }
    Ok((Shop::from_json(&lq_render::shop::generate(&Knobs::default())), "generated (run `lq profile init` to edit it)".into()))
}

fn load_costs(a: &Args) -> Result<CostModel, String> {
    let mut model = CostModel::builtin();
    let path = a.costs.clone().unwrap_or_else(|| lq_dir(a).join("profile-costs.json"));
    if path.exists() {
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        model.overlay(CostModel::from_json(&text, &path.display().to_string())?);
    } else if a.costs.is_some() {
        return Err(format!("{} doesn't exist", path.display()));
    }
    Ok(model)
}

fn load_shapes(a: &Args) -> HashMap<String, usize> {
    let path = lq_dir(a).join("profile-shapes.json");
    let Ok(text) = std::fs::read_to_string(path) else { return HashMap::new() };
    let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(&text) else { return HashMap::new() };
    m.into_iter().filter_map(|(k, v)| v.as_u64().map(|n| (k, n as usize))).collect()
}

fn limits(a: &Args) -> Limits {
    Limits { max_nodes: a.max_nodes, ..Limits::default() }
}

fn pages_for(a: &Args, theme: &Theme) -> Vec<String> {
    if a.all {
        return lq_render::PAGE_TYPES
            .iter()
            .filter(|p| theme.get(&format!("templates/{p}.json")).is_some() || theme.get(&format!("templates/{p}.liquid")).is_some())
            .map(|p| p.to_string())
            .collect();
    }
    if a.pages.is_empty() { vec!["index".into()] } else { a.pages.clone() }
}

fn run(a: &Args) -> Result<i32, String> {
    let theme = Theme::load(&a.path).map_err(|e| e.to_string())?;
    let (shop, shop_source) = load_shop(a)?;
    let model = load_costs(a)?;
    let shapes = load_shapes(a);
    let pages = pages_for(a, &theme);
    if a.compare.is_some() {
        return compare(a, &theme, &shop, &model, &shapes, &pages);
    }
    let mut json_pages = Vec::new();
    let mut texts = Vec::new();
    let mut table = Vec::new();
    for page in &pages {
        let req = PageRequest { template: page.clone(), handle: a.handle.clone() };
        let res = lq_render::render_page(&theme, &shop, &req, &model, limits(a), &shapes)?;
        let tree = speedscope::from_profile(&res.profile, &model);
        let summary = report::summarize(&tree);
        let db: Vec<(String, f64)> = res.profile.op_totals().into_iter().filter(|(k, _)| k.starts_with("db:")).collect();
        if let Some(path) = &a.speedscope {
            let path = per_page(path, page, pages.len());
            std::fs::write(&path, res.profile.to_speedscope(&model)).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        if let Some(path) = &a.html {
            let path = per_page(path, page, pages.len());
            std::fs::write(&path, &res.html).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        if a.json {
            let mut j = report::json(&summary);
            j["page"] = serde_json::json!(page);
            j["template"] = serde_json::json!(res.template_path);
            j["db_reads"] = db.iter().map(|(k, v)| serde_json::json!({"op": k, "count": v})).collect();
            j["errors"] = serde_json::json!(res.errors);
            j["unsupported"] = serde_json::json!(res.unsupported);
            json_pages.push(j);
        } else if pages.len() > 1 {
            table.push((page.clone(), summary.total_us, summary.nodes, db.iter().map(|d| d.1).sum::<f64>(), res.errors.len()));
        } else {
            let mut t = format!(
                "{page} ({}), {} nodes: estimated {} ms of Liquid\n",
                res.template_path,
                summary.nodes,
                report::ms(summary.total_us)
            );
            t.push_str(&format!("costs: {}{}; shop: {shop_source}\n\n", model.source, if model.calibrated { "" } else { " (uncalibrated)" }));
            t.push_str(&report::text(&summary, &report::TextOptions { top: a.top }));
            if !db.is_empty() {
                t.push_str("\nDatabase-backed reads\n");
                let mut db = db.clone();
                db.sort_by(|x, y| y.1.total_cmp(&x.1));
                for (k, v) in db.iter().take(a.top) {
                    t.push_str(&format!("  {:>6}  {}\n", v, k.trim_start_matches("db:")));
                }
            }
            if !res.unsupported.is_empty() {
                t.push_str(&format!("\nNot modelled, passed through: {}\n", res.unsupported.join(", ")));
            }
            if !res.errors.is_empty() {
                t.push_str(&format!("\nLiquid errors ({}):\n", res.errors.len()));
                for e in res.errors.iter().take(a.top) {
                    t.push_str(&format!("  {e}\n"));
                }
            }
            texts.push(t);
        }
    }
    if a.json {
        let out = serde_json::json!({
            "costs": model.source, "calibrated": model.calibrated, "shop": shop_source,
            "pages": json_pages,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else if !table.is_empty() {
        println!("costs: {}{}; shop: {shop_source}\n", model.source, if model.calibrated { "" } else { " (uncalibrated)" });
        println!("{:<22}{:>12}{:>10}{:>10}{:>8}", "page", "est. ms", "nodes", "db reads", "errors");
        for (p, us, nodes, db, errors) in table {
            println!("{:<22}{:>12}{:>10}{:>10}{:>8}", p, report::ms(us), nodes, db, errors);
        }
    } else {
        print!("{}", texts.join("\n"));
    }
    Ok(0)
}

fn per_page(path: &Path, page: &str, n: usize) -> PathBuf {
    if n <= 1 {
        return path.to_path_buf();
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    path.with_file_name(format!("{stem}.{page}{ext}"))
}

/// The theme at `spec`: another directory, or a git revision of the theme directory.
fn load_other(a: &Args, spec: &str) -> Result<Theme, String> {
    let as_dir = Path::new(spec);
    if as_dir.is_dir() {
        return Theme::load(as_dir).map_err(|e| e.to_string());
    }
    let git = |args: &[&str]| -> Result<Vec<u8>, String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&a.path)
            .args(args)
            .output()
            .map_err(|e| format!("git: {e}"))?;
        if !out.status.success() {
            return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(out.stdout)
    };
    // ls-tree prints paths relative to the theme directory; `rev:./path` reads them the same way.
    let list = git(&["ls-tree", "-r", "--name-only", spec, "--", "."])?;
    let mut files = Vec::new();
    for rel in String::from_utf8_lossy(&list).lines() {
        let top = rel.split('/').next().unwrap_or("");
        if !["layout", "templates", "sections", "snippets", "blocks", "config", "locales"].contains(&top) {
            continue;
        }
        if !(rel.ends_with(".liquid") || rel.ends_with(".json")) {
            continue;
        }
        let blob = git(&["show", &format!("{spec}:./{rel}")])?;
        files.push((rel.to_string(), String::from_utf8_lossy(&blob).to_string()));
    }
    if files.is_empty() {
        return Err(format!("no theme files at {spec}"));
    }
    Ok(Theme::from_files(files))
}

fn compare(
    a: &Args,
    theme: &Theme,
    shop: &Shop,
    model: &CostModel,
    shapes: &HashMap<String, usize>,
    pages: &[String],
) -> Result<i32, String> {
    let spec = a.compare.as_deref().unwrap();
    let base = load_other(a, spec)?;
    let mut json_pages = Vec::new();
    for page in pages {
        let req = PageRequest { template: page.clone(), handle: a.handle.clone() };
        let before = lq_render::render_page(&base, shop, &req, model, limits(a), shapes)?;
        let after = lq_render::render_page(theme, shop, &req, model, limits(a), shapes)?;
        let sb = report::summarize(&speedscope::from_profile(&before.profile, model));
        let sa = report::summarize(&speedscope::from_profile(&after.profile, model));
        if a.json {
            json_pages.push(serde_json::json!({
                "page": page, "base_ms": sb.total_us / 1000.0, "head_ms": sa.total_us / 1000.0,
                "base": report::json(&sb), "head": report::json(&sa),
            }));
        } else {
            println!("{page}: {spec} → working tree");
            print!("{}", report::diff_text(&sb, &sa, a.top));
        }
    }
    if a.json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({"compare": spec, "pages": json_pages})).unwrap());
    }
    Ok(0)
}

fn init(a: &Args) -> Result<i32, String> {
    let dir = lq_dir(a);
    let path = a.shop.clone().unwrap_or_else(|| dir.join("shop.json"));
    if path.exists() && !a.force {
        return Err(format!("{} exists; pass --force to replace it", path.display()));
    }
    let mut knobs = Knobs::default();
    for s in &a.sets {
        let (k, v) = s.split_once('=').ok_or_else(|| format!("--set needs name=value, got '{s}'"))?;
        knobs.set(k, v)?;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let shop = lq_render::shop::generate(&knobs);
    std::fs::write(&path, serde_json::to_string_pretty(&shop).unwrap() + "\n").map_err(|e| format!("{}: {e}", path.display()))?;
    println!("wrote {} ({} products, {} collections of {})", path.display(), knobs.products, knobs.collections, knobs.collection_size);
    Ok(0)
}

fn run_calibrate(a: &Args) -> Result<i32, String> {
    if a.positional.is_empty() {
        return Err("calibrate needs <page>=<speedscope.json> pairs, such as product=real/product.json".into());
    }
    let theme = Theme::load(&a.path).map_err(|e| e.to_string())?;
    let (shop, _) = load_shop(a)?;
    let model = load_costs(a)?;
    let mut samples = Vec::new();
    for p in &a.positional {
        let (page, file) = p.split_once('=').ok_or_else(|| format!("expected <page>=<file>, got '{p}'"))?;
        let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
        let real = speedscope::read(&text).map_err(|e| format!("{file}: {e}"))?;
        samples.push(Sample {
            page: PageRequest { template: page.to_string(), handle: a.handle.clone() },
            real,
            label: format!("{page} ({})", Path::new(file).file_name().unwrap_or_default().to_string_lossy()),
        });
    }
    let c = calibrate::calibrate(&theme, &shop, &samples, &model, limits(a), load_shapes(a))?;
    print!("{}", calibrate::report(&c));
    let dir = lq_dir(a);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let costs = a.costs.clone().unwrap_or_else(|| dir.join("profile-costs.json"));
    std::fs::write(&costs, c.model.to_json()).map_err(|e| format!("{}: {e}", costs.display()))?;
    let mut shapes: Vec<_> = c.shapes.iter().collect();
    shapes.sort();
    let shapes: serde_json::Map<String, serde_json::Value> = shapes.into_iter().map(|(k, v)| (k.clone(), serde_json::json!(v))).collect();
    let shapes_path = dir.join("profile-shapes.json");
    std::fs::write(&shapes_path, serde_json::to_string_pretty(&shapes).unwrap() + "\n").map_err(|e| format!("{}: {e}", shapes_path.display()))?;
    println!("\nwrote {} and {}", costs.display(), shapes_path.display());
    Ok(0)
}

fn analyze(a: &Args) -> Result<i32, String> {
    let [file] = a.positional.as_slice() else {
        return Err("analyze needs one Speedscope file".into());
    };
    let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
    let tree = speedscope::read(&text).map_err(|e| format!("{file}: {e}"))?;
    let summary = report::summarize(&tree);
    if a.json {
        println!("{}", serde_json::to_string_pretty(&report::json(&summary)).unwrap());
    } else {
        println!("{file}: {} nodes, {} ms\n", summary.nodes, report::ms(summary.total_us));
        print!("{}", report::text(&summary, &report::TextOptions { top: a.top }));
    }
    Ok(0)
}
