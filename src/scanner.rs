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

pub fn compare_dirs(source: &Path, destination: &Path, exclude: &[String]) -> Vec<FileDiff> {
    let style = ProgressStyle::default_bar()
        .template("{spinner:.cyan} {msg} [{bar:30.cyan/dim}] {pos}/{len}")
        .unwrap()
        .progress_chars("█▓░");
    let spinner_style = ProgressStyle::default_spinner()
        .template("{spinner:.cyan} {msg}")
        .unwrap();

    // Count subdirectories for progress bars
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(spinner_style.clone());
    spinner.set_message("Counting directories...");
    spinner.enable_steady_tick(std::time::Duration::from_millis(80));
    let src_dirs: u64 = WalkDir::new(source).min_depth(1).max_depth(1)
        .into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_dir()).count() as u64;
    let dst_dirs: u64 = WalkDir::new(destination).min_depth(1).max_depth(1)
        .into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_dir()).count() as u64;
    spinner.finish_and_clear();

    // Phase 1 & 2: Scan source and destination in parallel
    let source_owned = source.to_path_buf();
    let dest_owned = destination.to_path_buf();
    let exclude_src = exclude.to_vec();
    let exclude_dst = exclude.to_vec();

    let multi = indicatif::MultiProgress::new();
    let bar_src = multi.add(ProgressBar::new(src_dirs.max(1)));
    bar_src.set_style(style.clone());
    bar_src.set_message("Scanning source");
    let bar_dst = multi.add(ProgressBar::new(dst_dirs.max(1)));
    bar_dst.set_style(style.clone());
    bar_dst.set_message("Scanning dest  ");

    let src_handle = std::thread::spawn(move || {
        let mut files = Vec::new();
        for entry in WalkDir::new(&source_owned)
            .into_iter()
            .filter_entry(|e| !should_exclude(e.file_name().to_string_lossy().as_ref(), &exclude_src))
            .filter_map(|e| e.ok())
        {
            if entry.depth() == 1 && entry.file_type().is_dir() {
                bar_src.inc(1);
            }
            if !entry.file_type().is_file() { continue; }
            let Some(rel) = entry.path().strip_prefix(&source_owned).ok().map(|p| p.to_path_buf()) else { continue };
            let Some(meta) = entry.metadata().ok() else { continue };
            let Some(modified) = meta.modified().ok() else { continue };
            files.push((rel, meta.len(), modified));
        }
        bar_src.finish_and_clear();
        files
    });

    let dst_handle = std::thread::spawn(move || {
        let mut files: HashMap<PathBuf, SystemTime> = HashMap::new();
        for entry in WalkDir::new(&dest_owned)
            .into_iter()
            .filter_entry(|e| !should_exclude(e.file_name().to_string_lossy().as_ref(), &exclude_dst))
            .filter_map(|e| e.ok())
        {
            if entry.depth() == 1 && entry.file_type().is_dir() {
                bar_dst.inc(1);
            }
            if !entry.file_type().is_file() { continue; }
            let Some(rel) = entry.path().strip_prefix(&dest_owned).ok().map(|p| p.to_path_buf()) else { continue };
            let Some(modified) = entry.metadata().ok().and_then(|m| m.modified().ok()) else { continue };
            files.insert(rel, modified);
        }
        bar_dst.finish_and_clear();
        files
    });

    let source_files = src_handle.join().expect("source scan failed");
    let dest_files = dst_handle.join().expect("dest scan failed");
    drop(multi);

    // Phase 3: Compare (parallel hashing)
    let bar = ProgressBar::new(source_files.len() as u64);
    bar.set_style(style);
    bar.set_message("Comparing files");

    let diffs: Vec<FileDiff> = source_files.into_par_iter().filter_map(|(rel, size, src_mod)| {
        bar.inc(1);
        let reason = match dest_files.get(&rel) {
            None => DiffReason::Missing,
            Some(&dest_mod) => {
                // Use 2-second tolerance for filesystem timestamp granularity
                let diff = src_mod.duration_since(dest_mod).unwrap_or_default();
                if diff.as_secs() < 2 {
                    return None; // within tolerance, consider same
                }
                let src_path = source.join(&rel);
                let dst_path = destination.join(&rel);
                if hash_file(&src_path) == hash_file(&dst_path) {
                    return None;
                }
                DiffReason::Newer { source_mod: src_mod, dest_mod }
            }
        };
        Some(FileDiff { rel_path: rel, size, modified: Some(src_mod), reason })
    }).collect();

    bar.finish_and_clear();
    diffs
}

fn should_exclude(name: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pat| {
        if pat.contains('*') || pat.contains('?') {
            glob_match(pat, name)
        } else {
            name == pat
        }
    })
}

fn glob_match(pattern: &str, name: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    glob_match_recursive(&pat, &name)
}

fn glob_match_recursive(pat: &[char], name: &[char]) -> bool {
    match (pat.first(), name.first()) {
        (None, None) => true,
        (Some('*'), _) => {
            glob_match_recursive(&pat[1..], name) || (!name.is_empty() && glob_match_recursive(pat, &name[1..]))
        }
        (Some('?'), Some(_)) => glob_match_recursive(&pat[1..], &name[1..]),
        (Some(a), Some(b)) if a == b => glob_match_recursive(&pat[1..], &name[1..]),
        _ => false,
    }
}
