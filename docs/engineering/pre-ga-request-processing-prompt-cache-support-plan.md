# Pre-GA Request Processing and Provider Prompt-Cache Support Plan

- Status: current-main status update; internal raw same-protocol forwarding is present; coarse-grained provider-native prompt-cache request-control disposition remains visible in traces/hooks as `preserved_native` / `explicit_extension_mapped` / `dropped`; debug trace request entries additionally emit fine-grained `request.prompt_cache_request_control` details for existing preserve/map/drop behavior; OpenAI-family -> Anthropic top-level `extra_body.anthropic.cache_control` mapping and Anthropic -> OpenAI-family `extra_body.openai.prompt_cache_key` / `prompt_cache_retention` mapping are present; same-protocol wrong-target explicit extensions fail closed; usage hooks emit optional same-protocol zero-transform/native-preserved `ProviderCacheUsage` source-field telemetry for known OpenAI Chat, OpenAI Responses, and Anthropic cache usage fields; Conversation State Bridge supports visible reasoning summary replay and first-response streaming completed visible output capture, while `stream:true` + `previous_response_id` still fails closed
- Date: 2026-05-17
- Scope: internal request processing classification, internal raw same-protocol provider forwarding optimization, provider-native prompt-cache request-control support, provider-returned cache usage observation, and request-handling simplification under the single maximum safe compatibility goal
- Non-scope: any `llmup`-managed cache, gateway response/result cache, provider cache resource/lifecycle management, semantic cache, cache storage, persistence, Conversations API emulation, local retrieval, cache-aware routing, broad fallback DSLs, pricing catalogs, guardrails, prompt management, admin UI expansion

## Plan Coordination

This plan assumes [pre-ga-remove-native-gemini-format-plan.md](./pre-ga-remove-native-gemini-format-plan.md) is the owning decision for native Gemini removal. Current main has already removed active native Gemini runtime support: `UpstreamFormat` is Anthropic / OpenAI Chat / OpenAI Responses only, `google` / `gemini` config fails closed with a migration hint, and there is no active `/google/v1beta/*` route.

Active pre-GA protocol families for this plan are therefore:

- OpenAI Chat Completions
- OpenAI Responses
- Anthropic Messages

Gemini remains usable only as a provider brand behind an OpenAI-compatible upstream. That path is handled as OpenAI Chat wire protocol. This plan must not add Gemini `generateContent`, `cachedContent`, `cachedContents/*`, `thoughtSignature`, or `extra_body.google.cached_content` work. Any future Google-specific OpenAI-compatible extension requires separate scope review; it is not part of this pre-GA prompt-cache hardening.

## Goal

Make the pre-GA behavior easy to reason about:

- `llmup` has one product goal: maximum safe compatibility. It preserves portable semantics where possible, emits explicit warnings when supported translation omits non-portable detail, and fails closed when semantics cannot be preserved safely.
- Maximum safe compatibility is the implementation goal for every route.
- `RequestTransformationNotRequired` and `RequestTransformationRequired` are internal request-processing classifications, not product behavior.
- Observability records whether a request needs construction, protocol conversion, or enhancement. Same-protocol requests that do not need mutation use raw provider forwarding as an internal request-processing optimization under the same goal.
- Provider-native prompt-cache request controls are currently preserved as original payload on supported native paths. Delivered translated slices map OpenAI-family `extra_body.anthropic.cache_control` to Anthropic top-level `cache_control`, and Anthropic-shaped `extra_body.openai.prompt_cache_key` / `prompt_cache_retention` to OpenAI-family target top-level fields. Current main exposes the coarse-grained disposition in debug traces/hooks, emits fine-grained debug trace `request.prompt_cache_request_control` details inside the request object, and fails closed on same-protocol explicit extensions aimed at the wrong provider. Hooks and external `llmup` observability keep only the coarse `provider_prompt_cache_request_control` value. No additional translated cache-control expansion is in current deliverables; any expansion is a non-current separate scope review item.
- The proxy does not cache responses/results, prompts, embeddings, tokens, KV state, or provider cache resources.
- The proxy does not invent a cross-provider cache abstraction.

Same-format means the provider-facing wire protocol, not the provider brand. An OpenAI-compatible upstream can use the internal raw same-protocol forwarding path for OpenAI-shaped requests when the route does not require body mutation or response normalization. If a compatible upstream needs provider shims, model body rewrites, or response normalization, the request is constructed or translated under the same maximum safe compatibility goal.

Raw same-protocol forwarding is an internal request-processing optimization under maximum safe compatibility.

This is still a pre-GA plan for the prompt-cache disposition pieces. Raw same-protocol forwarding itself must remain an internal request-processing optimization.

## Design Principles

