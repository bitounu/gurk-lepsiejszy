# Local Arch Linux test package

This PKGBUILD packages the current checkout, including uncommitted changes.
It is intended for local testing, not submission to the AUR.

On Arch Linux, build from this directory with:

```sh
makepkg -s
sudo pacman -U ./gurk-local-aliases-0.10.2.aliases15-1-x86_64.pkg.tar.zst
```

The package installs `/usr/bin/gurk` and documentation. It provides `gurk`
and conflicts with `gurk`, `gurk-git` and `gurk-bin`, so pacman can replace an
existing installation. User configuration, databases and linked device data
are not included in the package and are not overwritten by installation.

Run `gurk` normally. F2 edits the current channel alias; F3 edits the selected
message author's alias, or the direct conversation's contact when no message
is selected. Enter saves, Escape cancels, and an empty alias removes it.

The first launch upgrades the gurk database using additive migrations 007 through 011. For tests
with an existing account, close the previous gurk instance and copy its data
directory before the first launch, so it is available if you want to revert
the application and database together.

To restore the official Arch package after testing:

```sh
sudo pacman -S gurk
```

This build also fixes F1 help scrolling and adds optional inline image previews.
To enable previews, add `show_image_previews = true` at the top level of
`gurk.toml` (before any table headers), then restart gurk. Old configurations
without this setting keep previews disabled. PNG, JPEG, GIF, WebP, BMP and TIFF
are detected by file contents and fitted to the conversation pane.

Alt+P opens the selected message image fullscreen, independently of inline
previews. The built-in viewer negotiates Kitty/Sixel/iTerm2 support and falls
back to Unicode half blocks. Esc/q returns to the conversation. Optional
`[image_viewer] mode = "external"` with `command = ["sxiv", "-a"]` launches sxiv;
see the project README for complete TOML examples and shortcut configuration.

This build persists conversation drafts and adds Alt+A file attachments and Alt+S
screenshots. Both stage attachments first; Enter confirms sending and Escape
removes pending attachments while preserving text. See the installed README for
file picker controls, icon fallback and screenshot directory/command settings.

This build adds experimental cloud Signal Secure Backup import. Start with
`gurk --restore-cloud-backup --backup-dry-run`; the recovery key is entered
locally without echo. See the installed README for resume, disk space and
unsupported-record limitations. Existing local aliases are preserved.

Conversation navigation: Ctrl+Home oldest, Ctrl+End newest/follow; / with an
empty composer or F8 opens Unicode literal search. Up/Down selects occurrences;
Enter keeps the result and Esc restores the previous view. See README for details.
