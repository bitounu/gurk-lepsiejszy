# Reconstructed development history

The extension series starts at upstream commit `49f4295` of
[boxdot/gurk-rs](https://github.com/boxdot/gurk-rs). The original upstream ancestry
and credits are preserved for a later fork and cumulative pull request.

The work initially existed as one working tree rather than a sequence of saved
release snapshots. Publication commits therefore group the existing implementation
by component and dependency. At the author's request, author and committer dates
reflect recorded conversation milestones in UTC rather than the later Git-object
creation time. For grouped work, the last relevant feature-completion milestone
is used; publication and integration commits retain their actual publication
dates. Dates are recorded to Git's one-second precision. The commits remain a
reconstruction, not evidence of source snapshots saved at those historical times.
Component commits are ordered by their recorded milestones. Shared application
activation and publication-only work follow at the actual publication date.
Early component commits introduce source modules;
the shared application/storage integration commit activates their connections.
Individual component commits must not be treated as independent historical
releases or cherry-picked without their integration dependencies. The complete
publication tip is the validated application.

## Development milestones

| Area | Result |
| --- | --- |
| Local aliases | Edit person and conversation display names without changing Signal names; retain compatibility with existing configuration and history. |
| Inline images and help | Opt-in bounded thumbnails, background decoding and cache; bounded F1 scrolling and documented alias shortcuts. |
| Attachment gallery | Internal and external viewers, selected attachment navigation, zoom and Alt+arrow panning; MIME opening and EDITOR fallback, temporary preview cleanup. |
| Persistent composers | Restore drafts, cursor positions, attachment selections and editing context in the correct conversation after restart. |
| Attachments and screenshots | Stage before send, explicit cancellation, home-based file picker with dates/human-readable sizes, dotfiles, sorting, directory shortcuts, up to ten selected files and per-file preview; configurable screenshot capture/directory. |
| Conversation visibility | Hide and silence conversations locally, restore them from a dialog, contextual F4 shortcut bar and configurable colors/themes. |
| Selection and ordering | Clear reply selection after the newest message; selectable sender-time or stable local arrival ordering; jump to oldest/newest, literal conversation search and keyboard copy selection. |
| Forwarding and linked devices | Forward text/attachments without source attribution; synchronize read state privately to linked devices without contact-facing activity receipts. |
| Message identity | Preserve different senders/messages sharing a timestamp using local storage identity; keep quotes, reactions and edit routing tied to protocol identity. |
| Secure Backup import | Authenticate an existing cloud archive, stream and validate staged data, dry-run/resume, retain encrypted reports and original local settings, restore media with explicit unavailable/error handling. |
| Sent-message editing | Edit selected own messages, enforce client-side eligibility/count/time rules, preserve revision history while displaying the current text in place. |
| Local spelling | Offline dictionary checks and correction dialog with language configuration and no remote text submission. |
| Image privacy | Strip descriptive JPEG/PNG metadata without recompressing image pixels; preserve rendering/orientation data, reject unsafe malformed data, leave unsupported formats explicit. |
| Attachment delivery | Correct encrypted upload length, validate upload counts, split incompatible visual/generic batches, preserve channel send order and unique timestamps, expose failures. |
| Native mentions | Complete contact names/aliases to ACI-bound Signal mentions; preserve identities through draft edits/captions using UTF-16 wire offsets, isolate labels from attachment syntax. |
| ASCII emoticons | Default-disabled configurable replacement of standalone ASCII faces; preserve URLs and confirmed names, fix Unicode cursor width and grapheme-aware vertical movement. |
| Arch packaging | Reproducible source-based local package recipe and installation/configuration documentation; generated packages are not versioned. |

The dated design and implementation notes under `docs/superpowers/` explain the
individual decisions, fixes and verification. They contain generalized examples;
private input artifacts and operational reports are not part of the repository.

## Publication and upstream workflow

1. Create the thematic commit series on the publication repository's `main` branch.
2. Publish that branch to the intended Gitea repository without overwriting an
   existing unrelated history.
3. Later create a GitHub fork of the original project and transfer the same branch
   (or rebase it onto the chosen upstream base).
4. Open one cumulative pull request. Keep upstream credits and discuss the broad
   scope and protocol/privacy behavior in the PR description.

Repository addresses and public author metadata are supplied separately. Do not
embed credentials, private server addresses, local logs, screenshots, database
files, backup staging or generated binary packages in source commits.
