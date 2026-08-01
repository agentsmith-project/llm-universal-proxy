# Reasoning Effort Unification + Per-Upstream `dialect` — Plan

- Date: 2026-08-01
- Owner: lizhijian
- Input: official DeepSeek / Zhipu(GLM) / Qwen provider docs (rechecked 2026-07-31) + the repo's own [`docs/protocol-baselines/capabilities/reasoning.md`](../../protocol-baselines/capabilities/reasoning.md) and the OpenAI/Anthropic baselines.
- Status: handoff-ready plan. Deliberately short. Implementation is in-place changes + tests; no separate evidence/gate/audit artifacts.

## Principles (binding for this work)

- **KISS / DRY / YAGNI.** One way per feature. Add only what mixed-provider operators hit. Prefer declaring provider shape in config over branching on provider names in code.
- **Additive only — no existing-config regression.** `dialect` is OPTIONAL. When `dialect` is absent, every behavior defined here must be identical to today (same-wire passthrough; cross-protocol reasoning_effort warn-and-drop).
- **No provider-specific hardcoding.** No `if upstream == "deepseek"`. Provider specifics live in the user's `dialect` block, not in `match` arms keyed by name.
- **In-place fix + test.** Each change lands with its test in the same change. No parallel redesign.
- **No new governance layers.** No evidence/gate/report/audit docs for this work. The protocol-baselines audit cadence is the only doc ritual.

## 1. Goal & scope

Two features, landed together because the second is the switch that safely turns on the first:

- **Feature A — Unified reasoning effort (union vocabulary).** Accept every reasoning-effort level any client protocol can send, map it to the upstream's native shape, and cap to the upstream's supported ceiling.
- **Feature B — Per-upstream `dialect` config.** An optional `dialect` block on each upstream that declares that provider's reasoning mechanism, whether it echoes reasoning output, and the effort levels it supports. The proxy's cross-protocol reasoning mapping activates **only** when `dialect.reasoning` is set; otherwise today's behavior holds.

Focus providers for the research and example configs: **DeepSeek**, **GLM/Zhipu** (OpenAI-coding + Anthropic-compat), **Qwen**. They are illustrative OpenAI/Anthropic-compatible upstreams, not special-cased in code.

Scope boundary: this plan covers **request-side effort mapping** (and the existing response-side reasoning echo). It does **not** add new wire protocols or new response translation paths.

## 2. Provider research findings

Rechecked against official docs 2026-07-31. "Native levels" = the values the provider actually distinguishes after its own internal remapping.

| Provider / endpoint | `format` | Reasoning mechanism | Accepts standard OpenAI `reasoning_effort`? | Reasoning output echoed | Native levels | Notable constraints |
| --- | --- | --- | --- | --- | --- | --- |
| **DeepSeek** `api.deepseek.com` | `openai-chat-completions` | `reasoning_effort` (V4 models) | **Yes** (V4 only; legacy R1/V3 have no effort knob) | **Yes** — `message.reasoning_content` / `delta.reasoning_content` | `low`, `high`, `max` (`medium`→high, `xhigh`→high/max tolerated; no `minimal`) | Uses `max_tokens`, **not** `max_completion_tokens`. Sampling params (`temperature`/`top_p`/penalties) ignored on V4, **rejected with 400** on legacy reasoner. With V4 + tool calls, prior-turn `reasoning_content` **must** be echoed back (400 if omitted); plain chat ignores it. |
| **GLM coding** `open.bigmodel.cn/api/coding/paas/v4` | `openai-chat-completions` | `reasoning_effort` **GLM-5.2+ only**; plus binary `thinking:{type:enabled\|disabled}` on all thinking-capable GLM | **Yes** (GLM-5.2+; Zhipu extension adds `xhigh`/`max`/`minimal`/`none`) | **Yes** — `reasoning_content` | effectively `high` vs `max` (`low`/`medium`→high, `xhigh`→max, `minimal`/`none`→skip) | `reasoning_effort` only takes effect when `thinking.type=enabled`. GLM-4.5/4.6/4.7 accept **only** the binary toggle, not effort levels. Preserved thinking is on by default on the coding endpoint. |
| **GLM Anthropic** `open.bigmodel.cn/api/anthropic` | `anthropic` | **Undocumented** — only `model`/`max_tokens`/`messages`/`stream` are reliable | No | **Unconfirmed** on the Anthropic side | model-selection only | No documented support for `thinking`, `budget_tokens`, or `output_config.effort`. Control reasoning by choosing the model. |
| **Qwen** `dashscope.aliyuncs.com/compatible-mode/v1` | `openai-chat-completions` | `enable_thinking` (bool) + `thinking_budget` (int) — **non-standard**; `reasoning_effort` is **not** a Chat-endpoint param (native/Responses APIs only, model-specific values) | **No** (tolerated only via value-mapping on some models) | **Yes** — `reasoning_content` | model-specific (`xhigh`/`medium`/`low` on qwen3.8-max-preview; `high`/`max` on others) | `enable_thinking` default is model-dependent (commercial=false, open-source=true, thinking-only=always-on). Thinking mode forces `temperature>=0.6`. `reasoning_effort` and `thinking_budget` are mutually exclusive. Multi-turn: `reasoning_content` ignored by default (`preserve_thinking` opt-in on newer models). |

