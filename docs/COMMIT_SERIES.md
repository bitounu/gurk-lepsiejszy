# Reconstructed publication commit series

This series reconstructs the existing work by source component and recorded
conversation milestones. Shared application/storage wiring follows the component
modules; see [DEVELOPMENT_HISTORY.md](DEVELOPMENT_HISTORY.md) for the dependency
and validation limits. Both Git dates are UTC milestone dates, not a claim that
individual source snapshots were committed at those historical times.

## 1. feat(aliases): add local person and conversation alias editing

Date (author and committer, UTC): `2026-11-05T22:59:35Z`.

Local alias implementation completion reported in the conversation.

- `docs/superpowers/plans/2026-11-05-local-aliases.md`
- `docs/superpowers/specs/2026-11-05-local-aliases-design.md`
- `src/app/aliases.rs`

## 2. feat(composer): add durable drafts, staged attachments and file picking

Date (author and committer, UTC): `2026-11-06T10:55:10Z`.

File picker multiselect and staged composer milestone, package aliases7.

- `docs/superpowers/plans/2026-11-06-composer.md`
- `docs/superpowers/plans/2026-11-06-file-picker-controls.md`
- `docs/superpowers/specs/2026-11-06-composer-design.md`
- `src/app/composer.rs`
- `src/screenshot.rs`
- `src/ui/file_picker.rs`

## 3. feat(conversations): add hidden conversations and contextual shortcut themes

Date (author and committer, UTC): `2026-11-06T11:35:44Z`.

Hidden conversations and shortcut themes milestone, package aliases8.

- `docs/superpowers/plans/2026-11-06-hidden-shortcuts.md`
- `src/app/hidden.rs`
- `src/ui/shortcut_bar.rs`

## 4. feat(messaging): add anonymous forwarding and private linked-device read state

Date (author and committer, UTC): `2026-11-06T12:19:52Z`.

Message order, forwarding and private linked-device reads, package aliases9.

- `docs/superpowers/plans/2026-11-06-order-forward-private-reads.md`
- `src/app/forward.rs`
- `src/app/read_sync.rs`

## 5. feat(backup): add authenticated resumable Secure Backup import and reports

Date (author and committer, UTC): `2026-11-07T14:15:53Z`.

Backup import/report and collision follow-up milestone, package aliases14.

- `docs/superpowers/plans/2026-11-07-secure-backup-import.md`
- `docs/superpowers/specs/2026-11-07-secure-backup-import.md`
- `src/backup/archive.rs`
- `src/backup/cli.rs`
- `src/backup/cloud.rs`
- `src/backup/import.rs`
- `src/backup/media.rs`
- `src/backup/mod.rs`
- `src/backup/report.rs`
- `src/backup/staging.rs`
- `src/backup/svrb.rs`
- `src/backup/types.rs`

## 6. feat(history): add literal search and keyboard message text selection

Date (author and committer, UTC): `2026-11-07T14:58:49Z`.

Conversation search and navigation completion, package aliases15; text selection was earlier that day.

- `docs/superpowers/plans/2026-11-07-attachment-labels-text-selection.md`
- `docs/superpowers/plans/2026-11-07-conversation-search.md`
- `src/app/search.rs`
- `src/app/text_copy.rs`
- `src/search.rs`
- `src/ui/text_selection.rs`

## 7. feat(privacy): add offline spelling and lossless image metadata cleaning

Date (author and committer, UTC): `2026-11-08T07:03:58Z`.

Offline spelling and image privacy completion, package aliases18.

- `docs/superpowers/plans/2026-11-08-spelling-and-image-privacy.md`
- `src/app/spelling.rs`
- `src/image_metadata.rs`
- `src/spellcheck.rs`

## 8. fix(attachments): preserve complete uploads with compatible ordered batches

Date (author and committer, UTC): `2026-11-08T08:41:49Z`.

Compatible attachment batch fix completion, package aliases20.

- `docs/superpowers/plans/2026-11-06-attachment-upload-fix.md`
- `docs/superpowers/plans/2026-11-08-attachment-delivery-investigation.md`
- `docs/superpowers/plans/2026-11-08-compatible-attachment-batches.md`
- `src/signal/attachment_batch.rs`
- `src/signal/send_order.rs`

## 9. build: pin Signal libraries and extension dependencies

Date (author and committer, UTC): `2026-11-09T22:13:52Z`.

Final media/editor dependency milestone, package aliases21; dependencies span earlier features.

