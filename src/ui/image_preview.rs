//! Bounded, background-decoded attachment thumbnails rendered as ordinary TUI cells.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use image::{ImageReader, RgbaImage};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const CACHE_ENTRIES: usize = 32;
const MAX_WORKERS: usize = 2;

type Stamp = (u64, Option<SystemTime>);

enum Thumbnail {
    Pending,
    Ready(Option<Arc<RgbaImage>>),
}

struct Entry {
    stamp: Stamp,
    used: u64,
    thumbnail: Thumbnail,
}

#[derive(Default)]
struct Cache {
    entries: BTreeMap<PathBuf, Entry>,
    clock: u64,
    workers: usize,
}

#[derive(Default)]
pub(crate) struct PreviewCache {
    inner: Arc<Mutex<Cache>>,
}

impl PreviewCache {
    /// Keep the visible working set within the cache capacity, including on very tall
    /// terminals or messages with many attachments. Duplicated paths use one cache slot.
    pub(crate) fn request_visible<'a>(&self, paths: impl IntoIterator<Item = &'a Path>) {
        let mut seen = std::collections::BTreeSet::new();
        for path in paths {
            if seen.insert(path) {
                self.request(path);
                if seen.len() == CACHE_ENTRIES {
                    break;
                }
            }
        }
    }

    /// A cache miss schedules work and immediately returns. Neither decoding nor resizing the
    /// original file runs on the UI thread. A changed file also invalidates negative results.
    pub(crate) fn request(&self, path: &Path) -> Option<Arc<RgbaImage>> {
        self.lookup(path, true)
    }

    pub(crate) fn cached(&self, path: &Path) -> Option<Arc<RgbaImage>> {
        self.lookup(path, false)
    }

    fn lookup(&self, path: &Path, schedule: bool) -> Option<Arc<RgbaImage>> {
        let metadata = std::fs::metadata(path).ok()?;
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            return None;
        }
        let stamp = (metadata.len(), metadata.modified().ok());
        let mut cache = self.inner.lock().ok()?;
        cache.clock = cache.clock.wrapping_add(1);
        let used = cache.clock;
        if let Some(entry) = cache.entries.get_mut(path)
            && entry.stamp == stamp
        {
            entry.used = used;
            return match &entry.thumbnail {
                Thumbnail::Ready(image) => image.clone(),
                Thumbnail::Pending => None,
            };
        }
        if !schedule || cache.workers >= MAX_WORKERS {
            return None;
        }
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        if !cache.entries.contains_key(path) && cache.entries.len() >= CACHE_ENTRIES {
            let oldest = cache
                .entries
                .iter()
                .filter(|(_, entry)| matches!(entry.thumbnail, Thumbnail::Ready(_)))
                .min_by_key(|(_, entry)| entry.used)
                .map(|(path, _)| path.clone())?;
            cache.entries.remove(&oldest);
        }
        let path = path.to_owned();
        cache.entries.insert(
            path.clone(),
            Entry {
                stamp,
                used,
                thumbnail: Thumbnail::Pending,
            },
        );
        cache.workers += 1;
        let inner = Arc::clone(&self.inner);
        runtime.spawn_blocking(move || {
            let image = std::panic::catch_unwind(|| load_thumbnail(&path))
                .unwrap_or(None)
                .map(Arc::new);
            if let Ok(mut cache) = inner.lock() {
                cache.workers = cache.workers.saturating_sub(1);
                if let Some(entry) = cache.entries.get_mut(&path)
                    && entry.stamp == stamp
                {
                    entry.thumbnail = Thumbnail::Ready(image);
                }
            }
        });
        None
    }
}

fn load_thumbnail(path: &Path) -> Option<RgbaImage> {
    let image = load_image(path)?;
    Some(
        image
            .thumbnail(image.width().min(256), image.height().min(256))
            .to_rgba8(),
    )
}

pub(crate) fn load_image(path: &Path) -> Option<image::DynamicImage> {
    let file = std::fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return None;
    }
    let (width, height) = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()?;
    if width == 0
        || height == 0
        || width > 16_384
        || height > 16_384
        || u64::from(width) * u64::from(height) > 16 * 1024 * 1024
    {
        return None;
    }
    let mut reader = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().ok()
}

