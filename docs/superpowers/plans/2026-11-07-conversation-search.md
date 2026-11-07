# Active conversation search and boundary jumps

Implement conversation search and verify Arch package upgrades.
Ctrl+Home oldest (visible-order first), Ctrl+End newest + resume follow.
Literal case-insensitive Unicode search across full current conversation body,
not only loaded window, excluding tombstones and edit snapshots. Match navigation
Up/Down and Shift+F3/F3 inside modal; Enter accepts position/highlight; Escape
restores presearch position and focus. Empty query shows no matches.
Slash opens only with empty composer, otherwise literal to preserve URLs; F8 opens
with any draft. Bindings configurable. Text, edit context, pending attachments
must not be sent/changed by search. Search state is RAM only; new arrivals don't
shift the active match. Matches have local IDs (collision-safe), byte ranges in
normalized body, occurrence ordinal. Queries never go to Signal or disk.

Interfaces: src/search.rs defines SearchHit {message_id:MessageId, range:Range<usize>,
occurrence:usize}, literal_regex(query)->Result<Option<regex::Regex>>, normalize_body.
Storage::search_messages(channel,query)->Result<Vec<SearchHit>>, oldest_message.
SQL overrides select only body+local ID with one ordered query; Rust Unicode regex
computes byte offsets. Storage order follows configured received/sent mode.
App fields message_search:Option<MessageSearch>, search_focus:Option<SearchFocus>.
app::search structs pub(crate), SearchFocus {channel_id, local_id, query:String,
range:Range<usize>, occurrence:usize}; state input/hits/current/originalposition.
App integration covers state, input, modal, paste and text-copy behavior. Storage
provides the search core, trait and SQLite implementation. Rendering covers the
search modal and help; show the matching fragment when the full message cannot
fit, highlight across wrapping and Unicode, and keep current focus on accept.
Bindings expose SearchMessages, JumpOldest and JumpNewest in configuration,
F1 help, the shortcut bar and README.

Tests: unloaded history, ordering, Unicode and literal punctuation, repeated
occurrences in one message, local ID collisions, drafts/modals/Enter/Esc/no-send,
empty/noresults, hidden/other channel exclusion, wrapped/oversized/tiny rendering,
Alt+V copy at found offset, binding defaults/remaps and old configs.
Use synthetic fixtures without reading personal data. Run tests, Clippy and package
upgrade checks before delivery. Preserve existing functionality.

Implementation and independent review: SQL one-query full conversation scans;
all match offsets on normalized body. Search preview/accept/cancel preserve drafts,
editing and pending attachments. Copy cursor starts at selected occurrence.
Renderer keeps metadata, maps mentions, wraps graphemes, uses binary match range
lookup (avoids quadratic common-character queries), clips long bodies to active
fragment and revalidates ranges after live edits. Header/date normal path unchanged.
Existing alias help remains near top; modal F3 references scoped independently of
custom alias F3 bindings. 263 library +1 CLI tests pass, including live-edit shift.

Final verification: all264 tests passed (263 lib +1 CLI), alltarget/allfeature
Clippy -Dwarnings, fmt and diff checks passed. aliases15 release package built;
isolated Arch upgrade from aliases14, installedfile checks, unchanged config
checksum, README equality, zstd integrity and CLI startup checks passed. No new
schema migration or real account/network interactions in this feature work.
