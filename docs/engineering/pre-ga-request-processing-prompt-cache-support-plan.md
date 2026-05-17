# Pre-GA Request Processing and Provider Prompt-Cache Support Plan

- Status: current-main status update; raw same-protocol forwarding, provider-native preserve/usage observation, OpenAI-family -> Anthropic top-level `extra_body.anthropic.cache_control` mapping, and Anthropic -> OpenAI-family `extra_body.openai.prompt_cache_key` / `prompt_cache_retention` mapping are present; remaining translated block-level support and future reviewed prompt-cache synthesis are pending
- Date: 2026-05-16
- Scope: internal request processing classification, raw same-protocol provider forwarding optimization, provider-native prompt-cache request-control support, provider-returned cache usage observation, and request-handling simplification under the single maximum safe compatibility strategy
- Non-scope: any `llmup`-managed cache, gateway response/result cache, semantic cache, cache storage, cache lifecycle management, broad fallback DSLs, pricing catalogs, guardrails, prompt management, admin UI expansion

## Plan Coordination

This plan assumes [pre-ga-remove-native-gemini-format-plan.md](./pre-ga-remove-native-gemini-format-plan.md) is the owning decision for native Gemini removal. Current main has already removed active native Gemini runtime support: `UpstreamFormat` is Anthropic / OpenAI Chat / OpenAI Responses only, `google` / `gemini` config fails closed with a migration hint, and there is no active `/google/v1beta/*` route.

Active pre-GA protocol families for this plan are therefore:

- OpenAI Chat Completions
- OpenAI Responses
- Anthropic Messages

Gemini remains usable only as a provider brand behind an OpenAI-compatible upstream. That path is handled as OpenAI Chat wire protocol. This plan must not add Gemini `generateContent`, `cachedContent`, `cachedContents/*`, `thoughtSignature`, or `extra_body.google.cached_content` work. Any future Google-specific OpenAI-compatible extension requires separate scope review; it is not part of this pre-GA prompt-cache hardening.

## Goal

Make the pre-GA behavior easy to reason about:

- `llmup` has one product strategy: maximum safe compatibility. It preserves portable semantics where possible, emits explicit warnings when supported translation omits non-portable detail, and fails closed when semantics cannot be preserved safely.
- Maximum safe compatibility is the implementation goal for every route.
- `RequestTransformationNotRequired` and `RequestTransformationRequired` are internal request-processing classifications, not product behavior.
- Observability records whether a request needs construction, protocol conversion, or enhancement. Same-protocol requests that do not need mutation use raw provider forwarding as an internal request-processing optimization under the same strategy.
- Provider-native prompt-cache request controls are currently preserved as original payload on supported native paths. Delivered translated slices map OpenAI-family `extra_body.anthropic.cache_control` to Anthropic top-level `cache_control`, and Anthropic-shaped `extra_body.openai.prompt_cache_key` / `prompt_cache_retention` to OpenAI-family target top-level fields. Remaining translated block-level support and future reviewed synthesis are still future work after the target request shape is known.
- The proxy does not cache responses/results, prompts, embeddings, tokens, KV state, or provider cache resources.
- The proxy does not invent a cross-provider cache abstraction.

Same-format means the provider-facing wire protocol, not the provider brand. An OpenAI-compatible upstream can use the internal raw same-protocol forwarding path for OpenAI-shaped requests when the route does not require body mutation or response normalization. If a compatible upstream needs provider shims, model body rewrites, or response normalization, the request is constructed or translated under the same maximum safe compatibility strategy.

Raw same-protocol forwarding is an internal request-processing optimization under maximum safe compatibility.

This is still a pre-GA plan for the prompt-cache disposition pieces. Future broader translated support and any reviewed synthesis may require test updates, but raw same-protocol forwarding itself must remain an internal request-processing optimization.

## Design Principles

1. Same-format requests that are eligible for raw provider forwarding must not be normalized, repaired, reserialized, or translated.
2. Translation and target-provider request construction must be explicit and maximum-safe. Provider/protocol shims and provider-native request-control synthesis require `RequestTransformationRequired`.
3. Cache is provider-owned. OpenAI `prompt_cache_key` / `prompt_cache_retention` and Anthropic `cache_control` are provider prompt-cache request mechanisms with different semantics, billing, retention, and lifecycle rules.
4. Preserve prefixes. Cache savings depend on stable prompt prefixes, message order, tool order, schemas, media detail, and provider-specific cache handles.
5. Keep the mental model small. Request observability should answer one question: "does this request need construction, conversion, or enhancement?"
6. Stop scope creep early. Do not add `llmup` cache storage, universal cache controls, response caching, semantic caching, fallback routing languages, or synthetic provider state while adding request-processing and prompt-cache request-control support.

## Current Findings

### Local Code Audit

Current main has the request-processing split in place. `RequestTransformationNotRequired` paths preserve the raw request body and raw upstream response body when the same-protocol route does not require body mutation or response normalization. Routes that need model body rewrite, forced streaming mutation, same-protocol repair/shims, state expansion, or cross-protocol conversion are classified as `RequestTransformationRequired` and keep using the maximum-safe construction path.

Current request-processing facts:

- [src/request_processing.rs](../../src/request_processing.rs) owns `RequestProcessing`, `StateBridgeModifier`, and `PromptCacheRequestControl`.
- [src/server/proxy.rs](../../src/server/proxy.rs) uses raw request bytes and raw non-stream response bytes on `RequestTransformationNotRequired`.
- Provider-native prompt-cache fields are classified as `Preserved` for OpenAI-family `prompt_cache_key` / `prompt_cache_retention` and same-protocol Anthropic `cache_control`.
- Hooks and translation code observe provider-returned cache usage counters such as OpenAI cached tokens and Anthropic cache read/write tokens.
- Current main includes the OpenAI-family -> Anthropic top-level `extra_body.anthropic.cache_control` explicit mapping and the Anthropic -> OpenAI-family top-level `extra_body.openai.prompt_cache_key` / `prompt_cache_retention` explicit mapping. Broader block-level marker mapping and prompt-cache synthesis are not implemented.

### External Product Patterns

Comparable gateways expose separate provider-native surfaces, gateway controls, or cache products. These are contrast examples only; `llmup` must not copy them into its request model, route configuration, or user-facing behavior.

