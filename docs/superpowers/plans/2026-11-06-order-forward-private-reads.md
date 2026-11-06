# Ordering, forwarding, private read synchronization

Stable sidecar010 records original store order atomically via trigger; Sent default
keeps oldconfiguration behavior, Received optsin; oldhistory seededtimestamporder.
Message IDs remainunchanged. UIpaging/window/anchors usemappedkeys, protocol
messages iterator remains timestampordered.

Alt+R forward snapshots selected text/files, visible destination exceptsource;
validated backgroundpreparation, noquote/edit/source metadata, reuploadsattachments.
Modal cancellation stalegeneration safe and draft/source selection preserved.

Own readsync only ownACI SyncMessage.read with binary+legacy author fields;
viewport/focus-aware, outbox011 commit beforelocalRead, retries/restart/ackcleanup.
Peer delivery/read receipts harddisabled; no typing sender. Incoming ownsyncmatches
strict author+timestamp and doesn't rely on sender clock boundary.

Tests207 allfeatures passed before finaldoc/lintformat; fake transports only, no
real Signal sends or account access. Tests include skewpagination/upsert/migration,
forwardfiles/noquote/hidden exclusion/cancelmissing, visibleonlyfocusedreadsync
and encryptedoutboxrestore, ownaddress and binaryACI validation.

Final compatibility: private_read_sync_confirmed makes legacy localRead messages
sync once when visible; ACK atomically confirms+dequeues. Failure rollback tested,
RAM confirmation cache avoids repeat framequeries. Incoming read sync uses indexed
exactlookup rather than scanningfullhistory. Terminal focus reporting suspended
around externaleditor as well. Fullsuite208 passed; final lint/package checks pending.

Final verification:208 fullsuite tests, Clippy warnings denied, fmt/diff checks. Independent review found no important issues including confirmedstate rollback/cache. aliases8 -> aliases9 isolatedArch upgrade passed, version runs, Qkk0altered, userconfig SHA unchanged, installedREADME matches source, confirmedstate migration embedded. zstd+SHA256 updated. Live cross-device delivery not exercised, only mock transport.