1. Same-format requests that are eligible for raw provider forwarding must not be normalized, repaired, reserialized, or translated.
2. Translation and target-provider request construction must be explicit and maximum-safe. Provider/protocol shims and any provider-native request-control mapping that mutates the body require `RequestTransformationRequired`.
3. Cache is provider-owned. OpenAI `prompt_cache_key` / `prompt_cache_retention` and Anthropic `cache_control` are provider prompt-cache request mechanisms with different semantics, billing, retention, and lifecycle rules.
4. Preserve prefixes. Cache savings depend on stable prompt prefixes, message order, tool order, schemas, media detail, and provider-specific cache handles.
5. Keep the mental model small. Request observability should answer one question: "does this request need construction, conversion, or enhancement?"
6. Stop scope creep early. Do not add `llmup` cache storage, universal cache controls, response caching, semantic caching, cache-aware routing, fallback routing languages, or synthetic provider state while adding request-processing and prompt-cache request-control support.
7. Cache observation must not affect upstream selection. Provider-returned cache usage is telemetry only.

## Current Findings

### Local Code Audit

Current main has the request-processing split in place. `RequestTransformationNotRequired` paths preserve the raw request body and raw upstream response body when the same-protocol route does not require body mutation or response normalization. Routes that need model body rewrite, forced streaming mutation, same-protocol repair/shims, state expansion, or cross-protocol conversion are classified as `RequestTransformationRequired` and keep using the maximum-safe construction path.

Current request-processing facts:

- [src/request_processing.rs](../../src/request_processing.rs) owns `RequestProcessing`, `StateBridgeModifier`, and `PromptCacheRequestControl`.
- [src/server/proxy.rs](../../src/server/proxy.rs) uses raw request bytes and raw non-stream response bytes on `RequestTransformationNotRequired`.
- Provider-native prompt-cache fields are recorded as preserved-native behavior for OpenAI-family `prompt_cache_key` / `prompt_cache_retention` and same-protocol Anthropic `cache_control`.
- `NormalizedUsage` provides baseline provider-returned usage observation, including known OpenAI cached-token and Anthropic cache read/write counters where already parsed. Usage hooks also emit optional `ProviderCacheUsage` source-field telemetry for known OpenAI Chat, OpenAI Responses, and Anthropic prompt-cache usage fields only on same-protocol zero-transform/native-preserved paths.
- Cross-protocol translated routes and same-format constructed routes currently omit `provider_cache_usage` even when the client-visible response contains cache usage counters. This avoids misattributing client-visible usage as raw provider source telemetry until upstream raw usage is separately observed by hooks.
- Current main includes the OpenAI-family -> Anthropic top-level `extra_body.anthropic.cache_control` explicit mapping and the Anthropic -> OpenAI-family top-level `extra_body.openai.prompt_cache_key` / `prompt_cache_retention` explicit mapping. Other translated cache-control marker mapping is not implemented.
- Current main includes debug trace request-level `request.prompt_cache_request_control` details for existing provider prompt-cache preserve/map/drop behavior. These details use field paths and reasons, not prompt text or prompt-cache key values. Hook payloads keep only the coarse `llmup.provider_prompt_cache_request_control` value and the existing `provider_cache_usage` rules.

### External Product Patterns

Comparable gateways expose separate provider-native surfaces, gateway controls, or cache products. These are contrast examples only; `llmup` must not copy them into its request model, route configuration, or user-facing behavior.

- LiteLLM has OpenAI-native integrations and separate OpenAI forwarding endpoints for newer or less-supported OpenAI endpoints.
- Cloudflare AI Gateway offers provider-native endpoints where a user replaces the provider base URL, and separately offers exact response caching.
- OpenRouter distinguishes provider prompt caching from its own response cache and also exposes provider routing controls. Those routing controls are contrast material only and are not a direction for `llmup` prompt-cache support.
- Helicone distinguishes provider-level prompt caching from Helicone response caching, while its AI Gateway route is a broader OpenAI-compatible translation layer.
- Portkey exposes OpenAI compliance controls because provider-native fields can be lost when normalized into a single schema.
- Vercel AI Gateway namespaces gateway behavior in `providerOptions.gateway`, including `caching: 'auto'`, instead of pretending provider cache controls are the same.
- Envoy AI Gateway exposes a provider-agnostic `cache_control` field, but scopes it to Anthropic-compatible targets where it can be translated into native Anthropic / Vertex Claude / Bedrock Claude controls.

The common lesson for `llmup`: keep one maximum safe compatibility goal, and classify only whether a request needs construction, conversion, or enhancement. Provider prompt-cache request fields should remain provider-native request controls, not `llmup` cache. Any cache usage observation must be read-only telemetry over provider-returned usage fields, not a `llmup` cache implementation.

## Target Architecture

Current main has an explicit request processing observation:

