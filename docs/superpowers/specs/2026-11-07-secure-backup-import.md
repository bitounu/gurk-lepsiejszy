# Signal Secure Backup import

Support importing an existing multi-gigabyte Signal Secure Backup into Gurk while
preserving local aliases. Recovery keys contain 64 characters. Existing linked
registration must stay usable; no primary registration or backup reservation,
key changes, uploads or deletion.

CLI --restore-cloud-backup promptssecretlocally; linkedaccountbasiccredentials
requestanonymousmessages/media auth; readexistingarchive, authenticatesbefore
import. ModernSVRB usesofficialSDK, limitedrestorationattempts,noautomaticretry
ofcryptographicfailure. Neverlogkeys/headers/passwords.

Streamingdecode withlibsignalFramesReader andtypedpinnedproto; metadata maps
backup-local IDs toACI/groupmasterkey stableGurkIDs. Preserve sourceoriginal
messageIDs and received/renderorder; neverUPSERTexistinglivecontentblindly.
Stagevalidatednormalizedrecords inencryptedDB, media inmanaged0600files with
resume andintegrityverification. Importdoesnotemitnotifications/receipts/messages.
Mergekeepsaliases,hidden,drafts andregistration. Missing/unsupportedrecords and
media reported; no falseclaimfullrestore. CLI inventory/dryrun beforecommit.
