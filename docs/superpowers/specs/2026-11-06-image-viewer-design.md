# Fullscreen and external image viewer

Design: Alt+P (configurable), both internal and external viewing, internal default; automatic Kitty/Sixel graphics with Unicode halfblock fallback. Include an installable Arch package.

Alt+P / `view_image` opens the selected message's first readable image, detected from file contents. Independent of the opt-in inline thumbnail setting. Read original data under existing decode limits, never enlarge the cached 256px thumbnail. Internal viewer occupies the terminal, preserves aspect ratio without cropping, adapts on resize, and closes with Esc (also q/Alt+P). Preserve draft, selected message, channel, and ordinary Ctrl+U/Ctrl+P bindings. Swallow unrelated keyboard, mouse and paste interactions while the viewer is open.

Optional `[image_viewer]` configuration: `mode = "internal" | "external"`, `command = ["sxiv", "-a"]`, `protocol = "auto" | "halfblocks" | "kitty" | "sixel"`. Missing table defaults to internal/auto. Spawn external programs without a shell and with the canonical image path as one argument; detach stdio from the terminal, reap children, keep the application's event loop responsive, and fall back to the internal viewer on launch/nonzero-exit errors. GUI programs own their own window controls.

Use ratatui-image without Chafa or extra default image codecs. Negotiate terminal capabilities before the event reader starts. Decode/resize/encode in bounded background jobs, notify the event loop when ready, ignore work from closed viewers. Clear terminal graphics when closing, resizing, or replacing a protocol. Missing/non-image/oversized images display a recoverable message; never quit the app. Prepare architecture for a future device renderer without implementing a reMarkable port now.

Preserve aliases, previews, old config/data, help bounds. Document controls/configuration and produce a new version of the existing Arch test package; test upgrade and startup against Arch runtime libraries.