- LiteLLM has OpenAI-native integrations and separate OpenAI forwarding endpoints for newer or less-supported OpenAI endpoints.
- Cloudflare AI Gateway offers provider-native endpoints where a user replaces the provider base URL, and separately offers exact response caching.
- OpenRouter distinguishes provider prompt caching from its own response cache, and uses sticky provider routing to preserve provider cache warmth.
- Helicone distinguishes provider-level prompt caching from Helicone response caching, while its AI Gateway route is a broader OpenAI-compatible translation layer.
- Portkey exposes OpenAI compliance controls because provider-native fields can be lost when normalized into a single schema.
- Vercel AI Gateway namespaces gateway behavior in `providerOptions.gateway`, including `caching: 'auto'`, instead of pretending provider cache controls are the same.
- Envoy AI Gateway exposes a provider-agnostic `cache_control` field, but scopes it to Anthropic-compatible targets where it can be translated into native Anthropic / Vertex Claude / Bedrock Claude controls.

The common lesson for `llmup`: keep one maximum safe compatibility strategy, and classify only whether a request needs construction, conversion, or enhancement. Provider prompt-cache request fields should remain provider-native request controls, not `llmup` cache. Any cache usage observation must be read-only telemetry over provider-returned usage fields, not a `llmup` cache implementation.

## Target Architecture

Current main has an explicit request processing observation:

```rust
enum RequestProcessing {
    RequestTransformationNotRequired,
    RequestTransformationRequired,
}

struct RequestProcessingInfo {
    request_processing: RequestProcessing,
    zero_transform_forwarding_active: bool,
    state_bridge: StateBridgeModifier, // off | capture_candidate | expanded
    provider_native_prompt_cache: PromptCacheRequestControl, // none | preserved | explicit_extension_mapped | synthesized
}
```

This plan owns the internal request processing classification and provider-native prompt-cache request-control support. [pre-ga-conversation-state-bridge-plan.md](./pre-ga-conversation-state-bridge-plan.md) owns the state-bridge field. This avoids inventing separate product behaviors for combinations such as "state bridge + translation + prompt-cache request controls"; provider prompt-cache synthesis remains provider-native request-control support. When the state bridge is enabled, execution order must be:

1. Conversation state expansion.
2. Source -> target protocol translation.
3. Provider-native prompt-cache request-control synthesis, if introduced by a future reviewed implementation.
4. Upstream request.

Request processing classification:

- `RequestTransformationNotRequired`: internal classification for a request where client protocol equals upstream wire protocol and no configured route feature requires body mutation, provider-native request-control synthesis, or response normalization.
- `RequestTransformationRequired`: internal classification for a request where protocols differ, or the selected route needs a provider/protocol shim, target-provider request construction, provider-native request-control synthesis, model alias body rewrite, provider-specific role repair, translation default injection, format conversion, or error-shape conversion. This still follows the single maximum safe compatibility strategy.

The route decision should be visible in debug traces and metrics as `llmup.request_processing`, with fields such as `llmup.state_bridge` and `llmup.provider_native_prompt_cache`.

### Internal Raw Same-Protocol Forwarding Contract

Allowed proxy behavior:

- Data-plane authentication and provider credential injection.
- Upstream selection, DNS/TLS, timeout, cancellation, and body/stream size limits.
- Namespace-to-upstream base URL mapping while preserving the provider path suffix, method, and query semantics.
- Hop-by-hop header stripping and configured auth/header policy.
- Trace IDs, metrics, and hooks that observe metadata.
- Fail-closed rejection if a request contains a known proxy-private artifact such as `_llmup_tool_bridge_context` or `__llmup_custom__*` in a structured public control field.

Disallowed in the internal raw same-protocol forwarding path:

- JSON reserialization of request or success response bodies.
- Role repair, role coalescing, tool-name repair, schema repair, MiniMax/provider shims, or translation defaults.
- `stream` insertion, `parallel_tool_calls` insertion, max-token insertion, Anthropic `disable_parallel_tool_use` insertion, or other body defaults.
- `model` field rewrite/removal. If alias expansion requires body mutation, the request must use construction/translation under maximum safe compatibility.
- Provider error wrapping into another protocol shape.
- SSE event parsing or rewriting for ordinary successful same-format streams.
- Response redaction that changes client-visible bytes. Redaction should apply to stored traces, hook payloads, and logs, not provider output.

Header policy:

- Request `Authorization` usually cannot be byte-identical when `llmup` owns provider credentials. That is explicit proxy behavior, not protocol translation.
- Preserve provider protocol headers where safe. Strip hop-by-hop headers, proxy-private headers, and headers that would leak downstream credentials.
- On the internal raw same-protocol forwarding path, do not synthesize protocol defaults such as `anthropic-version` unless the route is explicitly configured to do so; otherwise a client testing a provider's native failure mode will not see it.

### Translation Contract

The existing translation machinery should remain available, but only in `RequestTransformationRequired`.

Maximum-safe path responsibilities:

- Convert request and response schemas across OpenAI Chat Completions, OpenAI Responses, and Anthropic Messages.
- Apply surface gates, maximum-safe shims, warnings, and fail-closed portability boundaries.
- Translate portable tool calls, media, stop reasons, usage, and streaming lifecycle events.
- Fail closed on provider-owned lifecycle state and non-portable cache state that cannot be represented safely.

### Provider-Native Prompt-Cache Request Controls

This field keeps provider-native request-control disposition explicit without making it a separate product behavior or `llmup` cache. In the current `preserved` disposition, the client's native cache fields are carried unchanged. A future `synthesized` disposition would work on a constructed target-provider request, so raw provider forwarding would not be active.

Allowed behavior:

- Start from the target-provider request after routing and any required translation or request construction.
- Add only provider-native prompt-cache request controls from explicit extension mapping or future reviewed synthesis.
- Preserve all other request and response semantics as close to native as possible.
- Emit trace metadata for synthesized fields.

Disallowed behavior:

- General role repair, schema repair, model body rewrite, response normalization, or cross-protocol error shaping.
- Calling the result raw same-protocol forwarding.

## Provider Prompt-Cache Support Strategy

### Stance

`llmup` should support provider prompt-cache mechanisms so operators can use LLMs more economically. It should not implement cache storage or cache lookup itself.

There are two different activities:

