use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use chrono::{DateTime, Local};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use image::DynamicImage;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};
use ratatui_image::{Image, picker::Picker};

use crate::preview;
use crate::scanner::{DiffReason, FileDiff};
use crate::tree::{TreeNode, build_tree};

enum CachedPreview {
    Text(String),
    Image(DynamicImage),
    Unsupported,
}

#[derive(Clone)]
enum CopyStatus {
    Pending,
    Done,
    Failed(String),
}

enum DialogState {
    None,
    ConfirmCopy { scroll: ListState, button: ConfirmButton },
    Copying { items: Vec<(PathBuf, CopyStatus)>, total_bytes: u64, copied_bytes: u64, current: usize, scroll: ListState, file_bytes: u64, file_total: u64 },
}

#[derive(Clone, Copy, PartialEq)]
enum ConfirmButton { Cancel, Copy }

pub struct App {
    diffs: Vec<FileDiff>,
    tree: TreeNode,
    source: PathBuf,
    destination: PathBuf,
    state: ListState,
    picker: Picker,
    show_preview: bool,
    selected: BTreeSet<PathBuf>,
    dialog: DialogState,
    preview_cache: Option<(PathBuf, CachedPreview, CachedPreview)>,
    async_result: Arc<Mutex<Option<(PathBuf, CachedPreview, CachedPreview)>>>,
    loading_path: Option<PathBuf>,
    copy_handle: Option<(File, File)>,
}

impl App {
    pub fn new(diffs: Vec<FileDiff>, source: PathBuf, destination: PathBuf) -> Self {
        let tree = build_tree(&diffs);
        let mut state = ListState::default();
        if !tree.flatten().is_empty() {
            state.select(Some(0));
        }
        let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
        Self {
            diffs, tree, source, destination, state,
            picker, show_preview: true,
            selected: BTreeSet::new(),
            dialog: DialogState::None,
            preview_cache: None,
            async_result: Arc::new(Mutex::new(None)),
            loading_path: None,
            copy_handle: None,
        }
    }

