//! Async directory browser and file-type presentation for staged attachments.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    widgets::{Block, Clear, Paragraph, Row, Table, TableState},
};
use tokio::sync::mpsc;

use crate::{event::Event, input::Input};

#[derive(Debug, Clone)]
pub(crate) struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

pub(crate) fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = "B";
    for next in ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"] {
        value /= 1024.0;
        unit = next;
        if value < 1024.0 {
            break;
        }
    }
    format!("{value:.1} {unit}")
}

pub(crate) fn file_icon(path: &Path, directory: bool, nerd: bool) -> &'static str {
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let kind = if directory {
        0
    } else {
        match extension.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "svg" | "heic" => 1,
            "pdf" => 2,
            "zip" | "gz" | "xz" | "zst" | "7z" | "rar" | "tar" | "bz2" => 3,
            "mp3" | "ogg" | "wav" | "flac" | "m4a" | "opus" => 4,
            "mp4" | "mkv" | "webm" | "avi" | "mov" => 5,
            "txt" | "md" | "log" | "csv" => 6,
            "rs" | "py" | "js" | "ts" | "c" | "cpp" | "sh" | "json" | "toml" | "yaml" | "yml"
            | "html" => 7,
            "doc" | "docx" | "odt" | "rtf" => 8,
            "xls" | "xlsx" | "ods" => 9,
            "ppt" | "pptx" | "odp" => 10,
            _ => 11,
        }
    };
    if nerd {
        [
            "\u{f07b}", "\u{f1c5}", "\u{f1c1}", "\u{f1c6}", "\u{f1c7}", "\u{f1c8}", "\u{f0f6}",
            "\u{f1c9}", "\u{f1c2}", "\u{f1c3}", "\u{f1c4}", "\u{f15b}",
        ][kind]
    } else {
        [
            "[DIR]", "[IMG]", "[PDF]", "[ARC]", "[AUD]", "[VID]", "[TXT]", "[CODE]", "[DOC]",
            "[XLS]", "[PPT]", "[FILE]",
        ][kind]
    }
}

pub(crate) fn terminal_text(text: &str) -> String {
    strip_ansi_escapes::strip_str(text)
        .chars()
        .map(|c| if c.is_control() { '�' } else { c })
        .collect()
}

