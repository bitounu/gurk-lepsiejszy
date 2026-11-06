# Hidden conversations and contextual shortcuts

Requirements: persistent local hide/unhide+silence; footer toggle/theme/custom
color; PgDown newest unselect; propose chronology fix without rewriting history.

Design: additive009 hidden set, separate hidden channel cache, shared mutation
routing retaining messages/drafts, notification predicate hiddenchannel/sender.
F6 hide/F7 restorelist, F4 contextbar; hexRGB config defaultoff oldconfigsload.
One reservedrow in all renderers and modal input isolation.

Chronology proposal documented in README: stable local display_order sequence
independent of IDs/senttimes; received server metadata additional evidence, current
history preserved unless reliable sequence source exists. Pagination needs newkey.

Verification:193 all-feature tests passed; Clippy all-targets/all-features with
warnings denied passed. Review found hidden channels excluded from delivery/read
bookkeeping; regression failed Sent vsRead beforefix, all_channels iterator now
includes both caches for protocol operations and typingexpiry/namecache. UI remains
visible-only. Tests include allhidden restart, drafts/incominghistory preservation,
mutingpredicate, F4/F6/F7 UI+paste isolation, tinyterminal and footer colors/config.

Arch verification: aliases7 -> aliases8 upgrade passed in isolated root, zstd integrity passed, config SHA256 preserved, --version works, Qkk0 alteredfiles. Checksums updated. No live messages or desktop captures during verification.
