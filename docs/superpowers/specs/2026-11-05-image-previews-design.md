# Image previews and bounded help

Provide inline image attachment previews fitted to the conversation pane, disabled by default with backward-compatible configuration; bounded F1 scrolling; discoverable alias shortcuts; an installable Arch package preserving existing aliases.

Use true-color Unicode half blocks within normal ratatui message rows. Compared with Kitty/Sixel this works without terminal graphics protocol negotiation and follows existing clipping/scrolling automatically, at the cost of a lower-resolution preview. Decode based on file contents, not attachment MIME or extension. Support PNG, JPEG, GIF (first frame), WebP, BMP and TIFF. Missing, invalid or oversized files keep the existing attachment link.

`show_image_previews = false` is the default, including old TOML files. With previews enabled, decode and thumbnail in background workers using bounded allocations, concurrency and cache size. Render thumbnails preserving aspect ratio (two image pixels per terminal cell), bounded by current message pane width and available height, including resize and multiple attachments. Keep message text and attachment links, and do not preview deleted messages or quoted attachments.

F1 scrolling clamps to content height minus the current inner viewport, including after resize. Both keyboard and mouse scrolling use the help scroll path. Put the alias editing instructions near the start of help, showing actual configured bindings rather than promising disabled shortcuts.

Deliver a new version of the existing local Arch package, retaining its package name so pacman upgrades it; preserve the previous artifact for rollback. Validate source tests, lint/format, package integrity and isolated installation/start with Arch runtime libraries.
