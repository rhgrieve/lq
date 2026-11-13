//! Calibration against real profiles from `shopify theme profile --json`.
//!
//! 1. **Shape.** Frames are matched by template and line. Where a loop ran a different number of
//!    times on the real store than against the mock shop, lq records the real count so later
//!    renders iterate as often (`loop_shapes`). This needs no timing, and runs until stable.
//! 2. **Cost.** With the shapes fixed, each matched line gives one equation: its real own time
//!    against lq's operation counts on that line, plus one per page total. Two models are fitted:
//!    a multiplier per kind of cost, and a cost per operation on top of it. With two or more
//!    pages, each page is estimated by a model fitted on the others, and the model that predicts
//!    those held-out pages better is kept. Fewer parameters usually win: they generalize.

use crate::cost::CostModel;
use crate::interp::Limits;
use crate::page::{render_page, PageRequest};
use crate::shop::Shop;
use crate::speedscope::{attribution, match_key, Tree};
use crate::theme::Theme;
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

pub struct Sample {
    pub page: PageRequest,
    pub real: Tree,
    pub label: String,
}

pub struct PageFit {
    pub label: String,
    pub real_ms: f64,
    pub before_ms: f64,
    pub after_ms: f64,
    /// Share of the real profile's own time on lines lq also rendered.
    pub matched_share: f64,
    pub spearman: f64,
    /// The same, with the costs calibration started from.
    pub spearman_before: f64,
    /// Rank correlation of section, block and snippet totals, with the final costs.
    pub section_rho: f64,
    /// This page's estimate from costs fitted on the other pages only.
    pub held_out_ms: Option<f64>,
    pub held_out_section_rho: Option<f64>,
    /// How fast this capture ran against the first, from lines doing the same work in both.
    pub speed: f64,
}

pub struct Calibration {
    pub model: CostModel,
    pub choice: Choice,
    /// The best pull for each model tried, with its mean held-out error on page totals.
    pub modes_tried: Vec<(Choice, f64)>,
    pub shapes: HashMap<String, usize>,
    pub pages: Vec<PageFit>,
    pub shape_changes: Vec<String>,
    pub unmatched: Vec<(String, f64)>,
    /// Mock shop sizes changed to match the captures, as `name: old → new`.
    pub knob_changes: Vec<String>,
    /// The refitted mock shop, when its sizes changed.
    pub shop_json: Option<serde_json::Value>,
}

/// Which mock shop size a loop over `expr` measures, if any.
fn knob_for(expr: &str) -> Option<&'static str> {
    let e = expr.split('|').next().unwrap_or("").trim();
    let segs: Vec<&str> = e.split(['.', '[']).map(|s| s.trim_matches([']', '\'', '"', ' '])).filter(|s| !s.is_empty()).collect();
    let last = *segs.last()?;
    let parent = if segs.len() >= 2 { segs[segs.len() - 2] } else { "" };
    let product_like = parent.contains("product") || parent == "item";
    Some(match last {
        "variants" if product_like => "variants",
        "images" | "media" if product_like => "images_per_product",
        "products" if parent.contains("collection") => "collection_size",
        "options_with_values" if product_like => "options",
        "values" if parent.contains("option") => "values_per_option",
        "filters" => "filters",
        // `link.links` is a submenu; `menu.links` or `linklists.main-menu.links` a top-level menu.
        "links" if parent.ends_with("link") && !parent.ends_with("linklist") => "menu_children",
        "links" => "menu_links",
        "available_countries" => "countries",
        "available_languages" => "languages",
        "items" if parent == "cart" => "cart_items",
        "tags" if product_like => "tags_per_product",
        "articles" => "articles",
        _ => return None,
    })
}

type FittedShop = (Shop, serde_json::Value);

