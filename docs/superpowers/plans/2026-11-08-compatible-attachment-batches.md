# Compatible outgoing attachment batches

Signal Desktop getValidMessageAttachments keeps a prefix of visual media until the first ordinary file. An HEIC pointer classified as application/octet-stream therefore terminates a mixed visual-media sequence. Use synthetic mixed-format fixtures to verify compatible batching. Source: https://github.com/signalapp/Signal-Desktop/blob/main/ts/util/Attachment.std.ts ; supported MIME matching is exact in GoogleChrome.std.ts.

- [x] Pure compatible_batches: preserve contiguous canonical JPEG/PNG/WebP albums; all other values/voice notes are single-file messages; preserve order and bytes. Six-file regression shape3/1/2, document singles, MIME edge, empty text.
- [x] Share per-part queueing between composer and forward; caption/reply only first part; forward never takes source quote or composer state.
- [x] Unique outgoing timestamps shared by production/mock sends; tests same clock, rollback, concurrent reservations.
- [x] Shared per-conversation send FIFO guards retained across upload/submission and released on errors; tests independent conversations, FIFO and clones.
- [x] Full tests pass: 305 library + 1 CLI; Clippy all-targets/all-features -D warnings passes; fmt/diff clean. No live Signal traffic from agent.
- [x] Built/verified aliases20 Arch upgrade from aliases19 in isolated root, package integrity/README/CLI smoke tests and unchanged configuration fixture hash.
