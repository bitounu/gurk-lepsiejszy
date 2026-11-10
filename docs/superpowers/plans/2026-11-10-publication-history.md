# Sanitized publication and reconstructed commit series

**Goal:** Publish the existing development to an independent repository while preserving the upstream base and excluding private review artifacts.

**Architecture:** Audit and anonymize the working tree before creating new commits. Keep upstream commits unchanged. Prepare an independent local publication checkout, a thematic source-module series and an explicit integration milestone; isolate the latest optional ASCII-emoticon primitives as their own component change. The initial publication used creation-time dates. On request, author and committer dates were reconstructed from recorded feature-completion milestones in UTC, while administrative publication/integration dates were retained. Component grouping remains explicit rather than pretending to be historical release snapshots.

- [x] Inventory all changed/untracked candidates; exclude private screenshots, logs, generated binaries/packages and local databases.
- [x] Anonymize examples and remove personal operational facts; independently audit source and documentation.
- [x] Create an independent publication repository with upstream ancestry and a source-only snapshot.
- [x] Validate the sanitized tree with formatting, tests and static checks.
- [x] Record the original feature sequence and prepare thematic commit boundaries, identifying shared integration dependencies.
- [x] Configure the requested public author identity, create commits and verify every new tree/message/author for private identifiers and artifacts.
- [ ] Configure and push to the supplied Gitea repository without replacing existing remote history.

The destination URL and intended public author identity must be supplied before creating/publishing new commits. GitHub fork/PR is a later action, not part of this publication step. No original source history or private input artifacts are deleted.

Preparation verification: the sanitized source and publication checkout match byte-for-byte. 351 library + 1 CLI tests, formatting and Clippy pass. Independent code/documentation reviews found no remaining identified private information. Private review media/logs and generated packages are ignored and absent from the copied candidates. Sixteen component/dependency commits were created with the supplied public author identity. Date reconstruction uses the recorded conversation from November 5–10, with author and committer dates changed together. The original local history is retained under a recovery ref. Remote publication remains pending Git authentication.

Date ordering: component commits are ordered chronologically by their recorded milestone timestamps; integration and publication preparation retain their November 10 timestamps. Both author and committer dates are verified, with all application/build/migration/test files unchanged and the original sanitized series retained under a local recovery ref.