/// Refits the mock shop's sizes from the captures' loop counts. Returns the new shop and its JSON
/// when anything changed, and a line per change.
fn fit_knobs(
    theme: &Theme,
    shop: &Shop,
    samples: &[Sample],
    prior: &CostModel,
    limits: Limits,
) -> Result<(Option<FittedShop>, Vec<String>), String> {
    let mut knobs = shop.knobs.clone();
    let mut current: Option<Shop> = None;
    let mut changes = Vec::new();
    // Two rounds: a richer shop can reach loops (colour swatches, submenus) the first one skipped.
    for _round in 0..2 {
        let mut seen: HashMap<&'static str, f64> = HashMap::new();
        for s in samples {
            let res = render_page(theme, current.as_ref().unwrap_or(shop), &s.page, prior, limits, &HashMap::new())?;
            let sim = crate::speedscope::from_profile(&res.profile, prior);
            let real_iters = loop_iterations(&s.real, is_loop);
            let mut sim_keys: HashMap<Key, String> = HashMap::new();
            for f in &sim.frames {
                if is_loop(&f.name) {
                    sim_keys.insert(match_key(f), format!("{}:{}", f.file, f.line));
                }
            }
            for (key, loc) in &sim_keys {
                let (Some(&n), Some(expr)) = (real_iters.get(key), res.loop_sources.get(loc)) else { continue };
                if let Some(knob) = knob_for(expr) {
                    // The largest per-call count: loops with `limit:` or filters only see part of a list.
                    let e = seen.entry(knob).or_default();
                    *e = e.max(n);
                }
            }
        }
        let mut next = knobs.clone();
        let set = |v: f64, max: usize| (v.round() as usize).clamp(0, max);
        if let Some(&o) = seen.get("options") {
            next.options = set(o, 3);
        }
        if let Some(&v) = seen.get("values_per_option") {
            next.values_per_option = set(v, 100).max(1);
        }
        if let Some(&v) = seen.get("variants") {
            if !seen.contains_key("values_per_option") {
                let opts = next.options.max(1) as f64;
                next.values_per_option = set(v.powf(1.0 / opts).ceil(), 100).max(1);
            }
            next.variants_per_product = set(v, 2000).max(1);
        }
        for (knob, field, max) in [
            ("images_per_product", &mut next.images_per_product as &mut usize, 250usize),
            ("collection_size", &mut next.collection_size, 250),
            ("filters", &mut next.filters, 20),
            ("menu_links", &mut next.menu_links, 50),
            ("menu_children", &mut next.menu_children, 50),
            ("countries", &mut next.countries, 250),
            ("languages", &mut next.languages, 50),
            ("cart_items", &mut next.cart_items, 100),
            ("tags_per_product", &mut next.tags_per_product, 100),
            ("articles", &mut next.articles, 250),
        ] {
            if let Some(&v) = seen.get(knob) {
                *field = set(v, max);
            }
        }
        next.products = next.products.max(next.collection_size * 2);
        let (a, b) = (knobs.to_json(), next.to_json());
        if a == b {
            break;
        }
        for (k, v) in b.as_object().unwrap() {
            if a[k] != *v {
                changes.retain(|c: &String| !c.starts_with(&format!("{k}: ")));
                let was = shop.knobs.to_json()[k].clone();
                changes.push(format!("{k}: {was} → {v}"));
            }
        }
        knobs = next;
        let json = crate::shop::generate(&knobs);
        current = Some(Shop::from_json(&json));
    }
    if changes.is_empty() {
        return Ok((None, changes));
    }
    let json = crate::shop::generate(&knobs);
    Ok((Some((Shop::from_json(&json), json)), changes))
}

type Key = (String, u32);

/// Lines an op must appear on before its cost is fitted freely.
const MIN_SUPPORT: usize = 8;

/// Average iterations per call of each loop, from a tree: the most frequent direct child line.
fn loop_iterations(tree: &Tree, is_loop: impl Fn(&str) -> bool) -> HashMap<Key, f64> {
    let mut acc: HashMap<Key, (f64, f64)> = HashMap::new();
    for node in &tree.nodes {
        let f = &tree.frames[node.frame as usize];
        if !is_loop(&f.name) {
            continue;
        }
        let mut counts: HashMap<Key, usize> = HashMap::new();
        for &c in &node.children {
            *counts.entry(match_key(&tree.frames[tree.nodes[c as usize].frame as usize])).or_default() += 1;
        }
        let iters = counts.values().copied().max().unwrap_or(0) as f64;
        let e = acc.entry(match_key(f)).or_default();
        e.0 += iters;
        e.1 += 1.0;
    }
    acc.into_iter().map(|(k, (sum, n))| (k, sum / n)).collect()
}

