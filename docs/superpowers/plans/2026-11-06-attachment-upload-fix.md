# Attachment upload fix

Symptom: locally visible screenshot, recipient receives empty message.

Evidence: Presage upload_attachments returns nested per-file Results; Gurk
filtered Err values and continued sending an attachment-free DataMessage. Local
attachment copies/previews are created before upload.

Dependency defect: pinned libsignal-service fb92b13 requests an upload form without
uploadLength. Upstream PR501/7a75267 adds the encrypted attachment size to the
request. Vendored unmodified upstream revision (four small commits since original
pin), documented in vendor/README.md, with Cargo patch for reproducibility.

Regression tests: single image failure and mixed success/failure both failed
before fixing ignored errors; success preserves complete server pointers. Fix
collects all results before mutating payload, propagating error to existing send
failure event for both user and group. UI shows complete error chain.

No live Signal messages, account initialization or desktop capture during tests.

Upstream evidence: https://github.com/whisperfish/libsignal-service-rs/pull/501
confirms HTTP400 without positive uploadLength, merged 2026-11-05.
169 all-feature tests passed after dependency patch. Independent focused code
review found no important defects; real recipient confirmation remains required.

Final verification: Clippy all targets/features with -D warnings, fmt check and diff check passed. aliases5 package zstd integrity and isolated Arch aliases4 -> aliases5 upgrade passed; executable --version works, pacman -Qkk 0 altered files, existing configuration SHA256 preserved. dist/SHA256SUMS updated.
