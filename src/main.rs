mod scanner;
mod preview;
mod tree;
mod app;

use std::path::PathBuf;
use clap::Parser;

#[derive(Parser)]
#[command(name = "rustsync", about = "Compare two directories and show missing or newer files")]
struct Cli {
    /// Source directory (files to check)
    source: PathBuf,

    /// Destination directory (to compare against)
    destination: PathBuf,
}

fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    let source = cli.source.canonicalize().unwrap_or(cli.source);
    let destination = cli.destination.canonicalize().unwrap_or(cli.destination);

    eprintln!("Scanning source: {}", source.display());
    eprintln!("Comparing with:  {}", destination.display());

    let diffs = scanner::compare_dirs(&source, &destination);

    if diffs.is_empty() {
        eprintln!("Directories are in sync. No missing or newer files found.");
        return Ok(());
    }

    eprintln!("Found {} file(s) to sync.", diffs.len());

    let mut terminal = ratatui::init();
    let mut app = app::App::new(diffs, source, destination);
    let result = app.run(&mut terminal);
    ratatui::restore();
    result
}
