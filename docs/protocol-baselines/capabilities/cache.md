# Cache Capability Notes

- Layer: capability-diff
- Status: active
- Last refreshed: 2026-05-18
- Scope: prompt caching, cache handles, cache accounting, and non-portable cache semantics

## Summary

All major providers now expose some form of cache-aware generation, but the contract is different in each case:

| Provider | Mental model |
| --- | --- |
| OpenAI | Automatic prompt caching with optional routing hints like `prompt_cache_key` and retention policy controls |
| Anthropic | Cache breakpoints over prompt prefixes using `cache_control`, with read/write token accounting |

Google OpenAI-compatible Gemini is handled as the OpenAI Chat wire protocol in
the active proxy surface. Native Gemini cache behavior is retained only in the
retired historical Google Gemini baseline; it is not an active proxy capability.

`llmup` does not implement response/result cache, semantic cache, provider cache
resource management, or a cross-provider cache abstraction. Its active behavior
is provider prompt-cache request-control handling: pass through native fields
where the target can honor them, explicitly map supported target-provider fields,
or fail closed / drop with a portability warning when semantics are unknown or
provider-owned state cannot be reconstructed.

## Provider comparison

| Dimension | OpenAI Responses / Chat | Anthropic Messages | Proxy guidance |
| --- | --- | --- | --- |
| How caching is enabled | Automatic on cacheable prompts; official docs expose `prompt_cache_key` and retention controls | Explicit `cache_control`, available both as a top-level automatic control and at block level | Do not flatten these into one synthetic cross-provider feature flag. |
| What is being referenced | A cacheable prompt prefix, not a reusable named resource | A cacheable prefix breakpoint inside tools/system/messages | Provider cache handles do not map across protocol families. |
| Lifetime model | Provider-managed retention policy | 5-minute default TTL, optional 1-hour TTL in docs | TTL cannot be normalized safely across providers. |
| Usage fields | `cached_tokens` under prompt/input token details | Separate `cache_creation_input_tokens` and `cache_read_input_tokens` | Preserve raw counters where available and describe normalized values as approximate. |
| Relation to persistence | Separate from `store` / object retention | Separate from message history replay | Never treat cache presence as durable conversation state. |

## High-risk misunderstandings

| Misunderstanding | Correction |
| --- | --- |
| "Anthropic cache reads are the same as OpenAI cached token counts." | Anthropic splits cache writes and reads; OpenAI collapses the prompt cache view differently. |
| "`store` means prompt caching." | It does not. Storage, retrieval, and caching are separate features. |

## Implementation stance

1. Preserve cache knobs through same-wire native preservation only when no body mutation or response normalization is required and the same protocol can preserve native semantics.
2. During translation, treat provider prompt-cache support as target-provider request-control pass-through / explicit target-provider request-control mapping / drop behavior, not as `llmup` caching. OpenAI cache keys and Anthropic breakpoints have different billing and lifetime effects.
3. OpenAI-family `prompt_cache_key` may be synthesized only from a controlled, canonical stable static prefix after the target request shape is known. Do not synthesize retention, provider-owned state, Anthropic breakpoints, or keys from natural-language meaning, dynamic user text, request IDs, credentials, `previous_response_id`, `conversation`, or `resp_llmup_*`.
4. Normalize cache usage for reporting, but keep provider-native fields available when the client understands them.
5. Document each cache warn-and-omit behavior explicitly, especially when omitting Anthropic `cache_control` or unknown provider-side cache/state handles.
6. Prompt-cache synthesis and mapping must be trace-visible without leaking sensitive values: traces may expose disposition, target fields, reasons, and redacted/fingerprinted synthesized-key metadata, not the full key or prompt text.
7. Provider-cache auto-injection is out of scope; explicit mapping must be trace-visible. The only current non-explicit exception is the controlled OpenAI-family stable-prefix `prompt_cache_key` synthesis described above.
