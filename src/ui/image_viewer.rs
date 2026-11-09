//! Modal image viewing: original-sized decoding, background graphics encoding and GUI launch.

use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    Arc, LazyLock, Mutex,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, bail};
use image::DynamicImage;
use ratatui::{
    Frame,
    layout::{Rect, Size},
    style::{Color, Style},
    widgets::{Block, Paragraph, Wrap},
};
use ratatui_image::{
    Image, Resize,
    picker::{Picker, ProtocolType},
    protocol::Protocol,
};
use tokio::sync::{Semaphore, mpsc};

use crate::config::{ImageViewerConfig, ImageViewerMode, ImageViewerProtocol};
use crate::event::Event;

static JOB_SLOTS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(2)));

#[derive(Default)]
struct ViewerData {
    source: Option<(Arc<DynamicImage>, PathBuf)>,
    frame: Option<(Size, Protocol)>,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    external_started: bool,
    external_done: bool,
    internal_fallback: bool,
    editor_request: Option<PathBuf>,
    editor_started: bool,
    viewport: Option<Size>,
}

pub(crate) struct ImageViewer {
    paths: Vec<PathBuf>,
    index: usize,
    zoom: f64,
    pan: (f64, f64),
    config: ImageViewerConfig,
    picker: Picker,
    data: Arc<Mutex<ViewerData>>,
    cancelled: Arc<AtomicBool>,
    tx: mpsc::UnboundedSender<Event>,
}

impl Drop for ImageViewer {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        if let Ok(mut data) = self.data.lock() {
            data.source = None;
            data.frame = None;
        }
    }
}

impl ImageViewer {
    pub(crate) fn new(
        paths: Vec<PathBuf>,
        config: ImageViewerConfig,
        mut picker: Picker,
        tx: mpsc::UnboundedSender<Event>,
    ) -> Self {
        match config.protocol {
            ImageViewerProtocol::Auto => {}
            ImageViewerProtocol::Halfblocks => picker.set_protocol_type(ProtocolType::Halfblocks),
            ImageViewerProtocol::Kitty => picker.set_protocol_type(ProtocolType::Kitty),
            ImageViewerProtocol::Sixel => picker.set_protocol_type(ProtocolType::Sixel),
        }
        Self {
            paths,
            index: 0,
            zoom: 1.0,
            pan: (0.0, 0.0),
            config,
            picker,
            data: Arc::new(Mutex::new(ViewerData::default())),
            cancelled: Arc::new(AtomicBool::new(false)),
            tx,
        }
    }

    pub(crate) fn external_done(&self) -> bool {
        self.data.lock().is_ok_and(|data| data.external_done)
    }

    /// Retire the entire generation so stale decoders and GUI completions cannot
    /// publish a frame or editor request into the newly selected attachment.
    fn replace_generation(&mut self, reuse_source: bool, force_internal: bool) {
        self.cancelled.store(true, Ordering::Relaxed);
        let (source, viewport) = {
            let mut old = self.data.lock().unwrap_or_else(|e| e.into_inner());
            let source = old.source.take();
            old.frame = None;
            old.editor_request = None;
            (if reuse_source { source } else { None }, old.viewport)
        };
        self.data = Arc::new(Mutex::new(ViewerData {
            source,
            viewport,
            internal_fallback: force_internal,
            ..Default::default()
        }));
        self.cancelled = Arc::new(AtomicBool::new(false));
    }

    pub(crate) fn navigate(&mut self, delta: i32) -> bool {
        let next = self.index as i64 + i64::from(delta);
        if next < 0 || next >= self.paths.len() as i64 || next as usize == self.index {
            return false;
        }
        self.index = next as usize;
        self.zoom = 1.0;
        self.pan = (0.0, 0.0);
        self.replace_generation(false, false);
        true
    }

    pub(crate) fn change_zoom(&mut self, increase: bool) -> bool {
        {
            let data = self.data.lock().unwrap_or_else(|e| e.into_inner());
            if data.source.is_none() && (data.editor_started || data.error.is_some()) {
                return false;
            }
        }
        let next = if increase {
            self.zoom * 1.25
        } else {
            self.zoom / 1.25
        }
        .clamp(0.25, 8.0);
        if next == self.zoom {
            return false;
        }
        self.zoom = next;
        self.pan = (0.0, 0.0);
        self.replace_generation(true, true);
        true
    }