External `llmup` observability projection:

```rust
struct RequestProcessingObservability {
    request_body_handling: RequestBodyHandling, // client_body_preserved | constructed
    local_state_handling: Option<StateBridgeModifier>, // omitted | capture_candidate | expanded
    provider_prompt_cache_request_control: PromptCacheRequestControl, // none | preserved_native | explicit_extension_mapped | dropped
}
```

This plan owns the internal request processing classification and provider-native prompt-cache request-control support. [pre-ga-conversation-state-bridge-plan.md](./pre-ga-conversation-state-bridge-plan.md) owns the local-state handling field. This avoids inventing separate product behaviors for combinations such as "state replay + translation + prompt-cache request controls". When local state replay is active, execution order must be:

1. Conversation state expansion.
2. Source -> target protocol translation.
3. Explicit provider-native prompt-cache request-control mapping, if the target request supports it.
4. Upstream request.

Request processing classification:

- `RequestTransformationNotRequired`: internal classification for a request where client protocol equals upstream wire protocol and no configured route feature requires body mutation, provider-native request-control mapping, or response normalization.
- `RequestTransformationRequired`: internal classification for a request where protocols differ, or the selected route needs a provider/protocol shim, target-provider request construction, provider-native request-control mapping, model alias body rewrite, provider-specific role repair, translation default injection, format conversion, or error-shape conversion. This still follows the single maximum safe compatibility goal.

The route decision should be visible in debug traces and metrics through `llmup.request_body_handling`, `llmup.local_state_handling` when capture/replay is active, and `llmup.provider_prompt_cache_request_control`. Debug trace request entries may also include `request.prompt_cache_request_control` fine-grained details for provider prompt-cache request controls. Hook payloads and external `llmup` objects keep the coarse `provider_prompt_cache_request_control` projection; `provider_cache_usage` emission rules are unchanged.

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
- On the internal raw same-protocol forwarding path, do not add protocol defaults such as `anthropic-version` unless the route is explicitly configured to do so; otherwise a client testing a provider's native failure behavior will not see it.

### Translation Contract

The existing translation machinery should remain available, but only in `RequestTransformationRequired`.

Maximum-safe path responsibilities:

- Convert request and response schemas across OpenAI Chat Completions, OpenAI Responses, and Anthropic Messages.
- Apply surface gates, maximum-safe shims, warnings, and fail-closed portability boundaries.
- Translate portable tool calls, media, stop reasons, usage, and streaming lifecycle events.
- Fail closed on provider-owned lifecycle state and non-portable cache state that cannot be represented safely.

### Provider-Native Prompt-Cache Request Controls

This field keeps provider-native request-control disposition explicit without making it a separate product behavior or `llmup` cache. In the current `preserved_native` disposition, the client's native cache fields are carried unchanged. Explicit translated mappings work on a constructed target-provider request, so raw provider forwarding is not active on those requests.

Allowed behavior:

- Start from the target-provider request after routing and any required translation or request construction.
- Add only provider-native prompt-cache request controls from explicit extension mapping.
- Preserve all other request and response semantics as close to native as possible.
- Emit trace metadata for mapped or omitted fields.

Disallowed behavior:

- General role repair, schema repair, model body rewrite, response normalization, or cross-protocol error shaping.
- Calling the result raw same-protocol forwarding.

## Provider Prompt-Cache Support Strategy

### Stance

`llmup` should support provider prompt-cache mechanisms so operators can use LLMs more economically. It should not implement cache storage or cache lookup itself.

There are two different activities:

- Provider prompt-cache request-control support: `llmup` may preserve or explicitly map provider-native request controls that ask the selected upstream to use its own prompt-cache mechanism.
- `llmup` caching: `llmup` stores, indexes, evicts, or serves cached data itself.

This plan includes the first activity and excludes the second. An OpenAI `prompt_cache_key` or Anthropic `cache_control` remains a request to the provider; it is not a `llmup` cache.

Keep provider-native prompt-cache request-control facts explicit for the delivered preserve/map/drop behavior. Current main emits fine-grained debug trace request details as `request.prompt_cache_request_control`; hook payloads and external `llmup` observability intentionally remain coarse. Treat these fields as delivered trace vocabulary, not a new prompt-cache IR. Broader mapping or shared-IR work is a non-current separate scope review item.

Tracked facts for the delivered behavior:

- `target_provider`
- `disposition`: `none | preserved_native | explicit_extension_mapped | dropped`
- `source`: `native | explicit_extension`
- `openai.prompt_cache_key`
- `openai.prompt_cache_retention`
- `anthropic.cache_control`
- `anthropic.cache_control_location`: `top_level | block_level`
- `anthropic.ttl`
- `skipped_reason`

Do:

