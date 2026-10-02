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

The [official CLI ACP example](https://docs.x.ai/build/cli/headless-scripting) documents `grok agent stdio` and JSON-RPC framing, then continues through `authenticate`, `session/new`, and `session/prompt`; `session/prompt` generates content and is not used here. The [ACP extension rules](https://agentclientprotocol.com/protocol/v1/extensibility) require custom wire method names to start with `_`. xAI's candidate handlers use `x.ai/...` names internally, but the exact 1.0.44 mapping from those handlers to a public ACP wire request is not documented. No ACP wire method is implemented or tested in this change.

Billing and auto-top-up reads also cannot prove that a complete future generation call is protected from Extra Usage. A usage snapshot, disabled auto top-up, or a successful read is not a consumption guarantee. Missing evidence stays Unknown and generation remains denied.

## Offline verification

Run:

```sh
pnpm test:grok-contract
pnpm test:grok-billing-parser
pnpm test:grok-readonly-gate
```

These checks assert the static source gate and use the isolated AutoJev app. The UI acceptance asserts the gate is closed, every account/billing field remains Unknown, generation is off, subscription evidence is unchanged, and no Grok helper or model request was started. None of these checks authenticates a real account or proves the installed CLI contract.

## Remaining evidence needed

Before any ACP compatibility validation, identify and pin the exact helper artifact and use one bounded route: verify an official source/release mapping, select a source-mapped official release, or—if the 1.0.44 source remains unavailable—obtain separate approval for an isolated compatibility probe after its initialization side effects are understood. A probe establishes only the observed protocol contract, not source provenance, account identity, quota permission, or billing safety. Review the exact ACP method names, schemas, and credential/network effects; do not use `session/prompt` or `auth/check_subscription` as probes. Before generation, independently establish model qualification, subscription quota semantics, and a server-side end-to-end Extra Usage restriction. Until then, do not run login, ACP initialization, account reads, billing reads, model requests, or quota refreshes.