    /// Move the camera over the source crop, without re-decoding the image.
    pub(crate) fn pan(&mut self, dx: i32, dy: i32) -> bool {
        if self.zoom <= 1.0 {
            return false;
        }
        let (horizontal, vertical) = {
            let data = self.data.lock().unwrap_or_else(|e| e.into_inner());
            let (Some((image, _)), Some(size)) = (data.source.as_ref(), data.viewport) else {
                return false;
            };
            let font = self.picker.font_size();
            let (width, height) =
                crop_dimensions(image, size, (font.width, font.height), self.zoom);
            (width < image.width(), height < image.height())
        };
        let next = (
            if horizontal {
                (self.pan.0 + f64::from(dx) * 0.2).clamp(-1.0, 1.0)
            } else {
                self.pan.0
            },
            if vertical {
                (self.pan.1 + f64::from(dy) * 0.2).clamp(-1.0, 1.0)
            } else {
                self.pan.1
            },
        );
        if next == self.pan {
            return false;
        }
        self.pan = next;
        self.replace_generation(true, true);
        true
    }

    pub(crate) fn take_editor_request(&mut self) -> Option<PathBuf> {
        self.data
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .editor_request
            .take()
    }

    pub(crate) fn attachment_count(&self) -> usize {
        self.paths.len()
    }

    pub(crate) fn editor_finished(&self) {
        let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        data.editor_request = None;
        data.editor_started = true;
        data.busy = false;
        data.error = None;
        data.notice =
            Some("Editor closed. Use Left/Right for other attachments, Esc to return.".into());
    }

    pub(crate) fn editor_error(&self, error: String) {
        let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        data.editor_request = None;
        data.editor_started = true;
        data.busy = false;
        data.error = Some(format!("Could not open attachment in editor: {error}"));
    }

    /// Schedule one decode/encode job at a time; return true if an old graphic needs clearing.
    fn prepare(&self, size: Size) -> bool {
        if size.width == 0 || size.height == 0 {
            return false;
        }
        let mut data = self.data.lock().unwrap_or_else(|error| error.into_inner());
        data.viewport = Some(size);
        let resized = data.frame.as_ref().is_some_and(|(old, _)| *old != size);
        if resized {
            data.frame = None;
        }
        if data.busy
            || data.error.is_some()
            || data.editor_started
            || data.frame.as_ref().is_some_and(|(old, _)| *old == size)
        {
            return resized;
        }
        if self.config.mode == ImageViewerMode::External
            && data.external_started
            && !data.internal_fallback
        {
            return resized;
        }
        data.busy = true;
        let source = data.source.clone();
        let external = self.config.mode == ImageViewerMode::External && !data.internal_fallback;
        let path = self.paths.get(self.index).cloned();
        let shared = Arc::clone(&self.data);
        let cancelled = Arc::clone(&self.cancelled);
        let picker = self.picker.clone();
        let command = self.config.command.clone();
        let tx = self.tx.clone();
        let zoom = self.zoom;
        let pan = self.pan;
        drop(data);
        tokio::spawn(async move {
            let Ok(permit) = Arc::clone(&JOB_SLOTS).acquire_owned().await else {
                return;
            };
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            let failure_state = Arc::clone(&shared);
            let failure_tx = tx.clone();
            let failure_cancelled = Arc::clone(&cancelled);
            if let Err(error) = tokio::task::spawn_blocking(move || {
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                let loaded = match source {
                    Some((image, path)) => Ok(LoadedAttachment::Image(image, path)),
                    None => path
                        .as_deref()
                        .context("No attachment selected")
                        .and_then(load_attachment),
                };
                let (image, path) = {
                    let mut data = shared.lock().unwrap_or_else(|e| e.into_inner());
                    if cancelled.load(Ordering::Relaxed) {
                        return;
                    }
                    match loaded {
                        Ok(LoadedAttachment::Image(image, path)) => {
                            data.source = Some((Arc::clone(&image), path.clone()));
                            (image, path)
                        }
                        Ok(LoadedAttachment::Editor(path)) => {
                            data.busy = false;
                            data.editor_started = true;
                            data.editor_request = Some(path);
                            let _ = tx.send(Event::ImageViewerReady);
                            return;
                        }
                        Err(error) => {
                            data.busy = false;
                            data.error = Some(format!("Could not open attachment: {error:#}"));
                            let _ = tx.send(Event::ImageViewerReady);
                            return;
                        }
                    }
                };
                if external {
                    {
                        let mut data = shared.lock().unwrap_or_else(|e| e.into_inner());
                        if cancelled.load(Ordering::Relaxed) {
                            return;
                        }
                        data.external_started = true;
                        data.busy = false;
                    }
                    drop(permit);
                    let _ = tx.send(Event::ImageViewerReady);
                    std::thread::spawn(move || {
                        if cancelled.load(Ordering::Relaxed) {
                            return;
                        }
                        let result = run_external(&command, &path);
                        let mut data = shared.lock().unwrap_or_else(|e| e.into_inner());
                        if cancelled.load(Ordering::Relaxed) {
                            return;
                        }
                        match result {
                            Ok(()) => data.external_done = true,
                            Err(error) => {
                                data.internal_fallback = true;
                                data.notice = Some(format!(
                                    "External viewer failed: {error}. Using internal viewer."
                                ));
                            }
                        }
                        let _ = tx.send(Event::ImageViewerReady);
                    });
                    return;
                }
                let result = encode_zoomed_image(&picker, &image, size, zoom, pan);
                let mut data = shared.lock().unwrap_or_else(|e| e.into_inner());
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                data.busy = false;
                match result {
                    Ok((protocol, fallback)) => {
                        data.frame = Some((size, protocol));
                        if fallback {
                            data.notice =
                                Some("Graphics encoding failed; using Unicode half blocks.".into());
                        }
                    }
                    Err(error) => data.error = Some(format!("Could not display image: {error}")),
                }
                let _ = tx.send(Event::ImageViewerReady);
            })
            .await
            {
                let mut data = failure_state.lock().unwrap_or_else(|e| e.into_inner());
                if failure_cancelled.load(Ordering::Relaxed) {
                    return;
                }
                data.busy = false;
                data.error = Some(format!("Image processing failed: {error}"));
                let _ = failure_tx.send(Event::ImageViewerReady);
            }
        });
        resized
    }
}

