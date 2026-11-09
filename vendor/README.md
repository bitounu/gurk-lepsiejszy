# libsignal-service

Source: https://github.com/whisperfish/libsignal-service-rs
Revision: 7a7526741dfd9ad4c4894717f22651eb0506fb7e (unmodified git archive).
License: AGPL-3.0-only (see bundled license).

Pinned locally because Cargo cannot patch a git dependency with another revision
from the same source. This revision includes upstream PR #501, supplying encrypted
uploadLength to Signal attachment upload forms. Presage currently pins fb92b13.
Remove the patch once the pinned Presage revision includes this fix.

# presage

Source https://github.com/whisperfish/presage, pinned94b3978f5456469cfd4d9f72351a5eff93715f81.
Vendored corecrate adds a public service_credentials accessor delegating to
existing Registered::credentials, avoiding private registration serialization.
Dev-only pathdependencies removed from vendoredCargo.toml; productiondepsunchanged.

Presage also preserves an explicitly selected requiredProtocolVersion when sending
a direct message; previously it unconditionally overwrote the mention version
with INITIAL. Messages without an explicit version still default to INITIAL.
