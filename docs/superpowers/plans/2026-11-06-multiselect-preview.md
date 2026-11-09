# Multi-file picker and preview

Requirements: up to10 selected files with counter, highlighted-image-only
Alt+P fullscreen using existing renderer, Escape returns and clears rendering,
no disk artifacts; highlightedfile opens via MIME app; complete F1 shortcuts.

Choices: Space toggles marks across folders, Enter onfile stages marked batch
(or singleton), Enter ondirectory navigates. Pending attachments capped10 and
batch validation atomic. Preview forces internal renderer; external Alt+O uses
xdg-open desktop associations. README explains xdg-mime and desktop entries.

Tests:181 all-feature tests passed. Includes fixture-only highlightedblue vs
markedred image rendering, return+marks preserved, no newdiskfiles, no sends;
10-file cap across directories, batchvalidation atomic, MIME fake launcher with
spaces/metacharacters and errors, F1 allshortcutcoverage.

Independent focused read-only review found no important defects.

Final verification: Clippy all-targets/all-features with warnings denied, fmt and diff checks passed. aliases7 package integrity and isolated Arch aliases6 -> aliases7 upgrade passed; existing config SHA256 unchanged, --version works, Qkk0 alteredfiles. Checksums updated. All desktop opener tests fake-only, no actual GUI or Signal sends.
