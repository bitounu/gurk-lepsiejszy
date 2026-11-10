# README screenshot plan

Capture new screenshots of this fork using a separate demo account and synthetic
conversations. This document proposes assets; the image files do not exist yet.
Use invented contacts such as **Jan Kowalski** and **Zbigniew Malinowski**, generic
messages, and locally generated example files. Never reuse user-provided images,
logs, conversations, contact identifiers, filenames, paths, QR codes or metadata.
Do not expose phone numbers, account IDs, recovery keys, credentials or desktop
notifications. Construct safe demo data before capture rather than redacting a
real conversation afterward.

Keep a consistent terminal font, dimensions, theme and scale. Show enough context
to locate each control, crop unrelated desktop chrome, and retain readable text at
README width. Use portable text attachment labels (`nerd_font_icons = false`) for
the primary examples; a Nerd Font variant is optional. Store reviewed PNG assets
under `docs/images/` with the lowercase names below. Give every image descriptive
alt text and a caption naming the action it illustrates. Screenshots illustrate
appearance and controls, not delivery, privacy or backup guarantees.

The existing root `screenshot.png` is an upstream illustration. If retained,
attribute it explicitly to upstream; it must not be presented as a screenshot of
this fork or evidence of its added features.

## Prioritized captures

P1 is the first useful set. P2 explains additional interaction states. P3 is
optional and should be omitted if it makes the README too long. Placements refer
to headings in `README.md`; update these links if the final headings change.

