# Model speed routing

The model list supports selected-model speed tests, progress and cancellation, and scheduled tests (on by default, every 30 minutes; adjustable in Settings → General). Selected models are tested concurrently; each model receives three sequential streaming requests with a fixed short prompt, an output limit of 128 tokens and a 20-second total timeout. Tests consume provider API usage. Only one batch runs at a time. The scheduler works while the desktop app is running, including in the tray; it checks once a minute and skips models with samples newer than the configured interval (5–120 minutes).

Measurements are local and persist across restarts. Each model retains at most 30 samples, keyed by internal model ID plus provider ID, upstream model ID, endpoint and protocol. Changing endpoint/protocol invalidates prior samples. Samples retain timing, token counts, success and timestamp, never prompt text or credentials.

Real streaming requests update measurements after completion. Role-only events, usage events and heartbeats do not count as first content. Cancelled requests are excluded, and failed retry attempts count against reliability. Nonstreaming successes do not supply first-content measurements. Output rate is unknown if the provider does not supply complete token usage. The model table shows latency and output rate on one line; sample count, success rate and timestamp are available on hover. Expired samples are labeled explicitly.

For a named intelligent route with the speed preference:

1. Filter enabled, healthy candidates against configured context, tool/vision capabilities and the task's desired tier. These metadata are configured estimates, not verified quality guarantees.
2. Keep the highest available priority group (larger numbers first).
3. Use at most five samples from the last 30 minutes. Prefer real requests in the same context band (<4K, 4K–32K, >=32K input tokens); otherwise use synthetic probes as a cold-start estimate.
4. Exclude candidates without measured first-content latency or with less than 50% success in those samples. If every remaining candidate has output-rate data, compare `(median first-content ms + estimated output tokens / median tokens-per-second * 1000) / success rate`. Estimated output is 18% of input, bounded to 16–4096 tokens. Otherwise compare first-content latency divided by success rate for all candidates. Short synthetic probes cannot guarantee the fastest long-context request.
5. Select locally without calling Jev. If no comparable measurements remain, use weighted load balancing within that priority group. Scheduled tests, when enabled, replenish missing measurements. Existing sessions keep their selected model while eligible; new sessions use the newest ranking.

Balanced, cost and quality preferences still use Jev and fall back to weighted load balancing when it is unavailable. Speed mode hides the local-model preference because the measured ranking determines the choice.

Validation uses loopback mock services rather than paid providers. Run `cargo test --manifest-path src-tauri/Cargo.toml --lib`, `pnpm test`, `pnpm build`, and (with Vite running) `AUTOJEV_PREVIEW_URL=http://127.0.0.1:1433 node scripts/check-speed-ui.mjs`.
