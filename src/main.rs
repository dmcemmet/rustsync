mod scanner;
mod preview;
mod tree;
mod app;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use clap::Parser;

#[derive(Parser)]
#[command(name = "rustsync", about = "Compare two directories and show missing or newer files")]
struct Cli {
    /// Source directory (files to check)
    source: PathBuf,

    /// Destination directory (to compare against)
    destination: PathBuf,

    /// Exclude directories matching glob patterns (repeatable)
    #[arg(short = 'e', long = "exclude")]
    exclude: Vec<String>,

    /// Follow symbolic links
    #[arg(long = "follow-symlinks")]
    follow_symlinks: bool,

    /// Dry-run: print what would be synced without launching TUI
    #[arg(long = "dry-run")]
    dry_run: bool,

    /// Mirror mode: also delete files from dest that don't exist in source
    #[arg(long = "mirror")]
    mirror: bool,
}

fn manifest_path(source: &std::path::Path, dest: &std::path::Path) -> PathBuf {
    let key = format!("{}:{}", source.display(), dest.display());
    let hash = xxhash_rust::xxh3::xxh3_64(key.as_bytes());
    dirs::home_dir().unwrap_or_default().join(".rustsync").join(format!("{hash:x}.manifest"))
}

fn load_manifest(path: &std::path::Path) -> Option<HashMap<PathBuf, SystemTime>> {
    let data = std::fs::read(path).ok()?;
    bincode::deserialize(&data).ok()
}

#[allow(dead_code)]
fn save_manifest(path: &std::path::Path, files: &HashMap<PathBuf, SystemTime>) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(data) = bincode::serialize(files) {
        let _ = std::fs::write(path, data);
    }
}

fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    let source = cli.source.canonicalize().unwrap_or(cli.source);
    let destination = cli.destination.canonicalize().unwrap_or(cli.destination);

    eprintln!("Scanning source: {}", source.display());
    eprintln!("Comparing with:  {}", destination.display());

    let mpath = manifest_path(&source, &destination);
    let last_sync = load_manifest(&mpath);

    let diffs = scanner::compare_dirs(&source, &destination, &cli.exclude, cli.follow_symlinks, last_sync.as_ref());

    if diffs.is_empty() {
        eprintln!("Directories are in sync.");
        return Ok(());
    }

    if cli.dry_run {
        let mut total: u64 = 0;
        for d in &diffs {
            let label = match &d.reason {
                scanner::DiffReason::Missing => "NEW",
                scanner::DiffReason::Newer { .. } => "MOD",
                scanner::DiffReason::OnlyInDest => "DEL",
                scanner::DiffReason::Conflict { .. } => "CONFLICT",
            };
            eprintln!("  [{label:>8}] {} ({})", d.rel_path.display(), format_size(d.size));
            total += d.size;
        }
        eprintln!("\n{} file(s), {}", diffs.len(), format_size(total));
        return Ok(());
    }

    eprintln!("Found {} file(s) to sync.", diffs.len());

    let mut terminal = ratatui::init();
    let mut app = app::App::new(diffs, source.clone(), destination.clone(), cli.mirror, mpath);
    let result = app.run(&mut terminal);
    ratatui::restore();
    result
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    for unit in UNITS {
        if size < 1024.0 { return format!("{size:.1} {unit}"); }
        size /= 1024.0;
    }
    format!("{size:.1} PB")
}
