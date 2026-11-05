//! `.prettierignore` / `.gitignore` semantics of the `ignore` npm package, as Prettier
//! uses it: case-insensitive gitignore rules, later rules win, and a file under an
//! ignored directory stays ignored.

use std::path::{Path, PathBuf};

#[derive(Debug)]
struct Rule {
    negated: bool,
    dir_only: bool,
    /// Matched against the whole path when anchored, else against any suffix on a segment
    /// boundary.
    anchored: bool,
    segments: Vec<Vec<char>>,
}

fn parse_rule(line: &str) -> Option<Rule> {
    // Trailing unescaped spaces are insignificant.
    let mut line = line.trim_end_matches(['\r', '\t']);
    while line.ends_with(' ') && !line.ends_with("\\ ") {
        line = &line[..line.len() - 1];
    }
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (negated, line) = match line.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    let line = line.strip_prefix('\\').filter(|r| r.starts_with(['#', '!'])).unwrap_or(line);
    let (dir_only, line) = match line.strip_suffix('/') {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    if line.is_empty() {
        return None;
    }
    let anchored = line.contains('/');
    let line = line.strip_prefix('/').unwrap_or(line);
    let segments = line.split('/').map(|s| s.chars().collect()).collect();
    Some(Rule { negated, dir_only, anchored, segments })
}

fn matches_segments(p: &[Vec<char>], s: &[Vec<char>]) -> bool {
    let Some(first) = p.first() else {
        return s.is_empty();
    };
    if first.len() == 2 && first[0] == '*' && first[1] == '*' {
        if p.len() == 1 {
            // A trailing `/**` matches everything inside, not the directory itself.
            return !s.is_empty();
        }
        return (0..=s.len()).any(|i| matches_segments(&p[1..], &s[i..]));
    }
    !s.is_empty() && crate::glob_segment(first, &s[0]) && matches_segments(&p[1..], &s[1..])
}

impl Rule {
    fn matches(&self, segments: &[Vec<char>], is_dir: bool) -> bool {
        if self.dir_only && !is_dir {
            return false;
        }
        if self.anchored {
            return matches_segments(&self.segments, segments);
        }
        // Without a slash the pattern is matched against the last segment.
        match segments.last() {
            Some(last) => crate::glob_segment(&self.segments[0], last),
            None => false,
        }
    }
}

/// One ignore file, or the implicit `node_modules` rule.
#[derive(Debug)]
pub struct IgnoreFile {
    base: PathBuf,
    rules: Vec<Rule>,
}

impl IgnoreFile {
    pub fn from_content(base: &Path, content: &str) -> Self {
        IgnoreFile { base: base.to_path_buf(), rules: content.lines().filter_map(parse_rule).collect() }
    }

    /// `ignore.checkIgnore(relative).ignored`, testing parents first.
    pub fn is_ignored(&self, file: &Path) -> bool {
        let Ok(relative) = file.strip_prefix(&self.base) else {
            return self.is_ignored_outside_base(file);
        };
        let segments: Vec<Vec<char>> =
            relative.components().map(|c| c.as_os_str().to_string_lossy().chars().collect()).collect();
        self.check_segments(&segments)
    }

    /// Paths outside the base are `../x` relative paths; only slash-free rules can match.
    fn is_ignored_outside_base(&self, file: &Path) -> bool {
        let segments: Vec<Vec<char>> =
            file.components().map(|c| c.as_os_str().to_string_lossy().chars().collect()).collect();
        let mut state = false;
        if let Some(last) = segments.last() {
            for rule in &self.rules {
                if !rule.anchored && !rule.dir_only && crate::glob_segment(&rule.segments[0], last) {
                    state = !rule.negated;
                }
            }
        }
        state
    }

    fn check_segments(&self, segments: &[Vec<char>]) -> bool {
        for end in 1..=segments.len() {
            let is_dir = end < segments.len();
            let mut ignored = false;
            for rule in &self.rules {
                if rule.negated == ignored && rule.matches(&segments[..end], is_dir) {
                    ignored = !rule.negated;
                }
            }
            if ignored {
                return true;
            }
        }
        false
    }
}

/// `createIsIgnoredFunction(ignoreFiles, withNodeModules)`.
#[derive(Debug, Default)]
pub struct Ignorer {
    files: Vec<IgnoreFile>,
}

impl Ignorer {
    /// Reads `paths`, resolved against `cwd`; a missing file contributes only
    /// `node_modules`, as upstream's `content += "\nnode_modules"`.
    pub fn load(cwd: &Path, paths: &[PathBuf], with_node_modules: bool) -> std::io::Result<Self> {
        let mut files = Vec::new();
        for p in paths {
            let absolute = cwd.join(p);
            let mut content = match std::fs::read_to_string(&absolute) {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => return Err(e),
            };
            if !with_node_modules {
                content.push_str("\nnode_modules");
            }
            let base = absolute.parent().unwrap_or(cwd).to_path_buf();
            files.push(IgnoreFile::from_content(&base, &content));
        }
        Ok(Ignorer { files })
    }

    pub fn is_ignored(&self, file: &Path) -> bool {
        self.files.iter().any(|f| f.is_ignored(file))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ignored(content: &str, path: &str) -> bool {
        let f = IgnoreFile::from_content(Path::new("/r"), content);
        f.is_ignored(&Path::new("/r").join(path))
    }

    #[test]
    fn gitignore_rules() {
        assert!(ignored("/assets", "assets/a.liquid"));
        assert!(!ignored("/assets", "x/assets/a.liquid"));
        assert!(ignored("node_modules", "x/node_modules/a.liquid"));
        assert!(ignored("*.min.js", "a/b.min.js"));
        assert!(ignored("/lighthouse-results.claude", "lighthouse-results.claude"));
        assert!(ignored("foo/", "foo/x"));
        assert!(!ignored("foo/", "foo"));
        assert!(ignored("a/**", "a/b/c"));
        assert!(!ignored("a/**", "a"));
        assert!(ignored("**/x", "a/b/x"));
        assert!(!ignored("*.js\n!a.js", "a.js"));
        assert!(ignored("dir\n!dir/a.js", "dir/a.js"));
        assert!(ignored("ASSETS", "assets/x"));
        assert!(!ignored("# comment\n\n", "x"));
    }
}
