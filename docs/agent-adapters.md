# Agent configuration adapters

AutoJev edits each agent's native configuration when the user connects it. Detection alone does not change files. Paths below are defaults; supported environment overrides take precedence.

| Agent | Configuration | Adapter |
| --- | --- | --- |
| Codex | `~/.codex/config.toml` | Responses |
| Claude Code | `~/.claude/settings.json` | Messages |
| Grok Build | `~/.grok/config.toml` | Chat or Messages |
| Kimi | `~/.kimi-code/config.toml` (existing `~/.kimi/config.toml` also recognized) | Chat or Messages |
| OpenCode | `~/.config/opencode/opencode.json` or `opencode.jsonc` | Chat or Messages |
| OpenClaw | `~/.openclaw/openclaw.json` | Chat or Messages |
| Hermes | `~/.hermes/config.yaml` | Chat |
| OMP | `~/.omp/agent/models.yml`, `config.yml` | Chat or Messages |
| FastClaw | `~/.fastclaw/fastclaw.db` | Chat or Messages; system defaults |
| Gemini | `~/.gemini/settings.json`, `.env` | Gemini-to-Chat bridge |
| Cursor CLI | `~/.cursor/cli-config.json` | Detection only; no documented custom model endpoint setting |

OMP separates provider/model definitions from the default model selection. Gemini separates settings from environment variables. Both files therefore belong to one connection and are backed up together.

The new file adapters validate configuration before writing, retain an original-byte backup across repeated connections, and restore originally absent files by removing them. JSON5 and YAML comments may be reformatted while connected; restoring returns original bytes. FastClaw backs up only the affected database records in a transaction, preserving unrelated data on restore. Its agent/user overrides can supersede the system default; restart a running agent to load changed settings.

The Gemini bridge supports text, inline images, and function calls. Streaming currently buffers the upstream completion and emits one SSE event. Token counts are estimates. Cached content and non-image inline media are unsupported. This is a compatibility adapter, not complete Gemini API parity.

Validation uses isolated configuration fixtures, database fixtures, and protocol tests. It does not prove live compatibility with every installed agent version or change the user's real agent configuration during tests.

References: [Grok](https://docs.x.ai/build/settings), [Kimi](https://www.kimi.com/code/docs/en/kimi-code-cli/configuration/config-files), [OpenCode](https://opencode.ai/docs/providers/), [OpenClaw](https://docs.openclaw.ai/gateway/configuration-reference), [Hermes](https://hermes-agent.nousresearch.com/docs/user-guide/configuration), [OMP](https://github.com/can1357/oh-my-pi/blob/main/docs/providers.md), [Gemini CLI](https://geminicli.com/docs/reference/configuration/), [Cursor CLI](https://prod.cursor.com/docs/cli/reference/configuration).
