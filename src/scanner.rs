use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use walkdir::WalkDir;
use xxhash_rust::xxh3::Xxh3;

#[derive(Clone, Debug)]
pub enum DiffReason {
    Missing,
    Newer { source_mod: SystemTime, dest_mod: SystemTime },
}

#[derive(Clone, Debug)]
pub struct FileDiff {
    pub rel_path: PathBuf,
    pub size: u64,
    #[allow(dead_code)]
    pub modified: Option<SystemTime>,
    pub reason: DiffReason,
}

fn hash_file(path: &Path) -> Option<u64> {
    let mut file = File::open(path).ok()?;
    let mut hasher = Xxh3::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }
    Some(hasher.digest())
}

pub fn compare_dirs(source: &Path, destination: &Path) -> Vec<FileDiff> {
    let style = ProgressStyle::default_bar()
        .template("{spinner:.cyan} {msg} [{bar:30.cyan/dim}] {pos}/{len}")
        .unwrap()
        .progress_chars("█▓░");
    let spinner_style = ProgressStyle::default_spinner()
        .template("{spinner:.cyan} {msg}")
        .unwrap();

    // Phase 1: Scan destination
    let bar = ProgressBar::new_spinner();
    bar.set_style(spinner_style.clone());
    bar.set_message("Scanning destination...");
    bar.enable_steady_tick(std::time::Duration::from_millis(80));

    let dest_files: HashMap<PathBuf, SystemTime> = WalkDir::new(destination)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .inspect(|_| bar.inc(1))
        .filter_map(|e| {
            let rel = e.path().strip_prefix(destination).ok()?.to_path_buf();
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((rel, modified))
        })
        .collect();
    bar.finish_and_clear();

    // Phase 2: Scan source
    let bar = ProgressBar::new_spinner();
    bar.set_style(spinner_style);
    bar.set_message("Scanning source...");
    bar.enable_steady_tick(std::time::Duration::from_millis(80));

    let source_files: Vec<(PathBuf, u64, SystemTime)> = WalkDir::new(source)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .inspect(|_| bar.inc(1))
        .filter_map(|e| {
            let rel = e.path().strip_prefix(source).ok()?.to_path_buf();
            let meta = e.metadata().ok()?;
            let modified = meta.modified().ok()?;
            Some((rel, meta.len(), modified))
        })
        .collect();
    bar.finish_and_clear();

    // Phase 3: Compare (parallel hashing)
    let bar = ProgressBar::new(source_files.len() as u64);
    bar.set_style(style);
    bar.set_message("Comparing files");

    let diffs: Vec<FileDiff> = source_files.into_par_iter().filter_map(|(rel, size, src_mod)| {
        bar.inc(1);
        let reason = match dest_files.get(&rel) {
            None => DiffReason::Missing,
            Some(&dest_mod) if src_mod > dest_mod => {
                let src_path = source.join(&rel);
                let dst_path = destination.join(&rel);
                if hash_file(&src_path) == hash_file(&dst_path) {
                    return None;
                }
                DiffReason::Newer { source_mod: src_mod, dest_mod }
            }
            _ => return None,
        };
        Some(FileDiff { rel_path: rel, size, modified: Some(src_mod), reason })
    }).collect();

    bar.finish_and_clear();
    diffs
}