- Provider prompt-cache request-control support: `llmup` may synthesize provider-native request controls that ask the selected upstream to use its own prompt-cache mechanism, after implementation review.
- `llmup` caching: `llmup` stores, indexes, evicts, or serves cached data itself.

This plan includes the first activity and excludes the second. A synthesized OpenAI `prompt_cache_key` or Anthropic `cache_control` is still just a request to the provider; it is not a `llmup` cache.

Add a first-class internal Provider-Native Prompt-Cache Request-Control IR so the behavior is not scattered through translators:

- `target_provider`
- `disposition`: `none | preserved_native | explicit_extension_mapped | synthesized_after_review | dropped`
- `source`: `native | explicit_extension | internal_synthesis_review`
- `openai.prompt_cache_key`
- `openai.prompt_cache_retention`
- `anthropic.cache_control`
- `anthropic.breakpoint_strategy`: `top_level_auto | explicit_breakpoints | implementation_reviewed_breakpoints`
- `anthropic.ttl`
- `skipped_reason`

Do:

- Preserve provider-native prompt-cache request fields in the internal raw same-protocol forwarding path.
- Preserve known provider cache usage counters in client-visible native responses.
- Optionally observe provider-returned cache usage counters for metrics and debug traces.
- Keep translated-path support narrow: forward or map only explicit provider-native extension fields that are documented and intentionally supported.
- In translated paths, prefer provider-supported caching controls only when explicit provider-native intent is present in the request shape. For Anthropic, future internal synthesis review may allow top-level automatic `cache_control` for full-history conversation routes, or reviewed breakpoints for stable-prefix workloads.
- Keep future synthesized target-provider cache request controls behind an implementation review, not a user/operator route switch.
- Keep synthesized controls visible in debug traces and compatibility warnings so users can see exactly what `llmup` added.
- Fail closed when a provider cache handle contains non-reconstructable context and the target provider cannot honor it.
- Run all synthesis after the target-provider request has been built, because provider cache matching uses the exact target-side prompt prefix, order, and parameters.

Do not:

- Add any `llmup` cache store, cache lookup, cache eviction, cache key, or cache lifecycle manager.
- Add gateway response cache in pre-GA.
- Add semantic cache.
- Add a cross-provider `cache: true` request parameter.
- Treat OpenAI `prompt_cache_key` and Anthropic `cache_control` as direct semantic equivalents.
- Auto-insert provider cache controls without an implementation review and trace-visible disposition.
- Infer stable/static content from message text meaning. Heuristics must use protocol structure, reviewed cache group sources, and deterministic fingerprints, not LLM judgment.
- Add Google/Gemini-specific cache extensions, including `cachedContent`, `cached_content`, `cachedContents/*`, or `extra_body.google.cached_content`, in this plan.

### Provider-Native Prompt-Cache Request-Control Disposition

Prompt-cache behavior is computed internally from the target request shape and explicit provider-native fields. The handoff target is an internal disposition model:

- `preserved_native`: keep provider-native fields unchanged on same-protocol or OpenAI-family paths where the target can honor them.
- `explicit_extension_mapped`: map a documented explicit provider-native extension only after the target request shape is known.
- `dropped`: warn/drop provider-native cache controls when the target cannot honor them.
- `synthesized_after_review`: reserved for a future synthesis implementation review, not a current config surface.

`explicit support` and any future reviewed synthesis are internal implementation details under the same maximum safe compatibility goal.

Future synthesis review may allow synthesis from deterministic inputs only:

- route / namespace / model alias
- upstream provider, project, region, and model
- authenticated tenant or reviewed cache group source
- session/cache group identifiers that are already explicit inputs to the request or route identity
- a hash of the stable protocol prefix when the implementation review explicitly selects that source

It must not hash the entire request as the default OpenAI `prompt_cache_key`: the full request includes dynamic user turns and would over-partition cache routing. It also must not use `previous_response_id`, `conversation`, `resp_llmup_*`, request IDs, arbitrary metadata, or message prose as a cache key source.

### Current Functionality

Already present:

- OpenAI Chat/Responses `prompt_cache_key` and `prompt_cache_retention` are treated as OpenAI-native controls and are preserved on OpenAI-family targets when supported.
- OpenAI-family requests translated to Anthropic can map explicit `extra_body.anthropic.cache_control` to Anthropic top-level `cache_control`, with fail-closed validation and no `llmup` cache.
- Anthropic-shaped requests translated to OpenAI Chat/Responses can map explicit `extra_body.openai.prompt_cache_key` and optional `prompt_cache_retention` to OpenAI-family top-level fields, with fail-closed validation and no `llmup` cache.
- Anthropic `cache_control` is detected as non-portable when translating away from Anthropic and is warned/dropped instead of being mapped to unrelated provider controls.
- OpenAI Chat/Responses to Anthropic does not inject `cache_control` by default; [tests/integration_test.rs](../../tests/integration_test.rs) has a regression test for concurrent OpenAI-to-Anthropic requests that asserts no marker injection.
- Known OpenAI content/tool `cache_control` markers are rejected fail-closed before any Anthropic per-block marker is emitted.
- Anthropic to OpenAI translation strips `cache_control` and does not synthesize OpenAI `prompt_cache_key`.
- Active native Gemini cache-handle handling has been removed. Remaining Gemini references must be migration/retired/historical notes or Gemini-as-OpenAI-compatible examples.
- Hooks and translation code already observe provider-returned cache usage fields such as OpenAI cached tokens and Anthropic cache read/write tokens.
- Eligible same-protocol routes can use raw provider forwarding, preserving provider-native cache fields and raw usage bytes as an internal request-processing optimization.

Known gaps:

- Remaining translated provider-native request-control mapping beyond the delivered top-level explicit extensions is not implemented.
- Prompt-cache synthesis is not implemented and has no config/policy surface.
- OpenAI-shaped requests routed to Anthropic have only the explicit top-level `extra_body.anthropic.cache_control` mapping.
- OpenAI-shaped content parts and tool structures routed to Anthropic do not preserve explicit per-block Anthropic `cache_control` extension fields; known attempts fail closed.
- Anthropic-shaped requests routed to OpenAI can use only the explicit target-provider `extra_body.openai.prompt_cache_key` extension; there is no synthesis.
- There is no shared disposition object that explains why a cache control was preserved, dropped, or synthesized.
- There is no target-provider optimization matrix that says what `llmup` should do for every source/target pair.
- Deferred routing-affinity constraint: fallback or load-balanced routes must not scatter state-bridge continuations, and future provider-cache affinity work must not break explicit provider order or route ownership. This is not part of the active prompt-cache request-control delivery.
- Streaming raw-forwarding coverage and fine-grained prompt-cache trace metadata should be kept honest in tests, but the current product stance remains preserve/observe only.

