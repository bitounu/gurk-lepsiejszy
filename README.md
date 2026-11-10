# gurk 🥒
[![ci][ci-badge]][ci-link] [![chat][chat-badge]][chat-link] [![ko-fi][ko-fi-badge]][ko-fi-link]

[Signal Messenger] client for terminal.

![screenshot](screenshot.png)

# Installation

## Pre-compiled binary

Download a pre-compiled binary from [Releases].

Or, if you have [`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall):

```shell
cargo binstall --git https://github.com/boxdot/gurk-rs gurk
```

## From source (using `cargo`)

Prerequisites:

* [`protoc`] compiler
* `perl`

```shell
cargo install --git https://github.com/boxdot/gurk-rs gurk
```

## Arch Linux

```shell
pacman -S gurk
```
(as root)
- Official repository, tagged releases: [`gurk`](https://archlinux.org/packages/extra/x86_64/gurk)
```shell
yay -S gurk
```
- AUR source build from Git HEAD: [`gurk-git`](https://aur.archlinux.org/packages/gurk-bin)

## Nix/NixOS

Either per user:

```
$ nix-env --install gurk-rs
```

or system-wide:

```nix
environment.systemPackages = with pkgs; [ gurk-rs ];
```

# Freebsd
#### (as root)
```shell
pkg install net-im/gurk-rs
```
- pkg repositories


```shell
cd /usr/ports/net-im/gurk-rs
make install clean
```
- ports tree
# Usage

Run

```
gurk
```

On the first run, it will open a QR code in your favorite image viewer, such that you can link the
client as a new device. This will also create a configuration file at the default [config
location][config-location]. For the configuration directives, see [`src/config.rs`].

Note: The binary cannot be published on crates.io, because it depends on several official Signal
libraries that are not available on crates.io.

## Chat

[![chat-qr](chat-qr.png)][chat-link]

# Key bindings

## Default keybindings

* App navigation
  * `f1` Toggle help panel.
  * `ctrl+c` Quit.
* Message input
  * `tab` Send emoji from input line as reaction on selected message.
  * `alt+enter` Switch between multi-line and single-line input modes.
  * `alt+left`, `alt+right` Jump to previous/next word.
  * `ctrl+w / ctrl+backspace / alt+backspace` Delete last word.
  * `ctrl+u` Delete to the start of the line.
  * `enter` *when input box empty in single-line mode* Open URL from selected message.
  * `enter` *otherwise* Send message.
* Multi-line message input
  * `enter` New line
  * `ctrl+j / Up` Previous line
  * `ctrl+k / Down` Next line
* Cursor
  * `alt+f / alt+Right / ctrl+Right` Move forward one word.
  * `alt+b / alt+Left / ctrl+Left` Move backward one word.
  * `ctrl+a / Home` Move cursor to the beginning of the line.
  * `ctrl+e / End` Move cursor the the end of the line.
* Message/channel selection
  * `esc` Reset message selection or close channel selection popup.
  * `alt+Up / alt+k / PgUp` Select previous message.
  * `alt+Down / alt+j / PgDown` Select next message.
  * `ctrl+j / Up` Select previous channel.
  * `ctrl+k / Down` Select next channel.
  * `ctrl+p` Open / close channel selection popup.
  * `alt+m` Toggle mute for the selected channel (silences notifications; muted channels are marked with `[M]`).
* Clipboard
  * `alt+y` Copy selected message to clipboard.
* Help menu
  * `esc` Close help panel.
  * `ctrl+j / Up / PgUp` Previous line
  * `ctrl+k / Down / PgDown` Next line

## Search conversation history

Press `/` with an empty composer, or `F8` with any draft, to search the active
conversation's full message text, including older history outside the loaded
window. Search is literal and Unicode case insensitive; attachment filenames,
metadata, hidden conversations and other conversations are excluded. Each
occurrence is a separate match. Queries stay in memory and are never sent to Signal.

Use `Up` / `Down` or `Shift+F3` / `F3` to move between matches. `Enter` accepts
the current position and highlight; `Esc` restores the position from before the
search. Draft text and pending attachments are preserved. `/` in a nonempty
composer stays literal, so typing URLs works as usual; use `F8` to search then.

`Ctrl+Home` jumps to the oldest message; `Ctrl+End` jumps to the newest and
resumes following new messages. A found message stays selected for replies:
press `PgDown` through the newest selected message (or `Esc`) to clear the reply
selection. `Alt+V` opens keyboard text selection and copying. A matching message
is shown in full when it fits; longer messages show the matching fragment, with
`Alt+V` providing access to the full text for copying.

These commands can be configured in `gurk.toml`, for example:

```toml
[keybindings.normal]
"/" = "" # Disable the slash shortcut.
F8 = "search_messages"
ctrl-home = "jump_oldest"
ctrl-end = "jump_newest"
```

## Editing sent messages

Select your own message with `PgUp` / `PgDown`, then press `e` (or the
existing `Ctrl+E`) with an empty composer. Edit the text and press `Enter` to
send the change; `Esc` cancels. In multiline mode, `Enter` inserts a newline;
use `Alt+Enter` to switch back before sending. While composing, `e` remains
an ordinary letter. Other people's messages cannot be edited.

Signal supports up to 10 edits within 24 hours of sending; Note to Self has
no time limit, but keeps the 10-edit limit. See the
[Signal editing documentation](https://support.signal.org/hc/en-us/articles/6255134251546-Edit-Message).
Gurk checks these limits when editing starts and again before sending. An
expired editing draft remains available for copying, but cannot be sent as an
edit. The limit counts recorded edits, including those synchronized from other
devices; history missing from a client cannot be counted locally. Recipients
can see that a message was edited and inspect its history. History versions
are kept in storage but are not separate messages in the conversation.

The shortcut can be changed or disabled:

```toml
[keybindings.message_selected]
e = "edit_message" # Use an empty string to disable.
```

## Offline spelling

Spelling is checked locally in a Rust background worker after a 300 ms pause.
Install dictionaries on Arch with `sudo pacman -S hunspell-pl hunspell-en_us`.
The Hunspell executable/library is not used. Missing dictionaries are shown in
the Input title; text is never sent to a spelling service. Polish and US English
are enabled together: a word accepted by either dictionary is accepted.

Suspect words are red and underlined. Press `F9` to correct the suspect word
under the cursor, or the first suspect word otherwise. Use `Up`/`Down` to choose,
`Enter` to replace that word, `i` to ignore it for this session, and `Esc` to close.
Enter in this dialog never sends a message. There is no automatic replacement
or mandatory check before sending. Paths, links, email addresses, numeric tokens
and backtick-delimited code are skipped. This checks spelling, not grammar.
Checks cover the first 32 KiB of a draft; suggestions are generated on demand.

```toml
[spellcheck]
enabled = true
languages = ["pl_PL", "en_US"]
dictionary_dirs = [] # Additional directories containing NAME.aff and NAME.dic.
ignored_words = ["gurk"] # Persistent personal exceptions; i is session-only.

[keybindings.normal]
F9 = "check_spelling"
```

Dictionary search checks configured directories, then
`$XDG_DATA_HOME/gurk/dictionaries` (normally `~/.local/share/gurk/dictionaries`),
`/usr/share/hunspell`, `/usr/share/myspell/dicts` and `/usr/share/myspell`.
Dictionaries are loaded at startup. A changed language/dictionary configuration
requires restart. Keep both `.aff` and `.dic` files together; legacy dictionary
encodings such as ISO-8859-2 are decoded locally.

## Image metadata privacy

`clean_image_metadata = true` is the default **top-level** TOML option (place it
before `[user]` or any other table). Set it to `false` to send originals.
Cleaning covers **JPEG and PNG identified by their contents**, including picker
uploads, screenshots, `file://` paths, clipboard images and forwarded files.
HEIC and other formats are sent unchanged in this release.

Cleaning copies compressed image data without decoding/recompressing pixels.
It removes descriptive EXIF/XMP/IPTC/MakerNotes, comments, thumbnails and PNG
C2PA provenance blocks. JPEG Motion Photo video and extra images after the main
JPEG are removed; this also discards an appended HDR gain map. The main image
remains a normal SDR JPEG. ICC colour profiles, rendering data and a minimal
EXIF Orientation field are retained so rotation and colours keep working.
PNG transparency and supported APNG animation chunks are retained.

Original files and saved screenshots are not modified. Only outgoing bytes and
the local sent attachment copy are cleaned; no temporary cleaned files are
left behind. A malformed recognized JPEG/PNG stops the entire upload and keeps
the composer and staged files. Cleaning does not anonymize filenames or remove
information visible in the pixels. Retained ICC/rendering fields are not claimed
to be free of all possible identifying data; this is the agreed practical
metadata cleanup, not a forensic anonymization guarantee.

## File Uploads
  * `file:///path/to/file` Upload File "file" at path "/path/to/"
  * `file://clip` Upload Content of Clipboard

## Configuration

### Command-line options

```
-c, --config <PATH>     Path to config file
    --data-dir <PATH>   Path to data directory
```

### Environment variables

| Variable | Description |
|----------|-------------|
| `GURK_CONFIG` | Path to config file (same as `--config`) |
| `GURK_DATA_DIR` | Path to data directory (same as `--data-dir`) |
| `XDG_CONFIG_HOME` | Base directory for config files (works on all platforms) |
| `XDG_DATA_HOME` | Base directory for data files (works on all platforms) |

### Config file locations

If no config path is specified, `gurk` searches these locations in order:

1. `$XDG_CONFIG_HOME/gurk/gurk.toml` (if `XDG_CONFIG_HOME` is set)
2. `$XDG_CONFIG_HOME/gurk.toml` (if `XDG_CONFIG_HOME` is set)
3. Platform config directory (e.g. `~/Library/Application Support` on macOS, `~/.config` on Linux)
4. `$HOME/.gurk.toml`

### Data directory

The data directory (for messages, Signal database, attachments) is determined by:

1. `--data-dir` CLI option or `GURK_DATA_DIR` env var
2. `data_dir` setting in config file
3. `$XDG_DATA_HOME/gurk` (if `XDG_DATA_HOME` is set)
4. Platform data directory (e.g. `~/Library/Application Support/gurk` on macOS)

## Custom keybindings
The default keybindings can be overwritten at startup by configuring
keybindings in `gurk.toml` using the format `keybindings.<mode>.<keycombination> =
"<command>"`. Valid commands are `anywhere`, `normal`, `message_selected`,
`channel_modal`, `multiline`, and `help`. Valid key combination specifiers are e.g. `left,
alt-j, ctrl-f, backspace, pagedown`. The default keybindings can be disabled by
setting `default_keybindings = false`. An empty command removes an existing
binding if it exists in the given mode. Configuration troubleshooted by running
`RUST_LOG=gurk=trace,presage=trace,libsignal=trace gurk --verbose` and examining the resulting `gurk.log`.

### Supported commands
```
help
quit
toggle_channel_modal
toggle_multiline
react
scroll help up|down entry
move_text previous|next character|word|line
select_channel previous|next
select_channel_modal previous|next
select_message previous|next entry
kill_line
kill_whole_line
kill_backward_line
kill_word
copy_message selected
beginning_of_line
end_of_line
delete_character previous|next
edit_message
delete_message
open_url
open_file
toggle_mute_channel
edit_channel_alias
edit_person_alias
view_image
```

### Fullscreen and external image viewer

Select a message with **PgUp/PgDown**, then press **Alt+P** (`view_image`) to
open its attachment gallery. **Left/Right** selects the previous/next attachment;
**+** (or **=**) zooms in and **-** zooms out. **Alt+Left/Right/Up/Down** pans
the zoomed image within its edges. These keys apply only inside the preview;
conversation Alt+arrow bindings keep their existing meaning. At fit or below,
panning does nothing. The title shows the attachment index, filename and zoom.
Changing attachments resets zoom to fit; changing zoom recenters the image. This works even when inline previews
are disabled. **Esc**, **q**, or the configured viewer shortcut returns to the
same conversation and selected message, preserving the message draft.

The built-in viewer reads the original image and initially fits it to the terminal
without changing its proportions. Zooming above fit enlarges the center of the
image and crops the parts outside the viewport. Zooming out shrinks it. Zoom is
bounded and processed from native pixels without creating oversized bitmaps. It automatically detects Kitty, Sixel or
iTerm2 graphics support, with Unicode half blocks as a fallback. Image decoding,
scaling and encoding run in the background. Resizing the terminal rebuilds the
image for the new area. Missing files and recognized but invalid/oversized images
show an error that can be closed with Esc. Nonimage or unrecognized file contents
open in the program configured by `$EDITOR`, regardless of their filename/MIME.
For example, `export EDITOR='nvim'` or `export EDITOR='code --wait'`. The value can
contain quoted arguments and is parsed without a shell. EDITOR must wait until
its preview is closed, so GUI editors need their wait/foreground option.

Gurk pauses its terminal input reader and releases raw/alternate-screen mode
while EDITOR runs, then restores the conversation or file picker. The editor sees
a private, read-only copy with the original filename/extension (up to 100 MiB).
The original attachment is unchanged even if the copy is edited. Closing EDITOR
returns to a multi-attachment gallery, or to the conversation/picker for a single file. The preview
copy and editor sidecars are removed after exit, including failures; image zoom
and gallery navigation/panning create no image files on disk. An unset/malformed EDITOR
or failed launch is reported in the viewer. Drafts and message selection survive.
 Existing Ctrl+U and Ctrl+P behavior is unchanged.

No configuration is required. To explicitly select the built-in viewer:

```toml
[image_viewer]
mode = "internal"
protocol = "auto"
```

`protocol` can also be `"halfblocks"`, `"kitty"` or `"sixel"`. Use `auto` normally;
`halfblocks` avoids graphics-protocol queries on terminals without image support.
A multiplexer must support or pass through the selected graphics protocol.

To use a separately installed graphical viewer such as sxiv:

```toml
[image_viewer]
mode = "external"
command = ["sxiv", "-a"]
```

`command` is an array containing the program and arguments. Gurk appends the
absolute image path as one argument, without running a shell. External viewers
must be graphical programs, not interactive terminal commands such as `icat`.
Their standard input/output is disconnected from gurk's terminal. Gurk continues
processing events while the viewer is open. A launch failure or unsuccessful exit
falls back to the built-in viewer. Esc in gurk returns to the chat without killing
the external viewer window.

To change the shortcut:

```toml
[keybindings.normal]
alt-p = ""
alt-v = "view_image"
```

Old configuration files without an `[image_viewer]` table use the built-in,
automatically detected viewer. Fullscreen viewing uses the same file/decode size
limits as inline previews, but does not use the reduced thumbnail cache.

### Image attachment previews

Inline image previews are disabled by default. To enable them, add this at the
**top level** of `gurk.toml`, before any `[section]`, and restart gurk:

```toml
show_image_previews = true
```

Previews use colored Unicode half blocks in the message pane, without requiring
Kitty or Sixel graphics. A terminal with true-color support gives the best result.
Images keep their aspect ratio and shrink to fit the current conversation pane,
including after resizing the terminal. Attachment links remain available for
opening the original file.

PNG, JPEG, GIF (first frame), WebP, BMP and TIFF are recognized from file contents,
regardless of filename extension or reported MIME type. Decoding runs in the
background with a bounded thumbnail cache. Missing, invalid or oversized files
remain normal attachment links. Preview limits are 32 MiB per file and 16 million
pixels. Up to 32 visible attachment files are considered for previews at once;
additional files remain available as links. Transparent pixels are composited onto black.

F1 help includes alias editing instructions near the top. Its keyboard and mouse
scrolling stops at the end of the content and adjusts when the terminal is resized.

### Local display aliases

Press **F2** to edit the current conversation's local alias. Press **F3** to
edit the selected message author's alias, or the contact's alias in a direct
conversation when no message is selected. In a group, select a message first
to choose the person.

**Enter** saves, **Esc** cancels, and saving an empty name removes the alias.
Aliases support Unicode and spaces. Editing an alias preserves your message
draft. If saving fails, the editor stays open with the entered name and an
error so you can retry.

Person aliases apply to authors, quotes, mentions, typing indicators and
notifications. A conversation alias takes precedence over the person's alias
in conversation names, the channel picker and notifications. Person aliases
are displayed in full even with `first_name_only = true`.

Aliases are stored locally in gurk's database, survive restarts and are never
sent to Signal. Signal name updates continue in the background; removing an
alias restores the original name. Existing configuration files need no changes,
and existing databases are migrated automatically.

To change the shortcuts, configure `edit_channel_alias` and `edit_person_alias`:

```toml
[keybindings.normal]
F2 = "edit_channel_alias"
F3 = "edit_person_alias"
```

### Example configuration
```toml
default_keybindings = true

[keybindings.anywhere]
ctrl-c = ""
ctrl-q = "quit"

[keybindings.normal]
ctrl-j = ""
ctrl-k = "kill_line"
ctrl-n = "select_channel next"
ctrl-p = "select_channel previous"
alt-c = "toggle_channel_modal"
up = "select_message previous entry"
down = "select_message next entry"

[keybindings.channel_modal]
ctrl-j = ""
ctrl-k = ""
ctrl-n = "select_channel_modal next"
ctrl-p = "select_channel_modal previous"

[keybindings.message_selected]
alt-y = ""
alt-w = "copy_message selected"
ctrl-t = "react :thumbsup:"
ctrl-h = "react ❤️"
```

## License

 * GNU Affero General Public License v3 only ([AGPL-3.0-only](LICENSE-AGPL-3.0) or
   https://www.gnu.org/licenses/agpl-3.0.en.html)

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this document by you, as defined in the AGPL-3.0-only license,
shall be licensed as above, without any additional terms or conditions.

[Signal Messenger]: https://signal.org
[`presage`]: https://github.com/whisperfish/presage
[`src/config.rs`]: https://github.com/boxdot/gurk-rs/blob/main/src/config.rs
[chat-badge]: https://img.shields.io/badge/chat-on%20signal-brightgreen?logo=signal
[ci-badge]: https://github.com/boxdot/gurk-rs/actions/workflows/ci.yaml/badge.svg
[ci-link]: https://github.com/boxdot/gurk-rs/actions/workflows/ci.yaml
[chat-link]: https://signal.group/#CjQKILaqQTWUZks14mPRSn0m0zyU9A-buNMG6haQBmWrxJHeEhCc7HLIwCFZRNDw63MWj-fA
[config-location]: https://docs.rs/dirs/3.0.2/dirs/fn.config_dir.html
[Releases]: https://github.com/boxdot/gurk-rs/releases
[`protoc`]: https://github.com/protocolbuffers/protobuf?tab=readme-ov-file#protobuf-compiler-installation
[ko-fi-badge]: https://img.shields.io/badge/Ko--fi-F16061?logo=ko-fi&logoColor=white
[ko-fi-link]: https://ko-fi.com/boxdot


### Drafts, attachments and screenshots

Unsent text, cursor position, pending attachment paths and the active conversation
are saved per contact/group automatically (about once a second and on clean exit).
They are restored on restart. Existing configurations need no changes. Pending
attachments reference the original files; keep those files until you send them.

- **Alt+A** opens a file picker in your home directory. It shows names, modification
  dates and human-readable sizes. Use arrows/PgUp/PgDown to select, Enter to enter a
  directory or stage a file, Left/Backspace for the parent and Right to enter the
  highlighted directory. `~` opens home, `p` opens `~/Pictures`, `s` opens the
  configured screenshot directory (or its default), and `/` lets you type or paste
  a path. `.` toggles hidden files/directories (initially hidden); `t` switches
  between name and modification time (newest first), keeping directories first.
  Escape closes the picker.
- After staging, **Enter sends** the attachment with the current text. **Escape
  removes pending attachments**, keeping the text. Images have a pending thumbnail
  even when history previews are disabled; other files show a type icon. Files are
  validated before sending, with a 100 MiB total attachment limit per message.
- **Alt+S** captures a screenshot and stages it in the conversation where capture
  began. It also requires Enter to send. Screenshots remain saved after cancellation.

Nerd Fonts icons are used by default. Use a Nerd Font or Symbols Nerd Font Mono
fallback in your terminal, or disable icons for portable text labels. These optional
settings belong at the top level of `gurk.toml`, before any table headers:

```toml
nerd_font_icons = false
screenshot_dir = "~/Pictures/gurk"
# Optional explicit capture program; output path is appended as one argument.
# screenshot_command = ["maim", "-s"]
```

Without `screenshot_dir`, gurk uses `~/Pictures` if it exists, otherwise your home
directory. Directories are created when needed. On Linux capture first uses the
interactive desktop screenshot portal. If unavailable, it tries `grim` on Wayland
or `maim -s` / `scrot -s` on X11. A configured command bypasses this selection;
arguments are passed directly without a shell. PNG output is validated and saved
under a unique name. Cancellation never sends a message.

The commands `attach_files` and `capture_screenshot` can be rebound through the
same keybinding configuration as the other commands. Ctrl+T retains its reaction
binding.

### Message ordering

Gurk currently sorts messages by the sender timestamp stored as `arrived_at`.
It does not group messages by author. Signal uses server receive order
([Signal contributor explanation](https://github.com/signalapp/Signal-Desktop/issues/8069)).
Delayed delivery or clock differences can therefore produce different ordering.
The displayed sent times alone cannot establish which device clock was wrong.
Changing the ordering would require a separate stored sorting timestamp, preserving
existing message identifiers used by replies, receipts and edits.

Attachment uploads include the encrypted upload length required by Signal. If
any attachment fails to upload, sending the entire message stops and the failure
is displayed in the conversation, including the underlying error. A local preview
shows a locally saved file and does not itself confirm delivery to the recipient.


### Multiple files and picker previews

In the attachment picker, **Space** marks/unmarks the highlighted file. Mark up
to **10** files across directories; the title shows the count and `[x]` marks the
chosen entries. Hidden-file toggles and sorting preserve marks. Enter on a file
adds all marked files (or that single file when nothing is marked); Enter on a
directory still enters it. The message can contain at most 10 pending files in
total. Enter in the conversation confirms sending; Escape removes pending files.

**Alt+P** previews only the highlighted image, regardless of marked files. It uses
the built-in fullscreen renderer even if message previews use an external viewer.
Escape/q returns to the same picker with its selection intact. Decoded/rendered
image data stays in RAM and is released on close; no preview files are written.
The original file remains untouched.

**Alt+O** opens only the highlighted file in the desktop application's MIME
association, independently of image viewer settings. On Linux this requires
`xdg-utils` and a graphical desktop session. Gurk calls `xdg-open` with the original
file; it does not copy the file or create preview artifacts. External programs
manage their own windows and caches. Choose a graphical application so it does
not compete with gurk for the terminal.

To inspect and change associations (run as your regular desktop user):

```sh
xdg-mime query filetype "$HOME/Pictures/example.png"
xdg-mime query default image/png
# Substitute desktop IDs that are installed on your system:
xdg-mime default sxiv.desktop image/png image/jpeg
xdg-mime default org.pwmt.zathura.desktop application/pdf
```

Application IDs are `.desktop` filenames in `/usr/share/applications` or
`~/.local/share/applications`. If your graphical viewer has no desktop entry,
create `~/.local/share/applications/gurk-image-viewer.desktop`:

```ini
[Desktop Entry]
Type=Application
Name=Image viewer
Exec=sxiv %f
Terminal=false
MimeType=image/png;image/jpeg;image/webp;
```

Then associate it with `xdg-mime default gurk-image-viewer.desktop image/png
image/jpeg image/webp`. These are desktop-wide defaults, not gurk-only settings.
See `man xdg-mime` and `man xdg-open` for your desktop's association behavior.


### Hidden conversations

**F6** hides the current person/group conversation locally. It disappears from the
channel pane and Ctrl+P picker; messages continue to be stored and do not bring it
back. Desktop notifications and bells are suppressed for hidden conversations.
Hiding a person also suppresses notifications about that person's messages and
reactions in other groups. This is a local visibility setting, not Signal blocking.

**F7** opens the hidden conversation list, using current aliases/names and type.
Up/Down selects, Enter restores the selected conversation, and Escape closes the
list. History and per-conversation drafts survive hiding and restarts. Restoring
removes the hidden mute; an independently configured channel mute remains in force.
Commands `hide_channel` and `show_hidden_channels` can be rebound normally.

### Contextual shortcut bar

**F4** toggles a one-row shortcut bar at the bottom, including in dialogs and
image previews. It shows main actions for the active context. Configurable actions
use their actual keybindings. This runtime toggle lasts until exit; configure its
initial state and appearance in `gurk.toml`:

```toml
[shortcut_bar]
enabled = true
# contrast: light grey background, black text (default)
# dark: dark grey background, light text
theme = "contrast"
# Optional explicit overrides, exactly #RRGGBB:
# background = "#D3D3D3"
# foreground = "#000000"
```

Old configs default to the bar being disabled. Overrides replace only the specified
preset colors. Invalid colors report a configuration error. Command
`toggle_shortcut_bar` can be rebound; F1 documents the added actions.

**PgDown on the newest selected message clears its selection** and resumes following
new messages. The next send has no reply quote. Escape can also clear selection.

### Configurable message ordering

At the top level of `gurk.toml`, before table headers:

```toml
message_order = "received"
```

`received` uses a stable local sequence assigned when an incoming message is first
stored or an outgoing message is created. Sender clock differences cannot reshuffle
new dialogue. The visible clock still shows the original sender time; replies,
edits, reactions and receipts keep the original Signal identifiers. Updating an
existing message does not move it. Pagination and message selection follow the
chosen display order, including across restarts.

`sent` uses the previous sender-timestamp order and remains the default for old
configurations. The migration seeds existing messages in their previous timestamp
order: it cannot reconstruct historical reception order that was never recorded.
The new sequence is recorded even in sent mode, so switching modes later preserves
reception order for messages stored after this migration. This is this device's
local order; late synchronization/import may differ from another Signal client's
history. Changing the setting takes effect after restart.

### Forwarding without source attribution

Select a message with PgUp/PgDown and press **Alt+R** (`forward_message`). Choose
another visible person/group with Up/Down (Home/End also work), then **Enter sends**.
The source conversation and hidden conversations are excluded. Escape cancels;
source text drafts and message selection stay intact.

Gurk creates a fresh message with the original text and attachments. It does not
include the source quote, author/group identifiers, reactions, mention metadata,
or a source attribution header. Attachments are read and uploaded again, rather
than referencing the original upload. Text and file contents themselves are not
anonymized or rewritten. Missing/failed/pending attachments prevent a partial send;
download them successfully before forwarding. Standard attachment size limits apply.

### Private read synchronization between linked devices

Visible incoming messages are marked read through Signal's encrypted own-account
`SyncMessage.read` mechanism. The destination is exclusively the account's own ACI;
contacts are never destinations of these sync messages. Gurk does not emit contact
read/delivery receipts or typing indicators. The private sync carries only the
message author and original message timestamp required to identify the read message.
[Signal Desktop's read-sync handler](https://github.com/signalapp/Signal-Desktop/blob/main/ts/messageModifiers/ReadSyncs.preload.ts)
uses this state to update its own unread status; its read-sync conversation update
[disables contact read receipts](https://github.com/signalapp/Signal-Desktop/blob/main/ts/models/conversations.preload.ts).

Only messages actually included in the displayed viewport are enqueued, not every
message loaded into memory. Help, modal dialogs and image previews do not mark the
underlying conversation as read. Focus reporting is enabled: terminals reporting
FocusLost suspend new read marking until FocusGained. Terminals that do not report
focus are assumed active; gurk cannot detect whether a person is physically looking.
Other linked clients may apply their own conversation-level read rules.

The pending sync queue is stored inside the encrypted gurk database before local
read marking, retries when sending fails, and survives restart. Queue rows are
removed only after successful own-account transport acknowledgement, with durable
confirmation recorded in the same transaction. Previously locally-read messages
are synchronized once when displayed again; confirmed messages are not resent on
every redraw. Synchronization
requires connectivity and the other client to receive its pending Signal messages;
it is not a synchronous promise that another device's UI has already refreshed.
Existing settings/activity in other Signal clients are controlled by those clients.


### Attachment labels and keyboard selection

Attachments show only their filename by default. The stored path and opening,
preview and forwarding operations remain unchanged. To show complete file URLs,
add this at the top level of `gurk.toml`, before any table headers, and restart:

```toml
show_attachment_paths = true
```

**Alt+V** (`select_text`) opens a frozen text snapshot of the currently loaded
conversation fragment, starting at the selected message or current bottom anchor.
It contains message bodies and attachment labels, without names, timestamps,
borders or image previews. New incoming messages continue to be stored but do not
move the selection. To include older history, leave selection, navigate there with
PgUp in the conversation, and re-enter Alt+V.

- Arrows move the cursor; Home/End go to logical line boundaries.
- PgUp/PgDown move through the wrapped text view.
- Space starts selection; Space again clears it. Moving selects in either direction.
- Enter copies the exact selected text to the **system clipboard** and returns.
- Escape cancels; Alt+V also closes the mode. F4 can toggle its context shortcut bar.

Selection respects grapheme boundaries (including Polish letters, combining marks
and multi-codepoint emoji), and display wrapping does not add newlines to copied
text. Drafts, pending attachments and reply selection are preserved. If no range is
selected or the system clipboard is unavailable, the view remains open with an
error so the selection is not lost. System clipboard access needs a working desktop
clipboard (the same backend as existing message copying); no OSC52 fallback is
introduced in this build.

The binding is configurable through `[keybindings.normal]`, for example
`F9 = "select_text"`. No Ctrl+A or Ctrl+B prefix is used, so default Screen/tmux
prefixes remain independent. Custom multiplexer/terminal bindings may still
intercept Alt+V; change the gurk binding if necessary. Link handling, Shift+mouse
selection and Screen image rendering are unchanged.

### Restore Signal Secure Backup (experimental)

The startup importer downloads an **existing cloud Signal Secure Backup**, including
available attachments, into this linked Gurk account. It supports the 64-character
Secure Backup recovery key, including the current forward-secrecy format. It does
not read the old Android local `.backup` format or its 30-digit passphrase.
Keep the phone's backup enabled and link Gurk to the same Signal account first.
Close other Gurk instances using this database before importing.

```sh
gurk --restore-cloud-backup --backup-dry-run
gurk --restore-cloud-backup
```

Each command asks for the Gurk database passphrase normally and prompts separately
for the recovery key with input hidden. Do not put the recovery key in the command
line. As an alternative, `--backup-key-file /private/path/key` reads a regular file
with permissions `600`; remove that file when no longer needed. Spaces, hyphens,
capitalization and the phone's displayed `#`/`=` substitutions are normalized.

`--backup-dry-run` downloads, authenticates, validates and inventories the archive,
without changing local chat history. It still contacts Signal and may perform an
SVR-B recovery. A failed recovery is not automatically retried: the server limits
attempts. After a successful recovery, its token is cached in SQLCipher staging,
encrypted with your Gurk passphrase, so repeating either command for the same
archive reuses it. No backup reservation, upload, deletion, phone registration or
relinking is performed. Import does not send read receipts or activity updates.

The default working directory is `data_dir/backup-import`. Use
`--backup-work-dir /private/path` to place it elsewhere; reuse that directory when
resuming. Downloads support HTTP range/ETag checks. Media is authenticated and
hash-checked before being published under `data_dir/files/backup`. Interrupted
imports can be rerun: existing message IDs are deduplicated. The process streams
files and pages staged messages rather than loading the whole archive into RAM.
Allow disk space for the encrypted downloads **and** restored attachments and
staging; a multi-gigabyte backup may require substantially more temporary space
than its archive size.
The working directory remains after import for resume and diagnosis. Once the
result is verified, you can remove that directory; retain `data_dir/files/backup`,
which contains the attachments used by imported messages.

`--backup-messages-only` skips downloading attachments. Text messages can be
imported first; file-only messages are deferred and counts are printed. Rerun the
full command to hydrate missing attachments. Missing media, timestamp collisions
or oversized long-text records produce an incomplete-import report. Validated
records may already have been added; rerun to continue. Import is additive, rather
than a transaction replacing your entire history.

Contacts use their Signal ACI and groups use their group master key, so saved
local aliases continue to apply. Existing aliases, hidden conversations, drafts,
edited text, deletion markers and receipts are preserved. Expired disappearing
messages are omitted. The backup's received/render order is saved independently
of message timestamps. Set `message_order = "received"` in `gurk.toml` to use it;
the existing timestamp ordering remains available. Backup messages form the
history prefix; local messages absent from the backup follow in their previous
received order.

Gurk cannot represent every Signal record. Unsupported records (for example
polls, some service updates, contact records without an ACI, and full revision
history) are counted and retained in encrypted staging rather than silently
claimed as restored. Standard text, image/file attachments, quotes, reactions,
timers and supported receipts are converted; outgoing per-recipient receipts
are aggregated. Raw preservation does not make unsupported content visible in
Gurk. Review the printed inventory before treating the import as complete.
This importer has automated synthetic archive/media tests; restoration of a real
cloud account must still be verified during testing.

After an incomplete import, inspect the existing encrypted staging **offline**:

```sh
gurk --backup-report
# For a non-default import cache:
gurk --backup-report --backup-work-dir /private/path
```

This command asks only for the Gurk database passphrase. It reads the staging
database in SQLite read-only mode, without loading Signal registration credentials,
asking for the recovery key, contacting Signal, resetting staging or importing
messages. The output contains fixed error categories and counts, never raw error
strings, message text, contact identifiers, filenames, URLs or key material.
Repeated archive timestamps are classified as identical records, different
senders or different payloads. Messages from different senders at the same
time now receive separate local keys; their Signal timestamps remain unchanged.
These counts describe the source archive, not unresolved import conflicts. Keep the working directory until missing
media and collisions have been investigated. Opening Gurk after a partial import
displays the messages already committed; the final incomplete status does not
roll those additions back. The report itself does not change the local history.


Messages with the same Signal timestamp and different senders are preserved
separately. Existing databases are upgraded transactionally; local aliases and
existing messages remain in place. Local keys are used for selection and paging,
while Signal operations and displayed times use the original timestamps. Edits
retain their original snapshots without borrowing another message's timestamp.
The schema upgrade also backfills existing history. Older application builds
cannot open a database with this newer migration; keep a closed-database copy if
you need to return to an older build.

A `transit_http_404` report means the referenced file is unavailable at the given
temporary CDN location. Such references contain an encrypted digest rather than
a plaintext hash, so this importer cannot derive the archive media ID from that
reference. The Android restore client also chooses transit download when the
plaintext hash is absent. HTTP 404 does not establish whether another copy still
exists on the phone or with the sender. Repeating the same download does not
repair a missing object. The original reference remains in encrypted staging;
file-only messages with no restored attachment remain deferred. Other imported
messages and restored media are kept.

### Mixed attachment selections

Signal clients do not accept arbitrary mixtures in one message. A visual album
may be truncated at the first ordinary file; non-visual selections may keep only
one file. Gurk therefore preserves contiguous JPEG/PNG/WebP albums and sends
other files (including HEIC, documents, audio and GIF) as separate messages.
A JPEG/JPEG/JPEG/HEIC/JPEG/PNG selection becomes messages containing 3, 1 and 2
files, in that order. Caption text and an optional reply quote appear only in the
first message; forwarding never adds a source quote. Each message has a unique
sent timestamp and messages to the same conversation are submitted in order.
This is several independent Signal messages; a network failure is reported for
the affected message and does not make the whole selection atomic.

### Attachment delivery diagnostics

For a delivery investigation, start `RUST_LOG=gurk=info,libsignal_service=warn,presage=warn gurk --verbose`.
The local `gurk.log` records `attachment_batch_upload_start`, one
`attachment_upload_result` per returned file, `attachment_batch_upload_complete`,
`attachment_message_submit_start` and `attachment_message_submit_complete`.
The records share `message_timestamp`; counts and one-based indices contain no
message text, attachment bytes, encryption keys or CDN URLs. Existing local-save
records contain filenames. Successful submission is not a recipient delivery
receipt. A missing completion record can indicate an operation still pending;
keep Gurk running until it completes or reports an error. Returning fewer upload
results than the requested file count aborts message submission.

### Dismissing composer notices

`Esc` first dismisses an error/notice below Messages, keeping the draft, selected
message and pending attachments. Press `Esc` again to perform the usual action
such as removing staged files, canceling an edit or clearing reply selection.
An error without staged files is titled `Notice`, rather than `Attachments`.
The text `Only your own non-deleted messages can be edited` belongs to message
editing (`e`/`Ctrl+E`); attachment preview remains `Alt+P` and does not edit messages.

### Contact mentions

Type **@Barto** in the message composer to show matching local contacts, such as
**Bartosz Kowalski**. Matching ignores case and accepts name-word prefixes or
local aliases. Current group members appear first; other known contacts are also
available. Hidden people are excluded. No network lookup or typing notification
is sent while searching contacts. Email addresses do not trigger completion.

Use **Up/Down** to choose (also **Shift+Tab** for the previous choice), then
**Tab** or **Enter** to insert the full name. Accepting a suggestion does **not**
send; press Enter again after completing your message to send it. **Esc** closes
the suggestions without changing the draft. Names with identical labels include
a Signal account ID so you can select an identity explicitly. Confirmed mentions
are cyan/bold in the composer; typing an unconfirmed `@name` remains plain text.

A confirmed mention carries the person's Signal account identifier, using the
same [BodyRange protocol](https://github.com/signalapp/Signal-Desktop/blob/main/protos/SignalService.proto)
as official clients. The receiver resolves their own contact name; your local
alias is not transmitted. Mention notifications depend on the receiving client,
conversation membership and notification settings. This does not publish read
receipts or typing activity.

Mentions survive conversation switches, saved drafts, restarts, attachment
captions and message editing. Existing drafts load without losing their text.
Changing characters inside a confirmed name removes its identity and leaves plain
text; select the contact again if you want a mention. Changes before/after the
name adjust its position. Ambiguous compound changes in an external editor may
conservatively turn affected mentions into plain text. Plain text pasted into
Gurk is never automatically bound to a contact.

### Automatic ASCII emoticons

To replace common ASCII emoticons with Unicode emoji, add this **top-level**
setting to `gurk.toml`, before any `[section]` (for example before `[user]`):

```toml
auto_ascii_emoticons = true
```

It defaults to `false`, so existing configurations keep their behavior. Restart
Gurk after changing it. Conversion happens after typing whitespace/newline in
the message composer and just before sending, including pasted text, external
editor text and attachment captions. It does not modify received messages or
forwarded text. Existing `:shortcode:` emoji completion remains independent.

Only standalone tokens separated by whitespace or message boundaries are
converted. For example `hej :)` becomes `hej 🙂`; `https://example.test/:/`,
`C:/path`, `abc:)`, `:)word`, backtick-wrapped tokens and confirmed contact mention
labels stay literal. Editing a message applies the option to its plain text when
you submit the edit. Setting the option to `false` disables future ASCII
conversions; emoji already inserted into drafts remain emoji.

| ASCII forms | Emoji |
| --- | --- |
| `:)` `:-)` | 🙂 |
| `;)` `;-)` | 😉 |
| `:D` `:-D` | 😃 |
| `:P` `:p` `:-P` `:-p` | 😛 |
| `:(` `:-(` | 🙁 |
| `:/` `:-/` | 😕 |
| `:\|` `:-\|` | 😐 |
| `:O` `:o` `:-O` `:-o` | 😮 |
| `:'(` `:'-(` | 😢 |
| `<3` | ❤️ |
