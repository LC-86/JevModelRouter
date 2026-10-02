# Grok ACP read-only path (Issue #26)

**Status (2026-10-02): production ACP reads remain blocked by the source/version gate.** The Providers page now shows that state directly. Account identity, model catalog, billing, auto top-up, Extra Usage permission, and real generation remain Unknown or off.

## What this change implements

- `src-tauri/src/subscription/grok/source_gate.rs` returns a static closed source/version status. This change implements no ACP transport because the installed helper source and wire names are not verified.
- The Tauri `grok_readonly_status` command returns only the closed source gate and Unknown fields; it does not inspect the CLI version, start a process, read auth state, refresh an account, or contact xAI.
- The Providers page replaces the old synthetic billing panel with the production gate status. This is a status view, not a live read or fabricated billing result.
- The billing DTO parser remains Rust-test-only. It does not populate `QuotaEvidence`, account identity, catalog eligibility, admission, or generation.
- Real Grok generation remains disabled. No UI control can override the source gate.

## Why the source gate stays closed

An exact request to the [npm registry record for `@xai-official/grok@1.0.44`](https://registry.npmjs.org/@xai-official/grok/1.0.44) returned HTTP 200 without a redirect. Its `_id` is `@xai-official/grok@1.0.44`, its `version` is `1.0.44`, its `gitHead` is `5b807183dd7978a460f309132cf0d1183d743526`, and it has no `repository` field. The local installed package manifest omits `gitHead` and `repository`; that does not contradict the registry record. The historical CLI `--version` output `5b807183dd79` matches the registry `gitHead` prefix. A lookup of the full SHA in the public `xai-org/grok-build` repository returned HTTP 422, meaning that lookup did not resolve the SHA there; it does not rule out another source mapping. The local binary's exact bytes still lack a verified mapping to an official source or release. The public source snapshot at [`2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8`](https://github.com/xai-org/grok-build/commit/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8) declares `xai-grok-shell` version 1.0.45 in its [`Cargo.toml`](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/Cargo.toml), so it does not establish the implementation behind installed version 1.0.44.

That 1.0.45 source snapshot contains candidate handlers for [`x.ai/billing`](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/src/extensions/billing.rs) and `x.ai/auto-topup-rule`. These handlers require Grok authentication and are not proof that 1.0.44 exposes equivalent ACP methods. The local CLI help observations remain unchanged: no `account` command, no documented `models --json`, and `usage` is session-specific token/cost output.

The [official CLI ACP example](https://docs.x.ai/build/cli/headless-scripting) documents `grok agent stdio` and JSON-RPC framing, then continues through `authenticate`, `session/new`, and `session/prompt`; `session/prompt` generates content and is not used here. The [ACP extension rules](https://agentclientprotocol.com/protocol/v1/extensibility) require custom wire method names to start with `_`. xAI's candidate handlers use `x.ai/...` names internally, but the exact 1.0.44 mapping from those handlers to a public ACP wire request is not documented. No custom account/billing wire method is implemented or tested in the production path; the bounded standard authentication handshake is described below and was not added to AutoJev.

Billing and auto-top-up reads also cannot prove that a complete future generation call is protected from Extra Usage. A usage snapshot, disabled auto top-up, or a successful read is not a consumption guarantee. Missing evidence stays Unknown and generation remains denied.

## Bounded existing-session authentication check (2026-10-02)

The locally installed CLI reported grok 1.0.44 (5b807183dd79). One bounded ACP v1 check launched the documented grok agent stdio entry with auto-update disabled, /tmp as its working directory, and a temporary leader socket. It sent only initialize, then one standard authenticate request for the cached_token method advertised by initialization. Initialization negotiated protocol v1 and authentication returned success in 1.65 seconds. The response was non-empty; its payload was redacted in memory and not retained or printed, so no account name, email, or other identity was established. The process was terminated after the response (exit 143 from cleanup; no timeout). No session/new, session/prompt, model request, billing, quota, or credit request was sent. A generic stderr diagnostic contained a network-related keyword, but it was not retained and could not be classified; the ACP authentication response itself succeeded.

This verifies that the CLI accepted its existing cached authentication state at that time. It does not establish the account identifier, map the exact 1.0.44 binary to official source, prove billing safety, or change AutoJev's production source gate. The xAI CLI documentation demonstrates ACP authentication followed by session creation and session/prompt; session/prompt is generation-capable and was not called. No custom auth/info method was sent because its 1.0.44 wire contract is not documented.

## Offline verification

Run:

```sh
pnpm test:grok-contract
pnpm test:grok-billing-parser
pnpm test:grok-readonly-gate
```

These checks assert the static source gate and use the isolated AutoJev app. The UI acceptance asserts the gate is closed, every account/billing field remains Unknown, generation is off, subscription evidence is unchanged, and no Grok helper or model request was started. None of these checks authenticates a real account or proves the installed CLI contract.

## Remaining evidence needed

The bounded standard initialization/authentication check above does not validate any custom ACP method. Before further compatibility work, identify and pin the exact helper artifact and use one bounded route: verify an official source/release mapping, select a source-mapped official release, or—if the 1.0.44 source remains unavailable—obtain separate approval for an isolated compatibility probe after its initialization side effects are understood. A probe establishes only observed protocol behavior, not source provenance, account identity, quota permission, or billing safety. Do not use undocumented auth/info, session/prompt, or auth/check_subscription as probes. The one cached-token check does not authorize a new OAuth login, account-profile read, billing read, quota refresh, or model request. Before generation, independently establish model qualification, subscription quota semantics, and a server-side end-to-end Extra Usage restriction.