enum LoadedAttachment {
    Image(Arc<DynamicImage>, PathBuf),
    Editor(PathBuf),
}

fn load_attachment(path: &Path) -> anyhow::Result<LoadedAttachment> {
    let path = path
        .canonicalize()
        .context("Attachment is missing or unavailable")?;
    let file = std::fs::File::open(&path).context("Cannot read attachment")?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "Attachment is not a regular file"
    );
    // Construct from a reader rather than ImageReader::open: a misleading filename
    // must never turn an unknown document into a broken-image error.
    let reader = image::ImageReader::new(BufReader::new(file)).with_guessed_format()?;
    if reader.format().is_none() {
        return Ok(LoadedAttachment::Editor(path));
    }
    let image = super::image_preview::load_image(&path)
        .context("Recognized image is invalid, unavailable, or exceeds image size limits")?;
    Ok(LoadedAttachment::Image(Arc::new(image), path))
}

/// Zoom by cropping native source pixels, then fit the crop to the viewport. This
/// never allocates a bitmap multiplied by the zoom factor.
#[cfg(test)]
fn zoom_image(
    image: &DynamicImage,
    size: Size,
    font: (u16, u16),
    zoom: f64,
) -> (DynamicImage, Size) {
    zoom_image_panned(image, size, font, zoom, (0.0, 0.0))
}

fn crop_dimensions(image: &DynamicImage, size: Size, font: (u16, u16), zoom: f64) -> (u32, u32) {
    let pixel_width = f64::from(size.width) * f64::from(font.0.max(1));
    let pixel_height = f64::from(size.height) * f64::from(font.1.max(1));
    let scale = (pixel_width / f64::from(image.width()))
        .min(pixel_height / f64::from(image.height()))
        * zoom;
    let width = ((pixel_width / scale).round() as u32).clamp(1, image.width());
    let height = ((pixel_height / scale).round() as u32).clamp(1, image.height());
    (width, height)
}

fn zoom_image_panned(
    image: &DynamicImage,
    size: Size,
    font: (u16, u16),
    zoom: f64,
    pan: (f64, f64),
) -> (DynamicImage, Size) {
    if zoom <= 1.0 {
        let target = Size::new(
            ((f64::from(size.width) * zoom).round() as u16)
                .max(1)
                .min(size.width),
            ((f64::from(size.height) * zoom).round() as u16)
                .max(1)
                .min(size.height),
        );
        return (image.clone(), target);
    }
    let (width, height) = crop_dimensions(image, size, font, zoom);
    let x =
        (f64::from(image.width() - width) * (pan.0.clamp(-1.0, 1.0) + 1.0) / 2.0).round() as u32;
    let y =
        (f64::from(image.height() - height) * (pan.1.clamp(-1.0, 1.0) + 1.0) / 2.0).round() as u32;
    (image.crop_imm(x, y, width, height), size)
}