- Preserve provider-native prompt-cache request fields in the internal raw same-protocol forwarding path.
- Preserve known provider cache usage counters in client-visible native responses.
- Optionally observe provider-returned cache usage counters for metrics and debug traces.
- Keep translated-path support narrow: forward or map only explicit provider-native extension fields that are documented and intentionally supported.
- In translated paths, map provider-supported caching controls only when explicit provider-native intent is present in the request shape.
- Keep mapped or omitted controls visible in debug traces and portability warnings so users can see exactly what `llmup` changed.
- Fail closed when a provider cache handle contains non-reconstructable context and the target provider cannot honor it.
- Run all explicit mapping after the target-provider request has been built, because provider cache matching uses the exact target-side prompt prefix, order, and parameters.

Do not:

- Add any `llmup` cache store, cache lookup, cache eviction, cache key, or cache lifecycle manager.
- Add gateway response cache in pre-GA.
- Add semantic cache.
- Add a cross-provider `cache: true` request parameter.
- Treat OpenAI `prompt_cache_key` and Anthropic `cache_control` as direct semantic equivalents.
- Auto-insert provider cache controls.
- Infer stable/static content from message text meaning.
- Add Google/Gemini-specific cache extensions, including `cachedContent`, `cached_content`, `cachedContents/*`, or `extra_body.google.cached_content`, in this plan.

### Provider-Native Prompt-Cache Request-Control Trace Vocabulary

Prompt-cache trace vocabulary is computed internally from the target request shape and explicit provider-native fields. It describes the delivered preserve/map/omit behavior and does not create a prompt-cache IR:

- `preserved_native`: keep provider-native fields unchanged on same-protocol or OpenAI-family paths where the target can honor them.
- `explicit_extension_mapped`: map a documented explicit provider-native extension only after the target request shape is known.
- `dropped`: trace value for a portability omission; emit a portability warning and omit provider-native cache controls when the target cannot honor them.

Explicit provider-native support is an internal implementation detail under the same maximum safe compatibility goal. This plan does not define automatic prompt-cache key or breakpoint insertion.

### Current Functionality

Already present:

- OpenAI Chat/Responses `prompt_cache_key` and `prompt_cache_retention` are treated as OpenAI-native controls and are preserved on OpenAI-family targets when supported.
- OpenAI-family requests translated to Anthropic can map explicit `extra_body.anthropic.cache_control` to Anthropic top-level `cache_control`, with fail-closed validation and no `llmup` cache.
- Anthropic-shaped requests translated to OpenAI Chat/Responses can map explicit `extra_body.openai.prompt_cache_key` and optional `prompt_cache_retention` to OpenAI-family top-level fields, with fail-closed validation and no `llmup` cache.
- Anthropic `cache_control` is detected as non-portable when translating away from Anthropic and is omitted with a portability warning instead of being mapped to unrelated provider controls.
- OpenAI Chat/Responses to Anthropic does not inject `cache_control` by default; [tests/integration_test.rs](../../tests/integration_test.rs) has a regression test for concurrent OpenAI-to-Anthropic requests that asserts no marker injection.
- Known OpenAI content/tool `cache_control` markers are rejected fail-closed before any Anthropic per-block marker is emitted.
- Anthropic to OpenAI translation strips `cache_control` and does not create OpenAI `prompt_cache_key`.
- Active native Gemini cache-handle handling has been removed. Remaining Gemini references must be migration/retired/historical notes or Gemini-as-OpenAI-compatible examples.
- `NormalizedUsage` already provides baseline provider-returned usage observation, including known OpenAI cached-token and Anthropic cache read/write counters where already parsed.
- Eligible same-protocol routes can use raw provider forwarding, preserving provider-native cache fields and raw usage bytes as an internal request-processing optimization.

Known gaps / non-current scope:

- Debug trace request metadata for existing provider prompt-cache preserve/map/drop behavior is delivered as `request.prompt_cache_request_control`. Hook payloads and external `llmup` observability remain coarse by design.
- Remaining translated provider-native request-control mapping beyond the delivered top-level explicit extensions is intentionally not in current deliverables.
- OpenAI-shaped requests routed to Anthropic have only the explicit top-level `extra_body.anthropic.cache_control` mapping.
- OpenAI-shaped content parts and tool structures routed to Anthropic do not preserve explicit per-block Anthropic `cache_control` extension fields; known attempts fail closed.
- Anthropic-shaped requests routed to OpenAI can use only the explicit target-provider `extra_body.openai.prompt_cache_key` extension.
- Broader mapping or shared-IR work is a non-current separate scope review item and must not add automatic controls, `llmup` cache behavior, config, or mode.
- Streaming raw-forwarding coverage should be kept honest in tests, but it is not a prompt-cache handoff expansion.

### Translated Prompt-Cache Rules