fn is_loop(name: &str) -> bool {
    matches!(name, "For" | "TableRow")
}

fn spearman(pairs: &[(f64, f64)]) -> f64 {
    if pairs.len() < 3 {
        return f64::NAN;
    }
    let rank = |vals: Vec<f64>| -> Vec<f64> {
        let mut idx: Vec<usize> = (0..vals.len()).collect();
        idx.sort_by(|&a, &b| vals[a].total_cmp(&vals[b]));
        let mut r = vec![0.0; vals.len()];
        let mut i = 0;
        while i < idx.len() {
            let mut j = i;
            while j + 1 < idx.len() && vals[idx[j + 1]] == vals[idx[i]] {
                j += 1;
            }
            let avg = (i + j) as f64 / 2.0;
            for k in i..=j {
                r[idx[k]] = avg;
            }
            i = j + 1;
        }
        r
    };
    let a = rank(pairs.iter().map(|p| p.0).collect());
    let b = rank(pairs.iter().map(|p| p.1).collect());
    let n = a.len() as f64;
    let ma = a.iter().sum::<f64>() / n;
    let mb = b.iter().sum::<f64>() / n;
    let cov: f64 = a.iter().zip(&b).map(|(x, y)| (x - ma) * (y - mb)).sum();
    let va: f64 = a.iter().map(|x| (x - ma).powi(2)).sum();
    let vb: f64 = b.iter().map(|y| (y - mb).powi(2)).sum();
    if va == 0.0 || vb == 0.0 { f64::NAN } else { cov / (va * vb).sqrt() }
}

/// Calls per line, counting only frames that have a line.
fn calls_by_key(tree: &Tree) -> HashMap<Key, u32> {
    let mut m: HashMap<Key, u32> = HashMap::new();
    for n in &tree.nodes {
        let f = &tree.frames[n.frame as usize];
        if f.line > 0 {
            *m.entry(match_key(f)).or_default() += 1;
        }
    }
    m
}

/// Shopify's renderer runs some requests faster than others: the same line with the same calls
/// can take 1.5× as long in one capture as in another. Each capture's speed relative to the
/// first is the median time ratio over such lines, or 1 when they share too few.
fn capture_speeds(samples: &[Sample]) -> Vec<f64> {
    let Some(first) = samples.first() else { return Vec::new() };
    let (ref_self, ref_calls) = (self_by_key(&first.real), calls_by_key(&first.real));
    samples
        .iter()
        .map(|s| {
            let (own, calls) = (self_by_key(&s.real), calls_by_key(&s.real));
            let mut ratios: Vec<f64> = ref_self
                .iter()
                .filter(|(k, v)| k.1 > 0 && **v >= 200.0 && calls.get(*k) == ref_calls.get(*k))
                .filter_map(|(k, v)| own.get(k).filter(|o| **o > 0.0).map(|o| o / v))
                .collect();
            if ratios.len() < 10 {
                return 1.0;
            }
            ratios.sort_by(f64::total_cmp);
            ratios[ratios.len() / 2]
        })
        .collect()
}

/// Own time per matched line, summed over the tree.
fn self_by_key(tree: &Tree) -> HashMap<Key, f64> {
    let keys = attribution(tree);
    let mut m: HashMap<Key, f64> = HashMap::new();
    for (i, n) in tree.nodes.iter().enumerate() {
        *m.entry(keys[i].clone()).or_default() += n.self_us;
    }
    m
}