    pub fn run(&mut self, terminal: &mut ratatui::DefaultTerminal) -> std::io::Result<()> {
        loop {
            if let Ok(mut lock) = self.async_result.try_lock() {
                if let Some(result) = lock.take() {
                    self.preview_cache = Some(result);
                    self.loading_path = None;
                }
            }
            self.ensure_preview();
            terminal.draw(|f| self.draw(f))?;

            // If copying, process one chunk at a time for progress
            if let DialogState::Copying { items, total_bytes: _, copied_bytes, current, scroll, file_bytes, file_total } = &mut self.dialog {
                if *current < items.len() {
                    // Check for cancel (ESC)
                    if event::poll(Duration::from_millis(0))? {
                        if let Event::Key(key) = event::read()? {
                            if key.code == KeyCode::Esc {
                                for i in *current..items.len() {
                                    items[i].1 = CopyStatus::Failed("Cancelled".to_string());
                                }
                                *current = items.len();
                                self.copy_handle = None;
                                continue;
                            }
                        }
                    }

                    // Open file if not already open
                    if self.copy_handle.is_none() {
                        let rel = items[*current].0.clone();
                        let src = self.source.join(&rel);
                        let dst = self.destination.join(&rel);
                        let size = self.diffs.iter().find(|d| d.rel_path == rel).map_or(0, |d| d.size);
                        *file_total = size;
                        *file_bytes = 0;
                        if let Some(parent) = dst.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        match (File::open(&src), File::create(&dst)) {
                            (Ok(r), Ok(w)) => { self.copy_handle = Some((r, w)); }
                            (Err(e), _) | (_, Err(e)) => {
                                items[*current].1 = CopyStatus::Failed(e.to_string());
                                *current += 1;
                                scroll.select(Some((*current).min(items.len().saturating_sub(1))));
                                continue;
                            }
                        }
                    }

                    // Copy a chunk
                    if let Some((reader, writer)) = &mut self.copy_handle {
                        let mut buf = [0u8; 65536];
                        match reader.read(&mut buf) {
                            Ok(0) => {
                                // File done
                                items[*current].1 = CopyStatus::Done;
                                *current += 1;
                                scroll.select(Some((*current).min(items.len().saturating_sub(1))));
                                self.copy_handle = None;
                            }
                            Ok(n) => {
                                if let Err(e) = writer.write_all(&buf[..n]) {
                                    items[*current].1 = CopyStatus::Failed(e.to_string());
                                    *current += 1;
                                    scroll.select(Some((*current).min(items.len().saturating_sub(1))));
                                    self.copy_handle = None;
                                } else {
                                    *file_bytes += n as u64;
                                    *copied_bytes += n as u64;
                                }
                            }
                            Err(e) => {
                                items[*current].1 = CopyStatus::Failed(e.to_string());
                                *current += 1;
                                scroll.select(Some((*current).min(items.len().saturating_sub(1))));
                                self.copy_handle = None;
                            }
                        }
                    }
                    continue;
                }
                // Copy done — wait for keypress to dismiss
                if event::poll(Duration::from_millis(100))? {
                    if let Event::Key(_) = event::read()? {
                        self.dialog = DialogState::None;
                    }
                }
                continue;
            }

            if !event::poll(Duration::from_millis(100))? { continue; }
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press { continue; }
                match &self.dialog {
                    DialogState::None => match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                        KeyCode::Down | KeyCode::Char('j') => self.move_down(),
                        KeyCode::Up | KeyCode::Char('k') => self.move_up(),
                        KeyCode::Home => self.state.select(Some(0)),
                        KeyCode::End => {
                            let len = self.tree.flatten().len();
                            if len > 0 { self.state.select(Some(len - 1)); }
                        }
                        KeyCode::PageDown => {
                            let page = (terminal.size().map(|s| s.height).unwrap_or(24) as usize).saturating_sub(8);
                            let len = self.tree.flatten().len();
                            let i = self.state.selected().map_or(0, |i| (i + page).min(len.saturating_sub(1)));
                            self.state.select(Some(i));
                        }
                        KeyCode::PageUp => {
                            let page = (terminal.size().map(|s| s.height).unwrap_or(24) as usize).saturating_sub(8);
                            let i = self.state.selected().map_or(0, |i| i.saturating_sub(page));
                            self.state.select(Some(i));
                        }
                        KeyCode::Enter => self.toggle_expand(),
                        KeyCode::Char(' ') => self.toggle_select(),
                        KeyCode::Char('a') => self.select_all(),
                        KeyCode::Char('u') => { self.selected.clear(); }
                        KeyCode::Char('c') => self.open_copy_dialog(),
                        KeyCode::Char('p') => {
                            self.show_preview = !self.show_preview;
                            if !self.show_preview { self.preview_cache = None; }
                        }
                        _ => {}
                    },
                    DialogState::ConfirmCopy { .. } => match key.code {
                        KeyCode::Enter => {
                            if let DialogState::ConfirmCopy { button, .. } = &self.dialog {
                                if *button == ConfirmButton::Copy {
                                    self.start_copy();
                                } else {
                                    self.dialog = DialogState::None;
                                }
                            }
                        }
                        KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                            if let DialogState::ConfirmCopy { button, .. } = &mut self.dialog {
                                *button = match button {
                                    ConfirmButton::Cancel => ConfirmButton::Copy,
                                    ConfirmButton::Copy => ConfirmButton::Cancel,
                                };
                            }
                        }
                        KeyCode::Esc => { self.dialog = DialogState::None; }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if let DialogState::ConfirmCopy { scroll, .. } = &mut self.dialog {
                                let len = self.selected.len();
                                let i = scroll.selected().map_or(0, |i| (i + 1).min(len - 1));
                                scroll.select(Some(i));
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            if let DialogState::ConfirmCopy { scroll, .. } = &mut self.dialog {
                                let i = scroll.selected().map_or(0, |i| i.saturating_sub(1));
                                scroll.select(Some(i));
                            }
                        }
                        _ => {}
                    },
                    DialogState::Copying { .. } => {}
                }
            }
        }
    }

    fn toggle_expand(&mut self) {
        let flat = self.tree.flatten();
        if let Some(idx) = self.state.selected() {
            if let Some((_, node)) = flat.get(idx) {
                if node.is_dir {
                    let path = node.rel_path.clone();
                    self.tree.toggle_expand(&path);
                }
            }
        }
    }

    fn move_down(&mut self) {
        let len = self.tree.flatten().len();
        if len > 0 {
            let i = self.state.selected().map_or(0, |i| (i + 1).min(len - 1));
            self.state.select(Some(i));
        }
    }

    fn move_up(&mut self) {
        let i = self.state.selected().map_or(0, |i| i.saturating_sub(1));
        self.state.select(Some(i));
    }

    fn toggle_select(&mut self) {
        let flat = self.tree.flatten();
        let Some(idx) = self.state.selected() else { return };
        let Some((_, node)) = flat.get(idx) else { return };
        if node.is_dir {
            // Select/deselect all files in this directory
            let paths: Vec<PathBuf> = self.collect_file_paths(node);
            let all_selected = paths.iter().all(|p| self.selected.contains(p));
            for p in paths {
                if all_selected { self.selected.remove(&p); } else { self.selected.insert(p); }
            }
        } else {
            let path = node.rel_path.clone();
            if !self.selected.remove(&path) { self.selected.insert(path); }
        }
    }

    fn collect_file_paths(&self, node: &TreeNode) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if !node.is_dir {
            paths.push(node.rel_path.clone());
        } else {
            for child in node.children.values() {
                paths.extend(self.collect_file_paths(child));
            }
        }
        paths
    }

    fn select_all(&mut self) {
        for diff in &self.diffs {
            self.selected.insert(diff.rel_path.clone());
        }
    }

    fn open_copy_dialog(&mut self) {
        if self.selected.is_empty() { return; }
        self.dialog = DialogState::ConfirmCopy {
            scroll: ListState::default().with_selected(Some(0)),
            button: ConfirmButton::Cancel,
        };
    }

    fn start_copy(&mut self) {
        let items: Vec<(PathBuf, CopyStatus)> = self.selected.iter()
            .map(|p| (p.clone(), CopyStatus::Pending))
            .collect();
        let total_bytes: u64 = items.iter()
            .filter_map(|(p, _)| self.diffs.iter().find(|d| &d.rel_path == p))
            .map(|d| d.size)
            .sum();
        self.dialog = DialogState::Copying { items, total_bytes, copied_bytes: 0, current: 0, scroll: ListState::default().with_selected(Some(0)), file_bytes: 0, file_total: 0 };
    }

    fn selected_diff(&self) -> Option<&FileDiff> {
        let flat = self.tree.flatten();
        self.state.selected()
            .and_then(|i| flat.get(i))
            .and_then(|(_, node)| node.diff.as_ref())
    }

    fn ensure_preview(&mut self) {
        if !self.show_preview { return; }
        let Some(diff) = self.selected_diff() else {
            self.preview_cache = None;
            return;
        };
        let rel_path = diff.rel_path.clone();
        let src_path = self.source.join(&rel_path);
        // Already cached or loading
        if self.preview_cache.as_ref().is_some_and(|(p, _, _)| *p == src_path) { return; }
        if self.loading_path.as_ref().is_some_and(|p| *p == src_path) { return; }

        self.loading_path = Some(src_path.clone());
        let dst_path = self.destination.join(&rel_path);
        let result = Arc::clone(&self.async_result);

        thread::spawn(move || {
            let src_preview = load_preview(&src_path);
            let dst_preview = if dst_path.exists() {
                load_preview(&dst_path)
            } else {
                CachedPreview::Unsupported
            };
            if let Ok(mut lock) = result.lock() {
                *lock = Some((src_path, src_preview, dst_preview));
            }
        });
    }

    fn draw(&mut self, f: &mut Frame) {
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(5), Constraint::Length(4), Constraint::Length(1)])
            .split(f.area());

        let main = if self.show_preview {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
                .split(outer[0])
        } else {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(100), Constraint::Length(0)])
                .split(outer[0])
        };

        // File tree
        let flat = self.tree.flatten();
        let items: Vec<ListItem> = flat.iter().map(|(depth, node)| {
            let indent = "  ".repeat(*depth);
            if node.is_dir {
                let icon = if node.expanded { "▼ " } else { "▶ " };
                let count = node.file_count();
                let size = format_size(node.total_size());
                let all_selected = self.collect_file_paths(node).iter().all(|p| self.selected.contains(p));
                let mark = if all_selected && count > 0 { "✓" } else { " " };
                let label = format!("{mark}{indent}{icon}{} [{}, {}]", node.name, count, size);
                return ListItem::new(Line::from(Span::styled(label, Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD))));
            }
            let diff = node.diff.as_ref().unwrap();
            let (icon, style) = match &diff.reason {
                DiffReason::Missing => ("N", Style::default().fg(Color::Green)),
                DiffReason::Newer { .. } => ("M", Style::default().fg(Color::Yellow)),
            };
            let marked = self.selected.contains(&node.rel_path);
            let mark = if marked { "✓" } else { " " };
            let size = format_size(diff.size);
            ListItem::new(Line::from(vec![
                Span::styled(format!("{mark}"), if marked { Style::default().fg(Color::Green) } else { Style::default() }),
                Span::styled(format!(" {icon} "), style),
                Span::styled(format!("{indent}  "), Style::default()),
                Span::styled(format!("{:>8} ", size), Style::default().fg(Color::DarkGray)),
                Span::styled(node.name.clone(), style),
            ]))
        }).collect();

        let title = format!(
            " {} → {} ({} files) ",
            self.source.file_name().unwrap_or_default().to_string_lossy(),
            self.destination.file_name().unwrap_or_default().to_string_lossy(),
            self.diffs.len(),
        );
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(title).border_style(Style::default().fg(Color::Cyan)))
            .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));
        f.render_stateful_widget(list, main[0], &mut self.state);

        let mut scrollbar_state = ScrollbarState::new(flat.len())
            .position(self.state.selected().unwrap_or(0));
        f.render_stateful_widget(Scrollbar::new(ScrollbarOrientation::VerticalRight), main[0], &mut scrollbar_state);

        // Preview pane (right side, split top/bottom for source/dest)
        if self.show_preview {
            let preview_split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(main[1]);

            self.draw_single_preview(f, preview_split[0], " Source ", true);
            self.draw_single_preview(f, preview_split[1], " Destination ", false);
        }

        // Details
        let details = self.build_details();
        let details_widget = Paragraph::new(details)
            .block(Block::default().borders(Borders::ALL).title(" Details "));
        f.render_widget(details_widget, outer[1]);

        // Status bar
        let total_size: u64 = self.diffs.iter().map(|d| d.size).sum();
        let missing = self.diffs.iter().filter(|d| matches!(d.reason, DiffReason::Missing)).count();
        let newer = self.diffs.iter().filter(|d| matches!(d.reason, DiffReason::Newer { .. })).count();
        let sel_count = self.selected.len();
        let sel_size: u64 = self.selected.iter()
            .filter_map(|p| self.diffs.iter().find(|d| &d.rel_path == p))
            .map(|d| d.size).sum();
        let bar = Line::from(vec![
            Span::styled(format!(" {missing} new, {newer} modified"), Style::default().fg(Color::White)),
            Span::raw("  "),
            Span::styled(format!("Total: {}", format_size(total_size)), Style::default().fg(Color::Magenta)),
            Span::raw("  "),
            Span::styled(
                if sel_count > 0 { format!("{sel_count} sel ({})", format_size(sel_size)) } else { "0 sel".into() },
                if sel_count > 0 { Style::default().fg(Color::Green).add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::DarkGray) },
            ),
            Span::raw("  "),
            Span::styled("[Space]Sel [a]All [u]Desel [c]Copy [Enter]Expand [p]Preview [q]Quit", Style::default().fg(Color::DarkGray)),
        ]);
        f.render_widget(Paragraph::new(bar).style(Style::default().bg(Color::Black)), outer[2]);

        // Dialog overlay
        self.draw_dialog(f);
    }

    fn draw_dialog(&mut self, f: &mut Frame) {
        match &mut self.dialog {
            DialogState::None => {}
            DialogState::ConfirmCopy { scroll, button } => {
                let area = centered_rect(60, 60, f.area());
                f.render_widget(Clear, area);
                let layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(3), Constraint::Length(3)])
                    .split(area);

                let items: Vec<ListItem> = self.selected.iter().map(|p| {
                    ListItem::new(Line::from(Span::styled(
                        format!("  {}", p.display()), Style::default().fg(Color::Cyan),
                    )))
                }).collect();
                let sel_size: u64 = self.selected.iter()
                    .filter_map(|p| self.diffs.iter().find(|d| &d.rel_path == p))
                    .map(|d| d.size).sum();
                let title = format!(" Copy {} files ({})? ", self.selected.len(), format_size(sel_size));
                let list = List::new(items)
                    .block(Block::default().borders(Borders::ALL).title(title).border_style(Style::default().fg(Color::Cyan)))
                    .highlight_style(Style::default().bg(Color::DarkGray));
                f.render_stateful_widget(list, layout[0], scroll);

                let cancel_style = if *button == ConfirmButton::Cancel {
                    Style::default().bg(Color::White).fg(Color::Black).add_modifier(Modifier::BOLD)
                } else { Style::default().fg(Color::White) };
                let copy_style = if *button == ConfirmButton::Copy {
                    Style::default().bg(Color::Green).fg(Color::Black).add_modifier(Modifier::BOLD)
                } else { Style::default().fg(Color::Green) };
                let buttons = Line::from(vec![
                    Span::raw("  "),
                    Span::styled(" [ Cancel ] ", cancel_style),
                    Span::raw("    "),
                    Span::styled(" [ Copy ] ", copy_style),
                ]);
                f.render_widget(Paragraph::new(buttons).block(Block::default().borders(Borders::ALL)), layout[1]);
            }
            DialogState::Copying { items, total_bytes, copied_bytes, current, scroll, file_bytes, file_total } => {
                let area = centered_rect(70, 70, f.area());
                f.render_widget(Clear, area);
                let layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(3), Constraint::Length(3), Constraint::Length(3)])
                    .split(area);

                let file_items: Vec<ListItem> = items.iter().enumerate().map(|(i, (p, status))| {
                    let (icon, style) = match status {
                        CopyStatus::Pending => ("○", Style::default().fg(Color::DarkGray)),
                        CopyStatus::Done => ("✓", Style::default().fg(Color::Green)),
                        CopyStatus::Failed(_) => ("✗", Style::default().fg(Color::Red)),
                    };
                    let icon = if i == *current && *current < items.len() { "►" } else { icon };
                    let text = match status {
                        CopyStatus::Failed(e) => format!("{icon} {} — {e}", p.display()),
                        _ => format!("{icon} {}", p.display()),
                    };
                    ListItem::new(Line::from(Span::styled(text, style)))
                }).collect();

                let done = *current >= items.len();
                let title = if done { " Copy complete (press any key) " } else { " Copying... (ESC to cancel) " };
                let list = List::new(file_items)
                    .block(Block::default().borders(Borders::ALL).title(title).border_style(Style::default().fg(Color::Cyan)))
                    .highlight_style(Style::default().add_modifier(Modifier::BOLD));
                f.render_stateful_widget(list, layout[0], scroll);

                // Per-file progress
                let file_ratio = if *file_total > 0 { *file_bytes as f64 / *file_total as f64 } else { 0.0 };
                let file_label = if *current < items.len() {
                    format!("{} / {}", format_size(*file_bytes), format_size(*file_total))
                } else {
                    "Done".to_string()
                };
                let file_gauge = Gauge::default()
                    .block(Block::default().borders(Borders::ALL).title(" File "))
                    .gauge_style(Style::default().fg(Color::Yellow))
                    .ratio(file_ratio.min(1.0))
                    .label(file_label);
                f.render_widget(file_gauge, layout[1]);

                // Total progress
                let total_ratio = if *total_bytes > 0 { *copied_bytes as f64 / *total_bytes as f64 } else { 1.0 };
                let total_gauge = Gauge::default()
                    .block(Block::default().borders(Borders::ALL).title(" Total "))
                    .gauge_style(Style::default().fg(Color::Green))
                    .ratio(total_ratio.min(1.0))
                    .label(format!("{} / {}", format_size(*copied_bytes), format_size(*total_bytes)));
                f.render_widget(total_gauge, layout[2]);
            }
        }
    }

    fn draw_single_preview(&mut self, f: &mut Frame, area: ratatui::layout::Rect, title: &str, is_source: bool) {
        let block = Block::default().borders(Borders::ALL).title(title);
        let cached = self.preview_cache.as_ref().map(|(_, src, dst)| {
            if is_source { src } else { dst }
        });
        match cached {
            Some(CachedPreview::Text(text)) => {
                let para = Paragraph::new(text.clone()).block(block).wrap(Wrap { trim: false });
                f.render_widget(para, area);
            }
            Some(CachedPreview::Image(img)) => {
                let inner = block.inner(area);
                f.render_widget(block, area);
                if inner.width > 0 && inner.height > 0 {
                    if let Some(proto) = preview::make_image_protocol(&mut self.picker, img, inner) {
                        f.render_widget(Image::new(&proto), inner);
                    }
                }
            }
            Some(CachedPreview::Unsupported) | None => {
                let msg = if !is_source {
                    if self.selected_diff().is_some_and(|d| matches!(d.reason, DiffReason::Missing)) {
                        "New file — does not exist in destination"
                    } else {
                        "No preview available"
                    }
                } else {
                    "No preview available"
                };
                let para = Paragraph::new(msg).block(block).style(Style::default().fg(Color::DarkGray));
                f.render_widget(para, area);
            }
        }
    }

    fn build_details(&self) -> Vec<Line<'static>> {
        let Some(diff) = self.selected_diff() else {
            return vec![Line::from("No file selected")];
        };
        let src_path = self.source.join(&diff.rel_path);
        let meta = std::fs::metadata(&src_path).ok();
        let created = meta.as_ref()
            .and_then(|m| m.created().ok())
            .map(|t| DateTime::<Local>::from(t).format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "N/A".into());
        let modified = meta.as_ref()
            .and_then(|m| m.modified().ok())
            .map(|t| DateTime::<Local>::from(t).format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "N/A".into());
        let mut lines = vec![
            Line::from(vec![
                Span::styled("File: ", Style::default().fg(Color::Cyan)),
                Span::raw(diff.rel_path.display().to_string()),
                Span::raw(format!("  ({})", format_size(diff.size))),
            ]),
            Line::from(vec![
                Span::styled("Created: ", Style::default().fg(Color::DarkGray)),
                Span::raw(created),
                Span::raw("  "),
                Span::styled("Modified: ", Style::default().fg(Color::DarkGray)),
                Span::raw(modified),
            ]),
        ];
        match &diff.reason {
            DiffReason::Missing => {
                lines.push(Line::from(vec![
                    Span::styled("Status: ", Style::default().fg(Color::Green)),
                    Span::raw("New file (not in destination)"),
                ]));
            }
            DiffReason::Newer { source_mod, dest_mod } => {
                let src_t = DateTime::<Local>::from(*source_mod).format("%Y-%m-%d %H:%M:%S").to_string();
                let dst_t = DateTime::<Local>::from(*dest_mod).format("%Y-%m-%d %H:%M:%S").to_string();
                lines.push(Line::from(vec![
                    Span::styled("Status: ", Style::default().fg(Color::Yellow)),
                    Span::raw(format!("Modified (source: {src_t}, dest: {dst_t})")),
                ]));
            }
        }
        lines
    }
}

fn load_preview(path: &std::path::Path) -> CachedPreview {
    if preview::is_image(path) {
        match preview::load_image_thumbnail(path) {
            Some(img) => CachedPreview::Image(img),
            None => CachedPreview::Unsupported,
        }
    } else if preview::is_text(path) {
        match preview::load_text_preview(path) {
            Some(text) => CachedPreview::Text(text),
            None => CachedPreview::Unsupported,
        }
    } else {
        CachedPreview::Unsupported
    }
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    for unit in UNITS {
        if size < 1024.0 {
            return format!("{size:.1} {unit}");
        }
        size /= 1024.0;
    }
    format!("{size:.1} PB")
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(v[1])[1]
}
