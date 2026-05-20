# rustsync

A terminal-based directory comparison and sync tool. Compares two directories, shows differences in an interactive TUI, and copies files with real-time progress.

## Features

- **Bidirectional comparison** — finds new, modified, deleted, and conflicting files
- **Content verification** — hashes files with xxHash to detect truly changed content
- **TUI with directory tree** — collapsible tree with guide lines (`│`) for clear hierarchy
- **File preview** — side-by-side source/destination preview (text and images)
- **Selective sync** — select individual files or entire directories to copy
- **Copy with progress** — per-file and total progress bars, speed indicator, ETA
- **Verify after copy** — hashes both files post-copy to confirm integrity (✓✓)
- **Preserve timestamps** — destination mtime matches source after copy
- **Preserve permissions** — Unix file permissions copied
- **Parallel hashing** — uses rayon for multi-core hash comparison
- **Parallel scanning** — source and destination scanned simultaneously
- **Conflict detection** — flags files modified on both sides since last sync
- **Manifest tracking** — remembers last sync state for conflict detection
- **Filter by status** — show only new, modified, dest-only, or conflicting files
- **Mirror mode** — delete files from destination that don't exist in source
- **Exclude patterns** — glob-based directory exclusion
- **Symlink support** — optionally follow symbolic links
- **Dry-run mode** — print sync report without launching TUI
- **Cancellable copy** — ESC cancels mid-file transfer
- **Terminal bell** — notification sound when copy completes

## Installation

```sh
cargo build --release
```

The binary will be at `target/release/rustsync`.

## Usage

```
rustsync <source> <destination> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `-e, --exclude <pattern>` | Exclude directories matching glob patterns (repeatable) |
| `--follow-symlinks` | Follow symbolic links during scan |
| `--dry-run` | Print sync report without launching TUI |
| `--mirror` | Delete files from dest that don't exist in source |

### Examples

```sh
# Compare local directories
rustsync ~/Documents/project ~/Backup/project

# Sync a NAS mount, excluding temp files
rustsync /mnt/nas/photos ~/Photos --exclude "@*" --exclude ".git"

# Mirror mode (delete extra files in dest)
rustsync ~/Source ~/Backup --mirror

# Dry-run to see what would be synced
rustsync ~/Source ~/Backup --dry-run
```

## Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up |
| `Home` / `End` | Jump to top / bottom |
| `PgUp` / `PgDn` | Page up / page down |
| `Enter` | Expand/collapse directory |
| `Space` | Select/deselect file or directory |
| `a` | Select all |
| `u` | Deselect all |
| `c` | Copy selected files (opens confirmation) |
| `S` | Sync all (select all matching filter and copy) |
| `f` | Cycle filter (All → New → Modified → Dest-only → Conflict) |
| `v` | Toggle verify after copy |
| `p` | Toggle preview pane |
| `Esc` / `q` | Quit |

In the copy confirmation dialog:

| Key | Action |
|-----|--------|
| `←` / `→` / `Tab` | Switch between Cancel / Copy |
| `Enter` | Confirm |
| `Esc` | Cancel |

During copy:

| Key | Action |
|-----|--------|
| `Esc` | Cancel remaining copies |

## File Status Icons

| Icon | Color | Meaning |
|------|-------|---------|
| `N` | Green | New file (only in source) |
| `M` | Yellow | Modified (source newer with different content) |
| `D` | Red | Deleted (only in destination) |
| `C` | Magenta | Conflict (both sides changed since last sync) |

## How It Works

1. **Scan** — walks source and destination in parallel, collecting file paths and metadata
2. **Compare** — for each source file:
   - Not in destination → **New**
   - Newer by >2 seconds → hashes both to verify content differs → **Modified**
   - Both changed since last manifest → **Conflict**
3. **Reverse check** — files in destination but not source → **Deleted**
4. **Copy** — chunked 1MB transfers with real-time progress, preserves timestamps and permissions
5. **Verify** — post-copy hash comparison confirms integrity
6. **Manifest** — saves sync state to `~/.rustsync/` for future conflict detection

## State Files

rustsync stores sync manifests in `~/.rustsync/`:

| File | Purpose |
|------|---------|
| `<hash>.manifest` | Records file modification times at last sync for conflict detection |

The `<hash>` is an xxHash-64 of the source:destination path pair.

## License

MIT