#[cfg(test)]
fn encode_image(
    picker: &Picker,
    image: &DynamicImage,
    size: Size,
) -> anyhow::Result<(Protocol, bool)> {
    encode_zoomed_image(picker, image, size, 1.0, (0.0, 0.0))
}

fn encode_zoomed_image(
    picker: &Picker,
    image: &DynamicImage,
    size: Size,
    zoom: f64,
    pan: (f64, f64),
) -> anyhow::Result<(Protocol, bool)> {
    let font = picker.font_size();
    let (transformed, target) =
        zoom_image_panned(image, size, (font.width, font.height), zoom, pan);
    let resize = || Resize::Scale(Some(image::imageops::FilterType::Triangle));
    match picker.new_protocol(transformed, target, resize()) {
        Ok(protocol) => Ok((protocol, false)),
        Err(_) if picker.protocol_type() != ProtocolType::Halfblocks => {
            let mut fallback = picker.clone();
            fallback.set_protocol_type(ProtocolType::Halfblocks);
            let (transformed, target) =
                zoom_image_panned(image, size, (font.width, font.height), zoom, pan);
            Ok((fallback.new_protocol(transformed, target, resize())?, true))
        }
        Err(error) => Err(error.into()),
    }
}

fn run_external(command: &[String], path: &Path) -> anyhow::Result<()> {
    let Some((program, args)) = command
        .split_first()
        .filter(|(program, _)| !program.trim().is_empty())
    else {
        bail!("image_viewer.command must contain a program");
    };
    let path = path
        .canonicalize()
        .context("image file is no longer available")?;
    let status = Command::new(program)
        .args(args)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("cannot start {program}"))?;
    if !status.success() {
        bail!("{program} exited with {status}");
    }
    Ok(())
}

