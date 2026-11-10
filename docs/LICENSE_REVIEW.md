# License review

Reviewed 2026-11-10. This is a bounded repository review, not a legal opinion or a complete dependency compliance audit.

## Findings

The original gurk code is declared **AGPL-3.0-only** in [Cargo.toml](../Cargo.toml), with the license text in [LICENSE-AGPL-3.0](../LICENSE-AGPL-3.0). The metadata credits boxdot; the upstream project and its contributors retain their attribution. This fork does not replace their authorship with its curator or OpenAI. The declaration is version 3 only, not “version 3 or later.”

Both vendored libraries declare AGPL-3.0-only and retain their license files:

| Component | Evidence and retained attribution |
| --- | --- |
| Presage | [manifest](../vendor/presage/Cargo.toml), [license](../vendor/presage/LICENSE.md); Gabriel Féron |
| libsignal-service | [manifest](../vendor/libsignal-service/Cargo.toml), [license](../vendor/libsignal-service/LICENSE.md), [upstream copyright notices](../vendor/libsignal-service/README.md); Ruben De Smet, Gabriel Féron, Signal/Open Whisper Systems, Michael F Bryan and Whisperfish contributors |

[vendor/README.md](../vendor/README.md) records source revisions and Presage modifications. The build helper [xtask](../xtask/Cargo.toml) separately declares `MIT OR Apache-2.0`; this does not change gurk's license. The matching AGPL declarations of the application and these two vendors present no apparent license mismatch at that level. This finding does not establish compatibility of every transitive dependency.

## Conditions to preserve

The primary text reviewed is the bundled [GNU AGPL version 3](../LICENSE-AGPL-3.0); the [official GNU publication](https://www.gnu.org/licenses/agpl-3.0.html) is also linked for reference. Its web endpoint timed out during this review. The [SPDX AGPL-3.0-only entry](https://spdx.org/licenses/AGPL-3.0-only.html), including the published license text and the distinction from AGPL-3.0-or-later, was accessible for cross-checking.

- **Source copies (§4):** retain applicable copyright, license and warranty notices and provide the license text.
- **Modified source (§5):** prominently identify modifications with a relevant date, state the applicable license and license the covered combined work under the AGPL. Preserve any required interactive legal notices, subject to the section's exception for upstream interfaces without them.
- **Binaries (§6):** meet one of the section's applicable Corresponding Source delivery methods. A binary release needs the exact corresponding source, including required build/install material and covered dependencies, rather than only an unrelated upstream link. Applicable Installation Information requirements must also be considered.
- **Remote interaction (§13):** if the modified program supports remote user interaction over a network, prominently offer those users its Corresponding Source at no charge through a customary network copying mechanism. Merely connecting a local client to Signal does not by itself establish that this client is operated as a remote service for users.

The fork's disclosed extension work begins at commit `76a614d244afab1d1e2524b5f56b15584968b60b` (2026-11-05). Its OpenAI Codex generation and human curation disclosure describes the development process; it does not remove upstream licensing obligations or establish copyright ownership in AI output. [OpenAI's Europe Terms of Use, Content](https://openai.com/policies/eu-terms-of-use/#content) allocates output rights between the user and OpenAI only to the extent permitted by law. Those terms do not grant rights to third-party upstream code.

## Distribution work still requiring verification

The [Arch package recipe](../packaging/arch/PKGBUILD) installs the project AGPL text and README, but does not assemble dependency notices. The [xtask archive writer](../xtask/src/dist.rs) adds only the executable. These paths do **not** demonstrate a complete binary distribution compliance process. Before publishing binaries, audit the actual linked/bundled components, include their required license and attribution notices, and establish the applicable §6 source delivery route. This review made no packaging changes and did not inspect built release artifacts.

The initial offline metadata attempts failed on uncached dependencies. A subsequent `cargo metadata --filter-platform x86_64-unknown-linux-gnu --locked --format-version 1` succeeded after downloading missing public crates, without building or changing the lockfile. Traversing the resolved graph from gurk found **679 reachable package IDs**, including build and development dependencies; this is not a list of components actually linked into a release binary. Other platforms and optional feature combinations were not audited.

The declared license fields include 33 AGPL-3.0-only packages (including gurk), four MPL-2.0 packages (`spellbook`, `option-ext`, `hpke-rs`, `hpke-rs-crypto`), and mostly permissive declarations. `self_cell` offers `Apache-2.0 OR GPL-2.0-only`, so GPL-2.0-only is not its sole licensing route. Compound declarations such as those of `aws-lc-sys`, and Unicode/BSD additional terms, require preserving the applicable notices; choosing a permissive alternative does not waive its conditions. Some metadata uses legacy slash notation rather than a current SPDX expression.

One reachable package, `sqlcipher-crypto-provider 0.1.0`, has no manifest license declaration or local license file. Its pinned rusqlite repository (`a2f393913d9994645c94810498bef07b2183b3dc`) has a top-level MIT license, but the provider's coverage and attribution should be confirmed explicitly before distribution. Its missing field is not evidence that it is public domain or unlicensed for all purposes.

This declaration scan found no obvious sole GPL-2.0-only conflict, but it does not prove compatibility or complete compliance. A full transitive license-text/notice audit, bundled native-library review (including SQLCipher and cryptographic libraries), artifact inspection and source-delivery verification remain outstanding. No conclusion of full legal compliance is made.
