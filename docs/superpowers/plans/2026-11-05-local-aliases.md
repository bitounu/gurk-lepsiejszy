# Local Aliases Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Edit persistent local person and channel aliases without changing Signal names.
**Architecture:** Independent SQLite alias tables and an application cache; a modal input editor; shared display resolution.
**Tech Stack:** Rust, ratatui, sqlx/SQLite.
**Spec:** docs/superpowers/specs/2026-11-05-local-aliases-design.md

## Global Constraints

Preserve existing TOML configuration and original names. Migrate existing databases automatically. Never send alias changes to Signal. Keep drafts intact. Save failures leave the editor open.

## Review Focus

Unicode clipping; alias accidentally saved as original name; empty alias removal; failed persistence; configurable shortcuts and modal isolation.

### Task 1: Persistence

Files: migrations/007_local_aliases.{up,down}.sql, src/storage/{mod.rs,forgetful.rs,sql/storage.rs}.
Interface: AliasTarget distinguishes Person(Uuid) and Channel(ChannelId); aliases() returns persisted aliases; set_alias(target, Option<&str>) returns anyhow::Result<()>.
- [x] Add tests for migration with old data, independent aliases, reopening, update/removal and write failure; run before implementation.
- [x] Add separate person_aliases/channel_aliases tables and storage operations, leaving original names untouched.
- [x] Run storage tests.

### Task 2: Resolution

Files: src/app/{mod.rs,channel.rs,message.rs}, src/channels.rs, src/ui/{draw.rs,name_resolver.rs}.
Interface: App holds alias cache loaded from storage; channel_name and name_by_id_cached prefer aliases; name_by_id remains original-name resolution for internal synchronization.
- [x] Add tests covering precedence, full aliases with first_name_only, Signal updates, removal and channel picker search; run before implementation.
- [x] Route displayed names through alias-aware helpers, including notifications, quotes and mentions. Clip displayed text on character boundaries.
- [x] Run application and UI tests.

### Task 3: Editor and delivery

Files: src/app/aliases.rs, src/app/{mod.rs,input.rs}, src/command.rs, src/ui/draw.rs, src/shortcuts.rs, README.md.
Interface: AliasEditor owns target, Input and optional error. F2/edit_channel_alias and F3/edit_person_alias open it. Enter persists trimmed text (empty removes), Escape cancels. Input edits cannot trigger conversation actions.
- [x] Add tests for save/cancel, drafts, Unicode, selected author/direct-message fallback, no target and write failure; run before implementation.
- [x] Implement modal, configurable commands and help/documentation.
- [x] Run cargo fmt --check, cargo test and cargo clippy where available; obtain independent final review and fix material findings.

## Execution and verification

Implemented local alias editing and persistence with compatibility checks.

- `cargo test --locked --all-features`: 127 passed, 0 failed (plus binary and doc-test targets).
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
- Verification used Rust 1.94.0 and a temporary protoc installation; the system Rust 1.75 cannot parse the existing edition 2024 manifest.
- Independent read-only review found a mouse/modal isolation issue. Fixed by routing mouse events through App with a modal guard, covered by a regression test. Multiline paste is normalized only in the alias editor; message paste remains unchanged.
- Additional regression: a person alias equal to its UUID must still override the original conversation name. Fixed and tested.
- Existing TOML with legacy `user.name` and boolean notifications is loaded unchanged alongside persisted aliases. Migration from schema 006 preserves names, channels and message bodies.