### Translated Prompt-Cache Rules

Provider-native prompt-cache request-control support currently uses explicit support:

- `explicit support`: the proxy only forwards or maps documented explicit provider-native extension fields.

The proxy may help express a provider-native cache request only when cache optimization intent is explicit in the request payload. Any future synthesis must be introduced as an internal reviewed implementation path, not as a user/operator mode.

Recommended request extension pattern:

```json
{
  "extra_body": {
    "anthropic": {
      "cache_control": { "type": "ephemeral" }
    }
  }
}
```

This extension means: when the selected upstream is Anthropic, emit Anthropic top-level `cache_control` on the translated Messages request. It does not mean "enable `llmup` caching", and it must be ignored or rejected with a clear warning on non-Anthropic targets.

Anthropic-shaped requests can explicitly target OpenAI-family prompt-cache controls with:

```json
{
  "extra_body": {
    "openai": {
      "prompt_cache_key": "stable-prefix",
      "prompt_cache_retention": "24h"
    }
  }
}
```

This extension means: when the selected upstream is OpenAI Chat or OpenAI Responses, emit OpenAI-family top-level `prompt_cache_key` and optional `prompt_cache_retention`. `prompt_cache_key` must be a non-empty string, `prompt_cache_retention` must be `in_memory` or `24h` when present, and `extra_body.openai` does not accept additional keys.

Allowed translated support:

- OpenAI Chat/Responses to Anthropic: map explicit `extra_body.anthropic.cache_control` to Anthropic top-level `cache_control`.
- Anthropic to OpenAI Chat/Responses: map explicit `extra_body.openai.prompt_cache_key` and optional `prompt_cache_retention` to OpenAI-family top-level fields.
- OpenAI Chat/Responses to Anthropic: future reviewed synthesis may apply an implementation-owned Anthropic strategy when no explicit Anthropic cache control is present: top-level automatic caching for full-history conversations, or reviewed breakpoints for stable prefix blocks.
- Anthropic to OpenAI Chat/Responses: future reviewed synthesis may add OpenAI `prompt_cache_key` when no explicit OpenAI key is present.
- OpenAI Chat <-> OpenAI Responses: preserve OpenAI prompt-cache controls across OpenAI-family translation without changing retention spelling.

Disallowed translated support:

- Do not read an arbitrary prompt and decide which blocks are "static" from natural-language meaning.
- Do not add Anthropic block-level breakpoints based on content length, role, first message, last system message, or perceived repetition unless a future implementation review explicitly names that behavior.
- Do not preserve or map OpenAI content/tool block-level `cache_control` markers in current main; known markers fail closed until a future implementation review defines an unambiguous extension shape.
- Do not auto-upgrade Anthropic TTL to `1h`.
- Do not synthesize OpenAI `prompt_cache_key` from Anthropic `cache_control`, `cache_control.ttl`, `max_tokens: 0`, message text, metadata, conversation IDs, request IDs, or similar request content.
- Do not copy an OpenAI `prompt_cache_key` value into another provider field as if the semantics were identical. In future reviewed synthesis, the key can be treated only as evidence of cache intent.
- Do not support Google/Gemini `extra_body.google.cached_content` in the default OpenAI-compatible path. It is a provider-specific extension that would reintroduce native Gemini resource lifecycle scope.

Future Anthropic explicit block-marker shape:

- Current main does not implement OpenAI-shaped block-marker mapping. Known OpenAI content/tool `cache_control` markers fail closed rather than being silently dropped or partially preserved.
- A future reviewed implementation may accept `cache_control` only on content parts that translate to Anthropic cacheable blocks.
- Eligible target blocks under Anthropic's current docs: tool definitions, system text blocks, user/assistant text blocks, user image/document blocks, assistant `tool_use`, and user `tool_result`.
- For OpenAI-shaped tool calls and tool results, require an explicit extension shape that maps unambiguously to the produced Anthropic top-level block. Do not infer a marker from a surrounding assistant message when the generated `tool_use` block is only one of several blocks.
- Ineligible target blocks: thinking/redacted thinking blocks when marked directly, citation/sub-content children, empty text blocks, and any block whose target provider docs do not allow direct `cache_control`.
- Unsupported or ambiguous markers must fail closed rather than being silently dropped when the caller explicitly requested provider prompt caching.

Target-provider matrix:

| Target provider protocol | Current provider cache mechanism | What `llmup` should do |
| --- | --- | --- |
| OpenAI Chat / Responses | Automatic prompt caching; optional `prompt_cache_key` and `prompt_cache_retention` | Preserve explicit OpenAI fields. In future reviewed synthesis, synthesize `prompt_cache_key` only from deterministic cache group / tenant-route / stable-prefix fingerprint inputs. Do not set `24h` unless explicit in the request or future reviewed implementation contract. |
| Anthropic Messages | `cache_control` top-level automatic caching or per-block breakpoints | Keep explicit Anthropic fields unchanged. In future reviewed synthesis, choose an implementation-owned strategy: top-level automatic caching for growing full-history conversations, or reviewed breakpoints at stable prefix boundaries such as tools/system/docs. |
| Google Gemini through OpenAI-compatible upstream | OpenAI Chat wire protocol plus Google-specific optional extensions | Treat as OpenAI Chat for this plan. Do not support `extra_body.google.cached_content` or native Gemini cache resources in pre-GA. |

OpenAI `prompt_cache_key` synthesis:

- Source priority for future reviewed synthesis: explicit OpenAI extension field, reviewed cache group source, authenticated tenant + route + model alias, stable-prefix hash when selected by the implementation review.
- Stable-prefix hash, when implemented, should include only translated tool definitions, system/developer instructions, and static leading context before the first dynamic user turn. It must be a hash, never raw prompt text.
- The key must be bounded and deterministic, for example `llmup:v1:{namespace}:{alias}:{cache_group_or_hash}`.
- The trace should record `provider_native_prompt_cache.openai.key_source`.
- The request-control synthesis should not synthesize `prompt_cache_retention: "24h"` from Anthropic `ttl: "1h"`; those controls have different retention and billing semantics.