fn read_directory(path: &Path) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::new();
    for entry in
        std::fs::read_dir(path).with_context(|| format!("Cannot read {}", path.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let metadata = std::fs::metadata(&path).ok();
        entries.push(FileEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            path,
            is_dir: metadata.as_ref().is_some_and(|m| m.is_dir()),
            size: metadata.as_ref().map_or(0, |m| m.len()),
            modified: metadata.and_then(|m| m.modified().ok()),
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

pub(crate) enum PickerAction {
    None,
    Cancel,
    Selected(Vec<PathBuf>),
    Preview(PathBuf),
    OpenExternal(PathBuf),
}

type DirectoryResult = Arc<Mutex<Option<Result<Vec<FileEntry>>>>>;

pub(crate) struct FilePicker {
    pub directory: PathBuf,
    home: PathBuf,
    screenshot_dir: PathBuf,
    show_hidden: bool,
    sort_modified: bool,
    all_entries: Vec<FileEntry>,
    entries: Vec<FileEntry>,
    state: TableState,
    marked_paths: Vec<PathBuf>,
    result: DirectoryResult,
    loading: bool,
    pub error: Option<String>,
    path_input: Option<Input>,
    tx: mpsc::UnboundedSender<Event>,
}

impl FilePicker {
    pub(crate) fn new(home: PathBuf, tx: mpsc::UnboundedSender<Event>) -> Self {
        let mut picker = Self {
            directory: home.clone(),
            screenshot_dir: crate::screenshot::default_screenshot_dir(&home),
            home,
            show_hidden: false,
            sort_modified: false,
            all_entries: Vec::new(),
            entries: Vec::new(),
            state: TableState::default(),
            marked_paths: Vec::new(),
            result: Default::default(),
            loading: false,
            error: None,
            path_input: None,
            tx,
        };
        picker.navigate(picker.directory.clone());
        picker
    }

    pub(crate) fn set_screenshot_dir(&mut self, configured: Option<&Path>) {
        self.screenshot_dir = crate::screenshot::resolve_screenshot_dir(configured, &self.home);
    }

    fn refresh_entries(&mut self) {
        let selected = self
            .state
            .selected()
            .and_then(|index| self.entries.get(index))
            .map(|entry| entry.path.clone());
        self.entries = self
            .all_entries
            .iter()
            .filter(|entry| self.show_hidden || !entry.name.starts_with('.'))
            .cloned()
            .collect();
        self.entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| {
                    if self.sort_modified {
                        b.modified.cmp(&a.modified)
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.name.cmp(&b.name))
        });
        self.state = TableState::default();
        self.state.select(
            selected
                .and_then(|path| self.entries.iter().position(|entry| entry.path == path))
                .or_else(|| (!self.entries.is_empty()).then_some(0)),
        );
    }

    fn navigate(&mut self, path: PathBuf) {
        self.directory = path.clone();
        self.entries.clear();
        self.all_entries.clear();
        self.state = TableState::default();
        self.loading = true;
        self.error = None;
        let result: DirectoryResult = Default::default();
        self.result = result.clone();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let value = read_directory(&path);
            *result.lock().unwrap_or_else(|e| e.into_inner()) = Some(value);
            let _ = tx.send(Event::FilePickerUpdated);
        });
    }

    pub(crate) fn poll(&mut self) {
        let result = self.result.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(result) = result {
            self.loading = false;
            match result {
                Ok(entries) => {
                    self.all_entries = entries;
                    self.refresh_entries();
                }
                Err(error) => self.error = Some(error.to_string()),
            }
        }
    }

    fn choose(&mut self, path: PathBuf) -> PickerAction {
        if path.is_dir() {
            self.navigate(path);
            PickerAction::None
        } else if path.is_file() {
            PickerAction::Selected(if self.marked_paths.is_empty() {
                vec![path]
            } else {
                self.marked_paths.clone()
            })
        } else {
            self.error = Some("Select an existing regular file".into());
            PickerAction::None
        }
    }

    pub(crate) fn set_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
    }

    pub(crate) fn paste(&mut self, text: &str) {
        let input = self.path_input.get_or_insert_with(Input::default);
        for c in text
            .trim_end_matches(['\r', '\n'])
            .chars()
            .filter(|c| !c.is_control())
        {
            input.put_char(c);
        }
    }

    pub(crate) fn on_key(&mut self, key: KeyEvent) -> PickerAction {
        self.poll();
        if let Some(input) = self.path_input.as_mut() {
            match key.code {
                KeyCode::Esc => self.path_input = None,
                KeyCode::Backspace => input.on_backspace(),
                KeyCode::Left => input.on_left(),
                KeyCode::Right => input.on_right(),
                KeyCode::Enter => {
                    let typed = input.data.clone();
                    self.path_input = None;
                    let path = crate::screenshot::resolve_screenshot_dir(
                        Some(Path::new(&typed)),
                        &self.home,
                    );
                    let path = if path.is_absolute() {
                        path
                    } else {
                        self.directory.join(path)
                    };
                    return self.choose(path);
                }
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && !c.is_control() =>
                {
                    input.put_char(c)
                }
                _ => {}
            }
            return PickerAction::None;
        }
        if key.code == KeyCode::Esc {
            self.marked_paths.clear();
            return PickerAction::Cancel;
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            if let Some(entry) = self
                .state
                .selected()
                .and_then(|index| self.entries.get(index))
            {
                match key.code {
                    KeyCode::Char('p' | 'P') => return PickerAction::Preview(entry.path.clone()),
                    KeyCode::Char('o' | 'O') if !entry.is_dir && entry.path.is_file() => {
                        return PickerAction::OpenExternal(entry.path.clone());
                    }
                    _ => {}
                }
            }
            return PickerAction::None;
        }
        match key.code {
            KeyCode::Char(' ') if !self.loading => {
                if let Some(entry) = self
                    .state
                    .selected()
                    .and_then(|index| self.entries.get(index))
                    && !entry.is_dir
                    && entry.path.is_file()
                {
                    if let Some(index) = self
                        .marked_paths
                        .iter()
                        .position(|path| path == &entry.path)
                    {
                        self.marked_paths.remove(index);
                        self.error = None;
                    } else if self.marked_paths.len() < 10 {
                        self.marked_paths.push(entry.path.clone());
                        self.error = None;
                    } else {
                        self.error = Some("Select at most 10 files".into());
                    }
                }
            }
            KeyCode::Char('/') => {
                let mut input = Input::default();
                input.put_char('/');
                self.path_input = Some(input);
            }
            KeyCode::Char('~') => self.navigate(self.home.clone()),
            KeyCode::Char('p') => self.navigate(self.home.join("Pictures")),
            KeyCode::Char('s') => self.navigate(self.screenshot_dir.clone()),
            KeyCode::Char('.') => {
                self.show_hidden = !self.show_hidden;
                self.refresh_entries();
            }
            KeyCode::Char('t') => {
                self.sort_modified = !self.sort_modified;
                self.refresh_entries();
            }
            KeyCode::Right if !self.loading => {
                if let Some(entry) = self
                    .state
                    .selected()
                    .and_then(|index| self.entries.get(index))
                    && entry.is_dir
                {
                    self.navigate(entry.path.clone());
                }
            }
            KeyCode::Backspace | KeyCode::Left => {
                if let Some(parent) = self.directory.parent() {
                    self.navigate(parent.to_owned());
                }
            }
            KeyCode::Up | KeyCode::PageUp => self.state.select(Some(
                self.state
                    .selected()
                    .unwrap_or(0)
                    .saturating_sub(if key.code == KeyCode::PageUp { 10 } else { 1 }),
            )),
            KeyCode::Down | KeyCode::PageDown => self.state.select(Some(
                self.state
                    .selected()
                    .unwrap_or(0)
                    .saturating_add(if key.code == KeyCode::PageDown { 10 } else { 1 })
                    .min(self.entries.len().saturating_sub(1)),
            )),
            KeyCode::Home => self.state.select((!self.entries.is_empty()).then_some(0)),
            KeyCode::End => self.state.select(self.entries.len().checked_sub(1)),
            KeyCode::Enter if !self.loading => {
                if let Some(entry) = self.state.selected().and_then(|idx| self.entries.get(idx)) {
                    return self.choose(entry.path.clone());
                }
                if !self.marked_paths.is_empty() {
                    return PickerAction::Selected(self.marked_paths.clone());
                }
            }
            _ => {}
        }
        PickerAction::None
    }

    pub(crate) fn is_path_entry(&self) -> bool {
        self.path_input.is_some()
    }

    pub(crate) fn draw(&mut self, frame: &mut Frame, nerd: bool, area: ratatui::layout::Rect) {
        self.poll();
        let area = ratatui::layout::Rect::new(
            area.x + area.width / 20,
            area.y + area.height / 20,
            area.width.saturating_sub(area.width / 10),
            area.height.saturating_sub(area.height / 10),
        );
        frame.render_widget(Clear, area);
        let block = Block::bordered()
            .title(format!(
                "Attach {}/10: {}",
                self.marked_paths.len(),
                terminal_text(&self.directory.display().to_string())
            ))
            .title_bottom("/: path | ~: home | p: Pictures | s: screenshots | ←/→: folders | Space: mark | Enter: add | Alt+P: preview | Alt+O: open | Esc");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let areas = Layout::vertical([Constraint::Min(0), Constraint::Length(3)]).split(inner);
        let selected = self.state.selected().unwrap_or(0);
        let visible_count = areas[0].height.saturating_sub(1).max(1) as usize;
        let first = selected.saturating_sub(visible_count.saturating_sub(1));
        let rows = self
            .entries
            .iter()
            .skip(first)
            .take(visible_count)
            .map(|entry| {
                let name = format!(
                    "{} {} {}",
                    if self.marked_paths.contains(&entry.path) {
                        "[x]"
                    } else {
                        "[ ]"
                    },
                    file_icon(&entry.path, entry.is_dir, nerd),
                    terminal_text(&entry.name)
                );
                let date = entry
                    .modified
                    .map(|at| {
                        DateTime::<Local>::from(at)
                            .format("%Y-%m-%d %H:%M")
                            .to_string()
                    })
                    .unwrap_or_else(|| "—".into());
                let size = if entry.is_dir {
                    "—".into()
                } else {
                    human_size(entry.size)
                };
                Row::new([name, date, size])
            });
        let table = Table::new(
            rows,
            [
                Constraint::Min(8),
                Constraint::Length(16),
                Constraint::Length(10),
            ],
        )
        .header(Row::new(["Name", "Modified", "Size"]).style(Style::default().fg(Color::Yellow)))
        .row_highlight_style(Style::default().reversed());
        let mut visible_state = TableState::default();
        visible_state.select(self.state.selected().map(|selected| selected - first));
        frame.render_stateful_widget(table, areas[0], &mut visible_state);
        if let Some(input) = self.path_input.as_ref() {
            frame.render_widget(
                Paragraph::new(format!("Path: {}", terminal_text(&input.data))),
                areas[1],
            );
            if areas[1].width > 0 && areas[1].height > 0 {
                frame.set_cursor_position((
                    areas[1].x
                        + (6 + input.cursor.col).min(areas[1].width.saturating_sub(1) as usize)
                            as u16,
                    areas[1].y,
                ));
            }
        } else {
            let settings = format!(
                "Space: mark | Alt+P: image | Alt+O: MIME app\n.: hidden {} | t: sort {} (folders first)",
                if self.show_hidden { "on" } else { "off" },
                if self.sort_modified {
                    "modified, newest first"
                } else {
                    "name"
                }
            );
            let status = self.error.as_deref().unwrap_or(if self.loading {
                "Loading directory..."
            } else {
                "File selection only adds an attachment. Press Enter again in chat to send."
            });
            frame.render_widget(
                Paragraph::new(format!("{settings}\n{}", terminal_text(status))),
                areas[1],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_listing_has_directories_first_dates_and_human_sizes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("z file.pdf"), vec![0u8; 1536]).unwrap();
        std::fs::write(dir.path().join("Alpha.txt"), b"hello").unwrap();
        std::fs::create_dir(dir.path().join("Subfolder")).unwrap();
        let entries = read_directory(dir.path()).unwrap();
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].name, "Alpha.txt");
        assert_eq!(entries[2].name, "z file.pdf");
        assert_eq!(human_size(entries[2].size), "1.5 KiB");
        assert_eq!(human_size(5), "5 B");
        assert!(entries[2].modified.is_some());
        assert_ne!(
            file_icon(&entries[1].path, false, true),
            file_icon(&entries[2].path, false, true)
        );
        assert_eq!(file_icon(&entries[2].path, false, false), "[PDF]");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn selection_enters_directory_and_stages_only_after_file_enter() {
        let home = tempfile::tempdir().unwrap();
        let nested = home.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("with spaces.txt"), b"data").unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut picker = FilePicker::new(home.path().to_owned(), tx);
        rx.recv().await.unwrap();
        picker.poll();
        assert!(matches!(
            picker.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            PickerAction::None
        ));
        rx.recv().await.unwrap();
        picker.poll();
        assert_eq!(picker.directory, nested);
        let selected = picker.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(selected, PickerAction::Selected(paths) if paths == vec![nested.join("with spaces.txt")])
        );
        assert!(matches!(
            picker.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            PickerAction::Cancel
        ));
    }
    fn press(picker: &mut FilePicker, code: KeyCode) -> PickerAction {
        picker.on_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn hidden_toggle_sort_and_arrows_preserve_valid_selection() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("folder")).unwrap();
        std::fs::write(home.path().join(".hidden"), b"secret").unwrap();
        for (name, age) in [("a-old", 10), ("z-new", 20), (".hidden", 0)] {
            let file = std::fs::File::create(home.path().join(name)).unwrap();
            file.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(age))
                .unwrap();
        }
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut picker = FilePicker::new(home.path().to_owned(), tx);
        rx.recv().await.unwrap();
        picker.poll();
        assert!(!picker.entries.iter().any(|entry| entry.name == ".hidden"));
        press(&mut picker, KeyCode::Char('.'));
        assert!(picker.entries.iter().any(|entry| entry.name == ".hidden"));
        press(&mut picker, KeyCode::End);
        let selected = picker.entries[picker.state.selected().unwrap()]
            .path
            .clone();
        press(&mut picker, KeyCode::Char('t'));
        assert!(picker.entries[0].is_dir);
        assert_eq!(picker.entries[1].name, "z-new");
        assert_eq!(
            picker.entries[picker.state.selected().unwrap()].path,
            selected
        );
        press(&mut picker, KeyCode::Char('t'));
        press(&mut picker, KeyCode::Char('.'));
        assert_eq!(picker.entries[1].name, "a-old");
        press(&mut picker, KeyCode::End);
        assert!(matches!(
            press(&mut picker, KeyCode::Right),
            PickerAction::None
        ));
        assert_eq!(picker.directory, home.path());
        press(&mut picker, KeyCode::Home);
        press(&mut picker, KeyCode::Right);
        rx.recv().await.unwrap();
        picker.poll();
        assert_eq!(picker.directory, home.path().join("folder"));
        press(&mut picker, KeyCode::Left);
        rx.recv().await.unwrap();
        picker.poll();
        assert_eq!(picker.directory, home.path());
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn quick_directories_use_config_and_do_not_intercept_path_typing() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("Pictures")).unwrap();
        std::fs::create_dir(home.path().join("custom shots")).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut picker = FilePicker::new(home.path().to_owned(), tx);
        picker.set_screenshot_dir(Some(Path::new("~/custom shots")));
        rx.recv().await.unwrap();
        picker.poll();
        press(&mut picker, KeyCode::Char('p'));
        rx.recv().await.unwrap();
        picker.poll();
        assert_eq!(picker.directory, home.path().join("Pictures"));
        press(&mut picker, KeyCode::Char('s'));
        rx.recv().await.unwrap();
        picker.poll();
        assert_eq!(picker.directory, home.path().join("custom shots"));
        press(&mut picker, KeyCode::Char('~'));
        rx.recv().await.unwrap();
        picker.poll();
        assert_eq!(picker.directory, home.path());
        press(&mut picker, KeyCode::Char('/'));
        for c in "p.s~t".chars() {
            press(&mut picker, KeyCode::Char(c));
        }
        assert_eq!(picker.path_input.as_ref().unwrap().data, "/p.s~t");
        assert_eq!(picker.directory, home.path());
        assert!(!picker.show_hidden);
        assert!(!picker.sort_modified);
        press(&mut picker, KeyCode::Esc);
        picker.set_screenshot_dir(None);
        press(&mut picker, KeyCode::Char('s'));
        rx.recv().await.unwrap();
        picker.poll();
        assert_eq!(picker.directory, home.path().join("Pictures"));
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn marks_are_limited_to_files_and_survive_navigation_and_filtering() {
        let home = tempfile::tempdir().unwrap();
        let nested = home.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("inside"), b"data").unwrap();
        for index in 0..11 {
            std::fs::write(home.path().join(format!("file{index:02}")), b"data").unwrap();
        }
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut picker = FilePicker::new(home.path().to_owned(), tx);
        rx.recv().await.unwrap();
        picker.poll();
        press(&mut picker, KeyCode::Char(' '));
        assert!(
            picker.marked_paths.is_empty(),
            "directories cannot be marked"
        );
        press(&mut picker, KeyCode::Enter);
        rx.recv().await.unwrap();
        picker.poll();
        press(&mut picker, KeyCode::Char(' '));
        assert_eq!(picker.marked_paths, vec![nested.join("inside")]);
        press(&mut picker, KeyCode::Left);
        rx.recv().await.unwrap();
        picker.poll();
        for index in 1..=11 {
            picker.state.select(Some(index));
            press(&mut picker, KeyCode::Char(' '));
        }
        assert_eq!(picker.marked_paths.len(), 10);
        assert!(picker.error.as_deref().unwrap().contains("10"));
        let marked = picker.marked_paths.clone();
        press(&mut picker, KeyCode::Char('.'));
        press(&mut picker, KeyCode::Char('t'));
        assert_eq!(picker.marked_paths, marked);
        assert!(
            matches!(press(&mut picker, KeyCode::Enter), PickerAction::Selected(paths) if paths == marked)
        );
        press(&mut picker, KeyCode::Char(' '));
        assert_eq!(
            picker.marked_paths.len(),
            10,
            "unmarked file at limit remains unmarked"
        );
        let marked_index = picker
            .entries
            .iter()
            .position(|entry| marked.contains(&entry.path))
            .unwrap();
        picker.state.select(Some(marked_index));
        press(&mut picker, KeyCode::Char(' '));
        assert_eq!(picker.marked_paths.len(), 9);
        press(&mut picker, KeyCode::Esc);
        assert!(picker.marked_paths.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn preview_and_open_use_highlight_and_path_input_types_literal_controls() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("folder")).unwrap();
        std::fs::write(home.path().join("a"), b"a").unwrap();
        std::fs::write(home.path().join("b"), b"b").unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut picker = FilePicker::new(home.path().to_owned(), tx);
        rx.recv().await.unwrap();
        picker.poll();
        assert!(matches!(
            picker.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::ALT)),
            PickerAction::None
        ));
        assert!(
            matches!(picker.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT)), PickerAction::Preview(path) if path == home.path().join("folder"))
        );
        picker.state.select(Some(1));
        press(&mut picker, KeyCode::Char(' '));
        picker.state.select(Some(2));
        assert!(
            matches!(picker.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT)), PickerAction::Preview(path) if path == home.path().join("b"))
        );
        assert!(
            matches!(picker.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::ALT)), PickerAction::OpenExternal(path) if path == home.path().join("b"))
        );
        press(&mut picker, KeyCode::Char('/'));
        for character in "p o.s~t".chars() {
            press(&mut picker, KeyCode::Char(character));
        }
        picker.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::ALT));
        assert_eq!(picker.path_input.as_ref().unwrap().data, "/p o.s~t");
        assert_eq!(picker.marked_paths, vec![home.path().join("a")]);
    }
}
