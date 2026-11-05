# Local person and channel aliases

## Intent

Allow users to change the displayed names of people and conversations in gurk only. Preserve existing configurations, messages and Signal names.

## Approaches

1. TOML aliases: simple persistence, but requires editing configuration and restarting.
2. In-app editing with database persistence: convenient during normal use and independent of configuration. Selected architecture.

## Proposed interaction

- F2 opens an alias editor for the current channel.
- F3 opens an alias editor for the sender of the selected message, or the other person in a direct conversation when no message is selected. Without either target it does nothing.
- The editor identifies whether it edits a person or a channel, and initially contains the current alias, or an empty value if none exists.
- Enter saves; Escape cancels. Saving an empty or whitespace-only value removes the alias. Surrounding whitespace is trimmed; names support Unicode and spaces.
- Editing has its own input so message drafts remain intact. Both actions are available as configurable commands and documented in help.

These interaction details define the proposed editor behavior.

## Display rules

Person aliases apply wherever gurk displays the person's name: message authors, quotes, mentions, typing indicators and notifications. A person alias also names their direct conversation unless that conversation has its own channel alias.

Channel aliases apply to the channel list, conversation title, channel picker/search and notifications. Precedence for direct conversations is channel alias, then person alias, then existing name resolution. Group channel aliases do not change member names. Explicit aliases are displayed in full regardless of first_name_only.

Original names remain stored separately and continue receiving Signal updates. Removing an alias reveals the latest original name. Alias editing makes no Signal requests that change remote data.

## Persistence and compatibility

Add local alias tables using the existing SQLite migration mechanism, keyed separately by person UUID and channel ID. Existing databases migrate automatically with empty alias tables; existing names, messages and metadata remain intact. No new required TOML fields, configuration rewrite, relinking or manual migration.

Storage operations must preserve aliases across ordinary contact/group synchronization and application restarts. The in-memory storage implementation supports the same behavior for tests. Surface save failures without claiming success or losing the editor contents.

## Verification

Test migration of an existing database with preserved data, legacy configuration loading, persistence after reopening storage, independent person/channel aliases, precedence, synchronization resistance, removal, Unicode handling and editor save/cancel behavior including preservation of message drafts. Run formatting and the relevant project checks.

## Scope

No remote profile/group renaming, bulk alias management or configuration-based alias lists. Implementation follows the existing Rust, ratatui and SQLite architecture.
