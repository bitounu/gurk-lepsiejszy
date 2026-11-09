# Panning and dismissible composer errors

- [x] Reproduce persistent edit-error panel with an incoming selected message and e; Alt+P is a separate ViewImage action and does not call edit validation.
- [x] Escape dismisses composer_error first, preserving draft/selection/stagedfiles; subsequent Escape retains existing cancel/unselect semantics. Error-only status has Notice/Esc dismiss title, pendingfiles show appropriate dismiss-error hint.
- [x] Add bounded image panning with Alt+arrows in gallery only. Existing conversation Alt+arrows remain unchanged. Pan moves native crop, uses decoded source in RAM, no disk/large zoom bitmaps; fit/zoom/navigation resets center as documented.
- [x] F1, viewerfooter and keybar/README + aliases22 package.
- [x] Full tests/Clippy/fmt and Arch upgrade/config checks; preserve original files and avoid live Signal sends.

Verification: cargo test --all-targets (323 library + 1 CLI test), cargo clippy --all-targets -- -D warnings, cargo fmt --all -- --check passed. Arch aliases22 built; isolated pacman upgrade from aliases21 and Qkk passed, existing config hash unchanged; CLI help/conflict/offline report checks passed. Scoped independent review found no critical/important issues. No live Signal sends or real terminal hardware tests performed.
