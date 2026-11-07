# Secure Backup Import Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** RestoreexistingcloudSecureBackup intoGurk whilepreservinglocalidentity
andpreferences.
**Architecture:** readonlyendpointallowlistedclouddownloader+officialSVRB, typed
streamingauthenticatedarchiveadapter, encryptedstaging/merge and verifiedmedia.
**Tech Stack:** Rust1.94, existingPresage, pinnedlibsignal0.106.1 (backup), existing0.99 (messaging), reqwest,SQLCipher.
**Spec:** docs/superpowers/specs/2026-11-07-secure-backup-import.md

## Global Constraints
No liveuseraccounttests, no secrets inlogs/CLIargv, no phone re-registration.
Existing features remain compatible. Multi-gigabyte media must never be buffered as one Vec.

## Review Focus
Wrongkey/truncatedarchive mustnotpartiallyoverwritehistory.
Remoteendpointauthmustnotleak toCDN orredirects.
SVRB failuresmustnotretry andburnattempts.
Reimport skipsduplicates andpreservesaliases/drafts/hidden/edited/deletedstate.
Mediaauth/hash/errors neverproducefalseavailablefiles; interruptedworkresumable.

- [x] Investigate upstreamauth/SDK/format boundaries (linkeddownloadsupported).
- [x] Cloud downloader+auth/SVRB fixtures.
- [x] Authenticatedstreamingframeadapter+stableidentitymapping fixtures.
- [x] Media naming/auth/layereddecryption verifiedagainstupstream+fixtures.
- [x] Encryptedstage/merge/receivedorder/dedup tests.
- [x] CLIintegration/progress/resume/report/README.
- [x] Fullverification/independentreview/Archpackage.

Implementation review fixed duplicate attachment hydration over local edits,
long-text retry truncation, private-directory modes/ancestry, bounded one-record
staging reads, and shared live/exclusive restore process locks. Pinned backup SDK
was updated to 0.106.1 to match current Android Standard-mode server parameters;
existing messaging remains on its original dependency revision. Media location
credentials are cached in RAM for five minutes, scoped by key/type/CDN.
No real-account requests or user recovery key were used for development.

Final source validation: 239 all-feature tests pass; Clippy all-targets/all-features
with -D warnings passes; cargo fmt --check and git diff --check pass.

Arch aliases11: release build and package succeeded. Upgraded isolated Arch root
from aliases10; pacman -Qkk, CLI help/option conflict checks, README comparison,
configuration checksum and zstd integrity all passed. libstdc++ dependency added.

Follow-up aliases12: fixed credential selection expecting uppercase map keys,
where Signal-Server CredentialType.toValue serializes messages/media lowercase.
Regression fixture reproduced the original No current backup credential failure
before changing selection. All 240 tests, strict Clippy and formatting pass.
Release package upgrade aliases11→aliases12, package integrity/config preservation
and CLI options verified in isolated Arch. Live cloud restoration is outside automated verification.

Follow-up aliases13: add offline --backup-report for diagnosing incomplete imports
without reading Signal registration or prompting for the recovery key. Existing
SQLCipher staging opens read-only/query-only; only fixed error labels and counts
are emitted. Repeated source IDs classified by sender and payload digest; no raw
errors, messages, identifiers or filenames are printed. No speculative change to
media restoration or message identity handling. 242 library + 1 CLI tests pass,
strict Clippy passes. Arch upgrade, help/conflicts, missing-stage offline invocation,
package integrity and unchanged config verified in isolated Arch.