pub fn calibrate(
    theme: &Theme,
    shop: &Shop,
    samples: &[Sample],
    prior: &CostModel,
    limits: Limits,
    shapes: HashMap<String, usize>,
    force: Option<Mode>,
) -> Result<Calibration, String> {
    let mut shapes = shapes;
    let mut shape_changes = Vec::new();

    // 0. Data sizes: refit the mock shop so its products, menus and lists are as big as the real
    // store's, judging by how often loops over them ran.
    let (fitted, knob_changes) = fit_knobs(theme, shop, samples, prior, limits)?;
    let shop = fitted.as_ref().map_or(shop, |(s, _)| s);

    // 1. Shapes.
    for _round in 0..4 {
        let mut changed = false;
        for s in samples {
            let res = render_page(theme, shop, &s.page, prior, limits, &shapes)?;
            let sim = crate::speedscope::from_profile(&res.profile, prior);
            let real_iters = loop_iterations(&s.real, is_loop);
            let sim_iters = loop_iterations(&sim, is_loop);
            // Loop frames in lq's tree, keyed back to their full file path for `loop_shapes`.
            let mut sim_files: HashMap<Key, String> = HashMap::new();
            for f in &sim.frames {
                if is_loop(&f.name) {
                    sim_files.insert(match_key(f), format!("{}:{}", f.file, f.line));
                }
            }
            for (key, sim_n) in &sim_iters {
                let Some(&real_n) = real_iters.get(key) else { continue };
                let Some(loc) = sim_files.get(key) else { continue };
                let want = real_n.round() as usize;
                if want != sim_n.round() as usize && shapes.get(loc) != Some(&want) {
                    shape_changes.push(format!("{loc}: {sim_n:.1} → {want} iterations ({})", s.label));
                    shapes.insert(loc.clone(), want);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // 2. Costs. Render each page once with the final shop and shapes, and collect its equations.
    let speeds = capture_speeds(samples);
    let mut data = Vec::new();
    let mut unmatched_acc: HashMap<String, f64> = HashMap::new();
    for (si, s) in samples.iter().enumerate() {
        data.push(sample_data(theme, shop, s, prior, limits, &shapes, speeds[si], &mut unmatched_acc)?);
    }

    // 3. Choose the model, and how strongly to pull costs towards the defaults, by how well each
    // predicts pages it wasn't fitted on (leave one page out, for every page).
    let modes: Vec<Mode> = match force {
        Some(m) => vec![m],
        None => vec![Mode::Grouped, Mode::PerOp],
    };
    let mut held_out: Vec<(Choice, Vec<Eval>)> = Vec::new();
    if samples.len() >= 2 {
        for &mode in &modes {
            for pull in PULLS {
                let choice = Choice { mode, pull };
                let evals = (0..samples.len())
                    .map(|i| {
                        let train: Vec<&SampleData> = data.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, d)| d).collect();
                        let m = fit(prior, &train, choice);
                        evaluate(&samples[i], &data[i], &m)
                    })
                    .collect();
                held_out.push((choice, evals));
            }
        }
    }
    let score = |evals: &[Eval]| evals.iter().map(|e| e.total_error.abs()).sum::<f64>() / evals.len().max(1) as f64;
    // Ties go to the simpler model and the stronger pull.
    let choice = held_out
        .iter()
        .min_by(|a, b| {
            score(&a.1)
                .total_cmp(&score(&b.1))
                .then((a.0.mode == Mode::PerOp).cmp(&(b.0.mode == Mode::PerOp)))
                .then(b.0.pull.total_cmp(&a.0.pull))
        })
        .map_or(Choice { mode: modes[0], pull: 0.5 }, |h| h.0);
    let all: Vec<&SampleData> = data.iter().collect();
    let mut model = fit(prior, &all, choice);
    model.calibrated = true;
    model.source = format!("calibrated on {} profile(s), {}", samples.len(), choice.describe());

    // 4. Fit report.
    let chosen = held_out.iter().find(|h| h.0 == choice).map(|h| &h.1);
    let mut pages = Vec::new();
    for (i, s) in samples.iter().enumerate() {
        let before = evaluate(s, &data[i], prior);
        let after = evaluate(s, &data[i], &model);
        let ho = chosen.map(|c| &c[i]);
        pages.push(PageFit {
            label: s.label.clone(),
            real_ms: s.real.total_us() / 1000.0,
            before_ms: before.estimate_us / 1000.0,
            after_ms: after.estimate_us / 1000.0,
            held_out_ms: ho.map(|e| e.estimate_us / 1000.0),
            matched_share: data[i].matched_share,
            spearman: after.line_rho,
            spearman_before: before.line_rho,
            section_rho: after.section_rho,
            held_out_section_rho: ho.map(|e| e.section_rho),
            speed: data[i].speed,
        });
    }
    // The best pull for each model, for the report.
    let mut modes_tried: Vec<(Choice, f64)> = Vec::new();
    for &mode in &modes {
        if let Some(best) = held_out.iter().filter(|h| h.0.mode == mode).min_by(|a, b| score(&a.1).total_cmp(&score(&b.1))) {
            modes_tried.push((best.0, score(&best.1)));
        }
    }
    let mut unmatched: Vec<(String, f64)> = unmatched_acc.into_iter().collect();
    unmatched.sort_by(|a, b| b.1.total_cmp(&a.1));
    unmatched.truncate(15);
    let shop_json = fitted.map(|(_, j)| j);
    Ok(Calibration { model, choice, modes_tried, shapes, pages, shape_changes, unmatched, knob_changes, shop_json })
}

/// A model and how strongly its costs are pulled towards where they started.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Choice {
    pub mode: Mode,
    pub pull: f64,
}

impl Choice {
    pub fn describe(&self) -> String {
        let pull = match self.pull {
            p if p <= 0.03 => "light",
            p if p <= 0.2 => "medium",
            p if p <= 1.0 => "strong",
            _ => "very strong",
        };
        format!("{}, {pull} pull to the defaults", self.mode.describe())
    }
}

const PULLS: [f64; 4] = [0.02, 0.1, 0.5, 3.0];

/// How a cost model is fitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// One multiplier per kind of cost, keeping the built-in ratios within each: few parameters,
    /// so it holds up on pages it wasn't fitted on.
    Grouped,
    /// The grouped fit, then each operation's own cost, kept close to it.
    PerOp,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "grouped" => Some(Mode::Grouped),
            "per-op" => Some(Mode::PerOp),
            _ => None,
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Mode::Grouped => "one multiplier per kind of cost",
            Mode::PerOp => "a cost per operation",
        }
    }
}

