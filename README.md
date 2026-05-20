# rustsync

A terminal-based directory comparison and sync tool. Compares two directories and shows files that are missing or newer in the source, with an interactive TUI for reviewing and copying.

## Features

- **Directory comparison** — finds files missing from destination or newer in source
- **Content verification** — hashes files with xxHash to skip unchanged files (same content, different timestamp)
- **TUI with directory tree** — browse differences in a collapsible tree view
- **File preview** — side-by-side source/destination preview (text and images)
- **Selective sync** — select individual files or entire directories to copy
- **Copy with progress** — confirmation dialog, per-file status, size-based progress bar, ESC to cancel
- **Parallel hashing** — uses rayon for multi-core hash comparison
- **Progress bars** — scanning and comparison phases show progress

## Installation

```sh
cargo build --release
```

The binary will be at `target/release/rustsync`.

## Usage

```
rustsync <source> <destination>
```

### Examples

```sh
# Compare local directories
rustsync ~/Documents/project ~/Backup/project

# Compare with a NAS mount
rustsync /mnt/nas/photos ~/Photos
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
| `p` | Toggle preview pane |
| `Esc` / `q` | Quit (or cancel copy) |

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

## How It Works

1. **Scan destination** — walks the destination directory collecting file paths and modification times
2. **Scan source** — walks the source directory collecting file metadata
3. **Compare** — for each source file:
   - If not in destination → marked as **Missing**
   - If newer than destination → hashes both files to verify content differs → marked as **Newer**
   - If same or older → skipped

Hashing uses xxHash-64 and runs in parallel across CPU cores via rayon.

## License

MIT