Anthropic `cache_control` synthesis:

- Future reviewed synthesis must choose between top-level automatic caching and implementation-owned block-level breakpoints.
- Top-level request `cache_control` is appropriate for growing full-history conversations where the whole preceding transcript is expected to repeat and advance.
- Top-level request `cache_control` is not a safe universal default for one-shot requests with stable system/tools and varying final user text, because automatic caching writes only at the chosen breakpoint and can miss forever if the breakpoint includes a changing suffix.
- Reviewed block-level breakpoints are preferred for stable system prompts, tool definitions, examples, large documents, or repository context. The placement should come from explicit request markers or a reviewed implementation contract, not from prose classification.
- Default TTL should be provider default / 5-minute `ephemeral`.
- `ttl: "1h"` requires explicit user input or a future reviewed implementation contract because write cost is higher.
- Future block-level marker synthesis should use simple reviewed injection points such as `tools`, `system[last]`, `message[index]`, or `content_part[index]`, with a maximum of four Anthropic breakpoints and explicit rejection when TTL ordering would be invalid.

Google Gemini through OpenAI-compatible upstream:

- Treat the upstream as OpenAI Chat wire protocol for active pre-GA work.
- Do not synthesize, translate, or test `extra_body.google.cached_content` in this plan, even though Google documents the extension for OpenAI-compatible clients. Internal raw same-protocol forwarding may carry unknown OpenAI-compatible fields as bytes, but `llmup` should not claim provider-cache support for them.
- If future demand proves the economics justify it, run a separate scope review for a Google-specific OpenAI-compatible extension. Do not treat it as reserved work in this plan.

### Source To Target Coverage Matrix

This matrix is the handoff checklist for every active pre-GA protocol pair after native Gemini removal.

| Source client format | Target upstream format | Provider cache behavior |
| --- | --- | --- |
| OpenAI Chat | OpenAI Chat | Internal raw same-protocol forwarding preserves `prompt_cache_key`, `prompt_cache_retention`, automatic prompt caching, and raw usage. Future reviewed same-format synthesis would use the internal prompt-cache disposition on a constructed target-provider request, not raw same-protocol forwarding. |
| OpenAI Chat | OpenAI Responses | Preserve OpenAI prompt-cache controls during OpenAI-family translation. Do not alter retention spelling. Preserve cache usage mapping in the client response. |
| OpenAI Chat | Anthropic Messages | Delivered explicit slice: map `extra_body.anthropic.cache_control` to top-level `cache_control`. Known OpenAI block/tool `cache_control` markers fail closed; they are not preserved as Anthropic block-level markers. Future reviewed synthesis must use an implementation-owned Anthropic strategy: top-level for full-history conversations, or reviewed breakpoints for stable tools/system/docs. Do not infer block breakpoints from prose. |
| OpenAI Responses | OpenAI Chat | Preserve OpenAI prompt-cache controls during OpenAI-family translation. Do not use Responses `store` / `previous_response_id` / `conversation` as cache controls. |
| OpenAI Responses | OpenAI Responses | Internal raw same-protocol forwarding preserves `prompt_cache_key`, `prompt_cache_retention`, automatic prompt caching, and raw usage. Future reviewed same-format synthesis would use the internal prompt-cache disposition on a constructed target-provider request, not raw same-protocol forwarding. |
| OpenAI Responses | Anthropic Messages | Same as OpenAI Chat -> Anthropic, after Responses input is converted to the Messages pivot. Keep visible summaries/history stable, but do not translate OpenAI state controls into cache controls. |
| Anthropic Messages | OpenAI Chat | Delivered explicit slice: map `extra_body.openai.prompt_cache_key` and optional `prompt_cache_retention` to OpenAI Chat top-level fields. Warn/drop Anthropic `cache_control`; never copy raw prompt text, Anthropic TTL, `max_tokens: 0`, metadata, conversation IDs, or request IDs into the key. |
| Anthropic Messages | OpenAI Responses | Same as Anthropic -> OpenAI Chat, with OpenAI Responses `prompt_cache_key` and `prompt_cache_retention` as target fields. Do not map Anthropic `max_tokens: 0` prewarm into Responses state. |
| Anthropic Messages | Anthropic Messages | Internal raw same-protocol forwarding preserves top-level and block-level `cache_control`, TTL, `max_tokens: 0` prewarm, thinking cache behavior, and raw usage. |

Native Gemini rows are intentionally absent. `format: google`, `format: gemini`, and `/google/v1beta/*` are owned by the removal plan and must not receive new cache request-control work.

### Request-Control Synthesis Invariants

- Run request-control synthesis after any conversation-state expansion and after translation has produced the target-provider request shape, because cache hits depend on the bytes/structure the target provider sees.
- Preserve target prompt prefix order: tools, system/developer instructions, static media/document context, then dynamic user content. Avoid reordering tool definitions or schema keys during optimization.
- Never use natural-language classification to decide that content is stable. Use only reviewed cache group sources, route identity, tenant/session identifiers, and deterministic structural prefix fingerprints.
- Do not place timestamps, request IDs, random trace IDs, short-lived user text, or provider credentials inside synthesized cache keys.
- Emit trace fields for every synthesized or dropped cache control: target provider, disposition, field, key source, TTL/retention source, and reason.
- Current main has no synthesis path, so there is nothing to disable. Any future reviewed synthesis must include an implementation-level off path before it ships.

### Deferred Routing-Affinity Follow-Up

Narrow routing affinity is a deferred follow-up for deployments that already have multiple equivalent upstream choices. It may help preserve provider cache warmth, but it is not a hot-cache product behavior, not a response cache, and not part of the current prompt-cache request-control implementation.

Future-review guardrails:

- Route priority: explicit upstream/model choice > state-bridge continuation owner > native provider state owner > future routing-affinity sticky routing.
- Sticky key components: tenant/auth boundary, namespace, model alias, upstream provider, credential/project/region, target model, provider prompt-cache key or stable-prefix fingerprint.
- Never override a hard provider choice or explicit upstream order.
- If a warm provider is unavailable and failover is allowed, annotate the trace with `cache_warm_provider_unavailable`.
- Do not let sticky routing create a semantic response cache. The provider still generates every response.
- For OpenAI, sticky routing complements `prompt_cache_key`; for Anthropic, it matters when multiple workspaces/regions/providers can satisfy the same route.
- Conversation-state bridge continuations must keep their originally resolved route/upstream unless an explicit route policy says failover is allowed. Future routing-affinity work must not scatter a replay chain.