/// The kind of cost an operation is, for the grouped fit.
pub fn op_group(op: &str) -> &'static str {
    if ["db:", "load:", "preload:"].iter().any(|p| op.starts_with(p)) {
        "database"
    } else if op.starts_with("global:") {
        "platform"
    } else if op.starts_with("filter:") {
        "filters"
    } else if matches!(op, "template_load" | "template" | "section" | "render" | "include" | "block") {
        "templates"
    } else if matches!(op, "output_byte" | "string_byte") {
        "output"
    } else {
        "logic"
    }
}

type Row = (HashMap<String, f64>, f64);

/// One page's equations: a row per matched line (lq's op counts against the real own time) and a
/// row for the page total, with times divided by the capture's speed.
struct SampleData {
    rows: Vec<Row>,
    total: Row,
    profile: crate::Profile,
    matched_share: f64,
    speed: f64,
}

#[allow(clippy::too_many_arguments)]
fn sample_data(
    theme: &Theme,
    shop: &Shop,
    s: &Sample,
    prior: &CostModel,
    limits: Limits,
    shapes: &HashMap<String, usize>,
    speed: f64,
    unmatched_acc: &mut HashMap<String, f64>,
) -> Result<SampleData, String> {
    let res = render_page(theme, shop, &s.page, prior, limits, shapes)?;
    let p = &res.profile;
    let keys = attribution(&crate::speedscope::from_profile(p, prior));
    let mut counts: HashMap<Key, HashMap<String, f64>> = HashMap::new();
    for (i, n) in p.nodes.iter().enumerate() {
        let e = counts.entry(keys[i].clone()).or_default();
        for &(op, c) in &n.counts {
            *e.entry(p.ops.name(op).to_string()).or_default() += c;
        }
    }
    let real = self_by_key(&s.real);
    let real_total: f64 = real.values().sum();
    let mut rows = Vec::new();
    let mut matched = 0.0;
    for (key, y) in &real {
        match counts.get(key) {
            Some(x) => {
                rows.push((x.clone(), *y / speed));
                matched += y;
            }
            None => {
                // Frames lq never rendered: app blocks, Shopify's own wrappers, untouched branches.
                let f = s.real.frames.iter().find(|f| &match_key(f) == key);
                let label = match f {
                    Some(f) if f.line > 0 => format!("{}:{}  {}", f.file, f.line, f.name),
                    _ if key.0.is_empty() => "(page wrappers: json_template, raw_section)".to_string(),
                    _ => key.0.clone(),
                };
                *unmatched_acc.entry(label).or_default() += y;
            }
        }
    }
    let mut all: HashMap<String, f64> = HashMap::new();
    for x in counts.values() {
        for (op, c) in x {
            *all.entry(op.clone()).or_default() += c;
        }
    }
    Ok(SampleData {
        rows,
        total: (all, s.real.total_us() / speed),
        profile: res.profile,
        matched_share: if real_total > 0.0 { matched / real_total } else { 0.0 },
        speed,
    })
}

