# Grok billing parser and offline UI fixture (Issue #26 follow-up)

**State: parser and synthetic native-view acceptance only. Production billing, identity, model qualification, quota permission, Extra Usage permission, and generation remain Unknown or denied.** This work does not verify that an installed Grok CLI implements the candidate ACP methods.

## What is implemented

- `src-tauri/src/subscription/grok/billing.rs` is a pure JSON-to-DTO parser. It has no transport, auth, persistence, or subscription admission dependency.
- Parser field states distinguish `missing`, explicit `null`, malformed `invalid`, semantic `out_of_range`, and `available`. A valid numeric zero stays available.
- The input root uses the pinned source's snake_case `on_demand_enabled` and `subscription_tier`; fields under `config` use camelCase. Returned billing fields, period fields, legacy fields, and the opaque history items stay represented in the DTO.
- Credit values remain raw integer cents. No currency, display unit, permission, or purchasable balance is inferred.
- `creditUsagePercent` is accepted only within 0–100. Deprecated `used / monthlyLimit` is a fallback only if the direct field is missing or null and both legacy values are present, nonnegative integers with a positive denominator; computed values above 100 remain Unknown. The DTO records `config.creditUsagePercent` or `legacy.used/monthlyLimit` as provenance.
- `rule.enabled` is not defaulted to false when omitted. A billing sample and an auto-top-up sample failure stay separate, so a partial read failure cannot erase a parsed billing response or fabricate a rule.
- The scope guard accepts a response only for the same provider, connection instance, and connection generation. Account switches advance the generation; a response from the previous generation is rejected.
- `grok_billing_offline_fixture` is compiled and registered only with `isolation-check`; the desktop panel appears only after that explicit isolated test bootstrap. It uses fixed synthetic values and does not populate `QuotaEvidence`, identity, catalog eligibility, generation grants, or routing state.

## Source and TokenTracker comparison

The DTO follows the candidate shapes in the pinned official [`billing.rs`](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/src/extensions/billing.rs) at commit [`2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8`](https://github.com/xai-org/grok-build/commit/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8). It identifies the candidate `x.ai/billing` and `x.ai/auto-topup-rule` method shapes, not a contract for this app's installed `@xai-official/grok` 1.0.44. The candidate ACP methods require authentication; the app has no code path here to initialize ACP or issue either method.

The independent comparison source is [`xiufengsun/TokenTracker` commit `24614b13d37c0f602808fbd88f0aabba743c96a0`](https://github.com/xiufengsun/TokenTracker/commit/24614b13d37c0f602808fbd88f0aabba743c96a0), including its pinned [`src/lib/grok-limits.js`](https://github.com/xiufengsun/TokenTracker/blob/24614b13d37c0f602808fbd88f0aabba743c96a0/src/lib/grok-limits.js); its pinned [`LICENSE`](https://raw.githubusercontent.com/xiufengsun/TokenTracker/24614b13d37c0f602808fbd88f0aabba743c96a0/LICENSE) is MIT, copyright 2026 xiufengsun. The comparison reused only the design checklist: prefer `currentPeriod`, retain deprecated fields for compatibility, and derive a legacy percentage only from a complete numerator and denominator. No TokenTracker source was copied.

TokenTracker's `src/lib/grok-limits.js` was not reused as an adapter: it reads/resolves Grok credentials, can refresh auth and retry, calls private HTTP `/v1/billing?format=credits` with a bare `/v1/billing` fallback, optionally reads settings, clamps percentages, and defaults some missing usage to zero. Those choices are outside the verified ACP source and conflict with this parser's Unknown-preserving, no-auth, no-transport boundary.

The existing hand-run finding remains: local CLI 1.0.44 reports no `grok account`, no documented `models --json`, and `usage` is session-specific. The pinned official snapshot is not proof of compatibility with that CLI version. ACP `session/prompt` remains generation-capable and is not used as a read-only path.

## Local acceptance

Run `pnpm test:grok-billing-parser` for the pure Rust parser and stale-scope tests. Run `pnpm test:grok-billing-offline` for those tests, the frontend build, the `isolation-check` Rust build, and the focused native-webview fixture path in `scripts/check-isolated-desktop.mjs`.

After the focused command has built the local UI and isolation binary, the approved native-window screenshot can be refreshed with:

```sh
AUTOJEV_GROK_BILLING_ONLY=1 AUTOJEV_CAPTURE_NATIVE_SCREENSHOT=1 AUTOJEV_GROK_BILLING_SCREENSHOT_PATH=docs/screenshots/grok-billing-offline.png node scripts/check-isolated-desktop.mjs
```

That capture mode holds the synthetic panel visible, locates the isolated app window by its process ID, and uses the OS screenshot tool with that window ID; it does not capture the rest of the desktop. The command still runs the four fixture assertions and zero-helper/zero-model-request checks.

The focused desktop path starts a fresh isolated app with a loopback-only test server, renders a fixed local DTO, and asserts the subscription snapshot remains unchanged. It must not start either helper, send any model request, or contact xAI. Any request observed by the loopback model server is a failure. Its native-window-only screenshot is [`docs/screenshots/grok-billing-offline.png`](../screenshots/grok-billing-offline.png); it shows only synthetic values. The ordinary `pnpm test:grok-contract` result remains the prior PR #47 record; `pnpm test` and the full Rust suite were rerun for this follow-up. None proves real service behavior.

## Boundary for future work

Real billing access requires separate evidence for the exact installed CLI/ACP version, identity binding, current account state, subscription quota semantics, Extra Usage permission, and a mechanism that constrains the complete request's cost. This parser does not enable identity reads, model qualification, login, quota refresh, auto-top-up reads, generation, or billing calls. Keep all production gates closed until those conditions are independently established and reviewed.