| Priority | README placement | Capture contents | What it explains | Proposed asset |
| --- | --- | --- | --- | --- |
| P1 | [Features](../README.md#features), after the section heading and before Local aliases | Conversation list with Jan Kowalski and a fictional group; a short exchange with Zbigniew Malinowski; a local alias, a harmless image and a draft; F4 shortcut bar visible. | A current overview of the fork and the location of context-sensitive keyboard hints. Keep all text legible instead of trying to display every feature. | `docs/images/fork-overview.png` |
| P1 | [Attachments and screenshots](../README.md#attachments-and-screenshots), after picker instructions | Alt+A picker with a mixture of harmless PNG/JPEG/PDF/TXT examples, selected `[x]` marks and the selection counter at 10; names, sizes and dates readable. | Multi-select, the ten-file cap, mixed file types and the difference between marking files and sending them. Caption should mention Enter stages, then Enter in the composer sends. | `docs/images/attachment-picker.png` |
| P1 | [Attachment gallery](../README.md#attachment-gallery), after viewer controls | Alt+P gallery on a synthetic image, with a clearly readable attachment index such as 2/3, filename, zoom percentage and F4 controls; use a zoomed crop with a recognizable shape. | Gallery navigation, zoom and panning; the index makes it clear the viewer covers multiple attachments. | `docs/images/attachment-gallery.png` |
| P1 | [Search and text selection](../README.md#search-and-text-selection), after search instructions | Search for a harmless repeated word such as `meeting`; show query, highlighted occurrence, match counter and navigation controls in a synthetic conversation. | Search works within the active conversation and navigates individual occurrences. The screenshot does not establish full-history coverage. | `docs/images/message-search.png` |
| P1 | [Offline spelling](../README.md#offline-spelling), after F9 instructions | A synthetic draft with a visibly underlined misspelling and the F9 suggestion dialog open; show Polish and English example words without real message content. | A correction is chosen explicitly; Enter in this dialog replaces a word. A caption can state that checking runs locally. | `docs/images/spelling-suggestions.png` |
| P1 | [Contact mentions](../README.md#contact-mentions), after completion instructions | Type `@Zbi` and show Zbigniew Malinowski highlighted in the completion popup, with harmless surrounding draft text and context shortcuts. | Selecting a contact inserts a confirmed mention without sending the draft. Use unique fictional labels so no account ID appears. | `docs/images/contact-mentions.png` |
| P2 | [Hidden conversations](../README.md#hidden-conversations), after F7 instructions | F7 hidden-conversation list with two fictional entries, one selected and the restore control visible. | Where hidden conversations are found and how to restore one. Do not suggest that hiding blocks the contact on Signal. | `docs/images/hidden-conversations.png` |
| P2 | [Local aliases](../README.md#local-aliases), after F2/F3 instructions | F2 conversation alias editor with a harmless proposed name; the underlying conversation remains visible. | Alias editing is a dedicated local dialog. Caption distinguishes F2 conversation aliases from F3 person aliases. | `docs/images/local-alias-editor.png` |
| P2 | [Editing sent messages](../README.md#editing-sent-messages), after edit instructions | An own message selected for editing, the edit state visible in the composer, plus an earlier synthetic message showing the edited indicator if both fit. | How an edit differs from a new draft and where the user sees edit state. Do not imply screenshots prove the edit time/count limits. | `docs/images/edit-message.png` |
| P2 | [Search and text selection](../README.md#search-and-text-selection), after Alt+V instructions | Frozen text-selection view with a short range crossing two wrapped lines, Polish letters and an emoji; show copy/cancel controls. | Keyboard selection and exact text copying without author names, borders or timestamps. | `docs/images/text-selection.png` |
| P2 | [Drafts](../README.md#drafts), after persistence explanation | Two small captures of switching between Jan and Zbigniew, each with a different synthetic draft; one draft includes a pending attachment. | Per-conversation draft isolation. Describe restart restoration in text because a static screenshot cannot prove persistence. | `docs/images/draft-jan.png`, `docs/images/draft-zbigniew.png` |
| P2 | [Image previews](../README.md#image-previews), after enabling inline previews | Synthetic image visible as an inline half-block preview with its filename label; preserve nearby text and a normal nonimage attachment label for comparison. | Inline previews are distinct from the Alt+P gallery and leave attachments accessible. | `docs/images/inline-image-preview.png` |
| P2 | [Attachments and screenshots](../README.md#attachments-and-screenshots), after staging instructions | Composer with staged synthetic image and document, pending preview/type labels, harmless caption and send/cancel shortcuts. | Picker selection or screenshot capture stages content before an explicit send. The image must itself be synthetic. | `docs/images/staged-attachments.png` |
| P3 | [Message order](../README.md#message-order), after the ordering explanation | Paired captures of the same three synthetic messages under `sent` and `received`, using deliberately out-of-order sender times and identical message bodies in both. | Display order can differ from the visible sent time. Label each mode in its caption; capture actual results instead of rearranging image rows. | `docs/images/message-order-sent.png`, `docs/images/message-order-received.png` |
| P3 | [Keyboard shortcuts](../README.md#keyboard-shortcuts), near F4 | Paired small captures of the same harmless context with shortcut bar contrast and dark themes. | Optional visual theme choice. Prefer one overview image if the pair adds little value. | `docs/images/shortcut-bar-contrast.png`, `docs/images/shortcut-bar-dark.png` |
| P3 | [Forwarding](../README.md#forwarding), after target selection instructions | Forward dialog listing fictional targets and a harmless preview, with its send/cancel controls visible. | Forwarding has an explicit target-selection step. Caption must clearly state that Enter sends. | `docs/images/forward-target.png` |

## Prefer text for nonvisual behavior

Use concise TOML snippets for configuration and ASCII emoticons, a small command
sequence for installation and Secure Backup restore, and a synthetic inventory
example for backup reporting. Do not use real report output or logs.

Image metadata cleaning and private read synchronization should be explained in
text, optionally with a simple flow diagram. A chat screenshot cannot demonstrate
which bytes were removed, encryption, transport destinations or the absence of
receipts. Forwarding screenshots likewise cannot prove attribution removal or
mention protocol behavior. Keep those claims tied to their documented behavior
and tests.

## Before adding an asset

- Confirm the capture comes from the current fork and the named behavior actually
  appears in the frame.
- Inspect only the newly created synthetic capture for accidental identifying
  content, terminal paths, titles and background windows.
- Keep captions accurate about defaults and required settings; inline image
  previews and ASCII emoticon conversion need explicit enabling.
- Add useful alt text, verify the relative path and ensure the image remains
  readable at the width used by GitHub.
- Link only images that exist; until capture is complete, link to this plan rather
  than inserting broken image placeholders into the README.