- `Cargo.lock`
- `Cargo.toml`
- `vendor/README.md`
- `vendor/libsignal-service/.github/actions-rs/grcov.yml`
- `vendor/libsignal-service/.github/codecov.yml`
- `vendor/libsignal-service/.github/workflows/ci.yaml`
- `vendor/libsignal-service/.github/workflows/pages.yaml`
- `vendor/libsignal-service/.gitignore`
- `vendor/libsignal-service/Cargo.toml`
- `vendor/libsignal-service/LICENSE.md`
- `vendor/libsignal-service/README.md`
- `vendor/libsignal-service/build.rs`
- `vendor/libsignal-service/certs/production-root-ca.pem`
- `vendor/libsignal-service/certs/staging-root-ca.pem`
- `vendor/libsignal-service/examples/storage.rs`
- `vendor/libsignal-service/examples/usernames.rs`
- `vendor/libsignal-service/protobuf/DeviceName.proto`
- `vendor/libsignal-service/protobuf/Groups.proto`
- `vendor/libsignal-service/protobuf/Provisioning.proto`
- `vendor/libsignal-service/protobuf/SignalService.proto`
- `vendor/libsignal-service/protobuf/StickerResources.proto`
- `vendor/libsignal-service/protobuf/StorageService.proto`
- `vendor/libsignal-service/protobuf/WebSocketResources.proto`
- `vendor/libsignal-service/protobuf/update-protos.sh`
- `vendor/libsignal-service/rust-toolchain`
- `vendor/libsignal-service/rustfmt.toml`
- `vendor/libsignal-service/src/account_manager.rs`
- `vendor/libsignal-service/src/attachment_cipher.rs`
- `vendor/libsignal-service/src/cipher.rs`
- `vendor/libsignal-service/src/configuration.rs`
- `vendor/libsignal-service/src/content.rs`
- `vendor/libsignal-service/src/content/data_message.rs`
- `vendor/libsignal-service/src/content/story_message.rs`
- `vendor/libsignal-service/src/digeststream.rs`
- `vendor/libsignal-service/src/envelope.rs`
- `vendor/libsignal-service/src/groups_v2/manager.rs`
- `vendor/libsignal-service/src/groups_v2/mod.rs`
- `vendor/libsignal-service/src/groups_v2/model.rs`
- `vendor/libsignal-service/src/groups_v2/operations.rs`
- `vendor/libsignal-service/src/groups_v2/utils.rs`
- `vendor/libsignal-service/src/kat.bin.rs`
- `vendor/libsignal-service/src/lib.rs`
- `vendor/libsignal-service/src/master_key.rs`
- `vendor/libsignal-service/src/messagepipe.rs`
- `vendor/libsignal-service/src/models.rs`
- `vendor/libsignal-service/src/pre_keys.rs`
- `vendor/libsignal-service/src/profile_cipher.rs`
- `vendor/libsignal-service/src/profile_name.rs`
- `vendor/libsignal-service/src/proto.rs`
- `vendor/libsignal-service/src/provisioning/cipher.rs`
- `vendor/libsignal-service/src/provisioning/mod.rs`
- `vendor/libsignal-service/src/provisioning/pipe.rs`
- `vendor/libsignal-service/src/push_service/account.rs`
- `vendor/libsignal-service/src/push_service/cdn.rs`
- `vendor/libsignal-service/src/push_service/error.rs`
- `vendor/libsignal-service/src/push_service/linking.rs`
- `vendor/libsignal-service/src/push_service/mod.rs`
- `vendor/libsignal-service/src/push_service/response.rs`
- `vendor/libsignal-service/src/receiver.rs`
- `vendor/libsignal-service/src/sender.rs`
- `vendor/libsignal-service/src/service_address.rs`
- `vendor/libsignal-service/src/session_store.rs`
- `vendor/libsignal-service/src/sticker_cipher.rs`
- `vendor/libsignal-service/src/storage_service.rs`
- `vendor/libsignal-service/src/timestamp.rs`
- `vendor/libsignal-service/src/unidentified_access.rs`
- `vendor/libsignal-service/src/utils.rs`
- `vendor/libsignal-service/src/utils/phonenumber.rs`
- `vendor/libsignal-service/src/websocket/account.rs`
- `vendor/libsignal-service/src/websocket/calling.rs`
- `vendor/libsignal-service/src/websocket/directory.rs`
- `vendor/libsignal-service/src/websocket/keys.rs`
- `vendor/libsignal-service/src/websocket/linking.rs`
- `vendor/libsignal-service/src/websocket/mod.rs`
- `vendor/libsignal-service/src/websocket/profile.rs`
- `vendor/libsignal-service/src/websocket/registration.rs`
- `vendor/libsignal-service/src/websocket/request.rs`
- `vendor/libsignal-service/src/websocket/sender.rs`
- `vendor/libsignal-service/src/websocket/stickers.rs`
- `vendor/libsignal-service/src/websocket/usernames.rs`
- `vendor/presage/Cargo.toml`
- `vendor/presage/LICENSE.md`
- `vendor/presage/src/errors.rs`
- `vendor/presage/src/lib.rs`
- `vendor/presage/src/manager/confirmation.rs`
- `vendor/presage/src/manager/linking.rs`
- `vendor/presage/src/manager/mod.rs`
- `vendor/presage/src/manager/registered.rs`
- `vendor/presage/src/manager/registration.rs`
- `vendor/presage/src/model/contacts.rs`
- `vendor/presage/src/model/groups.rs`
- `vendor/presage/src/model/identity.rs`
- `vendor/presage/src/model/messages.rs`
- `vendor/presage/src/model/mod.rs`
- `vendor/presage/src/serde.rs`
- `vendor/presage/src/store.rs`

