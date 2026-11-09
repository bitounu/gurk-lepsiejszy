# Image viewer implementation plan

> Use superpowers:executing-plans inline and an independent final code review.

**Goal:** Alt+P image viewing with internal graphics/fallback and configurable external GUI, plus an Arch package.
**Spec:** docs/superpowers/specs/2026-11-06-image-viewer-design.md
**Architecture:** Separate modal state with original image and encoded protocol prepared by background workers; startup-only terminal negotiation; external GUI subprocess managed in background.
**Tech Stack:** Rust/image/tokio/ratatui-image, existing makepkg recipe.

## Global Constraints

Keep old configuration/data and existing features; previews default off; Ctrl+U/P preserved; no shell interpretation of attachment paths; no TTY competition; close/resize must clean graphics.

## Review Focus

Draft/selection protection; invalid/missing/deleted/multiple attachments; stale completion after close/resize; subprocess errors and unusual paths; protocol selection/cleanup in real terminal streams.

### Task 1: Config and decoding
- [x] Test legacy defaults, both modes/protocol choices and command serialization.
- [x] Add optional config and ratatui-image dependency; share bounded original-image decoder with thumbnails.

### Task 2: Viewer and controls
- [x] Test Alt+P and custom shortcut, no selection, actual-content selection among files, fullscreen geometry, resize, modal keyboard/mouse/paste isolation, draft/selection restoration, errors, subprocess argument safety and fallback.
- [x] Implement original decode and protocol jobs, modal rendering, external process handling, startup negotiation and terminal cleanup. Document in F1/README.

### Task 3: Delivery
- [x] Full tests, lint and formatting; independent review and material fixes.
- [x] Build updated release/package, verify upgrade/contents/integrity and Arch runtime startup; deliver artifact and checksum.

## Implementation and verification

- `cargo test --locked --all-features`: 147 passed, 0 failed.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: passed.
- `cargo fmt --all -- --check`, `git diff --check`, and PKGBUILD shell syntax: passed.
- Independent read-only review found no critical or important bugs. Resize rendering uses exact current dimensions; cancellation state belongs to a single modal; GUI waits use a detached child reaper and do not hold image slots or Tokio shutdown.
- Native Kitty and Sixel payload generation is tested through the viewer encoder and ratatui buffers. A physical Kitty/Sixel terminal is not available in this build environment; real-device appearance remains for user testing.
- Review note: Kitty uses the library's random image IDs. Closing removes visible placeholders via terminal clearing, while the terminal may retain cached image data until eviction; explicit per-image deletion can be added when the library exposes IDs/lifecycle control.
- Native terminal negotiation happens before the keyboard reader starts; explicit halfblocks mode avoids probing. All old data/config and prior functionality are preserved.

## Package delivery

- Built `dist/gurk-local-aliases-0.10.2.aliases3-1-x86_64.pkg.tar.zst` (9.23 MiB compressed) using makepkg; aliases1/aliases2 artifacts retained.
- Isolated pacman upgrade from aliases2 to aliases3 succeeded with dependency checks; legacy configuration fixture hash unchanged.
- Installed binary `--version` and `--help` succeed with Arch glibc/libgcc in chroot. `pacman -Qkk`: 10 files, 0 altered.
- zstd integrity and all SHA256 checks passed (`dist/SHA256SUMS`). Binary requires GLIBC_2.39 and no Chafa/native image libraries.
- No changes have been committed or pushed; implementation remains in the existing `feat/local-aliases` checkout for review/testing.