### Prewarm And Threshold Diagnostics

Provider prompt-cache economics depend on minimum prefix sizes and write/read timing. The request-control synthesis should expose diagnostics without silently adding expensive work:

- OpenAI: prompt caching is automatic above the provider threshold; `prompt_cache_key` only helps route similar prefixes. The request-control synthesis should record when it synthesizes a key and let `cached_tokens` prove effectiveness.
- Anthropic: prompts below model-specific minimum token thresholds silently receive no cache benefit. Future reviewed request-control synthesis may add top-level `cache_control`, but should surface `cache_creation_input_tokens == 0 && cache_read_input_tokens == 0` as a possible "not cached" diagnostic. A future optional `count_tokens` preflight may warn before sending, but should not be default because it adds latency and another upstream call.
- Anthropic prewarm: `max_tokens: 0` is useful only for explicit prewarm flows and has official restrictions. Cross-protocol translated prewarm should require an explicit Anthropic extension or future reviewed implementation contract; it should not be inferred from an ordinary OpenAI request.

### Provider Notes

OpenAI:

- Prompt caching is automatic for cacheable prompts on recent models.
- Preserve `prompt_cache_key` and `prompt_cache_retention` on OpenAI raw same-protocol forwarding.
- Track `usage.prompt_tokens_details.cached_tokens` and Responses `usage.input_tokens_details.cached_tokens`.
- Current official docs document `prompt_cache_retention` values `in_memory` and `24h`. Most models default to `in_memory`; `gpt-5.5`, `gpt-5.5-pro`, and future models default to `24h` and do not support `in_memory`.
- Request-control synthesis must not synthesize `24h` unless a future reviewed implementation contract explicitly includes that behavior. If the provider defaults a model to `24h`, that is provider-native behavior and should remain visible in usage/data-retention docs rather than being hidden by `llmup`.

Anthropic:

- Preserve top-level and block-level `cache_control`.
- Top-level `cache_control` is the preferred low-complexity translated-path target only for routes that represent growing full-history conversations. For one-shot requests with a stable prefix and dynamic suffix, explicit breakpoints at the last stable block are the safer cost-control default.
- Anthropic's OpenAI SDK compatibility surface is not the same as native Messages prompt caching; `llmup` translated support should target Anthropic Messages requests, not assume Anthropic's OpenAI-compatible endpoint can honor every cache control.
- Preserve `ttl: "1h"` only as user-supplied provider-native input; do not auto-upgrade TTLs because 1-hour writes cost more.
- Preserve `max_tokens: 0` prewarm requests in Anthropic raw same-protocol forwarding. Translation behavior must not inject minimum max-token defaults into this path.
- Track `cache_creation_input_tokens`, `cache_read_input_tokens`, and `cache_creation` subfields.
- Anthropic currently supports prompt caching on all active Claude models, with model-specific minimum token thresholds, a 20-block lookback window per breakpoint, and up to four breakpoints. It supports automatic top-level caching on Claude API, Claude Platform on AWS, and Microsoft Foundry; Bedrock and Vertex Claude routes require explicit block-level breakpoints.
- Tool definitions, text blocks, user image/document blocks, assistant tool-use blocks, and user tool-result blocks are cacheable. Thinking blocks cannot be directly marked, although previous thinking can be cached as part of a larger prefix.

## Development Plan

Current-main delivery status:

- Delivered: Phase 0/1 request-processing contract and observability, Phase 2/3 raw same-protocol forwarding for eligible non-mutating paths, and Phase 5 provider cache usage observation.
- Delivered slice: Responses stateful-control detector enabled-semantics alignment for `background` / `store`; `background:false|null` and `store:false|null` no longer trigger provider-owned stateful fail-closed, while enabled/present controls still fail closed.
- Delivered slice: OpenAI-family -> Anthropic explicit extension mapping for `extra_body.anthropic.cache_control` to top-level Anthropic `cache_control`, with fail-closed validation and no `llmup` cache.
- Delivered slice: Anthropic -> OpenAI-family explicit target-provider extension mapping for `extra_body.openai.prompt_cache_key` and optional `prompt_cache_retention`, with fail-closed validation and no `llmup` cache.
- Delivered dependency: Conversation State Bridge route/config owner hardening is complete. Continuations re-check the current runtime/internal fingerprint before upstream dispatch and fail closed on drift; this fingerprint is not a product feature or user configuration.
- Pending: Remaining prompt-cache translated support beyond the delivered explicit top-level extensions, plus any future reviewed synthesis. Do not add a policy/config surface for synthesized request controls.
- Guardrail: raw same-protocol forwarding remains an internal request-processing fact. It must not be documented or handed off as a product behavior.

### Phase 0: Freeze The Contract

Deliverables:

- Add the internal raw same-protocol forwarding contract tests before changing behavior.
- Update docs to say same-format raw forwarding is byte-preserving provider forwarding plus explicit proxy behavior.
- Mark current same-format mutation tests as expected-to-change.

Acceptance:

- Developers can point to one document that explains the internal boundary between raw same-protocol forwarding and request construction/translation under maximum safe compatibility.
- No new product feature is introduced in this phase.

### Phase 1: Introduce Request Processing Observability Plumbing

Deliverables:

- Add `RequestProcessing`, including `RequestTransformationNotRequired` and `RequestTransformationRequired`, plus state/cache observation fields.
- Route discovery returns both upstream format and request processing classification.
- Debug traces, metrics, and hooks include the request processing classification and provider-native request-control/state fields.
- Keep behavior unchanged while request processing classification is observable.

Acceptance:

- Unit tests prove same-format routes select `RequestTransformationNotRequired` unless model alias rewriting or a configured shim requires mutation or response normalization.
- Cross-format routes select `RequestTransformationRequired`.

### Phase 2: Raw Same-Protocol Request Forwarding

Deliverables:

