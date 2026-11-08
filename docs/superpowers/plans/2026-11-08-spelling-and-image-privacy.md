# Offline spelling and image privacy implementation plan

> Preserve existing functionality and original files; verify sanitization offline with fixtures.

- [x] JPEG/PNG sanitizer: bounded format parsing, compressed image payload preserved, keep minimal orientation and ICC, remove descriptive blocks/thumbnails/C2PA/trailing MotionPhoto media. Unsupported formats unchanged, malformed supported images fail closed. Use synthetic fixtures without embedding personal media.
- [x] Offline spelling: Spellbook worker loading pl_PL/en_US Hunspell dictionaries, Unicode ranges, ignore non-prose, generation-safe results and on-demand suggestions. No network calls or auto replacement.
- [x] Integrate spelling display and configurable correction shortcut, suggestion/ignore dialog; preserve drafts, edits, modals and cursor. Missing dictionaries visible, nonblocking.
- [x] Apply image sanitizing to picker/screenshots, legacy file paths, clipboard and forward; originals unchanged, sanitized lengths/digests correct, errors preserve composer and prevent sending.
- [x] Default-compatible TOML settings, README and F1/keybar; Arch dictionary optdepends and package aliases18.
- [x] Full tests, fmt, Clippy, bounded sanitizer verification and Arch upgrade verification.

Verification so far: 292 library + 1 CLI tests pass (two test threads). Actual Arch Hunspell pl_PL/en_US dictionaries loaded, valid Polish/English accepted and synthetic typos found. JPEG/PNG sanitizer verification checks that decoded RGBA, orientation and ICC remain unchanged while descriptive/GPS/C2PA/Google blocks are removed. HEIC remains byte-identical. Temporary verification outputs are removed. Fresh Clippy all targets/all features -D warnings passed; fmt/diff checks clean. aliases18 release package built and verified in isolated Arch root via upgrade from aliases17, integrity/README/private-file checks, CLI smoke tests, and unchanged existing config fixture. No real Signal sends performed.