Reference (real-Anthropic, from the repo baseline, not a third-party provider): on Anthropic 4.6+ the primary depth control is `output_config.effort` (`low`/`medium`/`high`/`xhigh`/`max`); the old `thinking:{type:enabled,budget_tokens}` is **removed** (400 on Opus 4.7+/Sonnet 5/Fable 5/Opus 5) and survives only on Haiku 4.5 (min 1024). This is why Feature B keeps `anthropic-effort` and `anthropic-thinking` as two distinct mechanisms.

## 3. Union vocabulary, naming, and cross-protocol mapping

### 3.1 Union vocabulary (ordered, low → high)

`none` → `minimal` → `low` → `medium` → `high` → `xhigh` → `max` → `ultra`

- Source: the OpenAI Chat/Responses ladder is `none…max` (see `docs/protocol-baselines/openai-{chat-completions,responses}.md`); `ultra` is reserved as a client-only ceiling above every known provider so the cap-and-warn path has a real trigger.
- The proxy normalizes inbound effort into this vocabulary regardless of client protocol (`reasoning_effort` string on Chat; `reasoning.effort` on Responses; future Anthropic `output_config.effort` / `thinking` on Anthropic clients).

### 3.2 Native emit shape per dialect mechanism

The unified level is emitted to the upstream in the shape chosen by `dialect.reasoning`:

| `dialect.reasoning` | Upstream emit shape | Native level ceiling (default if `reasoning_levels` unset) |
| --- | --- | --- |
| `openai-effort` | Chat: `reasoning_effort:"<level>"`; Responses: `reasoning:{effort:"<level>"}` | `none,minimal,low,medium,high,xhigh,max` |
| `anthropic-effort` | `output_config:{effort:"<level>"}` | `low,medium,high,xhigh,max` |
| `anthropic-thinking` | `thinking:{type:enabled, budget_tokens:<N>}` (or `thinking:{type:disabled}` for `none`) | budget table below (Haiku 4.5 / legacy only) |
| `auto-only` | none (drop + warn) | — (provider auto-reasons; no translatable knob) |
| `none` | none (drop + warn) | — (no reasoning) |

### 3.3 Effort → `budget_tokens` table (only for `anthropic-thinking`)

Fixed internal default; not user-configurable unless a later need appears (YAGNI).

| unified level | budget_tokens |
| --- | --- |
| `none` | `thinking:{type:disabled}` |
| `minimal` | `1024` (provider minimum) |
| `low` | `2048` |
| `medium` | `8000` |
| `high` | `16000` |
| `xhigh` | `24000` |
| `max` / `ultra` | `32000` |

### 3.4 Level capping

