# Filename labels and keyboard text selection

Use filename labels with full-path opt-in and keyboard copy mode.
Explicitly preserve terminal link handling, mouse selection and Screen thumbnails.

Config show_attachment_paths defaultfalse; internal paths untouched.
Alt+V select_text frozen RAM snapshot of loadedconversation body+attachmentlabels,
selected/currentbottom initial cursor, no headers/frames/thumbs, arrows/Space/Enter/
Esc, configurable defaultnotCtrlA/B. Unicode graphemeindices preserve exactcopy,
resize reflowsdisplay, no source draft/reply mutations. Clipboard failure retains
selection. Existing systemclipboard reused; OSC52 deliberately not added.

Tests cover default/opt-in filelabels andstoredpaths, Unicode/wrapping/selection
direction, tinyviewports, incomingdata freeze, F4resize, modalpasteisolation,
Enter no send, clipboardunavailable retainsrange, configurable key andallhidden.

Verification:219 all-feature tests passed, Clippy all-targets/all-features with
warnings denied passed, fmt/diffchecks passed. Independent review found exactwidth
newline verticalcursor ambiguity; regression failed copying newline insteadofc.
Fixed clampedmoves andtie resolution, regression+fullsuite pass. Clipboard tests
avoid realdesktop writes andverify failure retainsselection. No live Signal sends.

Arch verification: aliases9 -> aliases10 isolatedupgrade passed, config SHA256 preserved, version runs, Qkk0 alteredfiles, installedREADME matches source, zstd integrity+SHA updated, private screenshots excluded.