/// Non-negative least squares with a pull towards `prior` and per-column bounds, by coordinate
/// descent. `rows` are dense over the columns.
fn solve(rows: &[(Vec<f64>, f64)], prior: &[f64], lambda: &[f64], lo: &[f64], hi: &[f64]) -> Vec<f64> {
    let n = prior.len();
    let mut c = prior.to_vec();
    let diag: Vec<f64> = (0..n).map(|j| rows.iter().map(|(x, _)| x[j] * x[j]).sum()).collect();
    let mut resid: Vec<f64> = rows.iter().map(|(x, y)| x.iter().zip(&c).map(|(a, b)| a * b).sum::<f64>() - y).collect();
    for _sweep in 0..500 {
        let mut moved = 0.0f64;
        for j in 0..n {
            if diag[j] == 0.0 {
                continue;
            }
            let mut num = lambda[j] * prior[j];
            for (i, (x, _)) in rows.iter().enumerate() {
                if x[j] != 0.0 {
                    num -= x[j] * (resid[i] - x[j] * c[j]);
                }
            }
            let new = (num / (diag[j] + lambda[j])).clamp(lo[j], hi[j]);
            let delta = new - c[j];
            if delta != 0.0 {
                for (i, (x, _)) in rows.iter().enumerate() {
                    resid[i] += x[j] * delta;
                }
                moved = moved.max(delta.abs() / (c[j].abs() + 1e-9));
                c[j] = new;
            }
        }
        if moved < 1e-7 {
            break;
        }
    }
    c
}