pub(crate) fn draw(frame: &mut Frame, app: &mut crate::app::App, area: Rect) {
    let Some(viewer) = app.image_viewer.as_ref() else {
        return;
    };
    let data = viewer.data.lock().unwrap_or_else(|e| e.into_inner());
    let filename = viewer
        .paths
        .get(viewer.index)
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Image viewer".into());
    let filename = strip_ansi_escapes::strip_str(&filename)
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>();
    let backend = match data.frame.as_ref().map(|(_, protocol)| protocol) {
        Some(Protocol::Kitty(_)) => "Kitty",
        Some(Protocol::Sixel(_)) => "Sixel",
        Some(Protocol::ITerm2(_)) => "iTerm2",
        _ => "halfblocks",
    };
    let block = Block::bordered()
        .title(format!(
            "{}/{}  {filename}  {:.0}%",
            viewer.index + usize::from(!viewer.paths.is_empty()),
            viewer.paths.len(),
            viewer.zoom * 100.0
        ))
        .title_bottom(format!(
            "←/→: attachment | +/-: zoom | Alt+arrows: pan | Esc/q: back | {backend}"
        ));
    let inner = block.inner(area);
    drop(data);
    frame.render_widget(block, area);
    if viewer.prepare(inner.as_size()) {
        // Clear the old graphics before the first rendering of the replacement protocol.
        app.should_clear = true;
        return;
    }
    let data = viewer.data.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, protocol)) = data
        .frame
        .as_ref()
        .filter(|(size, _)| *size == inner.as_size())
    {
        let image_size = protocol.size();
        let area = Rect::new(
            inner.x + inner.width.saturating_sub(image_size.width) / 2,
            inner.y + inner.height.saturating_sub(image_size.height) / 2,
            image_size.width.min(inner.width),
            image_size.height.min(inner.height),
        );
        frame.render_widget(Image::new(protocol), area);
        if let Some(notice) = data.notice.as_ref() {
            frame.render_widget(
                Paragraph::new(strip_ansi_escapes::strip_str(notice))
                    .style(Style::default().fg(Color::Yellow)),
                Rect::new(inner.x, inner.y, inner.width, 1.min(inner.height)),
            );
        }
    } else {
        let status = data.error.as_deref().or(data.notice.as_deref()).unwrap_or(
            if data.editor_started {
                "Opening attachment in $EDITOR..."
            } else if data.external_started && !data.internal_fallback {
                "Image opened in external viewer. Close its window, or press Esc to return to gurk."
            } else {
                "Loading image..."
            },
        );
        frame.render_widget(
            Paragraph::new(status).wrap(Wrap { trim: true }).centered(),
            inner,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::test_app;
    use crate::config::ImageViewerMode;
    use crate::data::Message;
    use crate::signal::Attachment;
    use crate::storage::MessageId;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    fn alt(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
    }

    async fn fixture() -> (
        crate::app::App,
        tempfile::TempDir,
        mpsc::UnboundedReceiver<Event>,
    ) {
        let (mut app, events, _) = test_app().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.data");
        image::RgbImage::from_fn(800, 400, |x, y| {
            image::Rgb([(x % 255) as u8, (y % 255) as u8, 80])
        })
        .save_with_format(&path, image::ImageFormat::Png)
        .unwrap();
        let invalid = dir.path().join("fake.png");
        std::fs::write(&invalid, b"not an image").unwrap();
        let id = app.selected_channel_id().unwrap();
        let mut message: Message = app.storage.message(MessageId::new(id, 0)).unwrap();
        message.attachments = [path, invalid]
            .into_iter()
            .map(|filename| Attachment {
                id: filename.display().to_string(),
                filename,
                content_type: "application/octet-stream".into(),
                size: 0,
            })
            .collect();
        app.store_message(id, message);
        app.on_pgup();
        (app, dir, events)
    }

    async fn wait_for_frame(app: &mut crate::app::App, terminal: &mut Terminal<TestBackend>) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
                let ready = app
                    .image_viewer
                    .as_ref()
                    .unwrap()
                    .data
                    .lock()
                    .unwrap()
                    .frame
                    .is_some();
                if ready {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("viewer did not prepare a frame");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fullscreen_uses_original_image_and_preserves_draft_selection_and_controls() {
        let (mut app, _dir, _events) = fixture().await;
        assert!(!app.config.show_image_previews);
        let id = app.selected_channel_id().unwrap();
        let selected = app.positions[&id].selected;
        for c in "Unsent draft Żółw".chars() {
            app.input.put_char(c);
        }
        app.on_key(alt('p')).await.unwrap();
        assert!(app.image_viewer.is_some());
        let mut terminal = Terminal::new(TestBackend::new(140, 60)).unwrap();
        wait_for_frame(&mut app, &mut terminal).await;
        {
            let state = app.image_viewer.as_ref().unwrap().data.lock().unwrap();
            let (image, path) = state.source.as_ref().unwrap();
            assert_eq!((image.width(), image.height()), (800, 400));
            assert!(path.ends_with("photo.data"));
            let protocol = &state.frame.as_ref().unwrap().1;
            assert!(protocol.size().width > 72);
            assert!(protocol.size().height > 12);
            assert!(protocol.size().width <= 138 && protocol.size().height <= 58);
        }
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .await
            .unwrap();
        app.on_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE))
            .await
            .unwrap();
        app.on_mouse(
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 1,
                row: 1,
                modifiers: KeyModifiers::NONE,
            },
            Rect::new(0, 0, 140, 60),
        );
        app.on_paste("must not enter draft");
        assert_eq!(app.input.data, "Unsent draft Żółw");
        assert!(app.alias_editor.is_none());
        assert_eq!(app.positions[&id].selected, selected);
        terminal.backend_mut().resize(40, 14);
        terminal.autoresize().unwrap();
        wait_for_frame(&mut app, &mut terminal).await;
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        {
            let state = app.image_viewer.as_ref().unwrap().data.lock().unwrap();
            let (size, protocol) = state.frame.as_ref().unwrap();
            assert_eq!(*size, ratatui::layout::Size::new(38, 12));
            assert!(protocol.size().width <= 38 && protocol.size().height <= 12);
        }
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .await
            .unwrap();
        assert!(app.image_viewer.is_none());
        assert!(app.should_clear);
        assert_eq!(app.input.data, "Unsent draft Żółw");
        assert_eq!(app.positions[&id].selected, selected);
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL))
            .await
            .unwrap();
        assert!(app.input.data.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn custom_shortcut_and_missing_or_deleted_attachments_are_safe() {
        let (mut app, dir, _events) = fixture().await;
        let bindings = toml::from_str("[normal]\nalt-p = \"\"\nalt-v = \"view_image\"\n").unwrap();
        app.mode_keybindings = crate::command::get_keybindings(&bindings, true).unwrap();
        app.on_key(alt('p')).await.unwrap();
        assert!(app.image_viewer.is_none());
        std::fs::remove_file(dir.path().join("photo.data")).unwrap();
        app.on_key(alt('v')).await.unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
                if app
                    .image_viewer
                    .as_ref()
                    .unwrap()
                    .data
                    .lock()
                    .unwrap()
                    .error
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(!app.should_quit);
        app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .await
            .unwrap();
        let id = app.selected_channel_id().unwrap();
        let mut message = app.storage.message(MessageId::new(id, 0)).unwrap();
        message.deleted = true;
        app.store_message(id, message);
        app.on_key(alt('v')).await.unwrap();
        assert!(app.image_viewer.is_none());
        app.channels.state.select(None);
        app.on_key(alt('v')).await.unwrap();
        assert!(app.image_viewer.is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn external_launch_failure_falls_back_to_internal() {
        let (mut app, _dir, _events) = fixture().await;
        app.config.image_viewer.mode = ImageViewerMode::External;
        app.config.image_viewer.command = vec!["/does/not/exist/gurk-image-viewer".into()];
        app.on_key(alt('p')).await.unwrap();
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        wait_for_frame(&mut app, &mut terminal).await;
        assert!(
            app.image_viewer
                .as_ref()
                .unwrap()
                .data
                .lock()
                .unwrap()
                .notice
                .as_deref()
                .unwrap()
                .contains("External")
        );
    }

    fn gallery(paths: Vec<PathBuf>) -> ImageViewer {
        let (tx, _) = mpsc::unbounded_channel();
        ImageViewer::new(
            paths,
            ImageViewerConfig::default(),
            Picker::halfblocks(),
            tx,
        )
    }

    async fn wait_gallery(viewer: &ImageViewer, size: Size) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                viewer.prepare(size);
                let ready = {
                    let data = viewer.data.lock().unwrap();
                    data.frame.is_some() || data.error.is_some() || data.editor_request.is_some()
                };
                if ready {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn gallery_starts_at_first_attachment_and_unknown_requests_editor_once() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("document.png");
        std::fs::write(&text, b"plain text, misleading extension").unwrap();
        let photo = dir.path().join("photo.data");
        image::RgbImage::new(40, 20)
            .save_with_format(&photo, image::ImageFormat::Png)
            .unwrap();
        let mut viewer = gallery(vec![text.clone(), photo.clone()]);
        wait_gallery(&viewer, Size::new(20, 10)).await;
        assert!(viewer.data.lock().unwrap().source.is_none());
        assert_eq!(
            viewer.take_editor_request(),
            Some(text.canonicalize().unwrap())
        );
        assert_eq!(viewer.take_editor_request(), None);
        viewer.prepare(Size::new(20, 10));
        assert_eq!(viewer.take_editor_request(), None);
        viewer.editor_error("editor unavailable".into());
        assert!(
            viewer
                .data
                .lock()
                .unwrap()
                .error
                .as_deref()
                .unwrap()
                .contains("editor unavailable")
        );
        assert!(!viewer.navigate(-1));
        assert!(viewer.navigate(1));
        wait_gallery(&viewer, Size::new(20, 10)).await;
        assert!(
            viewer
                .data
                .lock()
                .unwrap()
                .source
                .as_ref()
                .unwrap()
                .1
                .ends_with("photo.data")
        );
        assert!(!viewer.navigate(1));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn navigation_cancels_stale_jobs_and_releases_old_source_and_editor_request() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.png");
        let second = dir.path().join("second.png");
        image::RgbImage::new(40, 20).save(&first).unwrap();
        image::RgbImage::new(30, 10).save(&second).unwrap();
        let mut viewer = gallery(vec![first.clone(), second.clone()]);
        wait_gallery(&viewer, Size::new(20, 10)).await;
        let weak = Arc::downgrade(&viewer.data.lock().unwrap().source.as_ref().unwrap().0);
        let old_data = Arc::clone(&viewer.data);
        let old_cancelled = Arc::clone(&viewer.cancelled);
        assert!(viewer.navigate(1));
        assert!(old_cancelled.load(Ordering::Relaxed));
        assert!(weak.upgrade().is_none());
        old_data.lock().unwrap().editor_request = Some(first);
        old_data.lock().unwrap().external_done = true;
        assert!(!viewer.external_done());
        assert!(viewer.take_editor_request().is_none());
        assert!(viewer.navigate(-1));
        assert!(viewer.navigate(1));
        wait_gallery(&viewer, Size::new(20, 10)).await;
        assert_eq!(
            viewer.data.lock().unwrap().source.as_ref().unwrap().1,
            second.canonicalize().unwrap()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn zoom_reuses_decoded_source_limits_resets_and_survives_tiny_window() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("photo.png");
        image::RgbImage::from_fn(100, 100, |x, y| image::Rgb([x as u8, y as u8, 0]))
            .save(&photo)
            .unwrap();
        let mut viewer = gallery(vec![photo.clone(), photo.clone()]);
        wait_gallery(&viewer, Size::new(20, 10)).await;
        let source = Arc::clone(&viewer.data.lock().unwrap().source.as_ref().unwrap().0);
        assert!(viewer.change_zoom(true));
        assert!(Arc::ptr_eq(
            &source,
            &viewer.data.lock().unwrap().source.as_ref().unwrap().0
        ));
        for _ in 0..100 {
            viewer.change_zoom(true);
        }
        assert_eq!(viewer.zoom, 8.0);
        assert!(!viewer.change_zoom(true));
        for _ in 0..100 {
            viewer.change_zoom(false);
        }
        assert_eq!(viewer.zoom, 0.25);
        assert!(!viewer.change_zoom(false));
        assert!(!viewer.prepare(Size::new(0, 0)));
        wait_gallery(&viewer, Size::new(1, 1)).await;
        assert!(viewer.data.lock().unwrap().frame.is_some());
        assert!(viewer.navigate(1));
        assert_eq!(viewer.zoom, 1.0);
    }

    #[test]
    fn panned_native_crop_reaches_each_edge_and_keeps_target_size() {
        let image = DynamicImage::ImageRgb8(image::RgbImage::from_fn(100, 100, |x, y| {
            image::Rgb([x as u8, y as u8, 0])
        }));
        for (pan, expected) in [
            ((-1.0, -1.0), [0, 0, 0]),
            ((0.0, 0.0), [25, 25, 0]),
            ((1.0, 1.0), [50, 50, 0]),
        ] {
            let (crop, size) = zoom_image_panned(&image, Size::new(20, 10), (1, 2), 2.0, pan);
            assert_eq!((crop.width(), crop.height()), (50, 50));
            assert_eq!(crop.to_rgb8().get_pixel(0, 0).0, expected);
            assert_eq!(size, Size::new(20, 10));
        }
        let (crop, _) = zoom_image_panned(&image, Size::new(100, 10), (1, 2), 2.0, (1.0, 1.0));
        assert_eq!(crop.width(), 100);
        assert_eq!(crop.to_rgb8().get_pixel(0, 0).0, [0, 50, 0]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pan_is_bounded_reuses_source_cancels_stale_state_and_resets_on_zoom_or_navigation() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("photo.png");
        image::RgbImage::new(100, 100).save(&photo).unwrap();
        let mut viewer = gallery(vec![photo.clone(), photo.clone()]);
        assert!(!viewer.pan(1, 0));
        wait_gallery(&viewer, Size::new(20, 10)).await;
        assert!(!viewer.pan(1, 0));
        assert!(viewer.change_zoom(true));
        let source = Arc::clone(&viewer.data.lock().unwrap().source.as_ref().unwrap().0);
        let old = Arc::clone(&viewer.data);
        let cancelled = Arc::clone(&viewer.cancelled);
        assert!(viewer.pan(1, 1));
        assert_eq!(viewer.pan, (0.2, 0.2));
        assert!(cancelled.load(Ordering::Relaxed));
        assert!(Arc::ptr_eq(
            &source,
            &viewer.data.lock().unwrap().source.as_ref().unwrap().0
        ));
        old.lock().unwrap().editor_request = Some(photo);
        old.lock().unwrap().external_done = true;
        assert!(viewer.take_editor_request().is_none());
        assert!(!viewer.external_done());
        for _ in 0..20 {
            viewer.pan(1, 1);
        }
        assert_eq!(viewer.pan, (1.0, 1.0));
        assert!(!viewer.pan(1, 1));
        assert!(!viewer.pan(0, 0));
        for _ in 0..20 {
            viewer.pan(-1, -1);
        }
        assert_eq!(viewer.pan, (-1.0, -1.0));
        assert!(!viewer.pan(-1, -1));
        assert!(viewer.change_zoom(true));
        assert_eq!(viewer.pan, (0.0, 0.0));
        assert!(viewer.pan(1, 0));
        assert!(viewer.navigate(1));
        assert_eq!(viewer.pan, (0.0, 0.0));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pan_ignores_nonoverflow_axis_unknown_file_and_fit_zoom() {
        let dir = tempfile::tempdir().unwrap();
        let photo = dir.path().join("photo.png");
        image::RgbImage::new(100, 100).save(&photo).unwrap();
        let mut viewer = gallery(vec![photo]);
        wait_gallery(&viewer, Size::new(100, 10)).await;
        assert!(!viewer.pan(0, 1));
        viewer.change_zoom(true);
        assert!(!viewer.pan(1, 0));
        assert!(viewer.pan(0, 1));
        assert_eq!(viewer.pan, (0.0, 0.2));
        let unknown = dir.path().join("document");
        std::fs::write(&unknown, b"document").unwrap();
        let mut viewer = gallery(vec![unknown]);
        wait_gallery(&viewer, Size::new(20, 10)).await;
        assert!(!viewer.pan(1, 1));
    }

    #[test]
    fn zoom_center_crops_native_pixels_and_zoomout_shrinks_target() {
        let image = DynamicImage::ImageRgb8(image::RgbImage::from_fn(100, 100, |x, y| {
            image::Rgb([x as u8, y as u8, 0])
        }));
        let (cropped, size) = zoom_image(&image, Size::new(20, 10), (1, 2), 2.0);
        assert_eq!(size, Size::new(20, 10));
        assert_eq!((cropped.width(), cropped.height()), (50, 50));
        assert_eq!(cropped.to_rgb8().get_pixel(0, 0).0, [25, 25, 0]);
        let (_, size) = zoom_image(&image, Size::new(20, 10), (1, 2), 0.5);
        assert_eq!(size, Size::new(10, 5));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn missing_and_recognized_broken_image_show_error_without_editor() {
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("broken.png");
        std::fs::write(&broken, b"\x89PNG\r\n\x1a\ninvalid").unwrap();
        for path in [dir.path().join("missing"), broken] {
            let mut viewer = gallery(vec![path]);
            wait_gallery(&viewer, Size::new(20, 10)).await;
            assert!(viewer.data.lock().unwrap().error.is_some());
            assert!(viewer.take_editor_request().is_none());
        }
    }

    #[test]
    fn internal_viewer_encodes_kitty_and_sixel_graphics() {
        use ratatui::{buffer::Buffer, widgets::Widget};
        let image = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            80,
            40,
            image::Rgb([210, 40, 90]),
        ));
        for (kind, marker) in [
            (ProtocolType::Kitty, "\u{1b}_G"),
            (ProtocolType::Sixel, "\u{1b}P"),
        ] {
            let mut picker = Picker::halfblocks();
            picker.set_protocol_type(kind);
            let (protocol, fallback) = encode_image(&picker, &image, Size::new(20, 10)).unwrap();
            assert!(!fallback);
            assert!(matches!(
                (&protocol, kind),
                (Protocol::Kitty(_), ProtocolType::Kitty)
                    | (Protocol::Sixel(_), ProtocolType::Sixel)
            ));
            let size = protocol.size();
            assert!(size.width <= 20 && size.height <= 10);
            let area = Rect::new(0, 0, size.width, size.height);
            let mut buffer = Buffer::empty(area);
            Image::new(&protocol).render(area, &mut buffer);
            let encoded: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
            assert!(
                encoded.contains(marker),
                "missing graphics payload for {kind:?}"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn successful_external_viewer_returns_to_same_message() {
        let (mut app, _dir, mut events) = fixture().await;
        app.config.image_viewer.mode = ImageViewerMode::External;
        app.config.image_viewer.command = vec!["/bin/true".into()];
        let id = app.selected_channel_id().unwrap();
        let selected = app.positions[&id].selected;
        app.input.put_char('x');
        app.on_key(alt('p')).await.unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while app.image_viewer.is_some() {
                app.handle_event(events.recv().await.unwrap()).unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(app.positions[&id].selected, selected);
        assert_eq!(app.input.data, "x");
        assert!(app.should_clear);
    }

    #[cfg(unix)]
    #[test]
    fn external_program_receives_path_as_one_argument_without_shell_expansion() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake viewer");
        let capture = dir.path().join("arguments");
        let text = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
            capture.display()
        );
        std::fs::write(&script, text).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = dir.path().join("photo $(touch BAD); 'quoted'.png");
        std::fs::write(&path, b"fixture").unwrap();
        run_external(
            &[
                script.display().to_string(),
                "argument with spaces".into(),
                "--".into(),
            ],
            &path,
        )
        .unwrap();
        let args = std::fs::read_to_string(&capture).unwrap();
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            vec!["argument with spaces", "--", path.to_str().unwrap()]
        );
        assert!(!dir.path().join("BAD").exists());
        assert!(run_external(&[], &path).is_err());
        assert!(run_external(&["/bin/sh".into(), "-c".into(), "exit 7".into()], &path).is_err());
    }
}
