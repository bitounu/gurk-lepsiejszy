# Image previews implementation plan

> Use superpowers:executing-plans inline; final independent review via superpowers:requesting-code-review.

**Goal:** Optional bounded inline image previews, bounded help, alias help and an Arch package.
**Architecture:** Cache background-decoded small RGBA thumbnails; render half-block rows in message ListItems; measure help extent during drawing and clamp input scroll.
**Tech Stack:** Rust, image, tokio, ratatui, makepkg.
**Spec:** docs/superpowers/specs/2026-11-05-image-previews-design.md

## Global Constraints

Default previews off; old config loads; keep aliases, original attachment links and user data; no raw terminal escape image output.

## Review Focus

Malformed/oversized images, missing files later appearing, resize and very small panes, multiple attachments with long captions, keyboard/mouse help isolation, custom/disabled alias bindings.

### Task 1: Help

Files: src/ui/draw.rs, src/app/{mod.rs,input.rs}.
- [x] Reproduce unlimited scrolling with tests at bottom and after resize, and verify visible alias instructions.
- [x] Track help_scroll_max from measured content; clamp keyboard and mouse scrolling; show alias editing instructions and configured shortcuts near top.
- [x] Run focused help tests.

### Task 2: Previews

Files: src/ui/image_preview.rs (new), src/ui/{mod.rs,draw.rs}, src/app/mod.rs, src/config.rs, Cargo.toml/Cargo.lock, README.md.
Interface: PreviewCache::request(path) -> Option<Arc<RgbaImage>>; load_thumbnail(path) decodes bounded content; render_preview(image,width,rows) -> Vec<Line<'static>>.
- [x] Add tests for signature detection, invalid/missing/oversized files, dimensions/aspect ratio, default and old config, preview rendering and deleted messages.
- [x] Implement bounded background cache, decoders and in-message scaling; opt in using show_image_previews.
- [x] Verify default rendering remains unchanged and enabled previews fit after resize.

### Task 3: Delivery

Files: packaging/arch/{PKGBUILD,README.md}, dist artifacts (ignored).
- [x] Run cargo test --locked --all-features, clippy and formatting; independent review and fixes.
- [x] Increase local package version, build optimized binary/package, verify pacman installation and Arch runtime startup; produce checksum and installation instructions.

## Verification and review

- `cargo test --locked --all-features`: 140 passed, 0 failed.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- Independent read-only review found no critical/important issues. Two minor observations were addressed: help now considers bindings used in conversation modes, and visible decode scheduling caps its working set at 32 unique attachment paths to prevent cache churn. Added regressions for both.
- No existing user configuration or data was read or modified during development. Preview and alias tests use temporary files/databases.
- Image decoding checks content signatures and bounded dimensions/allocation; PNG/JPEG/GIF/WebP/BMP/TIFF fixtures all decode successfully.
- Implementation preserves existing local alias functionality.

## Package delivery

- Built `dist/gurk-local-aliases-0.10.2.aliases2-1-x86_64.pkg.tar.zst` with makepkg; previous aliases1 artifact retained.
- Isolated pacman upgrade from aliases1 to aliases2 succeeded with dependency checks, preserving an existing legacy configuration fixture byte-for-byte.
- `pacman -Qkk gurk-local-aliases`: 10 files, 0 altered.
- Installed binary `--version` and `--help` succeeded under a chroot using Arch glibc/libgcc runtime libraries.
- zstd archive integrity and SHA256 checks passed; checksums in `dist/SHA256SUMS`.
- Package payload contains only `/usr/bin/gurk`, README and license; no user configuration or data.