- If the unified level exceeds the target's declared ceiling (`reasoning_levels`, or the mechanism default from §3.2), **cap to the target's maximum and emit one `x-llmup-portability-warning`**.
- Example: client sends `ultra` to an `anthropic-effort` upstream (ceiling `max`) → emit `output_config.effort:"max"` + warn. Same `ultra` to a DeepSeek upstream that declared `reasoning_levels:[low,high,max]` → emit `max` + warn.
- Below-ceiling levels the provider doesn't distinguish (e.g. GLM `medium`→`high`) are **not** the proxy's concern; the provider remaps natively and we forward the client's value as-is.

## 4. `dialect` config schema

Optional. Add a single field to `UpstreamConfig` (`src/config.rs:604`, which is `#[serde(deny_unknown_fields)]`) and to its runtime mirror `RuntimeUpstreamConfig` (`src/config.rs:710`):

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub dialect: Option<UpstreamDialect>,
```

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamDialect {
    pub reasoning: ReasoningMechanism,            // required
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_echo: Option<bool>,             // default: per-mechanism (see below)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_levels: Option<Vec<ReasoningLevel>>, // ordered subset of the union; declares the cap ceiling
}

pub enum ReasoningMechanism { OpenAiEffort, AnthropicEffort, AnthropicThinking, AutoOnly, None }
pub enum ReasoningLevel { None, Minimal, Low, Medium, High, Xhigh, Max, Ultra }
```

- `reasoning` — selects the emit shape from §3.2. This is the switch that turns on Feature A's mapping for this upstream.
- `reasoning_echo` — declares whether the provider returns `reasoning_content` / thinking blocks. When false (or unset-false), the proxy's response normalizer skips reasoning surfacing for this upstream. Default per mechanism: `openai-effort`/`anthropic-effort`/`anthropic-thinking` → `true`; `auto-only` → `true` (Qwen-style: auto-reasons but does echo `reasoning_content`); `none` → `false`.
- `reasoning_levels` — optional ordered subset of the union declaring the provider's supported ceiling. If absent, the mechanism default from §3.2 is used. This is how a user tells the proxy "DeepSeek caps at `[low, high, max]`" without the proxy hardcoding it.

### Examples (grounded in the §2 research)

```yaml
upstreams:
  DEEPSEEK:
    api_root: https://api.deepseek.com
    format: openai-chat-completions
    dialect:
      reasoning: openai-effort
      reasoning_echo: true
      reasoning_levels: [low, high, max]   # V4; minimal/medium/xhigh remapped by provider

  GLM_CODING:
    api_root: https://open.bigmodel.cn/api/coding/paas/v4
    format: openai-chat-completions
    dialect:
      reasoning: openai-effort              # GLM-5.2+; older models accept only the binary toggle
      reasoning_echo: true
      reasoning_levels: [none, minimal, high, max]   # effectively only high vs max are distinct

  GLM_ANTHROPIC:
    api_root: https://open.bigmodel.cn/api/anthropic
    format: anthropic
    dialect:
      reasoning: auto-only                  # Anthropic endpoint reasoning fields are undocumented
      reasoning_echo: false                 # thinking-block echo unconfirmed; do not surface

  QWEN:
    api_root: https://dashscope.aliyuncs.com/compatible-mode/v1
    format: openai-chat-completions
    dialect:
      reasoning: auto-only                  # enable_thinking/thinking_budget are non-standard, not translatable
      reasoning_echo: true                  # Qwen does return reasoning_content
```

Note: the GLM_ANTHROPIC block above corrects the illustrative assumption that it speaks `anthropic-thinking`; the research in §2 shows that endpoint does not document `thinking`/`budget_tokens`/`output_config.effort`, so the honest classification is `auto-only`.

## 5. Sequencing (TDD)

Implementation order, smallest safe steps first. Every step is one PR-sized change: test first, then code, in the same change.

