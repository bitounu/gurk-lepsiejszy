# Attachment delivery investigation

Mixed visual-media and generic-file submissions can be truncated by the receiving client. Local attachment saves alone do not establish complete remote delivery. Record successful upload and submission boundaries to distinguish preparation, transport and receiver compatibility failures.

- [x] Verify preparation and the send-manager boundary offline with mixed-format fixtures; all prepared attachments must reach the mocked send without modifying originals.
- [x] Read upstream Presage batch upload: join_all maps all inputs to per-file results. Existing application checks individual errors but lacked an explicit expected-result-count check.
- [x] Regression red: three successful result pointers with expected six incorrectly allowed. Guard now aborts without mutating DataMessage; six success pointers retain filenames and order.
- [x] Add local diagnostic counts: batch start, each returned result, batch completion, message submission start/completion, correlated by sent timestamp. No message body, file bytes, keys or CDN URLs added to records. Submission completion is not recipient delivery.
- [x] Tests: 294 library + 1 CLI passed; Clippy all-targets/all-features -D warnings passed; fmt/diff checks clean.
- [x] Built and verified aliases19 diagnostic Arch upgrade in isolated root; binary/README/integrity/CLI checks pass and existing config checksum unchanged.
- [x] Identify receiver compatibility rule: Desktop truncates a visual-media sequence at a generic HEIC attachment. Follow-up batching behavior and regression coverage are recorded in the compatible-attachment-batches plan. Verification uses no live Signal sends.
