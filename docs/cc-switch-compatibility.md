# CC Switch compatibility proposal

Status: proposed; no importer or synchronization is implemented.

## Product direction

AutoJev's core value is intelligent routing powered by the Jev model: choosing an eligible model for a task based on capability, complexity, context, and cost. Provider import should make an existing setup available to that router with less manual work.

Today, local rules filter and rank candidates, and Jev arbitrates only when scores are close. Expanding Jev's decision role is separate work that needs routing-quality, latency, cost, and fallback evaluation.

## Initial scope

- Import selected Claude Code and Codex provider connections from a user-selected CC Switch data directory, with the default directory offered as a convenience.
- Read the source database without modifying it. Keep AutoJev's configuration independent; do not share a writable database or continuously synchronize state.
- Preview provider names, endpoints, supported model identifiers, duplicates, and unsupported fields before applying an import. Mask credentials in previews and never log them.
- Store imported API credentials in the local SQLite credentials table, without returning their values to the UI. Report unsupported authentication types rather than treating login sessions as API keys.
- Preserve protocol and authentication distinctions. Import only connections the proxy can actually use; flag missing capability and price metadata for review before enabling automatic routing.
- Make repeated imports idempotent and preserve existing AutoJev changes unless the user explicitly selects replacement. A failed import must leave a recoverable state and report what was applied.

## Coexistence

A later integration can make AutoJev's local endpoint selectable as a provider in CC Switch. Prevent routes back into AutoJev or circular proxy chains. Define which application manages each agent's live configuration, detect external edits, and avoid silently overwriting them during connection or restore.

MCP, Skills, prompt presets, application preferences, and usage history are outside the initial provider-import scope.

## Acceptance criteria

Use sanitized fixtures for each explicitly supported source schema. Verify protocol and endpoint mapping, missing and unsupported credentials, duplicate imports, unsupported schema versions, partial failures, and source database immutability. Verify imported secrets never appear in AutoJev's JSON configuration, previews, or logs.

## Upstream references

CC Switch uses MIT licensing and documents SQLite as its primary store, with provider data in `cc-switch.db` and device preferences in `settings.json`. Inspect and pin the source schema before implementing the adapter; do not assume the preferences file contains provider connections.

- [License](https://github.com/farion1231/cc-switch/blob/main/LICENSE)
- [Configuration documentation](https://github.com/farion1231/cc-switch/blob/main/docs/user-manual/zh/5-faq/5.1-config-files.md)
