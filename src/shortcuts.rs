//! Here are all shortcuts documented

pub struct ShortCut {
    pub event: &'static str,
    pub description: &'static str,
}

pub static SHORTCUTS: &[ShortCut] = &[
    ShortCut {
        event: "ASCII emoticons (optional)",
        description: "auto_ascii_emoticons in gurk.toml converts standalone :) ;) :/ :| etc. after whitespace or on send; disabled by default.",
    },
    ShortCut {
        event: "@name (composer)",
        description: "Complete a contact mention: Up/Down selects, Tab/Enter accepts without sending, Esc dismisses. Confirmed mentions are cyan/bold; editing their name makes them plain text.",
    },
    ShortCut {
        event: "F9",
        description: "Check composer spelling; Up/Down selects, Enter corrects, i ignores for this session, Esc closes.",
    },
    ShortCut {
        event: "/ (empty composer) / F8",
        description: "Search active conversation text; Up/Down or Shift+F3/F3 moves between matches, Enter accepts, Esc restores position.",
    },
    ShortCut {
        event: "Ctrl+Home / Ctrl+End",
        description: "Jump to oldest message / newest message and resume following.",
    },
    ShortCut {
        event: "Alt+V",
        description: "Keyboard text selection: arrows move, Space selects, Enter copies, Esc cancels.",
    },
    ShortCut {
        event: "Alt+R",
        description: "Forward highlighted message without source attribution; choose visible recipient, Enter sends, Esc cancels.",
    },
    ShortCut {
        event: "F4",
        description: "Toggle bottom contextual shortcut bar.",
    },
    ShortCut {
        event: "F6 / F7",
        description: "Hide/silence current conversation / open hidden list; Enter restores, Esc closes.",
    },
    ShortCut {
        event: "PgDown on newest selected message",
        description: "Unselect message and resume following; next send has no reply quote.",
    },
    ShortCut {
        event: "Space / Alt+P / Alt+O (file picker)",
        description: "Mark up to 10 files / preview highlighted image / open highlighted file by MIME.",
    },
    ShortCut {
        event: ". / t / Left / Right (file picker)",
        description: "Toggle hidden files / sort name or modified newest first / parent / enter folder.",
    },
    ShortCut {
        event: "/ / ~ / p / s (file picker)",
        description: "Type path / home / Pictures / screenshot directory.",
    },
    ShortCut {
        event: "alt+a",
        description: "Choose attachment; Enter stages, Enter again sends; Esc cancels attachments.",
    },
    ShortCut {
        event: "alt+s",
        description: "Capture screenshot and stage it; Enter sends, Esc cancels attachment.",
    },
    ShortCut {
        event: "alt+p",
        description: "Preview attachments: Left/Right changes file, +/- zooms, Alt+arrows pans zoomed images, Esc/q returns. Nonimages/unknown types use EDITOR.",
    },
    ShortCut {
        event: "F2",
        description: "Edit local channel alias (Enter saves, Esc cancels, empty removes).",
    },
    ShortCut {
        event: "F3",
        description: "Edit local alias of the selected author or direct-message contact.",
    },
    ShortCut {
        event: "f1",
        description: "Toggle help panel.",
    },
    ShortCut {
        event: "tab",
        description: "Sends emoji from input line as reaction on selected message.",
    },
    ShortCut {
        event: "alt+enter",
        description: "Switch between single-line and multi-line modes.",
    },
    ShortCut {
        event: "ctrl+p",
        description: "Open pop-up for selecting a channel",
    },
    ShortCut {
        event: "ctrl+w / ctrl+backspace / alt+backspace",
        description: "Delete last word.",
    },
    ShortCut {
        event: "ctrl+u",
        description: "Delete to the start of the line.",
    },
    ShortCut {
        event: "enter, when input box empty in single-line mode",
        description: "Open URL from selected message.",
    },
    ShortCut {
        event: "enter, single-line mode",
        description: "Send message.",
    },
    ShortCut {
        event: "enter, multi-line mode",
        description: "New line message.",
    },
    ShortCut {
        event: "alt+f / alt+Right / ctrl+Right",
        description: "Move forward one word.",
    },
    ShortCut {
        event: "alt+b / alt+Left / ctrl+Left",
        description: "Move backward one word.",
    },
    ShortCut {
        event: "ctrl+a / home",
        description: "Move cursor to the beginning of the text.",
    },
    ShortCut {
        event: "ctrl+e / end",
        description: "Move cursor the the end of the text.",
    },
    ShortCut {
        event: "e / ctrl+e",
        description: "Edit own selected message with an empty composer; Enter saves, Esc cancels.",
    },
    ShortCut {
        event: "Esc",
        description: "Dismiss notice first; otherwise reset message selection / close popup or cancel attachments.",
    },
    ShortCut {
        event: "alt+Up / alt+k / PgUp",
        description: "Select previous message.",
    },
    ShortCut {
        event: "alt+Down / alt+j / PgDown",
        description: "Select next message.",
    },
    ShortCut {
        event: "ctrl+j / Up, single-line mode",
        description: "Select previous channel.",
    },
    ShortCut {
        event: "ctrl+j / Up, multi-line mode",
        description: "Previous line",
    },
    ShortCut {
        event: "ctrl+k / Down, single-line mode",
        description: "Select next channel.",
    },
    ShortCut {
        event: "ctrl+k / Down, multi-line mode",
        description: "Next line",
    },
    ShortCut {
        event: "alt+l",
        description: "Toggle channel list pane visibility",
    },
    ShortCut {
        event: "alt+y",
        description: "Copy selected message to clipboard",
    },
];