- Preserve raw request body bytes through routing.
- Split the request representation into `raw_bytes` plus a parsed `serde_json::Value` used only for routing/boundary decisions. The internal raw same-protocol forwarding path sends `raw_bytes`; constructed, cache-synthesized, and state-expanded paths use the parsed/mutable JSON.
- In `RequestTransformationNotRequired`, skip `translate_request_with_policy()`, role repair, translation defaults, MiniMax overrides, and body-level model rewrite.
- Replace body-mutating safety checks with narrow ingress checks that reject proxy-private structured artifacts without reserializing the body.
- Ensure forced streaming does not insert `stream` in the internal raw same-protocol forwarding path.

Acceptance tests:

- Golden upstream request bodies match client request bytes for OpenAI Chat, OpenAI Responses, and Anthropic Messages.
- Golden bodies include field order, whitespace, numeric formatting, unknown provider fields, and provider error bodies where relevant.
- Native cache fields remain byte-identical in the internal raw same-protocol forwarding path.
- Anthropic `max_tokens: 0` prewarm passes through unchanged.
- Alias routes that need `model` body rewrite are classified as `RequestTransformationRequired`, not raw same-protocol forwarding.

### Phase 3: Raw Same-Protocol Response And SSE Forwarding

Deliverables:

- In `RequestTransformationNotRequired`, forward upstream status, content type, selected safe response headers, and raw body bytes without JSON parse/translate/reserialize.
- Forward provider error bodies unchanged.
- Add a raw SSE forwarding path for internal raw same-protocol forwarding streams. Keep chunking transport-flexible, but preserve event bytes and event order.
- Move redaction to trace/log/hook storage. Do not redact client-visible raw same-protocol forwarding output.

Acceptance tests:

- Non-stream success and error response bodies match upstream bytes.
- SSE tests preserve `event`, `data`, `id`, `retry`, comments, blank lines, terminal events, and provider usage frames.
- No raw same-protocol forwarding response calls `translate_response_with_context()`.

### Phase 4: Provider-Native Prompt-Cache Request Controls

Deliverables:

- Add the Provider-Native Prompt-Cache Request-Control IR.
- Add internal prompt-cache disposition tracking after effective route/model resolution.
- Delivered: explicit OpenAI-shaped to Anthropic support for `extra_body.anthropic.cache_control` -> top-level Anthropic `cache_control`.
- Delivered: explicit Anthropic-shaped to OpenAI-family support for `extra_body.openai.prompt_cache_key` / `prompt_cache_retention` -> OpenAI-family top-level fields.
- Keep OpenAI-shaped content/tool block-level `cache_control` unsupported in current main; known markers fail closed until a future reviewed extension shape is defined.
- Keep any future reviewed Anthropic synthesis behind an implementation-owned strategy: top-level automatic caching for full-history routes or reviewed breakpoints for stable-prefix routes.
- Keep any future reviewed OpenAI `prompt_cache_key` synthesis behind deterministic inputs when no explicit cache key is present.
- Continue warning/dropping low-risk non-portable provider cache controls when the target provider cannot honor them.
- Keep translation marker-free unless an explicit extension is present or a future reviewed synthesis implementation is active.
- Add trace/debug fields that show internal disposition, target provider, synthesized fields, and key source.

Acceptance tests:

- Existing OpenAI-to-Anthropic requests still do not receive `cache_control` by default.
- `extra_body.anthropic.cache_control` maps exactly to Anthropic top-level `cache_control`.
- Invalid or conflicting Anthropic cache-control extension shapes fail closed before upstream.
- `extra_body.openai.prompt_cache_key` maps exactly to OpenAI-family top-level `prompt_cache_key`, and `prompt_cache_retention` maps only when explicitly supplied as `in_memory` or `24h`.
- Invalid `extra_body.openai` shapes fail closed before upstream.
- Explicit OpenAI content/tool `cache_control` markers fail closed in current main; they are not preserved as Anthropic block-level markers.
- Anthropic `cache_control` translated to OpenAI still warns/drops under the explicit-support disposition rather than becoming a synthetic OpenAI cache control.
- Anthropic-to-OpenAI future reviewed synthesis uses a bounded `prompt_cache_key` with the reviewed source and never copies raw prompt text into the key.
- OpenAI-to-Anthropic future reviewed synthesis applies the reviewed Anthropic strategy and does not add block-level markers unless the implementation contract explicitly allows them.
- `prompt_cache_retention: "24h"` and Anthropic `ttl: "1h"` are never synthesized from each other without a reviewed implementation contract.
- `extra_body.google.cached_content` fails closed in translated paths when treated as an explicit cache request; it is not mapped to a native Gemini field.

### Deferred Follow-Up: Narrow Routing-Affinity Guardrails

This is not a current delivery phase. Review it later only for deployments that already have multiple equivalent upstreams for the same model. It must not block raw same-protocol forwarding or basic provider prompt-cache request support.

Future-review deliverables:

- Add narrow sticky routing only for routes that already have multiple equivalent upstream choices, keyed by provider prompt-cache policy, tenant/auth boundary, namespace, model alias, upstream provider, credential/project/region, target model, and provider prompt-cache key or stable-prefix fingerprint.
- Keep hard provider overrides and explicit upstream order authoritative.
- Annotate failover from a warm route with `cache_warm_provider_unavailable`.
- Keep metrics separate from gateway response cache metrics.

Future-review tests:

- Equivalent cacheable requests route to the same upstream when multiple equivalent upstreams exist and routing affinity is explicitly enabled.
- Explicit provider order disables sticky rerouting.
- State-bridge replay chains keep their originally resolved upstream unless explicit failover policy allows otherwise.
- Failover emits a cache-warmth diagnostic and does not reuse response bodies.

### Phase 5: Provider Cache Usage Observation Only

Deliverables:

- Add an internal `ProviderCacheUsage` observation struct populated from raw provider usage after the response is already decided. This struct is telemetry only and must not drive cache lookup, cache keys, eviction, or response reuse.
- Preserve raw usage in raw same-protocol forwarding responses.
- Emit best-effort metrics:
  - `cache.read_tokens`
  - `cache.write_tokens`
  - `cache.hit_tokens`
  - `cache.provider`
  - `cache.source_field`
- Add docs that these metrics are approximate and provider-specific.

Acceptance tests:

- OpenAI cached tokens are observed without changing response body.
- Anthropic read/write counters are observed without changing response body.
- Unknown usage shapes do not fail requests.

### Phase 6: Remove Same-Format Compatibility Shims