## 10. feat(media): add image previews, attachment gallery and external editors

Date (author and committer, UTC): `2026-11-09T22:34:03Z`.

Gallery pan and error dismissal milestone, package aliases22; media work spans multiple dates.

- `docs/superpowers/plans/2026-11-05-image-previews.md`
- `docs/superpowers/plans/2026-11-06-image-viewer.md`
- `docs/superpowers/plans/2026-11-06-multiselect-preview.md`
- `docs/superpowers/plans/2026-11-09-gallery-editor-preview.md`
- `docs/superpowers/plans/2026-11-09-preview-pan-dismiss-errors.md`
- `docs/superpowers/specs/2026-11-05-image-previews-design.md`
- `docs/superpowers/specs/2026-11-06-image-viewer-design.md`
- `src/app/image_viewer.rs`
- `src/editor_preview.rs`
- `src/ui/file_opener.rs`
- `src/ui/image_preview.rs`
- `src/ui/image_viewer.rs`

## 11. feat(mentions): add identity-bound contact completion and UTF-16 mention model

Date (author and committer, UTC): `2026-11-09T22:59:06Z`.

Native contact mention completion, package aliases23.

- `docs/superpowers/plans/2026-11-09-contact-mentions.md`
- `src/app/mentions.rs`
- `src/mentions.rs`
- `src/ui/mentions.rs`

## 12. feat(emoji): add ASCII conversion primitives and Unicode cursor corrections

Date (author and committer, UTC): `2026-11-09T23:17:24Z`.

ASCII emoticon and Unicode cursor completion, package aliases24.

- `docs/superpowers/plans/2026-11-09-ascii-emoticons.md`
- `src/cursor.rs`
- `src/emoji.rs`

## 13. build(arch): add source package recipe and installation documentation

Date (author and committer, UTC): `2026-11-09T23:17:24Z`.

Final Arch recipe release milestone, package aliases24; earlier recipe releases remain documented.

- `packaging/arch/.gitignore`
- `packaging/arch/PKGBUILD`
- `packaging/arch/README.md`

## 14. chore(repo): exclude private artifacts and document reconstructed history

Date (author and committer, UTC): `2026-11-10T06:26:58Z`.

Original publication commit timestamp; repository hygiene is publication work.

- `.gitignore`
- `docs/COMMIT_SERIES.md`
- `docs/DEVELOPMENT_HISTORY.md`
- `docs/superpowers/plans/2026-11-10-publication-history.md`

## 15. feat(app): integrate extensions, storage identity and compatible migrations

Date (author and committer, UTC): `2026-11-10T06:26:58Z`.

Actual publication integration commit timestamp; not an earlier source snapshot.

- `docs/superpowers/plans/2026-11-07-message-time-collisions.md`
- `migrations/007_local_aliases.down.sql`
- `migrations/007_local_aliases.up.sql`
- `migrations/008_composer_drafts.down.sql`
- `migrations/008_composer_drafts.up.sql`
- `migrations/009_hidden_channels.down.sql`
- `migrations/009_hidden_channels.up.sql`
- `migrations/010_message_display_order.down.sql`
- `migrations/010_message_display_order.up.sql`
- `migrations/011_private_read_sync_outbox.down.sql`
- `migrations/011_private_read_sync_outbox.up.sql`
- `migrations/012_message_identity.down.sql`
- `migrations/012_message_identity.up.sql`
- `migrations/013_draft_mentions.down.sql`
- `migrations/013_draft_mentions.up.sql`
- `src/app/channel.rs`
- `src/app/input.rs`
- `src/app/message.rs`
- `src/app/mod.rs`
- `src/app/window.rs`
- `src/channels.rs`
- `src/command.rs`
- `src/config.rs`
- `src/data.rs`
- `src/event.rs`
- `src/input.rs`
- `src/lib.rs`
- `src/main.rs`
- `src/shortcuts.rs`
- `src/signal/impl.rs`
- `src/signal/manager.rs`
- `src/signal/mod.rs`
- `src/signal/test.rs`
- `src/storage/forgetful.rs`
- `src/storage/mod.rs`
- `src/storage/sql/storage.rs`
- `src/ui/draw.rs`
- `src/ui/mod.rs`
- `src/ui/name_resolver.rs`
- `src/util.rs`

## 16. docs: describe extension configuration, compatibility and user workflows

Date (author and committer, UTC): `2026-11-10T06:26:58Z`.

Actual consolidated/anonymized documentation publication timestamp.

- `README.md`