/// Two image pixels per cell, matching the usual terminal cell aspect ratio.
/// Alpha is composited onto black; all output stays in ratatui's normal cell buffer.
pub(crate) fn render_preview(image: &RgbaImage, width: usize, rows: usize) -> Vec<Line<'static>> {
    if width == 0 || rows == 0 || image.width() == 0 || image.height() == 0 {
        return Vec::new();
    }
    let mut width = (width.min(72) as u32).min(image.width());
    let mut pixels_high = ((rows.min(12) * 2) as u32).min(image.height());
    // imageops::thumbnail uses exact dimensions, unlike DynamicImage::thumbnail.
    if u64::from(image.width()) * u64::from(pixels_high)
        > u64::from(image.height()) * u64::from(width)
    {
        pixels_high = ((u64::from(image.height()) * u64::from(width)) / u64::from(image.width()))
            .max(1) as u32;
    } else {
        width = ((u64::from(image.width()) * u64::from(pixels_high)) / u64::from(image.height()))
            .max(1) as u32;
    }
    let resized = image::imageops::thumbnail(image, width, pixels_high);
    let color = |pixel: &image::Rgba<u8>| {
        let channel = |value: u8| ((u16::from(value) * u16::from(pixel[3])) / 255) as u8;
        Color::Rgb(channel(pixel[0]), channel(pixel[1]), channel(pixel[2]))
    };
    (0..resized.height().div_ceil(2))
        .map(|row| {
            Line::from(
                (0..resized.width())
                    .map(|col| {
                        let top = color(resized.get_pixel(col, row * 2));
                        let bottom =
                            color(resized.get_pixel(col, (row * 2 + 1).min(resized.height() - 1)));
                        Span::styled("▀", Style::default().fg(top).bg(bottom))
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgba, RgbaImage};
    use tempfile::tempdir;

    #[tokio::test(flavor = "multi_thread")]
    async fn many_visible_attachments_do_not_continually_evict_each_other() {
        let cache = PreviewCache::default();
        let dir = tempdir().unwrap();
        let paths: Vec<_> = (0..40)
            .map(|idx| {
                let path = dir.path().join(format!("{idx}.png"));
                RgbaImage::from_pixel(8, 4, Rgba([255, 0, 0, 255]))
                    .save(&path)
                    .unwrap();
                path
            })
            .collect();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                cache.request_visible(paths.iter().map(|path| path.as_path()));
                if paths[..32].iter().all(|path| cache.cached(path).is_some()) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        for _ in 0..10 {
            cache.request_visible(paths.iter().map(|path| path.as_path()));
        }
        assert_eq!(cache.inner.lock().unwrap().workers, 0);
        assert_eq!(cache.inner.lock().unwrap().entries.len(), 32);
        assert!(paths[32..].iter().all(|path| cache.cached(path).is_none()));
    }

    #[test]
    fn detects_image_contents_and_rejects_non_images() {
        let dir = tempdir().unwrap();
        let image = dir.path().join("not-an-image-extension.txt");
        RgbaImage::from_pixel(8, 4, Rgba([255, 0, 0, 255]))
            .save_with_format(&image, ImageFormat::Png)
            .unwrap();
        let thumb = load_thumbnail(&image).unwrap();
        assert_eq!(thumb.dimensions(), (8, 4));
        let fake = dir.path().join("fake.png");
        std::fs::write(&fake, b"this is not an image").unwrap();
        assert!(load_thumbnail(&fake).is_none());
        assert!(load_thumbnail(&dir.path().join("missing.jpg")).is_none());
    }

    #[test]
    fn decodes_supported_photo_formats_by_signature() {
        let dir = tempdir().unwrap();
        let original = image::RgbImage::from_pixel(8, 4, image::Rgb([200, 80, 30]));
        for (idx, format) in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Gif,
            ImageFormat::WebP,
            ImageFormat::Bmp,
            ImageFormat::Tiff,
        ]
        .into_iter()
        .enumerate()
        {
            let path = dir.path().join(format!("attachment-{idx}.data"));
            original.save_with_format(&path, format).unwrap();
            assert_eq!(
                load_thumbnail(&path)
                    .unwrap_or_else(|| panic!("failed to decode {format:?}"))
                    .dimensions(),
                (8, 4)
            );
        }
    }

    #[test]
    fn rejects_oversized_files_and_dimensions() {
        let dir = tempdir().unwrap();
        let large = dir.path().join("large.png");
        std::fs::File::create(&large)
            .unwrap()
            .set_len(33 * 1024 * 1024)
            .unwrap();
        assert!(load_thumbnail(&large).is_none());
        let wide = dir.path().join("wide.png");
        RgbaImage::from_pixel(20_000, 1, Rgba([0, 0, 0, 255]))
            .save(&wide)
            .unwrap();
        assert!(load_thumbnail(&wide).is_none());
    }

    #[test]
    fn previews_fit_both_dimensions_and_keep_aspect_ratio() {
        let wide = RgbaImage::from_pixel(320, 160, Rgba([255, 0, 0, 255]));
        let rows = render_preview(&wide, 20, 20);
        assert_eq!(rows.len(), 5);
        assert!(rows.iter().all(|row| row.width() == 20));
        let tall = RgbaImage::from_pixel(160, 320, Rgba([0, 0, 255, 255]));
        let rows = render_preview(&tall, 20, 4);
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|row| row.width() == 4));
        assert!(render_preview(&wide, 0, 10).is_empty());
        assert!(render_preview(&wide, 10, 0).is_empty());
    }

    #[test]
    fn half_blocks_use_top_and_bottom_pixel_colors() {
        let mut image = RgbaImage::new(1, 2);
        image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        image.put_pixel(0, 1, Rgba([0, 0, 255, 255]));
        let rows = render_preview(&image, 10, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].spans[0].content, "▀");
        assert_eq!(
            rows[0].spans[0].style.fg,
            Some(ratatui::style::Color::Rgb(255, 0, 0))
        );
        assert_eq!(
            rows[0].spans[0].style.bg,
            Some(ratatui::style::Color::Rgb(0, 0, 255))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cache_recovers_when_missing_or_invalid_file_becomes_an_image() {
        let cache = PreviewCache::default();
        let dir = tempdir().unwrap();
        let path = dir.path().join("picture.data");
        assert!(cache.request(&path).is_none());
        std::fs::write(&path, b"invalid").unwrap();
        assert!(cache.request(&path).is_none());
        RgbaImage::from_pixel(8, 4, Rgba([255, 0, 0, 255]))
            .save_with_format(&path, ImageFormat::Png)
            .unwrap();
        let image = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if let Some(image) = cache.request(&path) {
                    break image;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(image.dimensions(), (8, 4));
    }
}
