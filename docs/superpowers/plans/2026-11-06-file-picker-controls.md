# File picker controls

Requirements: dotfiles visibility toggle on `.`, toggle name/mtime sorting on `t`,
Left parent/Right selected directory only, shortcuts to Pictures and screenshot_dir.
Decisions: hidden initially, newest-first mtime, directories first in both modes;
p = ~/Pictures, s = resolved screenshot_dir with existing Pictures/home fallback.

Implementation: retain directory snapshot, filter/sort on toggles preserving selected
path, apply preferences to async navigation results. Path editor keeps literal keys.
Footer shows shortcuts, status shows hidden/sorting state. No configuration migration.

Verification: regression failed on old visibility; tests exercise toggle/sort,
selection preservation, right-on-file does not stage, directory arrows, configured
tilde path and defaults, shortcuts remain literal while typing paths.

Final checks: 171 all-feature tests passed; Clippy all-targets/all-features with warnings denied, formatting and diff checks passed. Independent focused review found no important issues. Final aliases6 package integrity and isolated Arch aliases5 -> aliases6 upgrade passed, config SHA256 unchanged, --version works, Qkk 0 altered files. Checksums updated.
