//! Calibration against real profiles from `shopify theme profile --json`.
//!
//! 1. **Shape.** Frames are matched by template and line. Where a loop ran a different number of
//!    times on the real store than against the mock shop, lq records the real count so later
//!    renders iterate as often (`loop_shapes`). This needs no timing, and runs until stable.
//! 2. **Cost.** With the shapes fixed, each matched line gives one equation: its real own time
//!    against lq's operation counts on that line. A non-negative least-squares fit, pulled lightly
//!    towards the current coefficients so ops with little data keep sensible values, gives the
//!    new cost model.

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
    /// How fast this capture ran against the first, from lines doing the same work in both.
    pub speed: f64,
}

pub struct Calibration {
    pub model: CostModel,
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

    // 2. Costs. One row per matched line across all samples.
    let mut rows: Vec<(HashMap<String, f64>, f64)> = Vec::new();
    let mut before = Vec::new();
    let mut unmatched_acc: HashMap<String, f64> = HashMap::new();
    let mut sims = Vec::new();
    let mut totals: Vec<(HashMap<String, f64>, f64)> = Vec::new();
    let speeds = capture_speeds(samples);
    for (si, s) in samples.iter().enumerate() {
        let speed = speeds[si];
        let res = render_page(theme, shop, &s.page, prior, limits, &shapes)?;
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
        for (key, y) in &real {
            match counts.get(key) {
                Some(x) => rows.push((x.clone(), *y / speed)),
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
        before.push(p.total(prior));
        let mut all: HashMap<String, f64> = HashMap::new();
        for x in counts.values() {
            for (op, c) in x {
                *all.entry(op.clone()).or_default() += c;
            }
        }
        totals.push((all, s.real.total_us() / speed));
        sims.push((res.profile, counts));
    }

    // Each page's total is one more equation, weighted so the fit can't trade the page total away
    // for a closer fit on individual lines.
    let weight = (rows.len() as f64).sqrt().max(1.0);
    for (x, y) in totals {
        rows.push((x.into_iter().map(|(k, v)| (k, v * weight)).collect(), y * weight));
    }
    // Columns: every op seen. Coordinate descent on the ridge-to-prior NNLS objective.
    let ops: Vec<String> = rows.iter().flat_map(|(x, _)| x.keys().cloned()).collect::<BTreeSet<_>>().into_iter().collect();
    let col = |r: &HashMap<String, f64>, j: usize| r.get(&ops[j]).copied().unwrap_or(0.0);
    let mut c: Vec<f64> = ops.iter().map(|o| prior.coefficient(o)).collect();
    let p: Vec<f64> = c.clone();
    let diag: Vec<f64> = (0..ops.len()).map(|j| rows.iter().map(|(x, _)| col(x, j).powi(2)).sum()).collect();
    // An op seen on only a few lines can't be told apart from whatever else those lines do, so
    // it stays close to its current value; well-supported ops move freely.
    // On a tiny profile every op is rare, so the bar scales down with the number of lines.
    let min_support = MIN_SUPPORT.min((rows.len() / 3).max(1));
    let support: Vec<usize> = (0..ops.len()).map(|j| rows.iter().filter(|(x, _)| col(x, j) != 0.0).count()).collect();
    let lambda: Vec<f64> = diag
        .iter()
        .zip(&support)
        .map(|(d, &s)| if s >= min_support { 0.1 * d + 1e-9 } else { 20.0 * d + 1e-9 })
        .collect();
    // How far each cost may move from where it started. Pure computation (filters, lookups,
    // iterations, output) behaves alike on every store; database work and loading vary more. The
    // bounds stop the fit from handing time it can't explain to whichever cheap op shares a line.
    let (lo, hi): (Vec<f64>, Vec<f64>) = ops
        .iter()
        .zip(&p)
        .map(|(op, &prior)| {
            let wide = ["db:", "load:", "preload:", "global:"].iter().any(|x| op.starts_with(x))
                || matches!(op.as_str(), "template_load" | "section" | "render" | "include" | "block");
            let f = if wide { 20.0 } else { 10.0 };
            (prior / f, (prior * f).max(1.0))
        })
        .unzip();
    let mut resid: Vec<f64> = rows.iter().map(|(x, y)| (0..ops.len()).map(|j| col(x, j) * c[j]).sum::<f64>() - y).collect();
    for _sweep in 0..300 {
        let mut moved = 0.0f64;
        for j in 0..ops.len() {
            if diag[j] == 0.0 {
                continue;
            }
            // Residual without column j's contribution.
            let mut num = lambda[j] * p[j];
            for (i, (x, _)) in rows.iter().enumerate() {
                let xj = col(x, j);
                if xj != 0.0 {
                    num -= xj * (resid[i] - xj * c[j]);
                }
            }
            let new = (num / (diag[j] + lambda[j])).clamp(lo[j], hi[j]);
            let delta = new - c[j];
            if delta != 0.0 {
                for (i, (x, _)) in rows.iter().enumerate() {
                    resid[i] += col(x, j) * delta;
                }
                moved = moved.max(delta.abs() / (c[j].abs() + 1e-6));
                c[j] = new;
            }
        }
        if moved < 1e-6 {
            break;
        }
    }
    let mut model = prior.clone();
    for (j, op) in ops.iter().enumerate() {
        model.coefficients.insert(op.clone(), c[j]);
    }
    model.calibrated = true;
    model.source = format!("calibrated on {} profile(s)", samples.len());

    // 3. Fit report.
    let mut pages = Vec::new();
    for (i, s) in samples.iter().enumerate() {
        let (profile, counts) = &sims[i];
        let real = self_by_key(&s.real);
        let real_total: f64 = real.values().sum();
        let matched: f64 = real.iter().filter(|(k, _)| counts.contains_key(*k)).map(|(_, v)| v).sum();
        // Every line with at least 0.1% of the real time; one lq didn't render counts as zero.
        let rho = |m: &CostModel| {
            let sim_self = self_by_key(&crate::speedscope::from_profile(profile, m));
            let pairs: Vec<(f64, f64)> = real
                .iter()
                .filter(|(k, v)| **v >= 0.001 * real_total && k.1 > 0)
                .map(|(k, v)| (*v, sim_self.get(k).copied().unwrap_or(0.0)))
                .collect();
            spearman(&pairs)
        };
        pages.push(PageFit {
            label: s.label.clone(),
            real_ms: s.real.total_us() / 1000.0,
            before_ms: before[i] * speeds[i] / 1000.0,
            after_ms: profile.total(&model) * speeds[i] / 1000.0,
            matched_share: if real_total > 0.0 { matched / real_total } else { 0.0 },
            spearman: rho(&model),
            spearman_before: rho(prior),
            speed: speeds[i],
        });
    }
    let mut unmatched: Vec<(String, f64)> = unmatched_acc.into_iter().collect();
    unmatched.sort_by(|a, b| b.1.total_cmp(&a.1));
    unmatched.truncate(15);
    let shop_json = fitted.map(|(_, j)| j);
    Ok(Calibration { model, shapes, pages, shape_changes, unmatched, knob_changes, shop_json })
}

pub fn report(c: &Calibration) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "{:<40}{:>8}{:>10}{:>10}{:>10}{:>10}{:>10}{:>8}", "page", "speed", "real ms", "before", "after", "matched", "ρ before", "ρ after");
    for p in &c.pages {
        let _ = writeln!(
            o,
            "{:<40}{:>8.2}{:>10.1}{:>10.1}{:>10.1}{:>9.0}%{:>10}{:>8}",
            p.label,
            p.speed,
            p.real_ms,
            p.before_ms,
            p.after_ms,
            p.matched_share * 100.0,
            if p.spearman_before.is_nan() { "-".to_string() } else { format!("{:.2}", p.spearman_before) },
            if p.spearman.is_nan() { "-".to_string() } else { format!("{:.2}", p.spearman) }
        );
    }
    if c.pages.len() > 1 {
        let _ = writeln!(o, "\nspeed: how fast each capture ran against the first, from lines doing the same work. Times are\nscaled by it, as captures of the same code vary.");
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
