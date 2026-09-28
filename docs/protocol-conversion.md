# Agent protocols and upstream model APIs

AutoJev separates the protocol an agent speaks from the API offered by an upstream model. A model's **API type** describes its upstream service; do not change it to Responses merely because you connect Codex.

| Agent-facing endpoint | Chat Completions upstream | Responses upstream | Messages upstream |
| --- | --- | --- | --- |
| `POST /v1/chat/completions` | Native | Converted | Converted |
| `POST /v1/responses` | Converted | Native | Converted |
| `POST /v1/messages` | Converted | Converted | Native |

For example, Codex sends Responses requests to AutoJev. AutoJev can select a model configured with Chat Completions, translate the request and tool history, call the provider's `/v1/chat/completions`, and translate its response back into Responses events. Claude Code can use the same upstream through `/v1/messages`.

## Supported conversion

- System instructions, text conversations, image URLs and base64 images.
- Function tool definitions, tool choices, tool calls and results with stable call IDs.
- Responses custom tools translated to a function with a string `input` argument; returned arguments are decoded back into custom tool input. Grammar constraints are not enforced by the upstream function API.
- Streaming text and tool arguments, completion status, token usage and protocol-shaped errors.
- Portable sampling and output-token limits. Chat Completions and Responses structured output formats are translated between those two APIs.
- Upstream paths and authentication headers follow the selected model's API type.

Native requests preserve provider-specific request and response fields. Cross-protocol streams forward text as it arrives, support UTF-8 split across network chunks, and report interrupted or malformed streams as errors instead of successful completions. Converted responses are bounded to 16 MiB, SSE frames to 2 MiB, and upstream idle reads to 120 seconds.

## Compatibility boundaries

Protocol conversion does not add vision, tool use, reasoning ability, or context capacity to a model. Tool support is assumed by default and tool requests are forwarded for upstream validation. Vision support and context limits are also validated by the upstream service; local capability flags and token estimates do not reject requests. Enabled state and route configuration still apply.

Cross-protocol conversion requires full conversation history. Responses `previous_response_id` and `conversation`, hosted tools such as web search, namespaced tool definitions, audio/file content, multiple choices, and unsupported structured-output constraints are rejected. Use a native upstream for these features. Some non-text tool results also require a native API.

Provider-specific thinking blocks, encrypted reasoning and signatures are not portable and are omitted during conversion. Reasoning controls and prompt-cache hints are not translated. Models that require their original private reasoning state to accompany later tool results are therefore not guaranteed to support a complete cross-protocol agent loop; use their native API for that workflow. This adapter does not claim complete equivalence between provider APIs.

## Verification

Rust tests cover all six conversion directions, request history, tools, images, JSON responses, byte-fragmented SSE, errors and a slow HTTP stream. Proxy integration tests use local HTTP upstreams to check paths, authentication and both streaming and non-streaming responses. These tests do not certify every real provider or a full Codex/Claude Code session.

Protocol references: [OpenAI Responses streaming](https://developers.openai.com/api/docs/guides/streaming-responses), [OpenAI Chat Completions](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create), and [Anthropic Messages streaming](https://platform.claude.com/docs/en/build-with-claude/streaming).