/// Fits a cost model to some pages' equations.
fn fit(prior: &CostModel, data: &[&SampleData], choice: Choice) -> CostModel {
    let (mode, pull) = (choice.mode, choice.pull);
    let mut rows: Vec<&Row> = data.iter().flat_map(|d| d.rows.iter()).collect();
    // Each page's total is one more equation, weighted so the fit can't trade a page's total away
    // for a closer fit on individual lines.
    let weight = (rows.len() as f64).sqrt().max(1.0);
    let totals: Vec<Row> = data
        .iter()
        .map(|d| (d.total.0.iter().map(|(k, v)| (k.clone(), v * weight)).collect(), d.total.1 * weight))
        .collect();
    rows.extend(totals.iter());

    // Stage 1: a multiplier per group, over the prior's own costs.
    let groups = ["logic", "filters", "output", "templates", "database", "platform"];
    let dense: Vec<(Vec<f64>, f64)> = rows
        .iter()
        .map(|(x, y)| {
            let mut v = vec![0.0; groups.len()];
            for (op, c) in x {
                let g = groups.iter().position(|g| *g == op_group(op)).unwrap();
                v[g] += c * prior.coefficient(op);
            }
            (v, *y)
        })
        .collect();
    let ones = vec![1.0; groups.len()];
    let diag: Vec<f64> = (0..groups.len()).map(|j| dense.iter().map(|(x, _)| x[j] * x[j]).sum()).collect();
    let lambda: Vec<f64> = diag.iter().map(|d| pull * d + 1e-9).collect();
    let m = solve(&dense, &ones, &lambda, &vec![0.05; groups.len()], &vec![20.0; groups.len()]);
    let mut model = prior.clone();
    let ops: Vec<String> = rows.iter().flat_map(|(x, _)| x.keys().cloned()).collect::<BTreeSet<_>>().into_iter().collect();
    for op in &ops {
        let g = groups.iter().position(|g| *g == op_group(op)).unwrap();
        model.coefficients.insert(op.clone(), prior.coefficient(op) * m[g]);
    }
    if mode == Mode::Grouped {
        return model;
    }

    // Stage 2: each op's cost, starting from and pulled towards the grouped fit, within bounds.
    let base: Vec<f64> = ops.iter().map(|o| model.coefficient(o)).collect();
    let dense: Vec<(Vec<f64>, f64)> =
        rows.iter().map(|(x, y)| (ops.iter().map(|o| x.get(o).copied().unwrap_or(0.0)).collect(), *y)).collect();
    let diag: Vec<f64> = (0..ops.len()).map(|j| dense.iter().map(|(x, _)| x[j] * x[j]).sum()).collect();
    // An op seen on only a few lines can't be told apart from whatever else those lines do, so it
    // stays close to the grouped fit. On a tiny profile every op is rare, so the bar scales down.
    let min_support = MIN_SUPPORT.min((rows.len() / 3).max(1));
    let support: Vec<usize> = (0..ops.len()).map(|j| dense.iter().filter(|(x, _)| x[j] != 0.0).count()).collect();
    let lambda: Vec<f64> =
        diag.iter().zip(&support).map(|(d, &s)| if s >= min_support { (pull * 5.0).max(0.1) * d + 1e-9 } else { 20.0 * d + 1e-9 }).collect();
    // How far each cost may move. Computation (filters, lookups, iterations) behaves alike across
    // stores; database work and loading vary more. Bounds stop the fit from handing time it can't
    // explain to whichever cheap op shares a line.
    let (lo, hi): (Vec<f64>, Vec<f64>) = ops
        .iter()
        .zip(&base)
        .map(|(op, &b)| {
            let f = if matches!(op_group(op), "database" | "platform" | "templates") { 4.0 } else { 3.0 };
            (b / f, (b * f).max(1e-3))
        })
        .unzip();
    let c = solve(&dense, &base, &lambda, &lo, &hi);
    for (j, op) in ops.iter().enumerate() {
        model.coefficients.insert(op.clone(), c[j]);
    }
    model
}

/// How a model does on one page: its estimate (at the capture's speed) and the error against the
/// real total, and how well it ranks sections and snippets, and lines.
pub struct Eval {
    pub estimate_us: f64,
    pub total_error: f64,
    pub section_rho: f64,
    pub line_rho: f64,
}

fn evaluate(s: &Sample, d: &SampleData, model: &CostModel) -> Eval {
    let sim = crate::speedscope::from_profile(&d.profile, model);
    let estimate_us = sim.total_us() * d.speed;
    let real_us = s.real.total_us();
    // Lines: every line with at least 0.1% of the real time; one lq didn't render counts as zero.
    let real = self_by_key(&s.real);
    let sim_self = self_by_key(&sim);
    let pairs: Vec<(f64, f64)> = real
        .iter()
        .filter(|(k, v)| **v >= 0.001 * real_us && k.1 > 0)
        .map(|(k, v)| (*v, sim_self.get(k).copied().unwrap_or(0.0)))
        .collect();
    // Sections, blocks and snippets: every one with at least 0.2% of the real time.
    let (rs, ss) = (crate::report::summarize(&s.real, None), crate::report::summarize(&sim, None));
    let named = |sm: &crate::report::Summary| -> HashMap<String, f64> {
        let mut m = HashMap::new();
        for (kind, rows) in [("section", &sm.sections), ("block", &sm.blocks), ("snippet", &sm.snippets)] {
            for r in rows {
                m.insert(format!("{kind}:{}", r.key), r.total_us);
            }
        }
        m
    };
    let (rn, sn) = (named(&rs), named(&ss));
    let sec: Vec<(f64, f64)> =
        rn.iter().filter(|(_, v)| **v >= 0.002 * real_us).map(|(k, v)| (*v, sn.get(k).copied().unwrap_or(0.0) * d.speed)).collect();
    Eval {
        estimate_us,
        total_error: if real_us > 0.0 { estimate_us / real_us - 1.0 } else { 0.0 },
        section_rho: spearman(&sec),
        line_rho: spearman(&pairs),
    }
}

