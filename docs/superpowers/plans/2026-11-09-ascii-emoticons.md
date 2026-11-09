# Optional ASCII emoticons implementation plan

**Goal:** Offer opt-in ASCII emoticon to Unicode emoji conversion without breaking existing configuration or confirmed contact mentions.

**Architecture:** Add top-level `auto_ascii_emoticons` (default false). Recognize exact whitespace-delimited emoticon tokens only. Apply replacements in descending byte order, reconcile confirmed mention ranges after each edit, preserve cursor byte/line/column. Convert after typing whitespace/newline in the composer and before send (including pasted/editor text and attachment captions); exclude contact labels and all modal inputs. Existing shortcode behavior remains independent. No received-history or forward conversion.

**Tech:** Rust, existing Input/Cursor, serde configuration, no dependencies.

- [x] Add failing config compatibility, converter/cursor/mention and App integration tests.
- [x] Implement token mappings, protected mention spans and composer hooks.
- [x] README configuration/mapping explanation and aliases24 package version.
- [x] Full tests, fmt/Clippy, independent scoped review, build Arch and isolated upgrade/config verification.

Preserve existing functionality and verify offline without live Signal sends.

Implementation note: review found the existing Cursor's character-width arithmetic drifted after the new `<3` → `❤️` conversion. A regression reproduced the empty input with col=1. Cursor now calculates actual prefix string width consistently, targets grapheme boundaries on vertical movement, and Ctrl+U deletes the current line prefix by bytes while retaining deletion of the prior newline at line start. Regression covers heart deletion/navigation/Home/End/Ctrl+U plus desired-column preservation across lines. Follow-up independent review found no further important issues.

Verification so far: 351 library tests + 1 CLI test passed. Native mention labels and offsets, default-disabled compatibility and attachment captions covered. No live Signal sends performed.

Release verification: cargo fmt --all -- --check and cargo clippy --all-targets -- -D warnings passed. aliases24 Arch package built, isolated pacman upgrade from aliases23 passed Qkk (10 files, 0 altered) and existing config hash unchanged. README contents match, no private media/config in package; CLI help, flag conflict and offline report checks passed.