Deliverables:

- Move same-format role repair, translation defaults, MiniMax overrides, and model rewrite tests into the maximum-safe path or delete them.
- Rename any raw-forwarding tests that actually assert request mutation.
- Update protocol docs and matrix to reflect raw same-protocol forwarding as the intended internal request-processing result when the route avoids mutation and normalization.

Acceptance:

- Test names and docs no longer describe a mutating request path as raw same-protocol forwarding.
- All same-format raw same-protocol forwarding tests pass without hidden exceptions.

## Test Matrix

Required local tests:

| Area | Required coverage |
| --- | --- |
| Request payload | Byte-for-byte upstream body equality for OpenAI Chat, OpenAI Responses, and Anthropic Messages |
| Response payload | Byte-for-byte downstream body equality for success and provider error responses |
| Streaming | Raw SSE event preservation for ordinary success streams |
| Headers | Auth rewrite and hop-by-hop stripping are explicit; provider protocol headers are preserved where safe |
| Provider prompt-cache support | Native cache request fields and usage fields preserved; explicit translated extensions map only to their target provider; any future synthesis is deterministic, trace-visible, and provider-scoped; optional observation does not mutate output and does not cache anything |
| Provider cache matrix | Every source/target pair in the Source To Target Coverage Matrix has at least one positive test and one non-portable/fail-closed test where applicable |
| Request processing | Same-format routes that avoid mutation and normalization use internal raw same-protocol forwarding; cross-format or shimmed routes use maximum-safe request construction |
| Regressions | Maximum-safe cross-format behavior remains in the maximum-safe path |

Routing-affinity tests are intentionally absent from the required local matrix. They belong to the deferred follow-up review above.

## Handoff Tasks

Recommended next order:

1. Remaining prompt-cache translated/block-level review next: build on the delivered top-level explicit extensions and state-bridge owner hardening.
2. Tool/custom tool replay later: keep function/custom tool replay behind the remaining prompt-cache translated/block-level review.
3. Stream capture later: keep streaming response capture behind tool/custom tool replay.
4. Keep any future synthesis behind a later policy review with trace-visible deterministic inputs and no user/operator mode.

Deferred: review narrow routing-affinity only after basic provider prompt-cache request controls are complete.

Primary code areas:

- [src/discovery.rs](../../src/discovery.rs)
- [src/config.rs](../../src/config.rs)
- [src/server/proxy.rs](../../src/server/proxy.rs)
- [src/server/headers.rs](../../src/server/headers.rs)
- [src/streaming/stream.rs](../../src/streaming/stream.rs)
- [src/translate/internal.rs](../../src/translate/internal.rs)
- [src/translate/internal/tests/mod.rs](../../src/translate/internal/tests/mod.rs)
- [tests/integration_test.rs](../../tests/integration_test.rs)

## Explicitly Out Of Scope

- Gateway response cache.
- Semantic cache.
- Any `llmup` cache store, cache lookup, cache eviction, response-reuse cache key, or cache lifecycle manager.
- Universal cache TTL or cache key schema.
- Automatic provider cache marker insertion without explicit extension input, reviewed implementation contract, trace, and implementation-level off path.
- Provider-owned state reconstruction for OpenAI Responses or Anthropic thinking/tool state.
- Google/Gemini native cache resource lifecycle management, including `cachedContent`, `cachedContents/*`, `thoughtSignature`, and `extra_body.google.cached_content`.
- Broad fallback, retry, load-balancing, budget, pricing, model catalog, virtual-key, guardrail, prompt-management, or eval features.
- Making `llmup` a LiteLLM/Portkey/OpenRouter-style universal API product.

## Reference Material

Provider official references:

- OpenAI prompt caching: <https://developers.openai.com/api/docs/guides/prompt-caching>
- OpenAI Responses create reference: <https://platform.openai.com/docs/api-reference/responses/create>
- OpenAI Chat Completions create reference: <https://platform.openai.com/docs/api-reference/chat/create>
- Anthropic prompt caching: <https://platform.claude.com/docs/en/build-with-claude/prompt-caching>
- Anthropic OpenAI SDK compatibility: <https://platform.claude.com/docs/en/api/openai-sdk>
- Google Gemini OpenAI compatibility, for migration context only: <https://ai.google.dev/gemini-api/docs/openai>

Comparable gateway references:

- LiteLLM OpenAI forwarding endpoint docs: <https://docs.litellm.ai/docs/>
- LiteLLM prompt caching: <https://docs.litellm.ai/docs/completion/prompt_caching>
- LiteLLM auto-inject prompt caching checkpoints: <https://docs.litellm.ai/docs/tutorials/prompt_caching>
- LiteLLM proxy caching: <https://docs.litellm.ai/docs/proxy/caching>
- Cloudflare AI Gateway provider-native endpoints: <https://developers.cloudflare.com/ai-gateway/usage/providers/>
- Cloudflare AI Gateway caching: <https://developers.cloudflare.com/ai-gateway/features/caching/>
- OpenRouter prompt caching: <https://openrouter.ai/docs/features/prompt-caching>
- OpenRouter response caching: <https://openrouter.ai/docs/guides/features/response-caching>
- Helicone AI Gateway overview: <https://docs.helicone.ai/gateway/overview>
- Helicone provider prompt caching: <https://docs.helicone.ai/gateway/concepts/prompt-caching>
- Helicone LLM caching: <https://docs.helicone.ai/features/advanced-usage/caching>
- Portkey Anthropic prompt caching: <https://portkey.ai/docs/integrations/llms/anthropic/prompt-caching>
- Portkey Bedrock prompt caching: <https://portkey.ai/docs/virtual_key_old/integrations/llms/bedrock/prompt-caching>
- Portkey Messages API provider-native cache note: <https://portkey.ai/docs/product/ai-gateway/messages-api>
- Portkey AI Gateway docs: <https://portkey.ai/docs/product/ai-gateway>
- Vercel AI Gateway provider options: <https://vercel.com/docs/ai-gateway/provider-options>
- Vercel AI Gateway OpenAI-compatible advanced prompt caching: <https://vercel.com/docs/ai-gateway/sdks-and-apis/openai-compat/advanced>
- Envoy AI Gateway prompt caching: <https://aigateway.envoyproxy.io/docs/capabilities/llm-integrations/prompt-caching/>
