# Preserve messages with equal protocol timestamps

Implement stable local identities and verify Arch package upgrades.
Equal (channel,time) records from different senders must remain distinct.
Transit-only404 media cannot be located in archive without plaintextHash;
Android's own restore uses transit in that case. Do not invent recovered media.

Design: preserve Message.arrived_at as Signal protocol time. Add serde(skip)
Message.local_id:Option<u64> and storage_id() -> local_id.unwrap_or(arrived_at).
UI windows, selections and pagination use local IDs. Protocol messages, displayed
times, private read sync, quotes' sent timestamps always use original arrived_at.
SQLite gains encrypted identity sidecar (channel,local_id,sender,protocol_time),
backfilled from legacy rows, with stable unique(channel,sender,protocol_time).
Allocate a checked free local ID for collisions. Store row and mapping atomically;
never overwrite a different sender. Retain original quote snapshots where needed.
No change to aliases, drafts, protocol registration or old staged postcard layout.
Storage::message_by_protocol(channel,sender,timestamp) resolves protocol events.
Sorting uses actual times with local-ID tie break, or received order. Reimport
must resolve both senders, preserve edits/deletes, restore order and deduplicate.

Work boundaries: storage/migration; UI identities; protocol event lookups; root
Message fields and backup merge. Maintain the interfaces above across these components and verify their integration. No real user data/credentials read. Tests: migration,
identical timestamps/different senders, paging/window coexistence, network targets,
quote sender, repeated import and unchanged aliases. Full tests/Clippy/Arch upgrade.

Implementation: serde-skipped local IDs preserve old encrypted staging bytes;
identity sidecar backfills legacy rows, uses partial unique normal protocol index,
and keeps edit snapshots distinct. Per-message identity hydration uses one SQL
query. UI ties use (sortkey,localID); protocol events resolve author+actualtime.
Independent review confirmed network timestamp routing and caught the old t+1
edit-snapshot overwrite; snapshots now retain original time and own local IDs.
Media investigation: Android RestoreAttachmentJob selects transit without dataHash;
the encryptedDigest branch can leave transit-only media unavailable. No invented archive IDs or
claims of recovered unavailable media. Missing-file handling remains explicit.

Final review additionally fixed repeated edits targeting original time overwriting
the first snapshot. A regression keeps original100 +real101 through edits200/201.
Backup merge repairs wrong quoted authors from legacy timestamp-only joins using
source author/time without replacing current message text. Regression imports
both same-time senders then repairs an existing quote to the second sender.

Verification complete: 251 library +1 CLI tests pass, strict all-target/all-feature
Clippy, fmt and diff checks pass. Arch aliases14 release built; isolated upgrade
aliases13→aliases14, package checks, CLI conflicts/offline-report handling, README
comparison and unchanged config checksum pass. No real account data read or
network restore performed by developers. Media404 still explicitly reported.
