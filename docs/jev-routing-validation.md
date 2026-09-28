# Jev routing validation (2026-09-27)

## Reference and scope

The supplied X post (`https://x.com/OpenRouter/status/2103610898690855161`) could not be retrieved (403). This change therefore follows independently verified official documentation, not a claimed reading of that post:

- [OpenRouter Jev Router](https://openrouter.ai/typesafe/jev-router): balances model quality, speed and cost.
- [Decisions API](https://openrouter.ai/docs/api/api-reference/alphadecisions/submit-a-decisions-request): typed Choice responses include confidence.
- [TypeSafe confidence](https://docs.typesafe.ai/confidence): uncertainty can drive fallback; thresholds need evaluation for the specific application.

This uses our existing decision service and candidate pool. It does not replace the gateway with the managed `typesafe/jev-router` model or add reasoning-effort rewriting.

## Changes

- Chat Completions, Messages and Responses extract the latest user task for local classification. System prompts, assistant turns, tool schemas and tool results do not substitute for that task. Text extraction is limited to 4,000 Unicode characters.
- Jev receives derived task features and precomputed cost estimates, not conversation text. Tier-based speed and configured capabilities are explicitly labeled as hints.
- Cost, quality and balanced preferences have distinct Jev selection instructions. Speed priority now uses local measured routing; see [Model speed routing](model-speed-routing.md).
- Decisions responses require a unique candidate ID and numeric confidence in [0, 1]. Values below 0.5 trigger fallback. Legacy custom services can omit confidence, which is recorded as 0 with an explicit “not supplied” reason; this is not measured certainty.
- Automatic-selection fallback uses weighted load balancing within the highest available priority group (larger priority numbers first). Existing session stickiness remains in effect.

## Manual checks

1. Run `pnpm dev`. Configure Jev in Settings and choose an automatic route containing a fast inexpensive model and a strong model with higher prices. Ensure their tiers and prices are set correctly.
2. Start a new agent session for each test; existing sessions retain their selected model. Try a simple translation and a complex security review. Inspect routing reasons and selected models.
3. Compare cost, quality and balanced preferences with new sessions. The service sees explicit instructions for each preference; model outcomes still depend on the configured pool. For speed priority, run model speed tests and verify local measured selection without a Jev call.
4. Test fallback with Jev temporarily unconfigured, then restore its configuration. Fresh sessions should distribute by weight within the highest available priority group, regardless of decision preference.
5. Run `cargo test --manifest-path src-tauri/Cargo.toml` in an environment that permits loopback listeners. The mock service tests cover low confidence, malformed confidence, invalid candidates, request metadata and fallback. No real API key is used by those tests.

## Validation in this environment

`pnpm build` passed. Rust test compilation passed. Full test execution is restricted by the environment's denial of loopback listeners; those integration tests fail at `TcpListener::bind` with `Operation not permitted`, before contacting a mock upstream. Pure routing/parser tests run locally. No live Jev inference or desktop restart has been verified here. Repository-wide `cargo fmt --check` also reports existing formatting differences in unrelated modules.