1. **Extend the normalized model + union type (no behavior change).** Introduce `ReasoningLevel` / `ReasoningMechanism` enums and a typed `reasoning_effort` representation. Keep the existing `NormalizedRequestControls.reasoning_effort: Option<Value>` field (`src/translate/internal/models.rs:145`) as the raw value; add a parsed typed view next to it. Tests: enum parse/round-trip, ordering.
2. **`dialect` config plumbing.** Add `UpstreamDialect` to `UpstreamConfig` + `RuntimeUpstreamConfig` + YAML load + validation (mechanism required; `reasoning_levels` must be an ordered subset of the union; reject duplicates). Tests: config parse, deny-unknown-fields, validation errors. No runtime effect yet.
3. **Feature B switch + Feature A emit (the core).** At the upstream-body emit sites, branch on `dialect.reasoning` when a dialect is present:
   - OpenAI Chat emit (`src/translate/internal/openai_responses.rs:1044`) and Responses emit (`:1529`, `:1905`) already forward `reasoning_effort`; extend them to apply the cap from `reasoning_levels`.
   - Add the new Anthropic emit paths (`output_config.effort` for `anthropic-effort`; `thinking`+`budget_tokens` from §3.3 for `anthropic-thinking`) where the request body is built for an Anthropic upstream. This is the new code; today reasoning_effort is warn-dropped for Anthropic targets (`src/translate/internal/assessment.rs:536`, `SharedControlProfile` at `:272`).
   - Level cap → emit `x-llmup-portability-warning` (same header channel used by existing portability warnings).
   - When `dialect` is **absent**, behavior is byte-identical to today (passthrough on OpenAI targets; warn-and-drop to Anthropic). Tests: (a) DeepSeek-dialect `ultra`→`max`+warn; (b) GLM-coding `medium` passes through to `reasoning_effort:"medium"`; (c) anthropic-effort `high`→`output_config.effort:"high"`; (d) anthropic-thinking `high`→`thinking:{type:enabled,budget_tokens:16000}`; (e) no-dialect Anthropic target still warn-drops (regression guard).
4. **Response echo gating.** Make the existing response-side reasoning normalizers (`src/translate/internal/response_protocols.rs:100` `normalize_openai_completion_response`, `:124` `prepare_openai_message_for_claude_response`) honor `reasoning_echo`. When `reasoning_echo:false`, skip surfacing reasoning for that upstream. Default unchanged. Tests: GLM_ANTHROPIC (echo false) omits reasoning; Qwen (echo true) surfaces `reasoning_content`.
5. **`extra_body.openai.reasoning_effort` allowlist.** Relax the enum at `src/translate/internal.rs:1884` (currently `minimal`/`low`/`medium`/`high`) to the union vocabulary, reusing the `ReasoningLevel` parser from step 1. Tests: accept `xhigh`/`max`/`none`; reject unknown strings.
6. **Integration coverage.** Add cases to `tests/reasoning_test.rs` (mock upstreams already exercise `reasoning_content`/thinking): cross-protocol `reasoning_effort` → `output_config.effort`, and cap-and-warn for `ultra`.

## 6. Non-goals (scope guard)

- **No provider name branching in code.** DeepSeek/GLM/Qwen specifics are declared per-upstream via `dialect`; the proxy never matches on upstream name or API root.
- **No new wire protocols.** No Gemini/MCP/Responses-as-a-new-client additions. The three existing `UpstreamFormat` values (`src/formats.rs:8`) are the only targets.
- **No interface change for existing configs.** Any config without a `dialect` block must behave exactly as today (no new warnings, no new drops, no new fields required).
- **No Qwen `enable_thinking`/`thinking_budget` emission.** Those are non-standard and model-specific; mapping unified effort to them is out of scope. Qwen is classified `auto-only`. (A generic `enable-thinking-budget` mechanism is deferred until a second provider needs it — YAGNI.)
- **No DeepSeek `max_tokens`↔`max_completion_tokens` rewrite.** DeepSeek uses `max_tokens`; the proxy already forwards client token fields natively per format. Do not add a param-name translation here.
- **No multi-turn `reasoning_content` echo policy.** DeepSeek-with-tools requires replaying prior `reasoning_content`; the proxy already preserves reasoning on assistant history (`tests/reasoning_test.rs` section D). That behavior is unchanged; this plan does not add provider-specific replay rules.
- **No new governance/evidence docs.** Update [`docs/protocol-baselines/capabilities/reasoning.md`](../../protocol-baselines/capabilities/reasoning.md) only if the cross-protocol posture changes materially; otherwise leave it.
