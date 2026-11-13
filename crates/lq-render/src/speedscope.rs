//! Reads Speedscope files, such as `shopify theme profile --json` output, into a call tree with
//! self and total times in microseconds.

use crate::profile::FrameKey;
use serde_json::Value as J;
use std::rc::Rc;

#[derive(Clone, Debug, Default)]
pub struct TreeNode {
    pub frame: u32,
    pub self_us: f64,
    pub total_us: f64,
    pub children: Vec<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub name: String,
    pub frames: Vec<FrameKey>,
    pub nodes: Vec<TreeNode>,
    pub roots: Vec<u32>,
}

impl Tree {
    pub fn total_us(&self) -> f64 {
        self.roots.iter().map(|&r| self.nodes[r as usize].total_us).sum()
    }
}

fn unit_to_us(unit: &str) -> f64 {
    match unit {
        "nanoseconds" => 0.001,
        "microseconds" => 1.0,
        "milliseconds" => 1000.0,
        "seconds" => 1_000_000.0,
        _ => 1.0,
    }
}

/// Parses the active profile in a Speedscope file.
pub fn read(text: &str) -> Result<Tree, String> {
    let j: J = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let frames: Vec<FrameKey> = j["shared"]["frames"]
        .as_array()
        .ok_or("no shared.frames: is this a Speedscope file?")?
        .iter()
        .map(|f| FrameKey {
            name: f["name"].as_str().unwrap_or("").to_string(),
            file: Rc::from(f["file"].as_str().unwrap_or("")),
            line: f["line"].as_f64().unwrap_or(0.0).max(0.0) as u32,
            col: f["col"].as_f64().unwrap_or(0.0).max(0.0) as u32,
        })
        .collect();
    let profiles = j["profiles"].as_array().ok_or("no profiles")?;
    let idx = j["activeProfileIndex"].as_u64().unwrap_or(0) as usize;
    let p = profiles.get(idx).or(profiles.first()).ok_or("empty profiles")?;
    let scale = unit_to_us(p["unit"].as_str().unwrap_or("milliseconds"));
    let mut tree = Tree {
        name: j["name"].as_str().or(p["name"].as_str()).unwrap_or("").to_string(),
        frames,
        ..Default::default()
    };
    match p["type"].as_str() {
        Some("evented") => {
            let mut stack: Vec<(u32, f64)> = Vec::new();
            for e in p["events"].as_array().ok_or("no events")? {
                let at = e["at"].as_f64().unwrap_or(0.0) * scale;
                let frame = e["frame"].as_u64().unwrap_or(0) as u32;
                match e["type"].as_str() {
                    Some("O") => {
                        let id = tree.nodes.len() as u32;
                        tree.nodes.push(TreeNode { frame, ..Default::default() });
                        match stack.last() {
                            Some(&(parent, _)) => tree.nodes[parent as usize].children.push(id),
                            None => tree.roots.push(id),
                        }
                        stack.push((id, at));
                    }
                    Some("C") => {
                        if let Some((id, start)) = stack.pop() {
                            tree.nodes[id as usize].total_us = at - start;
                        }
                    }
                    _ => {}
                }
            }
        }
        Some("sampled") => {
            let samples = p["samples"].as_array().ok_or("no samples")?;
            let weights = p["weights"].as_array().ok_or("no weights")?;
            for (s, w) in samples.iter().zip(weights) {
                let w = w.as_f64().unwrap_or(0.0) * scale;
                let mut parent: Option<u32> = None;
                for f in s.as_array().into_iter().flatten() {
                    let frame = f.as_u64().unwrap_or(0) as u32;
                    let siblings = match parent {
                        Some(p) => &tree.nodes[p as usize].children,
                        None => &tree.roots,
                    };
                    let existing = siblings.iter().copied().find(|&c| tree.nodes[c as usize].frame == frame);
                    let id = match existing {
                        Some(id) => id,
                        None => {
                            let id = tree.nodes.len() as u32;
                            tree.nodes.push(TreeNode { frame, ..Default::default() });
                            match parent {
                                Some(p) => tree.nodes[p as usize].children.push(id),
                                None => tree.roots.push(id),
                            }
                            id
                        }
                    };
                    tree.nodes[id as usize].total_us += w;
                    parent = Some(id);
                }
            }
        }
        other => return Err(format!("unsupported profile type {other:?}")),
    }
    for i in 0..tree.nodes.len() {
        let kids: f64 = tree.nodes[i].children.iter().map(|&c| tree.nodes[c as usize].total_us).sum();
        tree.nodes[i].self_us = (tree.nodes[i].total_us - kids).max(0.0);
    }
    Ok(tree)
}

/// lq's own profile as a tree, with times from the cost model.
pub fn from_profile(p: &crate::Profile, model: &crate::CostModel) -> Tree {
    let totals = p.totals(model);
    Tree {
        name: p.name.clone(),
        frames: p.frames.clone(),
        nodes: p
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| TreeNode {
                frame: n.frame,
                self_us: p.self_cost(i as u32, model),
                total_us: totals[i],
                children: n.children.clone(),
            })
            .collect(),
        roots: p.roots.clone(),
    }
}

/// The key two profiles' frames are matched on: the template without its extension, and the line.
pub fn match_key(f: &FrameKey) -> (String, u32) {
    let file = f.file.trim_start_matches('/');
    let file = file.strip_suffix(".liquid").unwrap_or(file);
    (file.to_string(), f.line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_evented() {
        let t = read(r#"{"$schema":"https://www.speedscope.app/file-format-schema.json","shared":{"frames":[{"name":"a","file":"sections/x","line":3},{"name":"b"}]},
            "profiles":[{"type":"evented","name":"p","unit":"milliseconds","startValue":0,"endValue":5,
            "events":[{"type":"O","at":0,"frame":0},{"type":"O","at":1,"frame":1},{"type":"C","at":3,"frame":1},{"type":"C","at":5,"frame":0}]}]}"#).unwrap();
        assert_eq!(t.nodes[0].total_us, 5000.0);
        assert_eq!(t.nodes[0].self_us, 3000.0);
        assert_eq!(match_key(&t.frames[0]), ("sections/x".to_string(), 3));
    }
}