Provider-native prompt-cache request-control support currently uses explicit support:

- `explicit support`: the proxy only forwards or maps documented explicit provider-native extension fields.

The proxy may help express a provider-native cache request only when cache optimization intent is explicit in the request payload. This plan does not add automatic cache key, breakpoint, or route selection behavior.

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
- OpenAI Chat <-> OpenAI Responses: preserve OpenAI prompt-cache controls across OpenAI-family translation without changing retention spelling.

Disallowed translated support:

- Do not read an arbitrary prompt and decide which blocks are "static" from natural-language meaning.
- Do not add Anthropic block-level breakpoints based on content length, role, first message, last system message, or perceived repetition.
- Do not preserve or map OpenAI content/tool `cache_control` markers in current main; known markers fail closed.
- Do not auto-upgrade Anthropic TTL to `1h`.
- Do not derive OpenAI `prompt_cache_key` from Anthropic `cache_control`, `cache_control.ttl`, `max_tokens: 0`, message text, metadata, conversation IDs, request IDs, or similar request content.
- Do not copy an OpenAI `prompt_cache_key` value into another provider field as if the semantics were identical. If the target provider cannot honor a supplied control, emit a portability warning and omit it.
- Do not support Google/Gemini `extra_body.google.cached_content` in the default OpenAI-compatible path. It is a provider-specific extension that would reintroduce native Gemini resource lifecycle scope.

Out-of-scope marker notes:

- Current main does not implement OpenAI-shaped block-marker mapping. Known OpenAI content/tool `cache_control` markers fail closed rather than being silently omitted or partially preserved.
- Additional translated cache-control mapping is not a current deliverable. Any expansion must start with a separate scope review and must not add automatic markers or `llmup` cache behavior.
- Unsupported or ambiguous markers fail closed when the caller explicitly requested provider prompt caching.

Provider-specific request-control notes:

- OpenAI Chat / Responses: preserve explicit `prompt_cache_key` and `prompt_cache_retention` on OpenAI-family targets. Do not generate a new key and do not change retention spelling.
- Anthropic Messages: preserve explicit top-level and supported block-level `cache_control` fields on Anthropic targets. Do not add breakpoints unless an explicit supported extension requested them.
- Google Gemini through OpenAI-compatible upstream: treat the upstream as OpenAI Chat wire protocol for this plan. Do not support `extra_body.google.cached_content` or native Gemini cache resources in pre-GA.

Google Gemini through OpenAI-compatible upstream:

- Treat the upstream as OpenAI Chat wire protocol for active pre-GA work.
- Do not translate or test `extra_body.google.cached_content` in this plan, even though Google documents the extension for OpenAI-compatible clients. Internal raw same-protocol forwarding may carry unknown OpenAI-compatible fields as bytes, but `llmup` should not claim provider-cache support for them.
- If future demand proves the economics justify it, run a separate scope review for a Google-specific OpenAI-compatible extension. Do not treat it as reserved work in this plan.

### Explicit Provider-Native Coverage Checklist

This checklist records the explicit provider-native request controls and usage fields for every active pre-GA protocol pair after native Gemini removal.

| Source client format | Target upstream format | Provider cache behavior |
| --- | --- | --- |
| OpenAI Chat | OpenAI Chat | Internal raw same-protocol forwarding preserves `prompt_cache_key`, `prompt_cache_retention`, automatic prompt caching, and raw usage. |
| OpenAI Chat | OpenAI Responses | Preserve OpenAI prompt-cache controls during OpenAI-family translation. Do not alter retention spelling. Preserve cache usage mapping in the client response. |
| OpenAI Chat | Anthropic Messages | Delivered explicit slice: map `extra_body.anthropic.cache_control` to top-level `cache_control`. Known OpenAI block/tool `cache_control` markers fail closed; they are not preserved as Anthropic block-level markers. Do not infer block breakpoints from prose. |
| OpenAI Responses | OpenAI Chat | Preserve OpenAI prompt-cache controls during OpenAI-family translation. Do not use Responses `store` / `previous_response_id` / `conversation` as cache controls. |
| OpenAI Responses | OpenAI Responses | Internal raw same-protocol forwarding preserves `prompt_cache_key`, `prompt_cache_retention`, automatic prompt caching, and raw usage. |
| OpenAI Responses | Anthropic Messages | Same as OpenAI Chat -> Anthropic, after Responses input is converted to the Messages pivot. Keep visible summaries/history stable, but do not translate OpenAI state controls into cache controls. |
| Anthropic Messages | OpenAI Chat | Delivered explicit slice: map `extra_body.openai.prompt_cache_key` and optional `prompt_cache_retention` to OpenAI Chat top-level fields. Omit Anthropic `cache_control` with a portability warning; never copy raw prompt text, Anthropic TTL, `max_tokens: 0`, metadata, conversation IDs, or request IDs into the key. |
| Anthropic Messages | OpenAI Responses | Same as Anthropic -> OpenAI Chat, with OpenAI Responses `prompt_cache_key` and `prompt_cache_retention` as target fields. Do not map Anthropic `max_tokens: 0` prewarm into Responses state. |
| Anthropic Messages | Anthropic Messages | Internal raw same-protocol forwarding preserves top-level and block-level `cache_control`, TTL, `max_tokens: 0` prewarm, thinking cache behavior, and raw usage. |