fn rho_text(r: f64) -> String {
    if r.is_nan() { "-".to_string() } else { format!("{r:.2}") }
}

pub fn report(c: &Calibration) -> String {
    let mut o = String::new();
    let held = c.pages.iter().any(|p| p.held_out_ms.is_some());
    let _ = writeln!(
        o,
        "{:<34}{:>7}{:>9}{:>9}{:>10}{:>9}{:>9}{:>11}{:>9}",
        "page", "speed", "real ms", "before", "held out", "after", "matched", "sections ρ", "lines ρ"
    );
    for p in &c.pages {
        let label: String = if p.label.chars().count() > 33 { p.label.chars().take(32).collect::<String>() + "…" } else { p.label.clone() };
        let _ = writeln!(
            o,
            "{:<34}{:>7.2}{:>9.1}{:>9.1}{:>10}{:>9.1}{:>8.0}%{:>11}{:>9}",
            label,
            p.speed,
            p.real_ms,
            p.before_ms,
            p.held_out_ms.map_or("-".to_string(), |h| format!("{h:.1}")),
            p.after_ms,
            p.matched_share * 100.0,
            rho_text(p.held_out_section_rho.unwrap_or(p.section_rho)),
            rho_text(p.spearman),
        );
    }
    let _ = writeln!(o, "\nmodel: {}.", c.choice.describe());
    if held {
        let _ = writeln!(
            o,
            "held out: each page estimated by costs fitted on the other pages only, which is how the costs\n\
             will do on pages you haven't captured. sections ρ is held out too. Mean held-out error:"
        );
        for (m, err) in &c.modes_tried {
            let _ = writeln!(o, "  {:<58}{:>5.0}%{}", m.describe(), err * 100.0, if *m == c.choice { "  (chosen)" } else { "" });
        }
    } else {
        let _ = writeln!(o, "Capture two or more pages to check how the costs do on pages they weren't fitted on.");
    }
    if c.pages.len() > 1 {
        let _ = writeln!(o, "speed: how fast each capture ran against the first, from lines doing the same work. Times are\nscaled by it, as captures of the same code vary.");
    }
    if !c.knob_changes.is_empty() {
        let _ = writeln!(o, "\nMock shop sizes taken from the real profiles' loops:");
        for k in &c.knob_changes {
            let _ = writeln!(o, "  {k}");
        }
    }
    if !c.shape_changes.is_empty() {
        let _ = writeln!(o, "\nLoop counts taken from the real profiles:");
        for s in c.shape_changes.iter().take(20) {
            let _ = writeln!(o, "  {s}");
        }
        if c.shape_changes.len() > 20 {
            let _ = writeln!(o, "  ... and {} more", c.shape_changes.len() - 20);
        }
    }
    if !c.unmatched.is_empty() {
        let _ = writeln!(o, "\nReal time lq didn't render (app blocks, Shopify wrappers, branches the mock shop skips):");
        for (k, us) in &c.unmatched {
            let _ = writeln!(o, "  {:>8} ms  {}", crate::report::ms(*us), k);
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loops_map_to_shop_sizes() {
        assert_eq!(knob_for("product.variants"), Some("variants"));
        assert_eq!(knob_for("card_product.media"), Some("images_per_product"));
        assert_eq!(knob_for("collection.products"), Some("collection_size"));
        assert_eq!(knob_for("option.values"), Some("values_per_option"));
        assert_eq!(knob_for("link.links"), Some("menu_children"));
        assert_eq!(knob_for("section.settings.menu.links"), Some("menu_links"));
        assert_eq!(knob_for("linklists['main-menu'].links"), Some("menu_links"));
        assert_eq!(knob_for("localization.available_countries | sort: 'name'"), Some("countries"));
        assert_eq!(knob_for("section.blocks"), None);
    }

    #[test]
    fn spearman_ranks() {
        assert!((spearman(&[(1.0, 10.0), (2.0, 20.0), (3.0, 35.0)]) - 1.0).abs() < 1e-9);
        assert!((spearman(&[(1.0, 3.0), (2.0, 2.0), (3.0, 1.0)]) + 1.0).abs() < 1e-9);
    }
}
