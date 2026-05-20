use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::scanner::FileDiff;

#[derive(Debug, Clone)]
pub struct TreeNode {
    pub name: String,
    pub rel_path: PathBuf,
    pub is_dir: bool,
    pub expanded: bool,
    pub children: BTreeMap<String, TreeNode>,
    pub diff: Option<FileDiff>,
}

impl TreeNode {
    pub fn new_root() -> Self {
        Self {
            name: String::new(),
            rel_path: PathBuf::new(),
            is_dir: true,
            expanded: true,
            children: BTreeMap::new(),
            diff: None,
        }
    }

    pub fn insert(&mut self, diff: FileDiff) {
        let rel = diff.rel_path.clone();
        let components: Vec<_> = rel.components().collect();
        self.insert_recursive(&components, diff);
    }

    fn insert_recursive(&mut self, components: &[std::path::Component], diff: FileDiff) {
        if components.is_empty() { return; }
        let name = components[0].as_os_str().to_string_lossy().to_string();
        if components.len() == 1 {
            self.children.entry(name.clone()).or_insert_with(|| TreeNode {
                name: name.clone(),
                rel_path: diff.rel_path.clone(),
                is_dir: false,
                expanded: false,
                children: BTreeMap::new(),
                diff: Some(diff),
            });
        } else {
            let rel = self.rel_path.join(&name);
            let child = self.children.entry(name.clone()).or_insert_with(|| TreeNode {
                name: name.clone(),
                rel_path: rel,
                is_dir: true,
                expanded: true,
                children: BTreeMap::new(),
                diff: None,
            });
            child.insert_recursive(&components[1..], diff);
        }
    }

    pub fn flatten(&self) -> Vec<(usize, &TreeNode)> {
        let mut result = Vec::new();
        for child in self.children.values() {
            Self::flatten_recursive(child, 0, &mut result);
        }
        result
    }

    fn flatten_recursive<'a>(node: &'a TreeNode, depth: usize, result: &mut Vec<(usize, &'a TreeNode)>) {
        result.push((depth, node));
        if node.is_dir && node.expanded {
            for child in node.children.values() {
                Self::flatten_recursive(child, depth + 1, result);
            }
        }
    }

    pub fn toggle_expand(&mut self, rel_path: &Path) {
        if self.rel_path == rel_path && self.is_dir {
            self.expanded = !self.expanded;
            return;
        }
        for child in self.children.values_mut() {
            child.toggle_expand(rel_path);
        }
    }

    pub fn file_count(&self) -> usize {
        if !self.is_dir { return 1; }
        self.children.values().map(|c| c.file_count()).sum()
    }

    pub fn total_size(&self) -> u64 {
        if let Some(d) = &self.diff { return d.size; }
        self.children.values().map(|c| c.total_size()).sum()
    }
}

pub fn build_tree(diffs: &[FileDiff]) -> TreeNode {
    let mut tree = TreeNode::new_root();
    for diff in diffs {
        tree.insert(diff.clone());
    }
    tree
}
