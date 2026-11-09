# Attachment gallery and editor preview implementation plan

Extend gallery and editor previews while preserving existing functionality and original files.

- [x] Extend existing image viewer to select attachment index with Left/Right, show index/filename/zoom, fit default, +/- bounded zoom centered without reencoding source on disk. Navigation resetszoom, asynchronous stale jobs cannot replace newer selection; RAM source released on close.
- [x] Unknown/nonimage readable files request EDITOR preview; missing or recognized-but-oversized images show expliciterror. Editorcommand parsed without shell, fileargumentliteral, readonly privatecopy withcleanup and unchangedoriginal.
- [x] Appkeyboard/event hooks plus terminal handoff (abortreader, disablemouse/focus/raw/alternate, editorinheritsstdio, restoreterminal and reader on success/failure). Preserve composer/drafts/message/picker selection and no sends.
- [x] Update F1/keybar/README and aliases21 Arch recipe.
- [x] Syntheticregressions, fulltests, Clippy/fmt/diff, Arch packageupgrade/integrity/configpreservation verification. No live Signal operations.

Verification so far: 317 library + 1 CLI tests pass with two testthreads, including end-to-end Appkey handling and unknowncontents editorrequest/error flow. Clippy all-target/all-feature -D warnings passes. Scoped integration review found no blockers. EDITOR helper tests execute a real fakeeditor and confirm immutable originals, mode0400 and sidecar cleanup on exit0/7; no actual userEDITOR or liveSignal invoked. Temporary toolchain/packageverificationroot restored after ephemeral /tmp reset. Release aliases21 built; upgraded aliases20->21 in isolated Archroot with official Archruntime libraries. Integrity, README/privatefile contents scan, CLI smoke/conflicts and unchanged existing configuration hash all pass. No live Signal operations or real graphicalterminaleditor session performed.
