//! `AbstractFileSystem` with its node and mock implementations.

use crate::utils::js_len;
use lq_json::uri;
use lq_json::Value;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileType {
    Unknown = 0,
    File = 1,
    Directory = 2,
    SymbolicLink = 64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStat {
    pub file_type: FileType,
    pub size: u64,
}

/// Sync `AbstractFileSystem`. `Err` holds what upstream would reject with.
pub trait FileSystem: Send + Sync {
    fn stat(&self, uri: &str) -> Result<FileStat, String>;
    fn read_file(&self, uri: &str) -> Result<String, String>;
    fn read_directory(&self, uri: &str) -> Result<Vec<(String, FileType)>, String>;
}

/// `NodeFileSystem`.
#[derive(Clone, Copy, Debug, Default)]
pub struct NodeFileSystem;

impl FileSystem for NodeFileSystem {
    fn stat(&self, uri: &str) -> Result<FileStat, String> {
        let meta = std::fs::metadata(uri::fs_path(uri))
            .map_err(|e| format!("Failed to get file stat: {e}"))?;
        Ok(FileStat {
            file_type: if meta.is_dir() { FileType::Directory } else { FileType::File },
            size: meta.len(),
        })
    }

    fn read_file(&self, uri: &str) -> Result<String, String> {
        read_utf8_lossy(Path::new(&uri::fs_path(uri))).map_err(|e| e.to_string())
    }

    fn read_directory(&self, uri_str: &str) -> Result<Vec<(String, FileType)>, String> {
        let entries = std::fs::read_dir(uri::fs_path(uri_str)).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            out.push((
                uri::join(uri_str, &[name]),
                if is_dir { FileType::Directory } else { FileType::File },
            ));
        }
        // readdir order is the OS's in node too; sorted here for determinism.
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }
}

impl NodeFileSystem {
    /// The entries of a directory, as uris, for debug tooling.
    pub fn read_dir_for_debug(&self, u: &str) -> Result<Vec<String>, String> {
        Ok(self.read_directory(u)?.into_iter().map(|(e, _)| e).collect())
    }
}

/// `fs.readFile(path, 'utf8')`: invalid sequences become U+FFFD, a BOM is kept.
pub fn read_utf8_lossy(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}

/// `MockFileSystem` from upstream's test helpers, for golden replay.
pub struct MockFileSystem {
    /// Relative path to source, in JS key order.
    pub theme: lq_json::Object,
    pub root_uri: String,
}

impl MockFileSystem {
    pub fn new(theme: lq_json::Object, root_uri: &str) -> Self {
        MockFileSystem { theme, root_uri: uri::normalize(root_uri) }
    }

    fn root_relative(&self, u: &str) -> String {
        uri::relative(&uri::normalize(u), &self.root_uri)
    }

    /// `this.fileTree`, descended to `segments`: `None` is `undefined`.
    fn tree_entries(&self, segments: &[&str]) -> Option<TreeAt> {
        let mut children: Vec<(String, bool)> = Vec::new();
        let mut found_dir = false;
        let mut file = false;
        let prefix_len = segments.len();
        for (path, source) in self.theme.iter() {
            let parts: Vec<&str> = path.split('/').collect();
            if parts.len() < prefix_len || parts[..prefix_len] != *segments {
                continue;
            }
            if parts.len() == prefix_len {
                // A file at exactly this path.
                let _ = source;
                file = true;
                continue;
            }
            found_dir = true;
            let name = parts[prefix_len];
            let is_file = parts.len() == prefix_len + 1;
            match children.iter_mut().find(|(n, _)| n == name) {
                // `current[segment] ??= {}` keeps a directory; a later file overwrites it.
                Some(entry) => {
                    if is_file {
                        entry.1 = true;
                    }
                }
                None => children.push((name.to_string(), is_file)),
            }
        }
        // Object key order: integer-like names first.
        let mut order = lq_json::Object::new();
        for (n, f) in children {
            order.insert(n, Value::Bool(f));
        }
        if file && !found_dir {
            return Some(TreeAt::File);
        }
        if found_dir || segments.is_empty() {
            return Some(TreeAt::Dir(
                order.iter().map(|(k, v)| (k.to_string(), v.as_bool().unwrap_or(false))).collect(),
            ));
        }
        None
    }
}

enum TreeAt {
    File,
    Dir(Vec<(String, bool)>),
}

impl FileSystem for MockFileSystem {
    fn read_file(&self, u: &str) -> Result<String, String> {
        match self.theme.get(&self.root_relative(u)).and_then(Value::as_str) {
            Some(s) => Ok(s.to_string()),
            None => Err("File not found".into()),
        }
    }

    fn read_directory(&self, u: &str) -> Result<Vec<(String, FileType)>, String> {
        let trimmed = u.strip_suffix('/').unwrap_or(u);
        let relative = self.root_relative(trimmed);
        let is_root = uri::normalize(trimmed) == self.root_uri;
        let segments: Vec<&str> = if is_root { Vec::new() } else { relative.split('/').collect() };
        match self.tree_entries(&segments) {
            None => Err(format!("Directory not found: {trimmed} for {}", self.root_uri)),
            Some(TreeAt::File) => Ok(vec![(trimmed.to_string(), FileType::File)]),
            Some(TreeAt::Dir(entries)) => Ok(entries
                .into_iter()
                .map(|(name, is_file)| {
                    (
                        format!("{trimmed}/{name}"),
                        if is_file { FileType::File } else { FileType::Directory },
                    )
                })
                .collect()),
        }
    }

    fn stat(&self, u: &str) -> Result<FileStat, String> {
        let relative = self.root_relative(u);
        if let Some(source) = self.theme.get(&relative).and_then(Value::as_str) {
            // `if (source)`: an empty file falls through to readDirectory.
            if !source.is_empty() {
                return Ok(FileStat { file_type: FileType::File, size: js_len(source) as u64 });
            }
        }
        self.read_directory(u)?;
        Ok(FileStat { file_type: FileType::Directory, size: 0 })
    }
}
