//! The profile tree, in the shape of the Liquid gem's profiler: one node per rendered tag or
//! output, nested under the tag that rendered it, with the node's raw code, template and line.
//! Each node holds its own operation counts; time comes from the cost model.

use crate::cost::{CostModel, Ops};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FrameKey {
    pub name: String,
    pub file: Rc<str>,
    pub line: u32,
    pub col: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Node {
    pub frame: u32,
    /// `(op id, count)`, this node's own operations.
    pub counts: Vec<(u32, f64)>,
    pub children: Vec<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub name: String,
    pub frames: Vec<FrameKey>,
    frame_index: HashMap<FrameKey, u32>,
    pub nodes: Vec<Node>,
    pub roots: Vec<u32>,
    pub ops: Ops,
    stack: Vec<u32>,
}

impl Profile {
    pub fn new(name: &str) -> Profile {
        Profile { name: name.to_string(), ..Default::default() }
    }

    pub fn open(&mut self, key: FrameKey) {
        let frame = match self.frame_index.get(&key) {
            Some(&f) => f,
            None => {
                let f = self.frames.len() as u32;
                self.frame_index.insert(key.clone(), f);
                self.frames.push(key);
                f
            }
        };
        let id = self.nodes.len() as u32;
        self.nodes.push(Node { frame, ..Default::default() });
        match self.stack.last() {
            Some(&parent) => self.nodes[parent as usize].children.push(id),
            None => self.roots.push(id),
        }
        self.stack.push(id);
    }

    pub fn close(&mut self) {
        self.stack.pop();
    }

    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// Adds `n` of `op` to the open node. Work outside any node lands on a root `(page)` frame.
    pub fn count(&mut self, op: &str, n: f64) {
        if n == 0.0 {
            return;
        }
        let id = self.ops.id(op);
        let node = match self.stack.last() {
            Some(&node) => node,
            None => {
                self.open(FrameKey { name: "(page)".into(), file: Rc::from(""), line: 0, col: 0 });
                let node = *self.stack.last().unwrap();
                self.stack.pop();
                node
            }
        };
        let counts = &mut self.nodes[node as usize].counts;
        match counts.iter_mut().find(|(o, _)| *o == id) {
            Some((_, c)) => *c += n,
            None => counts.push((id, n)),
        }
    }

    pub fn self_cost(&self, node: u32, model: &CostModel) -> f64 {
        self.nodes[node as usize]
            .counts
            .iter()
            .map(|&(op, n)| n * model.coefficient(self.ops.name(op)))
            .sum()
    }

    /// Total cost of every node, children included, in microseconds.
    pub fn totals(&self, model: &CostModel) -> Vec<f64> {
        let mut totals = vec![0.0; self.nodes.len()];
        // Children always have higher ids than their parent, so a reverse pass is post-order.
        for i in (0..self.nodes.len()).rev() {
            let own = self.self_cost(i as u32, model);
            let kids: f64 = self.nodes[i].children.iter().map(|&c| totals[c as usize]).sum();
            totals[i] = own + kids;
        }
        totals
    }

    pub fn total(&self, model: &CostModel) -> f64 {
        let totals = self.totals(model);
        self.roots.iter().map(|&r| totals[r as usize]).sum()
    }

    /// Speedscope's evented format, as `shopify theme profile --json` prints it. Times are in
    /// milliseconds. A node's own time comes before its children's.
    pub fn to_speedscope(&self, model: &CostModel) -> String {
        let totals = self.totals(model);
        let mut events = String::new();
        let mut t = 0.0f64;
        let mut first = true;
        let mut push = |kind: char, at: f64, frame: u32| {
            if !first {
                events.push(',');
            }
            first = false;
            let _ = write!(events, "{{\"type\":\"{kind}\",\"at\":{},\"frame\":{frame}}}", round(at / 1000.0));
        };
        // Iterative DFS: (node, exiting?)
        let mut stack: Vec<(u32, bool)> = self.roots.iter().rev().map(|&r| (r, false)).collect();
        let mut close_at: Vec<f64> = vec![0.0; self.nodes.len()];
        while let Some((n, exiting)) = stack.pop() {
            let node = &self.nodes[n as usize];
            if exiting {
                t = close_at[n as usize];
                push('C', t, node.frame);
                continue;
            }
            push('O', t, node.frame);
            close_at[n as usize] = t + totals[n as usize];
            t += self.self_cost(n, model);
            stack.push((n, true));
            for &c in node.children.iter().rev() {
                stack.push((c, false));
            }
        }
        let end = round(t / 1000.0);
        let mut frames = String::new();
        for (i, f) in self.frames.iter().enumerate() {
            if i > 0 {
                frames.push(',');
            }
            let _ = write!(
                frames,
                "{{\"name\":{},\"file\":{},\"line\":{},\"col\":{}}}",
                json_str(&f.name),
                json_str(&f.file),
                f.line,
                f.col
            );
        }
        format!(
            "{{\"$schema\":\"https://www.speedscope.app/file-format-schema.json\",\"exporter\":\"lq {}\",\
             \"name\":{},\"activeProfileIndex\":0,\"shared\":{{\"frames\":[{frames}]}},\
             \"profiles\":[{{\"type\":\"evented\",\"name\":{},\"unit\":\"milliseconds\",\"startValue\":0,\
             \"endValue\":{end},\"events\":[{events}]}}]}}\n",
            env!("CARGO_PKG_VERSION"),
            json_str(&self.name),
            json_str(&self.name),
        )
    }

    /// Sums per frame: self cost, total cost (counted once per outermost occurrence, so recursion
    /// isn't double counted) and calls.
    pub fn frame_stats(&self, model: &CostModel) -> Vec<FrameStat> {
        let totals = self.totals(model);
        let mut stats: Vec<FrameStat> = (0..self.frames.len())
            .map(|f| FrameStat { frame: f as u32, ..Default::default() })
            .collect();
        let mut on_stack = vec![0u32; self.frames.len()];
        let mut stack: Vec<(u32, bool)> = self.roots.iter().rev().map(|&r| (r, false)).collect();
        while let Some((n, exiting)) = stack.pop() {
            let node = &self.nodes[n as usize];
            let f = node.frame as usize;
            if exiting {
                on_stack[f] -= 1;
                continue;
            }
            let s = &mut stats[f];
            s.calls += 1;
            s.self_us += self.self_cost(n, model);
            if on_stack[f] == 0 {
                s.total_us += totals[n as usize];
            }
            on_stack[f] += 1;
            stack.push((n, true));
            for &c in node.children.iter().rev() {
                stack.push((c, false));
            }
        }
        stats
    }

    /// Op counts summed over the whole page.
    pub fn op_totals(&self) -> Vec<(String, f64)> {
        let mut sums = vec![0.0; self.ops.names.len()];
        for n in &self.nodes {
            for &(op, c) in &n.counts {
                sums[op as usize] += c;
            }
        }
        let mut out: Vec<_> =
            self.ops.names.iter().cloned().zip(sums).filter(|(_, c)| *c > 0.0).collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

#[derive(Clone, Debug, Default)]
pub struct FrameStat {
    pub frame: u32,
    pub calls: u32,
    pub self_us: f64,
    pub total_us: f64,
}

fn round(x: f64) -> f64 {
    (x * 1_000_000.0).round() / 1_000_000.0
}

pub fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str, line: u32) -> FrameKey {
        FrameKey { name: name.into(), file: Rc::from("a.liquid"), line, col: 1 }
    }

    #[test]
    fn totals_and_speedscope() {
        let model = CostModel::from_json(r#"{"coefficients":{"x":1000}}"#, "t").unwrap();
        let mut p = Profile::new("t");
        p.open(key("outer", 1));
        p.count("x", 1.0);
        p.open(key("inner", 2));
        p.count("x", 2.0);
        p.close();
        p.close();
        assert_eq!(p.total(&model), 3000.0);
        let s = p.to_speedscope(&model);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        let events = v["profiles"][0]["events"].as_array().unwrap();
        let ats: Vec<f64> = events.iter().map(|e| e["at"].as_f64().unwrap()).collect();
        assert_eq!(ats, [0.0, 1.0, 3.0, 3.0]);
        assert_eq!(v["profiles"][0]["endValue"], 3.0);
        let stats = p.frame_stats(&model);
        assert_eq!(stats[0].total_us, 3000.0);
        assert_eq!(stats[1].self_us, 2000.0);
    }
}
