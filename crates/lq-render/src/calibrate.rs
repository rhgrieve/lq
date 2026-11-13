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
use crate::speedscope::{match_key, Tree};
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
}

pub struct Calibration {
    pub model: CostModel,
    pub shapes: HashMap<String, usize>,
    pub pages: Vec<PageFit>,
    pub shape_changes: Vec<String>,
    pub unmatched: Vec<(String, f64)>,
}

type Key = (String, u32);

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
    name.starts_with("for ") || name.starts_with("tablerow ")
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

/// Own time per matched line, summed over the tree.
fn self_by_key(tree: &Tree) -> HashMap<Key, f64> {
    let mut m: HashMap<Key, f64> = HashMap::new();
    for n in &tree.nodes {
        *m.entry(match_key(&tree.frames[n.frame as usize])).or_default() += n.self_us;
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

    // 1. Shapes.
    for _round in 0..4 {
        let mut changed = false;
        for s in samples {
            let res = render_page(theme, shop, &s.page, prior, limits, &shapes)?;
            let sim = crate::speedscope::from_profile(&res.profile, prior);
            let real_iters = loop_iterations(&s.real, |_| true);
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
    for s in samples {
        let res = render_page(theme, shop, &s.page, prior, limits, &shapes)?;
        let p = &res.profile;
        let mut counts: HashMap<Key, HashMap<String, f64>> = HashMap::new();
        for n in &p.nodes {
            let e = counts.entry(match_key(&p.frames[n.frame as usize])).or_default();
            for &(op, c) in &n.counts {
                *e.entry(p.ops.name(op).to_string()).or_default() += c;
            }
        }
        let real = self_by_key(&s.real);
        for (key, y) in &real {
            match counts.get(key) {
                Some(x) => rows.push((x.clone(), *y)),
                None => {
                    // Frames lq never rendered: app blocks, Shopify's own wrappers, untouched branches.
                    let f = s.real.frames.iter().find(|f| &match_key(f) == key);
                    let label = f.map(|f| format!("{}:{}  {}", f.file, f.line, f.name)).unwrap_or_default();
                    *unmatched_acc.entry(label).or_default() += y;
                }
            }
        }
        before.push(p.total(prior));
        sims.push((res.profile, counts));
    }

    // Columns: every op seen. Coordinate descent on the ridge-to-prior NNLS objective.
    let ops: Vec<String> = rows.iter().flat_map(|(x, _)| x.keys().cloned()).collect::<BTreeSet<_>>().into_iter().collect();
    let col = |r: &HashMap<String, f64>, j: usize| r.get(&ops[j]).copied().unwrap_or(0.0);
    let mut c: Vec<f64> = ops.iter().map(|o| prior.coefficient(o)).collect();
    let p: Vec<f64> = c.clone();
    let diag: Vec<f64> = (0..ops.len()).map(|j| rows.iter().map(|(x, _)| col(x, j).powi(2)).sum()).collect();
    let lambda: Vec<f64> = diag.iter().map(|d| 0.05 * d + 1e-9).collect();
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
            let new = (num / (diag[j] + lambda[j])).max(0.0);
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
        let sim_tree = crate::speedscope::from_profile(profile, &model);
        let sim_self = self_by_key(&sim_tree);
        let pairs: Vec<(f64, f64)> = real
            .iter()
            .filter(|(_, v)| **v >= 0.01 * real_total)
            .filter_map(|(k, v)| sim_self.get(k).map(|s| (*v, *s)))
            .collect();
        pages.push(PageFit {
            label: s.label.clone(),
            real_ms: s.real.total_us() / 1000.0,
            before_ms: before[i] / 1000.0,
            after_ms: profile.total(&model) / 1000.0,
            matched_share: if real_total > 0.0 { matched / real_total } else { 0.0 },
            spearman: spearman(&pairs),
        });
    }
    let mut unmatched: Vec<(String, f64)> = unmatched_acc.into_iter().collect();
    unmatched.sort_by(|a, b| b.1.total_cmp(&a.1));
    unmatched.truncate(15);
    Ok(Calibration { model, shapes, pages, shape_changes, unmatched })
}

pub fn report(c: &Calibration) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "{:<40}{:>10}{:>10}{:>10}{:>10}{:>8}", "page", "real ms", "before", "after", "matched", "rank ρ");
    for p in &c.pages {
        let _ = writeln!(
            o,
            "{:<40}{:>10.1}{:>10.1}{:>10.1}{:>9.0}%{:>8}",
            p.label,
            p.real_ms,
            p.before_ms,
            p.after_ms,
            p.matched_share * 100.0,
            if p.spearman.is_nan() { "-".to_string() } else { format!("{:.2}", p.spearman) }
        );
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
    fn spearman_ranks() {
        assert!((spearman(&[(1.0, 10.0), (2.0, 20.0), (3.0, 35.0)]) - 1.0).abs() < 1e-9);
        assert!((spearman(&[(1.0, 3.0), (2.0, 2.0), (3.0, 1.0)]) + 1.0).abs() < 1e-9);
    }
}
