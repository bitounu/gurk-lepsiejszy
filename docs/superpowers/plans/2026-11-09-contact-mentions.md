# Contact mentions implementation plan

**Goal:** Complete @contact names into confirmed Signal identity mentions, not plain strings.

**Architecture:** Visible draft stores full @name plus byte ranges/ACI, shifts ranges on edits and discards identity if the name is edited. Wire encoder replaces confirmed spans with U+FFFC, encodes mention ranges in UTF-16 after emoji conversion. Existing stored drafts remain readable using serde defaults. Popup filters local known contacts/aliases, supports arrows/Tab/Enter/Escape; accepting never sends. Group membership is prioritized; only local sources, no typing broadcasts or contact lookup requests while typing.

**Tech:** Rust, existing ratatui, serde, presage BodyRange.

- [x] Add tested pure mention model/UTF-16 encoding, edit reconciliation and decoding to editable draft.
- [x] Preserve mention spans across input edits, per-channel persisted drafts/restarts, image preparation, message edits; send through real and mock manager with local echo body ranges.
- [x] Implement popup query @prefix, deterministic duplicate-name identity selection, alias matching and modal-safe input, bottom hints and F1.
- [x] Integration tests: acceptance doesn't send; Unicode emoji offsets; invalidated mentions plain; draft restore; group/contact candidates; edit/attachment send.
- [x] README/aliases23, full tests, Clippy/fmt, independent review, Arch build and isolated upgrade.

No live Signal sends, no TOML change required; additive SQLite migration 013 gives legacy drafts an empty mentions array, no commits requested.

Verification so far: 344 library + 1 CLI tests passed (full confirmation run), cargo clippy --all-targets -- -D warnings and cargo fmt --all -- --check passed. Independent review found a label-isolation bug: file:// text within confirmed contact labels was parsed as an attachment. Added regression reproducing a panic before fixing all matcher paths. Labels are now excluded from attachment syntax and the test verifies no label file bytes are uploaded, including alongside explicitly staged files. Follow-up review found no further issues. One prior suite run had a transient ETXTBSY launching a freshly created screenshot test helper; unchanged confirmation run passed.

Presage's vendored direct-message send now keeps explicit requiredProtocolVersion instead of overwriting the mention version with INITIAL. Default messages retain INITIAL. No live Signal sends performed.

Release complete: aliases23 package built; isolated pacman upgrade from aliases22 passed Qkk (10 files, 0 altered). Existing config hash unchanged; CLI help, conflicting flags and offline backup report checks passed. No private images/config included in package.