Native Gemini rows are intentionally absent. `format: google`, `format: gemini`, and `/google/v1beta/*` are owned by the removal plan and must not receive new cache request-control work.

### Explicit Request-Control Mapping Invariants

- Run explicit request-control mapping after any conversation-state expansion and after translation has produced the target-provider request shape, because provider cache matching uses the exact prompt prefix, order, and parameters the target provider sees.
- Preserve target prompt prefix order: tools, system/developer instructions, static media/document context, then dynamic user content. Avoid reordering tool definitions or schema keys.
- Never use natural-language classification to decide that content is stable.
- Do not derive OpenAI `prompt_cache_key` from timestamps, request IDs, random trace IDs, short-lived user text, provider credentials, `previous_response_id`, `conversation`, or `resp_llmup_*`.
- Debug traces emit `request.prompt_cache_request_control` details for delivered preserve/map/drop behavior: target provider, disposition, field, explicit source, TTL/retention source where applicable, and reason. Hooks and external `llmup` observability stay coarse.
- Cache usage telemetry must not influence routing, fallback, or upstream selection.

### Usage And Threshold Diagnostics

Provider prompt-cache economics depend on minimum prefix sizes and write/read timing. The proxy should expose diagnostics without silently adding expensive work:

- OpenAI: prompt caching is automatic above the provider threshold. Preserve explicit `prompt_cache_key` and let `cached_tokens` prove effectiveness.
- Anthropic: prompts below model-specific minimum token thresholds silently receive no cache benefit. Surface `cache_creation_input_tokens == 0 && cache_read_input_tokens == 0` as a possible "not cached" diagnostic from provider-returned usage only.
- Anthropic prewarm: `max_tokens: 0` is useful only for explicit prewarm flows and has official restrictions. Cross-protocol translated prewarm requires an explicit Anthropic extension; it should not be inferred from an ordinary OpenAI request.

### Provider Notes

OpenAI:

- Prompt caching is automatic for cacheable prompts on recent models.
- Preserve `prompt_cache_key` and `prompt_cache_retention` on OpenAI raw same-protocol forwarding.
- Track `usage.prompt_tokens_details.cached_tokens` and Responses `usage.input_tokens_details.cached_tokens`.
- Current official docs document `prompt_cache_retention` values `in_memory` and `24h`. Most models default to `in_memory`; `gpt-5.5`, `gpt-5.5-pro`, and future models default to `24h` and do not support `in_memory`.
- `llmup` must not set `24h` unless the request explicitly supplied that provider-native control. If the provider defaults a model to `24h`, that is provider-native behavior and should remain visible in usage/data-retention docs rather than being hidden by `llmup`.

Anthropic:

- Preserve top-level and block-level `cache_control`.
- Top-level `cache_control` is allowed only when explicitly supplied through a supported provider-native request field.
- Anthropic's OpenAI SDK compatibility surface is not the same as native Messages prompt caching; `llmup` translated support should target Anthropic Messages requests, not assume Anthropic's OpenAI-compatible endpoint can honor every cache control.
- Preserve `ttl: "1h"` only as user-supplied provider-native input; do not auto-upgrade TTLs because 1-hour writes cost more.
- Preserve `max_tokens: 0` prewarm requests in Anthropic raw same-protocol forwarding. Translation behavior must not inject minimum max-token defaults into this path.
- Track `cache_creation_input_tokens`, `cache_read_input_tokens`, and `cache_creation` subfields.
- Anthropic currently supports prompt caching on all active Claude models, with model-specific minimum token thresholds, a 20-block lookback window per breakpoint, and up to four breakpoints. It supports automatic top-level caching on Claude API, Claude Platform on AWS, and Microsoft Foundry; Bedrock and Vertex Claude routes require explicit block-level breakpoints.
- Tool definitions, text blocks, user image/document blocks, assistant tool-use blocks, and user tool-result blocks are cacheable. Thinking blocks cannot be directly marked, although previous thinking can be cached as part of a larger prefix.

## Development Plan

Current-main delivery status:

- Delivered: Phase 0/1 request-processing contract and observability, Phase 2/3 raw same-protocol forwarding for eligible non-mutating paths, coarse-grained provider-native prompt-cache request-control disposition plus trace/hook visibility for `preserved_native` / `explicit_extension_mapped` / `dropped`, and same-protocol wrong-target explicit extension fail-closed.
- Delivered telemetry baseline: `NormalizedUsage` provides basic provider-returned usage observation, including known OpenAI cached-token and Anthropic cache read/write counters where already parsed.
- Delivered slice: usage hooks emit optional `provider_cache_usage` telemetry from same-protocol zero-transform/native-preserved raw observed provider usage source fields for OpenAI Chat, OpenAI Responses, and Anthropic, omitting the field entirely when no known cache usage source field is present.
- Delivered guardrail: cross-protocol translated routes and same-format constructed routes omit `provider_cache_usage` for now, even when the client-visible response has cache counters, because hooks do not yet receive a separate upstream raw usage object for attribution.
- Delivered slice: Responses stateful-control detector enabled-semantics alignment for `background` / `store`; `background:false|null` and `store:false|null` no longer trigger provider-owned stateful fail-closed, while enabled/present controls still fail closed.
- Delivered slice: shared stateful/prompt-cache detector cleanup; Responses stateful control order now has one source for request-processing, resource routing, and translation assessment, and provider prompt-cache coarse detection reuses the same read-only helpers without changing the external `provider_prompt_cache_request_control` values.
- Delivered slice: OpenAI-family -> Anthropic explicit extension mapping for `extra_body.anthropic.cache_control` to top-level Anthropic `cache_control`, with fail-closed validation and no `llmup` cache.
- Delivered slice: Anthropic -> OpenAI-family explicit target-provider extension mapping for `extra_body.openai.prompt_cache_key` and optional `prompt_cache_retention`, with fail-closed validation and no `llmup` cache.
- Delivered slice: prompt-cache trace/docs guardrails are in place. Debug trace request objects emit fine-grained `request.prompt_cache_request_control` details for existing provider prompt-cache preserve/map/drop behavior; hook payloads and external `llmup` observability remain coarse; `provider_cache_usage` emission rules are unchanged; the field mapping matrix documents explicit mapping, warn/omit, fail-closed, and not-`llmup`-cache guardrails.
- Delivered dependency: Conversation State Bridge route/config owner hardening is complete. Continuations re-check the current runtime/internal fingerprint before upstream dispatch and fail closed on drift; this fingerprint is not a product feature or user configuration.
- Delivered dependency: Conversation State Bridge now supports ordinary Responses `function_call` / `function_call_output` and portable `custom_tool_call` / `custom_tool_call_output` local replay for non-streaming translated continuation, visible reasoning summary replay, plus first-response streaming completed visible output capture. Only the first `stream:true` response can commit local replay state after the completed terminal event; later continuation still uses non-streaming replay, and `stream:true` + `previous_response_id` still fails closed.
- Pending/deferred: no prompt-cache request-control expansion is in current deliverables. Anything beyond the delivered explicit top-level extensions requires separate scope review. Do not add a policy/config surface for cache-aware routing or automatic provider cache controls.
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
- Split the request representation into `raw_bytes` plus a parsed `serde_json::Value` used only for routing/boundary decisions. The internal raw same-protocol forwarding path sends `raw_bytes`; constructed, explicit-cache-mapped, and state-expanded paths use the parsed/mutable JSON.
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

- Keep provider-native prompt-cache request-control facts explicit for delivered preserve/map/drop behavior; broader shared-IR work is not a current deliverable and requires non-current separate scope review.
- Emit the delivered trace vocabulary after effective route/model resolution.
- Delivered: explicit OpenAI-shaped to Anthropic support for `extra_body.anthropic.cache_control` -> top-level Anthropic `cache_control`.
- Delivered: explicit Anthropic-shaped to OpenAI-family support for `extra_body.openai.prompt_cache_key` / `prompt_cache_retention` -> OpenAI-family top-level fields.
- Keep OpenAI-shaped content/tool `cache_control` unsupported in current main; known markers fail closed.
- Do not add top-level Anthropic `cache_control` unless the request supplied an explicit supported Anthropic extension.
- Do not add OpenAI `prompt_cache_key` unless the request supplied an explicit supported OpenAI extension.
- Continue emitting portability warnings and omitting low-risk non-portable provider cache controls when the target provider cannot honor them.
- Keep translation marker-free unless an explicit extension is present.
- Delivered: debug trace request fields show the trace value, target provider, mapped/target fields, explicit source, optional TTL/retention source, and omit reason for existing provider prompt-cache request-control behavior.

Acceptance tests:

