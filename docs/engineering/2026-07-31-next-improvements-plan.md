# Next-Step Improvements — Plan

- Date: 2026-07-31
- Owner: lizhijian
- Input: refreshed official-provider baselines ([`../protocol-baselines/`](../protocol-baselines/)) and the [2026-07-31 online recheck audit](../protocol-baselines/audits/2026-07-31-online-recheck.md).
- Status: handoff-ready plan. Deliberately short. Implementation is in-place changes + tests; no separate evidence/gate/audit artifacts are produced for this work.

## Principles (binding for this work)

- **KISS / DRY / YAGNI.** Add only what users hit; prefer pass-through over new logic; do not build for hypothetical providers or features.
- **In-place fix + test.** Each change lands with a test in the same change. No parallel redesign.
- **No new governance layers.** Do not create evidence/gate/report/audit docs for this work. The protocol-baselines audit cadence is the only doc ritual; nothing new here.
- **Clean overhead as you pass it.** When a change touches an area, remove dead governance/complexity next to it in the same change. Do not launch a separate cleanup workstream.
- **Scope guard.** Explicit non-goals below. Push back on scope creep during review.

## Product-manager framing

Users are Codex CLI and Claude Code operators who route through `llmup` to mixed/non-official upstreams. The product promise is *maximum safe compatibility*: an agent client that works against the official API should keep working through the proxy.

The refreshed provider docs expose one immediate threat to that promise and a few value-adds:

1. **Threat (P0):** Anthropic's current models (Opus 4.7+, Sonnet 5, Fable 5, Opus 5) **reject non-default `temperature`/`top_p`/`top_k` with HTTP 400**. The proxy's **cross-protocol OpenAI→Anthropic** translation injects `temperature`/`top_p` today (`src/translate/internal.rs:2299` `openai_to_claude`, forwarding at `:2312`/`:2315`). So a **Codex CLI (OpenAI-protocol) client routed to a current Claude/Anthropic upstream** silently 400s. (Same-wire Anthropic→Anthropic bypasses translation (`:88-97`) and is intentionally not altered — see P0.) This is the highest-value fix.
2. **Value-add (P1, cheap):** `reasoning.effort: "max"` is the new top effort. It already passes through at the value level (`src/translate/internal/assessment.rs:443`), but has no regression test — add one before it silently regresses.
3. **Value-add (P2, YAGNI-gated):** the new OpenAI explicit prompt-cache controls (`prompt_cache_options`, `prompt_cache_breakpoint`) are not surfaced. Only pursue if a same-wire caller is dropping them; otherwise leave native forwarding alone.

Everything else in the refreshed docs (multi-agent orchestration, hosted shell / computer-use, provider-specific hosted tools, MCP connectors) is **non-portable across protocol schemas** and out of scope for a translation proxy (see Non-goals).

## Prioritized work

### P0 — Anthropic newer-model request compatibility (prevent 400s)
- **Affected user / path:** Codex CLI (OpenAI-protocol) clients routed to a current Claude/Anthropic upstream. On this **cross-protocol OpenAI→Anthropic** path the translator injects `temperature`/`top_p` that the upstream now rejects with 400 (`src/translate/internal.rs:2299` `openai_to_claude`, forwarding at `:2312`/`:2315`). Same-wire Anthropic→Anthropic bypasses translation (`internal.rs:88-97`) and is **intentionally not altered** — native Anthropic 400s there too, so the client config is at fault; the proxy stays a faithful forwarder, not a client-misconfig rewriter.
- **What:** On the cross-protocol OpenAI→Anthropic path, withhold non-default `temperature`/`top_p` (default `1.0`) when the resolved Anthropic upstream model is **Opus 4.7+, Sonnet 5, Fable 5, or Opus 5** (per the refreshed Anthropic baseline's sampling-param constraint). Express the match as a **prefix/range rule** (e.g. `claude-opus-4-7` and above), not an enumeration, so it tracks the next Opus point release without edits. `top_k` is not on this path (OpenAI has no `top_k`), so it is not part of this guard. Emit an `x-llmup-portability-warning` for each withheld value. **Keep the withhold decision and the warning in one place** (the assessment/surface layer that records portability warnings), so the header cannot diverge from what was actually dropped.
- **Why not also rewrite `thinking`:** verified the request translator does **not** synthesize Anthropic `thinking:{budget_tokens}` from OpenAI reasoning — `thinking` handling is response-side only (`response_protocols.rs:138`, `openai_responses.rs:131`). So there is no cross-protocol thinking hazard to fix. Do not invent a `thinking`/`output_config` rewrite (YAGNI).
- **Tests:** unit test that an OpenAI request with `temperature` routed to a `claude-sonnet-5*` upstream produces an Anthropic body without sampling params + a portability warning; legacy Opus/Sonnet 4.x keep forwarding.
- **Effort:** small, contained, in `translate` + assessment.

### P1 — `reasoning.effort: "max"` regression coverage
- **What:** add tests asserting `reasoning.effort:"max"` (Responses) and `reasoning_effort:"max"` (Chat) traverse request translation unchanged on same-wire OpenAI routes.
- **Why:** it works today by value pass-through (`assessment.rs:443`); lock it before someone routes top-level effort through an enum allowlist that omits `max`. Such an allowlist already exists on a different channel (`internal.rs:1883` restricts Anthropic-client `extra_body.openai.reasoning_effort` to `minimal`/`low`/`medium`/`high`, excluding `max`), so the regression vector is concrete.
- **Effort:** trivial.

### P2 — Explicit prompt-cache pass-through (only if needed)
- **What:** confirm same-wire OpenAI callers' `prompt_cache_options` / `prompt_cache_breakpoint` are not stripped; if they are, pass them through natively.
- **Gate:** do this only if P0/P1 review or a real caller shows the fields being dropped. No proactive policy (YAGNI).

## Governance/complexity cleanup (opportunistic, same-change only)

Not a workstream. While implementing the above, remove adjacent dead weight in the same change if trivially safe:

- **CI duplication (DRY):** `ci.yml` and `release.yml` now both run on `v*` tags and overlap heavily (governance, fmt, clippy, test, mock-endpoint-matrix, perf-gate, supply-chain, container). On a tag both fire redundantly. Consolidate to one pipeline on tags (keep the release-specific jobs there), or drop the overlap. Low priority — only if touching CI anyway.
- Do **not** delete the protocol-baselines audits/snapshots layer (just refreshed; it is the documented source of truth) and do **not** add a new audit for this plan.

## Non-goals (scope guard)

- Multi-agent orchestration, hosted shell / `computer` / `apply_patch`-as-hosted-tool, provider-hosted tools, MCP connector translation — non-portable, low demand, **skip**.
- Anthropic refusal / `stop_details` cross-protocol mapping — already within the existing stop-reason translation surface (per the 2026-07-31 audit); no new work unless a cross-protocol refusal is mis-mapped in practice.
- Big architecture rewrite / the broader surface-reduction idea — **out of scope** here; revisit separately if ever justified.
- Adopting every new provider field — only P0 (correctness) and cheap P1 land now; everything else needs a concrete user trigger.
- New evidence/gate/report docs for this work — **explicitly avoided**.

## Sequencing

1. P0 (Anthropic sampling-param guard + warning + tests), commit.
2. P1 (reasoning `max` tests), commit.
3. P2 only if triggered.
4. CI de-dup if touched opportunistically.

Each step is independently shippable and test-backed.
