# Grok ACP read-only path (Issue #26)

**Status (2026-10-02): production ACP reads remain blocked by the source/version gate.** The Providers page now shows that state directly. Account identity, model catalog, billing, auto top-up, Extra Usage permission, and real generation remain Unknown or off.

## What this change implements

- `src-tauri/src/subscription/grok/readonly.rs` contains an ACP JSON-RPC line transport with a compile-time method allowlist for candidate `auth/info`, `models/list`, `billing`, and `auto-topup-rule` reads. It has no process launcher, credential loader, login method, session creation, prompt method, or write method. Fake ACP tests cover request correlation, protocol-version mismatch, and error redaction.
- Production does not construct that transport. The Tauri `grok_readonly_status` command returns only the closed source gate and Unknown fields; it does not inspect the CLI version, start a process, read auth state, refresh an account, or contact xAI.
- The Providers page replaces the old synthetic billing panel with the production gate status. This is a status view, not a live read or fabricated billing result.
- The billing DTO parser remains Rust-test-only. It does not populate `QuotaEvidence`, account identity, catalog eligibility, admission, or generation.
- Real Grok generation remains disabled. No UI control can override the source gate.

## Why the source gate stays closed

The public npm metadata for the locally observed `@xai-official/grok` 1.0.44 identifies `gitHead` `5b807183dd7978a460f309132cf0d1183d743526`; that source revision is not available as a matching public `xai-org/grok-build` commit. The public source snapshot at [`2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8`](https://github.com/xai-org/grok-build/commit/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8) declares `xai-grok-shell` version 1.0.45 in its [`Cargo.toml`](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/Cargo.toml), so it does not establish the implementation behind installed version 1.0.44.

That 1.0.45 source snapshot contains candidate handlers for [`x.ai/billing`](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/src/extensions/billing.rs) and `x.ai/auto-topup-rule`. These handlers require Grok authentication and are not proof that 1.0.44 exposes equivalent ACP methods. The local CLI help observations remain unchanged: no `account` command, no documented `models --json`, and `usage` is session-specific token/cost output.

The [official CLI ACP example](https://docs.x.ai/build/cli/headless-scripting) documents `grok agent stdio` and JSON-RPC framing, then continues through `authenticate`, `session/new`, and `session/prompt`; `session/prompt` generates content and is not used here. The [ACP extension rules](https://agentclientprotocol.com/protocol/v1/extensibility) require custom wire method names to start with `_`. xAI's candidate handlers use `x.ai/...` names internally, but the exact 1.0.44 mapping from those handlers to a public ACP wire request is not documented. The code therefore keeps the candidate transport unconnected to production.

Billing and auto-top-up reads also cannot prove that a complete future generation call is protected from Extra Usage. A usage snapshot, disabled auto top-up, or a successful read is not a consumption guarantee. Missing evidence stays Unknown and generation remains denied.

## Offline verification

Run:

```sh
pnpm test:grok-contract
pnpm test:grok-billing-parser
pnpm test:grok-readonly-gate
```

These checks use in-memory fake ACP messages and the isolated AutoJev app. The UI acceptance asserts the source gate is closed, every account/billing field remains Unknown, generation is off, subscription evidence is unchanged, and no Grok helper or model request was started. None of these checks authenticates a real account or proves the installed CLI contract.

## Remaining evidence needed

Before any production process launch, identify the exact installed package/build, map its immutable source revision to the official source, confirm the ACP wire names and response schemas for each read, and review their credential and network side effects. Before any generation, independently establish model qualification, subscription quota semantics, and an end-to-end Extra Usage restriction. Until then, do not run login, ACP initialization, account reads, billing reads, model requests, or quota refreshes.