- Existing OpenAI-to-Anthropic requests still do not receive `cache_control` by default.
- `extra_body.anthropic.cache_control` maps exactly to Anthropic top-level `cache_control`.
- Invalid or conflicting Anthropic cache-control extension shapes fail closed before upstream.
- `extra_body.openai.prompt_cache_key` maps exactly to OpenAI-family top-level `prompt_cache_key`, and `prompt_cache_retention` maps only when explicitly supplied as `in_memory` or `24h`.
- Invalid `extra_body.openai` shapes fail closed before upstream.
- Explicit OpenAI content/tool `cache_control` markers fail closed in current main; they are not preserved as Anthropic block-level markers.
- Anthropic `cache_control` translated to OpenAI still emits a portability warning and is omitted under the explicit-support disposition rather than becoming an OpenAI cache control.
- No translated path derives `prompt_cache_key` from raw prompt text, Anthropic TTL, `max_tokens: 0`, metadata, conversation IDs, request IDs, or `resp_llmup_*`.
- OpenAI-to-Anthropic does not add block-level markers in current main.
- `prompt_cache_retention: "24h"` and Anthropic `ttl: "1h"` are never derived from each other.
- `extra_body.google.cached_content` fails closed in translated paths when treated as an explicit cache request; it is not mapped to a native Gemini field.

### Phase 5: Provider Cache Usage Observation Only

Current-main status:

- Delivered: preserve raw usage in raw same-protocol forwarding responses.
- Delivered: keep existing `NormalizedUsage` baseline observation for provider-returned usage, including known OpenAI cached-token and Anthropic cache read/write counters where already parsed.
- Delivered: usage hooks add an optional sibling `provider_cache_usage` populated from same-protocol zero-transform/native-preserved raw observed provider usage after the response is already decided. This telemetry must not drive cache lookup, cache keys, eviction, response reuse, routing, or fallback.
- Delivered: cross-protocol translated routes and same-format constructed routes do not emit `provider_cache_usage` yet. They may still expose cache counters through `usage`, but those are client-visible normalized/translated fields, not raw provider source telemetry.
- Delivered: emit best-effort source-field details in usage hook payloads:
  - `provider_cache_usage.read_tokens`
  - `provider_cache_usage.write_tokens`
  - `provider_cache_usage.hit_tokens`
  - `provider_cache_usage.provider`
  - `provider_cache_usage.source_fields[].source_field`
  - `provider_cache_usage.source_fields[].counters`
  - `provider_cache_usage.source_fields[].value`
- Deferred/non-blocking: add docs that these metrics are approximate and provider-specific.

Acceptance tests:

- Current baseline: provider usage is observed without changing response bodies where `NormalizedUsage` already parses it.
- Current same-protocol zero-transform/native-preserved telemetry: OpenAI cached tokens and Anthropic read/write counters are attributed to provider/source fields without changing response bodies.
- Current attribution guardrail: cross-protocol routes and same-format constructed routes omit `provider_cache_usage` until hooks receive upstream raw usage separately from client-visible usage.
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
| Provider prompt-cache support | Native cache request fields and usage fields preserved; explicit translated extensions map only to their target provider; optional same-protocol zero-transform/native-preserved observation does not mutate output, drive routing, or cache anything |
| Provider prompt-cache coverage | Delivered preserve/map/drop behavior is covered across the Explicit Provider-Native Coverage Checklist; broader mapping or shared IR expansion is not current deliverable work |
| Request processing | Same-format routes that avoid mutation and normalization use internal raw same-protocol forwarding; cross-format or shimmed routes use maximum-safe request construction |
| Regressions | Maximum-safe cross-format behavior remains in the maximum-safe path |

## Handoff 状态

Current status:

1. Delivered: shared detector / trace cleanup keeps state/cache detector metadata consistent without adding a product configuration surface.
2. Delivered: prompt-cache trace/docs guardrails. Debug trace request objects emit `request.prompt_cache_request_control` for delivered preserve/map/drop behavior; hook payloads and external `llmup` observability remain coarse.
3. No current prompt-cache handoff: broader IR/disposition refactors or additional mapping require separate scope review and must not add config, mode, automatic controls, or `llmup` cache behavior.
4. Streaming extensions later: any streaming continuation capture is a later State Bridge extension, not the current prompt-cache handoff item.

Guardrail: keep prompt-cache support limited to explicit provider-native request controls and read-only usage telemetry. Do not add response cache, provider cache management, semantic cache, persistence, Conversations API emulation, or local retrieval.

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
- Provider cache resource or lifecycle management.
- Semantic cache.
- Any `llmup` cache store, cache lookup, cache eviction, response-reuse cache key, or cache lifecycle manager.
- Persistence, Conversations API emulation, or local retrieval.
- Universal cache TTL or cache key schema.
- Automatic provider cache key, marker, or breakpoint insertion. Only provider-native controls explicitly supplied by the request may be preserved or mapped.
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
