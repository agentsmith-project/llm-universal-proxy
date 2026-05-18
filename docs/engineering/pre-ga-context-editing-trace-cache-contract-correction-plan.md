# Pre-GA Context Editing / Trace / Cache Contract 修正计划

- 状态：implementation handoff plan
- 日期：2026-05-18
- 范围：Anthropic context editing 语义修正、translation boundary 顺序修正、OpenAI-family prompt-cache hint 收敛、debug trace / CLI fixture / verifier 加固、协议文档合同对齐
- 批次原则：这是一个 pre-GA 直接修到目标状态的开发批次，不拆长期阶段；P0 是必须同批落地的正确性合同，P1 是同批优先完成的 trace/fixture 覆盖增强
- 非范围：任何 `llmup` cache、gateway response cache、semantic cache、cache-aware routing、provider resource/state lifecycle、用户可见兼容等级或模式开关

## 背景与目标

当前 main 已经把 Claude / Anthropic Messages -> OpenAI-family provider 作为最大兼容方向推进，但 context editing、prompt-cache hint、trace verifier 和 docs contract 之间还存在几处会在 pre-GA 后变成用户可见语义债的问题：

- Anthropic `context_management.edits` 中的 `clear_tool_uses_20250919` 与 `clear_thinking_20251015` 不能只做字段白名单或粗略 `keep` 清理；它们是 provider-side context editing hint，有触发阈值、模型默认值、prompt-cache interaction 和可见 history 边界。
- 当前 portability assessment 在 context edit 前运行，会先 fail closed 掉本可由 deterministic edit 消除的问题，也会让 translator 和 boundary decision 使用不同的事实来源。
- OpenAI-family `prompt_cache_key` synthesis 是 provider routing/cache-hit hint，不是 `llmup` cache；当前触发面应收窄到“存在非空、稳定、明显达到 provider cache 最小长度的 static prefix”时才合成，避免普通动态请求被误标成可缓存前缀。
- debug trace / real CLI verifier 目前还不足以防住重复 `request_id` 假通过、Responses tool-call 摘要不足、以及 Python long-horizon fixture 通过不可达代码或改写 `main.py` 假通过。
- docs/protocol matrix 中 “not synthesized” 与 controlled OpenAI-family synthesis 的表述冲突，需要统一为一个小而清晰的合同。

本批次目标：

1. 将 server 真实入口顺序固定为 `preflight schema validation -> conversation-state bridge expansion -> deterministic context edit preparation -> classify_request_processing / boundary on effective body -> translation`。
2. 让 server boundary、translator、debug trace 和 warning headers 消费同一个 prepared body 和同一份 edit outcome，禁止 `server/proxy.rs` 与 `translate/internal` 各自 preparation / assessment 分叉。
3. 对 Anthropic context editing 做最大兼容、低惊讶实现：能确定执行的才本地执行；不能复刻 provider 默认或 token threshold 的场景在完整可见 history 下 warning/omit，不提前 400；opaque/provider-owned state 继续 fail closed。
4. Anthropic `cache_control` 只能在 Anthropic target same-wire/native 或显式 Anthropic-native 映射中 preserve；cross-provider 时 drop+warning，不能派生 OpenAI key/retention。OpenAI `prompt_cache_key` 只作为 provider routing hint，且只在非空、稳定、明显达到缓存最小长度的 static prefix 下受控合成。
5. 加固 trace matrix、fixture 和 verifier，保证真实 CLI 报告不能靠重复 ID、弱摘要、不可达源码或入口点伪装通过。
6. 更新 docs contract，把禁止项锁死：Anthropic breakpoint synthesis、retention synthesis、provider resource/state synthesis、cache-aware routing、`llmup` cache 都不进入 pre-GA。

## 非目标 / Guardrails

- 不实现 `llmup` response cache、result cache、semantic cache、embedding cache、KV cache 保存、cache lookup、cache eviction、cache hit response replay。
- 不新增 route/cache affinity DSL、sticky routing、cache warmth routing、fallback routing 语言、用户可见兼容等级或模式开关。
- 不合成 Anthropic `cache_control` breakpoint，不自动插入 Anthropic prompt-cache marker。
- 不从 Anthropic `cache_control` 派生 OpenAI `prompt_cache_key`，也不从 OpenAI `prompt_cache_key` / `prompt_cache_retention` 派生 Anthropic `cache_control`。
- `llmup` 不设置、不合成 OpenAI `prompt_cache_retention`。省略该字段不等于关闭 provider prompt-cache retention；provider default retention 仍可能生效。`store:false` 只控制 OpenAI Responses application state，不等于关闭 prompt-cache retention。
- 不合成 OpenAI Responses `store:true`。Anthropic -> OpenAI Responses 的 stateless translated request 必须显式设置 `store:false`，除非请求本身是 OpenAI-native 显式 provider-state path 或 same-wire native preserve。
- 不合成 provider-owned conversation/container/resource/state、hosted prompt/resource、external `previous_response_id`、opaque thinking/reasoning carrier。
- 不引入 token-count endpoint 作为本批次依赖；没有精确 token 事实时，context edit 的 token threshold / `clear_at_least` 只能保守 no-op warning，不能猜测执行。
- 不做 LLM summarization、SDK compaction、memory tool integration 或 provider resource lifecycle。
- 不扩大 provider product surface；同类产品只作为边界参考，`llmup` 不复制 LiteLLM/OpenRouter/Cloudflare/Helicone/Portkey/Vercel/Envoy 的 gateway cache 产品。

## 已核验 Findings

| ID | 领域 | 已核验结论 | 风险 | 本批次修正 |
| --- | --- | --- | --- | --- |
| F1 | Anthropic `clear_tool_uses_20250919` | 官方字段包括 `trigger` / `clear_at_least` / `keep` / `exclude_tools` / `clear_tool_inputs`；当前只按 `keep` 立即清理旧 tool pair，拒绝 `trigger` / `clear_at_least`。 | 小上下文被过度清理；合法请求被本地 400；tool call/result 结构可能被破坏。 | schema 接受并校验全部字段；只有 trigger 可确定满足且 `clear_at_least` 可满足时才执行；否则完整可见 history 下 warning/omit；清理时保留 pair 结构和 call id。 |
| F2 | Anthropic `clear_thinking_20251015` | 默认 `keep` 是模型相关；当前硬编码 `keep=1` 不能等价 provider 语义。 | 错删 thinking，破坏 reasoning continuity 或 cache 行为。 | 只在显式 `keep` 下执行；缺省 `keep` 在完整可见 history 下 warning/omit 并保留内容；opaque/provider-owned state 继续 fail closed。 |
| F3 | Pipeline 顺序 | assessment 在 context edits 前运行，会先 fail closed 本应由 deterministic edit 消除的问题。 | 用户看到错误边界不稳定，translator 需要 workaround。 | 固定为 preflight -> preparation -> assessment on effective body -> translation；boundary 和 translator 共享 prepared body/outcome。 |
| F4 | Context editing + prompt caching | Anthropic context editing 与 prompt caching 的 interaction 随策略变化：tool result clearing 会 invalidate cached prompt prefixes；thinking clearing 保留 thinking 时 cache preserved，清理时从清理点 invalidated。 | 把两者误判为全局禁止会破坏合法 native cache control；跨 provider 硬转又会制造错误 cache 语义。 | Anthropic target same-wire/native 或显式 Anthropic-native `cache_control` preserve；cross-provider drop+warning；不合成 Anthropic breakpoint，不从 Anthropic cache 派生 OpenAI key/retention。 |
| F5 | OpenAI `prompt_cache_key` synthesis | 当前实现自洽但触发过宽；OpenAI prompt cache 需要 1024+ token cacheable prefix，`prompt_cache_key` 与 prefix hash 组合，是 provider routing/cache-hit hint。 | 普通动态请求被赋予不稳定 key；cache hit 预期混乱；trace 暴露面增大。 | 仅在存在非空稳定且明显达到最小缓存长度的 static prefix 时合成；无法确认时跳过；排除动态尾部、最后用户消息、trace/id/timestamp/credential、provider state、`resp_llmup_*`、原始 prompt 文本泄露和 Anthropic `cache_control`。 |
| F6 | CLI trace verifier | `require_response_for_each_request` 对重复 `request_id` 假通过。 | trace 矩阵漏掉请求无响应、stream 异常或请求折叠 bug。 | TDD 修复为拒绝重复 matching request `request_id`；或按 request entry 计数，推荐拒绝重复。 |
| F7 | Responses trace/fixture | Responses trace/fixture 弱于 Chat；缺少低敏 input/tool-call pair 摘要。 | Responses target 看似通过，但 function call/output 配对缺陷不可见。 | 增加 `upstream_summary.input_types` 与 function-call pair/call-id-present 摘要；fixture 断言成对，不锁动态 `call_id` 字面值。 |
| F8 | `python_source_and_output` verifier | 可被不可达 `return` 与改写 `main.py` 假通过。 | long-horizon coding fixture 不能证明目标函数行为。 | 加 `source.behavior_cases` 直接 import/call 目标函数；可选 `entrypoint.required_calls` 做 AST 级验证。 |
| F9 | Docs contract | “not synthesized” 与 controlled synthesis 矛盾。 | 文档和测试锁住错误合同，后续实现反复踩线。 | 改为“除 controlled non-empty stable-prefix-above-cache-minimum OpenAI-family `prompt_cache_key` synthesis 外不合成”，并锁禁止项。 |
| F10 | 同类产品参考 | LiteLLM/OpenRouter/Cloudflare/Helicone/Portkey/Vercel/Envoy 都区分 provider-native prompt cache、gateway cache、routing/unsupported params。 | 范围蔓延成重型网关。 | 文档明确 `llmup` 不复制 gateway cache 产品，只保留 provider-native request-control 支持和只读 usage telemetry。 |
| F11 | OpenAI Responses `store` | OpenAI Responses 默认或 `store:true` 会产生 30 day Application State retention；`store:false` 才是 stateless。 | Anthropic -> Responses translated request 如果省略 `store`，会意外要求 provider 应用状态保留。 | Stateless cross-provider Anthropic -> OpenAI Responses 显式设置 `store:false`；永不合成 `store:true`；trace/docs 分开说明 `store`、`prompt_cache_retention`、`previous_response_id`。 |
| F12 | Server boundary 入口 | 当前 `server/proxy.rs` 会在 translation 前用 `original_body` 做 classify/boundary reject。 | 本应由 preparation 消除或 warning/omit 的内容仍可能在 server boundary 先被拒绝；server 与 translator 形成双份判断。 | Conversation-state bridge 后、`classify_request_processing` / boundary 前生成唯一 `PreparedRequestBody`；server boundary、translation、trace 和 warning headers 共享同一 outcome。 |
| F13 | OpenAI prompt-cache retention default | 省略 `prompt_cache_retention` 不表示无 provider retention；多数模型默认 `in_memory`，`gpt-5.5` / `gpt-5.5-pro` 和未来模型默认 `24h` 且不支持 `in_memory`。 | 文档或 trace 可能误导用户以为 `llmup` 关闭了 prompt-cache retention，或把 `store:false` 当成 prompt-cache retention 控制。 | `llmup` 不设置/不合成 retention；omitted trace/docs 标为 provider default may apply；`store`、`prompt_cache_retention`、`previous_response_id` 分开说明。 |
| F14 | Anthropic context editing beta header | Anthropic context editing 需要 `context-management-2025-06-27` beta header 或 SDK `betas`。 | same-wire native context editing 如果丢 header 会失效；cross-provider 透出 Anthropic beta header 会污染 OpenAI-family upstream。 | Same-wire Anthropic native path preserve `anthropic-beta` / SDK betas；cross-provider local apply 不把 Anthropic beta header 转发到 OpenAI-family。 |

## 目标设计

### Context Edit Preparation

新增一个很小的 request-local preparation 层，不做新产品抽象。建议命名为 `PreparedRequestBody` / `ContextEditPreparation`，可放在 `src/translate/internal/context_editing.rs`，并由现有 `assessment` / translator 入口调用。

建议结构：

```rust
struct PreparedRequestBody {
    original_body: serde_json::Value,
    effective_body: serde_json::Value,
    context_edit_outcome: ContextEditOutcome,
    warnings: Vec<PortabilityWarning>,
}
```

合同：

1. `original_body` 只用于 trace、debug 和差异说明。
2. `effective_body` 是 assessment 和 translator 唯一消费的请求体。
3. `context_edit_outcome` 记录 `preserved_native` / `applied_local` / `warn_omitted` / `fail_closed`，但不作为用户可选行为。
4. preparation 只能做 deterministic edit；不得调用 LLM、不得摘要、不得导入 provider state。
5. same-wire Anthropic 且可交给 provider 原生执行时，可以 preserve `context_management` 和合法 Anthropic-native `cache_control`；context editing 与 prompt caching 的 cache invalidation 语义由 Anthropic provider 处理，`llmup` 不猜测重写。
6. cross-provider translation 时，`context_management` 不向 OpenAI-family 泄露；本地可确定执行的 edit 反映到 `effective_body`，不可确定但不影响完整可见 history 的 edit warning/omit。

### Server Boundary Integration

当前 `server/proxy.rs` 会在 translation 前基于 `original_body` 做 `classify_request_processing` 和 boundary reject；这是本批次必须修正的真实入口，不只是 translator 内部重构。

目标 server 流程：

1. 读取并解析 `original_body`，做基础 JSON / route preflight。
2. 如果 conversation-state bridge 命中并能展开 llmup-owned visible transcript，先生成 bridge-expanded body。
3. 在 bridge-expanded body 上生成一次 `PreparedRequestBody`。
4. `classify_request_processing`、provider-state boundary、portability assessment、translation、debug trace、hook summary 和 `x-llmup-portability-warning` headers 全部使用同一个 `prepared.effective_body` 和 `prepared.context_edit_outcome`。
5. Same-wire Anthropic native preserve/raw forward 不要求 mutation；当 request 能由 provider native 语义处理时，保持 raw forward / native preserve 是正确方向。Preparation 在这种场景只产生 `preserved_native` outcome 和低敏 trace，不重写 body。

实现约束：

- `translate/internal` 可以提供 preparation/assessment helper，但不能在 translator 内部再次从 `original_body` 做另一份 preparation。
- `server/proxy.rs` 不得先用 `original_body` boundary reject 再让 translator 使用 `effective_body`；所有 fail-closed 判断必须基于同一份 prepared outcome。
- Warning headers 来自 prepared outcome 与 assessment issues 的合并结果；不得由 server 和 translator 分别生成两套不一致 warning。
- Debug trace 必须能同时显示 `client_summary`、`effective_summary` 或等价低敏字段、`upstream_summary` 和 `context_edit` outcome，且不泄露被清理内容原文。

`clear_tool_uses_20250919` 支持规则：

- schema 接受 `type`、`trigger`、`clear_at_least`、`keep`、`exclude_tools`、`clear_tool_inputs`。
- `trigger` 默认按 Anthropic 语义理解为 provider-side `input_tokens` threshold；如果请求未显式提供 `trigger`，本地没有 token facts 时不得按默认 `keep` 立即清理，只能 warning/omit 并保留 visible history。
- 本地只有在可确定 trigger 满足时才执行。`trigger.type == "tool_uses"` 可用可见 transcript 精确计数；`trigger.type == "input_tokens"` 没有精确 token 事实时不执行，只 warning/omit。
- `clear_at_least` 需要精确可证明的清理量；即使 `trigger.tool_uses` 已满足，只要 `clear_at_least.input_tokens` 需要 token facts 且本地不可证明，也不得执行清理。
- `keep` 默认值只在 edit 真正执行时使用；未执行时不得按默认清理。
- `exclude_tools` 中的 tool name 永不清理。
- `clear_tool_inputs: false` 时只清理 tool result 内容，保留 assistant tool use 参数；`true` 时才清理 tool input。
- 清理时必须保留 Anthropic `tool_use.id`、paired `tool_result.tool_use_id`、OpenAI-family `call_id`、`function_call_output.call_id` 和消息顺序。只允许把 input/result payload 替换为稳定 placeholder、空对象或等价低敏结构；不能删除 pair、不能生成空消息、不能让后续 tool mapping 失配。
- 清理输出使用稳定 placeholder，不包含原 tool input/result 原文，不制造模型响应。

`clear_thinking_20251015` 支持规则：

- schema 接受 `keep: "all"` 或 `keep: { "type": "thinking_turns", "value": N }`，`N > 0`。
- 多 edit 时仍要求 `clear_thinking_20251015` 位于 `edits[0]`。
- 只有显式 `keep` 才本地执行。`keep: "all"` 是 no-op applied outcome；`thinking_turns` 保留最近 N 个含 thinking 的 assistant turn，其余 visible unsigned thinking 可清理。
- 缺省 `keep` 不硬编码。根据已核验结论，默认随模型变化；本批次不维护一份容易过期的模型默认表。在完整可见 history 下 warning/omit 并保留内容；遇到 opaque/signed/redacted/encrypted-only carrier 或 provider-owned state 继续 fail closed。
- 清理 signed/opaque thinking 时不得伪造 signature 或 encrypted carrier。若只有 opaque carrier 可理解历史，fail closed。

Context editing 与 Anthropic prompt caching：

- Anthropic target same-wire/native request 可以同时 preserve `context_management` 与合法 Anthropic `cache_control`；`llmup` 按原样保留 native cache_control，也不尝试模拟 Anthropic 的 cache invalidation 细节。
- OpenAI-family -> Anthropic 的显式 `extra_body.anthropic.cache_control` 仍是 Anthropic-native request control；目标是 Anthropic 且 schema 合法时可以映射/preserve。
- Anthropic -> OpenAI-family cross-provider translation 时，Anthropic `cache_control` 必须 drop+warning；不得从 block-level breakpoint、TTL、context edit outcome 或清理点派生 OpenAI `prompt_cache_key` / `prompt_cache_retention`。
- Tool result clearing 可能让 Anthropic cached prefix invalidated，thinking clearing 取决于是否保留 thinking；这些 provider cache effects 只应进入 warning/trace 说明，不应驱动本地 cache 或 routing。

### OpenAI Responses Store / Application State

OpenAI Responses 的 `store` 与 prompt cache 不是同一个概念，必须分开处理：

- Anthropic -> OpenAI Responses 的 stateless translated request 必须显式设置 `store:false`，避免触发 Responses 默认 Application State retention。
- 只有 OpenAI-native same-wire request 或用户显式选择的 provider-state path 才能 preserve 合法 `store:true` / omitted-store native behavior。
- `llmup` 永远不为 cross-provider request 合成 `store:true`，也不为了 prompt caching、trace continuity 或 `previous_response_id` 兼容而打开 store。
- Trace 和 docs 必须分开标注：`store` 是 Responses application state retention，`previous_response_id` 是 provider state continuation，`prompt_cache_retention` 是 prompt-cache retention control。三者不能互相替代；`store:false` 不关闭 provider prompt-cache retention。

### Anthropic Beta Header Handling

Anthropic context editing 是 provider-native beta surface，header/SDK betas 是 same-wire native 语义的一部分：

- Same-wire Anthropic native path 必须 preserve 合法 `anthropic-beta` header 或 SDK `betas` 中的 `context-management-2025-06-27`，让 provider 自己执行 context editing。
- Cross-provider local apply / warning-omit 时，不得把 Anthropic beta header 泄露到 OpenAI-family upstream。
- 不新增配置开关；header policy 跟随 request target protocol。Anthropic target preserve，OpenAI-family target strip Anthropic-only beta headers。

### Portability Assessment

目标顺序：

1. `preflight schema validation`：只验证请求 JSON、route 基础形状、显式 provider extension schema、明显非法字段类型。
2. `conversation-state bridge expansion`：只展开 `llmup` 自有 visible transcript；外部 provider state 仍 fail closed。
3. `deterministic context edit normalization/preparation`：在 bridge-expanded body 上生成 `effective_body` 和 edit outcome。
4. `classify_request_processing / portability assessment on effective body`：server boundary 只看 prepared outcome，决定 raw native preserve、constructed translation、`warn+drop`、`fallback-text` 或 `fail-closed`。
5. `translation`：只能消费同一份 `effective_body` 和 assessment outcome。

实现边界：

- assessment 不再因为原始 body 中可被 preparation 消除的旧 thinking/tool-result 内容提前 fail closed。
- `server/proxy.rs` 不得在 preparation 前基于 `original_body` 对可 deterministic edit 的内容做 boundary reject。
- provider-owned state/resource、opaque-only reasoning/thinking、未展开 `previous_response_id` / `conversation`、server-side tool resource 仍 fail closed。
- 如果 preparation warning/omit 了 context edit，assessment 必须基于未编辑但完整可见的 `effective_body` 判断；不能假装 edit 已经发生。
- debug trace 同时显示 `request.client_summary`、`request.upstream_summary` 和低敏 `request.context_edit`，但不输出被清理的原文。

### Prompt Cache Hint

本批次保留“无 `llmup` cache”的产品合同，只修正 provider hint。

OpenAI-family `prompt_cache_key` synthesis 收敛为：

- 仅目标是 OpenAI Chat Completions 或 OpenAI Responses 时考虑。
- 显式合法 `prompt_cache_key` 优先；存在显式 key 时不合成。
- 存在 `prompt_cache_retention` 时只校验并保留合法显式值；永远不因为合成 key 自动设置 retention。
- 存在 provider-owned state controls，例如 `previous_response_id`、`conversation`、hosted `prompt` 或 `resp_llmup_*` 展开边界不清时，不合成。
- 只有当 canonical target request 存在非空稳定 static prefix，且该 prefix 明显达到 OpenAI prompt cache 的最小 cacheable 长度时才合成。若本批次不引入 token endpoint，只能使用保守估算；估算不足或无法判断时不合成。
- 稳定 prefix 至少来自 system/developer instructions、工具定义/schema、结构化输出 schema 或稳定 static config；只有普通动态 user tail 时不合成。
- key material 必须排除动态尾部、最后用户消息、request/trace id、timestamp、random、credentials、conversation/container/resource id、`previous_response_id`、`resp_llmup_*`、Anthropic `cache_control`。
- key 和 trace 不得包含原始 prompt 文本。实现可在内存中计算稳定组件 fingerprint，但 debug trace、hook、fixture、error message 只能出现 redacted/fingerprinted 元数据。
- Anthropic `cache_control` 与 OpenAI `prompt_cache_key` / `prompt_cache_retention` 不互转。
- 当 `prompt_cache_retention` 被省略时，trace/docs 必须标为 `omitted_provider_default` 或等价说明：`llmup` 未设置 retention，但 provider default retention may apply。不得把 omitted 表述为 disabled。
- 合成 key 只表示 provider routing/cache-hit hint；cache hit 只能由 provider response usage 中的 cached token counters 证明，`llmup` trace 不能声称命中。

推荐 trace 字段：

- `disposition`: `none | preserved_native | explicit_extension_mapped | synthesized | dropped`
- `source`: `native | explicit_extension | controlled_stable_prefix`
- `synthesis_reason`: `non_empty_stable_prefix_above_cache_minimum`
- `key_fingerprint`: redacted hash only
- `skipped_reason`: `no_non_empty_static_prefix | stable_prefix_below_cache_minimum | cacheability_unknown | explicit_key_present | provider_state_present | target_not_openai_family | anthropic_cache_control_not_openai_hint`
- `hint_semantics`: `provider_routing_hint`
- `retention_control`: `explicit | omitted_provider_default`

### Trace Matrix Hardening

Responses target 的 trace 要达到 Chat 同等可验证性，但保持低敏：

- `request.upstream_summary.input_types`: 按顺序列出 Responses `input[].type`，例如 `message`、`function_call`、`function_call_output`、`reasoning`。
- `request.upstream_summary.tool_call_pairs`: 只记录 pair 级事实，不记录完整 args/output：
  - `function_call_count`
  - `function_call_output_count`
  - `call_id_present_count`
  - `paired_call_id_count`
  - `unpaired_function_call_count`
  - `unpaired_function_call_output_count`
  - optional `custom_tool_call_count`
- fixture 断言 `function_call` 与 `function_call_output` 成对、`call_id` 存在且可配对，但不锁动态 `call_id` 字面值。
- Chat fixture 继续断言 `tool_calls` / `role:"tool"`，Responses fixture 必须断言 `function_call` / `function_call_output`。

### Verifier Hardening

`trace_request_contract.require_response_for_each_request`：

- 推荐策略：当该 verifier 开启时，matching request entries 中 `request_id` 必须非空且唯一；重复 ID 直接失败。
- response matching 使用唯一 request id 集合；如果未来确实需要同 ID 多 request，应新增显式 entry index 机制，不在本批次扩大。

`python_source_and_output`：

- 在 `source` 下新增 `behavior_cases`：

```json
{
  "type": "python_source_and_output",
  "source": {
    "path": "calc.py",
    "behavior_cases": [
      {
        "import": "calc",
        "call": "add",
        "args": [2, 3],
        "expected": 5
      }
    ]
  },
  "entrypoint": {
    "path": "main.py",
    "required_calls": [
      { "module": "calc", "function": "add" }
    ]
  }
}
```

- `behavior_cases` 在 workspace 内隔离 import，直接调用目标函数，验证 return value 或 exception。
- `entrypoint.required_calls` 是可选 AST 验证，只检查入口点是否实际引用目标函数，不执行不可信入口代码。
- 不允许只靠源码包含某字符串或程序 stdout 通过；旧 fixture 可逐步补 behavior cases，但 P0 要先覆盖已知会被假通过的 Python bugfix fixture。

### Docs Contract Alignment

文档统一表述：

> 除 controlled non-empty stable-prefix-above-cache-minimum OpenAI-family `prompt_cache_key` synthesis 外，`llmup` 不合成 provider prompt-cache control。

需要同步的文档：

- `docs/protocol-compatibility-matrix.md`
- `docs/protocol-baselines/capabilities/cache.md`
- `docs/protocol-baselines/matrices/field-mapping-matrix.md`
- `docs/max-compat-design.md`
- `docs/engineering/pre-ga-request-processing-prompt-cache-support-plan.md`
- 相关 public README / PRD / DESIGN 中涉及 prompt cache 或 context editing 的合同段落

必须锁死的禁止项：

- Anthropic breakpoint synthesis
- OpenAI `prompt_cache_retention` synthesis
- Claiming omitted `prompt_cache_retention` disables provider default retention
- OpenAI Responses `store:true` synthesis
- provider resource/state synthesis
- Anthropic cache 与 OpenAI key/retention 硬转换
- cache-aware routing / sticky routing / provider-cache affinity
- `llmup` response cache、semantic cache、result cache、KV cache

## TDD 实施顺序

| 顺序 | 优先级 | 测试文件建议 | 测试名建议 | 先写断言 | 实现目标 |
| --- | --- | --- | --- | --- | --- |
| 1 | P0 | `src/server/tests/proxy.rs` | `server_prepares_context_edit_after_state_bridge_before_boundary` | bridge-expanded body 先生成 `PreparedRequestBody`，`classify_request_processing` / boundary 不再用 raw `original_body` 误拒绝。 | 修正真实 server boundary 入口。 |
| 2 | P0 | `src/server/tests/proxy.rs` | `server_boundary_translation_trace_and_warning_headers_share_prepared_body` | boundary、translation、debug trace、warning header 看到同一个 `effective_body` 和 outcome。 | 避免 server/proxy 与 translate/internal 双份判断。 |
| 3 | P0 | `src/server/tests/proxy.rs` | `same_wire_anthropic_context_management_and_cache_control_preserves_raw_native_body` | same-wire Anthropic native path 不 mutation，raw forward/preserve `context_management` 与合法 `cache_control`。 | 锁 same-wire native preserve 正确方向。 |
| 4 | P0 | `src/translate/internal/context_editing.rs` 或 `src/translate/internal/tests/context_editing.rs` | `clear_tool_uses_accepts_trigger_clear_at_least_keep_exclude_and_clear_inputs` | 合法 Anthropic edit schema 不再 400。 | schema validator 接受官方字段并给出 typed config。 |
| 5 | P0 | 同上 | `clear_tool_uses_without_trigger_warns_and_preserves_visible_history` | 只传 `{type: clear_tool_uses_20250919}` 时不按默认 `keep=3` 清理；有 warning，history 保留。 | 默认 trigger 是 provider-side token threshold，本地无 token facts 不猜测。 |
| 6 | P0 | 同上 | `clear_tool_uses_does_not_apply_when_trigger_cannot_be_proven` | `trigger.input_tokens` 且无 token facts 时 `effective_body == original_body`，有 warning。 | 阻止当前“只看 keep 立即清理”的过度清理。 |
| 7 | P0 | 同上 | `clear_tool_uses_trigger_satisfied_but_clear_at_least_input_tokens_unproven_warns_and_preserves` | tool_uses trigger 满足但 `clear_at_least.input_tokens` 不可证明时不清理。 | `clear_at_least` 不能靠估算执行。 |
| 8 | P0 | 同上 | `clear_tool_uses_applies_when_tool_use_trigger_is_satisfied_and_preserves_pairs` | 可见 tool_use 数超过 threshold 且 clear_at_least 可证明时才清旧内容，保留 call id / pair 结构。 | deterministic local edit 正确执行。 |
| 9 | P0 | 同上 | `clear_tool_uses_clear_tool_inputs_true_preserves_pair_ids_and_replaces_payloads` | `tool_use.id` / `tool_result.tool_use_id` / OpenAI `call_id` 成对保留；input/result payload 变 placeholder 或空对象；无空消息。 | 让 `clear_tool_inputs:true` 目标形状可执行。 |
| 10 | P0 | 同上 | `clear_tool_uses_respects_exclude_tools` | excluded tool 的 input/result 原样保留。 | 补齐官方字段语义。 |
| 11 | P0 | 同上 | `clear_thinking_without_explicit_keep_warns_and_preserves_visible_history` | 缺省 `keep` 不再按 `keep=1` 清理。 | 移除硬编码默认。 |
| 12 | P0 | 同上 | `clear_thinking_explicit_keep_applies_before_assessment` | 旧 thinking 被 preparation 清理后，assessment 不再基于原始 body fail closed。 | 建立 preparation -> assessment 顺序。 |
| 13 | P0 | `src/translate/internal/assessment.rs` tests | `assessment_uses_context_edit_effective_body_for_boundary_decision` | boundary issue 只看 `effective_body`。 | assessment 入口改为 prepared body。 |
| 14 | P0 | `src/translate/internal/openai_family.rs` / `src/translate/internal/openai_responses.rs` tests | `translator_consumes_same_prepared_body_as_assessment` | translated upstream 不含已清理内容和 `context_management`。 | translator 移除 workaround，消费 shared outcome。 |
| 15 | P0 | `src/translate/internal/context_editing.rs` tests | `context_management_with_cache_control_same_wire_preserves_native_controls` | Anthropic same-wire/native path 同时保留 `context_management` 与合法 `cache_control`。 | 锁 same-wire/native preserve 合同。 |
| 16 | P0 | `src/translate/internal/context_editing.rs` / `src/prompt_cache_controls.rs` tests | `anthropic_cache_control_cross_provider_drops_with_warning_and_does_not_derive_openai_controls` | Anthropic -> OpenAI-family translation warning/drop `cache_control`，不生成 `prompt_cache_key` / `prompt_cache_retention`。 | 锁跨 provider 不硬转换。 |
| 17 | P0 | `src/translate/internal/openai_responses.rs` tests | `anthropic_to_openai_responses_stateless_translation_sets_store_false` | Anthropic -> Responses translated request 显式 `store:false`。 | 避免默认 Application State retention。 |
| 18 | P0 | `src/translate/internal/openai_responses.rs` tests | `openai_native_responses_store_policy_is_preserved_only_on_native_path` | OpenAI-native same-wire 可 preserve 合法 `store`；cross-provider 永不合成 `store:true`。 | 分开 native state 与 translated stateless。 |
| 19 | P0 | `src/server/tests/headers.rs` / `src/server/tests/proxy.rs` | `same_wire_anthropic_context_editing_preserves_beta_header` | Anthropic target same-wire/native path preserve `anthropic-beta: context-management-2025-06-27` 或 SDK betas。 | 保证 provider-native context editing 可用。 |
| 20 | P0 | `src/server/tests/headers.rs` / `src/server/tests/proxy.rs` | `cross_provider_context_editing_does_not_forward_anthropic_beta_header_to_openai_family` | Anthropic -> OpenAI-family local apply / warning-omit 不转发 Anthropic beta header。 | 防止 provider-specific header 泄露。 |
| 21 | P0 | `src/prompt_cache_controls.rs` | `does_not_synthesize_prompt_cache_key_without_non_empty_static_prefix` | 只有动态 user message 不合成 key。 | 收窄 synthesis 触发面。 |
| 22 | P0 | `src/prompt_cache_controls.rs` | `does_not_synthesize_prompt_cache_key_when_static_prefix_below_cache_minimum_or_unknown` | prefix 估算低于 OpenAI cache minimum 或 cacheability unknown 时不合成，并记录 skipped reason。 | 避免把短 prefix 当 provider cache hint。 |
| 23 | P0 | `src/prompt_cache_controls.rs` / `src/debug_trace.rs` | `omitted_prompt_cache_retention_traces_provider_default_may_apply` | 未设置 `prompt_cache_retention` 时 trace 为 `omitted_provider_default`，不写 disabled。 | 防止把省略误解为关闭 provider retention。 |
| 24 | P0 | `src/prompt_cache_controls.rs` | `synthesized_prompt_cache_key_ignores_dynamic_tail_and_runtime_identity` | 最后用户消息、request id、timestamp、trace id、credential 改变不影响 key。 | 明确稳定输入边界。 |
| 25 | P0 | `src/prompt_cache_controls.rs` | `synthesized_prompt_cache_key_changes_when_static_prefix_changes` | system/developer/tools/schema 变化改变 key。 | 保留 stable prefix 区分能力。 |
| 26 | P0 | `src/prompt_cache_controls.rs` | `does_not_synthesize_prompt_cache_key_from_anthropic_cache_control_or_provider_state` | `cache_control`、`previous_response_id`、`conversation`、`resp_llmup_*` 不进入 synthesis。 | 禁止 cache/state 混转。 |
| 27 | P0 | `src/debug_trace.rs` | `synthesized_prompt_cache_trace_marks_routing_hint_and_omits_raw_prompt_text` | trace 只有 fingerprint/reason/hint semantics，无原文、完整 key 或 cache-hit 声明。 | 降低敏感信息暴露并避免误报 cache hit。 |
| 28 | P0 | `tests/test_real_cli_matrix.py` | `test_trace_request_contract_rejects_duplicate_request_id_when_response_required` | 两个 request 同 ID、一个 response 不得通过。 | 修复 `require_response_for_each_request` 假通过。 |
| 29 | P0 | `src/debug_trace.rs` | `responses_request_summary_includes_input_types_and_tool_call_pair_counts` | Responses upstream summary 暴露 input types 与 pair counts。 | Trace Matrix Hardening。 |
| 30 | P0 | `tests/test_real_cli_matrix.py` | `test_responses_trace_contract_requires_function_call_pairs_without_literal_call_id` | fixture 可断言配对但不锁 `call_id` 字符串。 | CLI verifier 支持动态 ID 断言。 |
| 31 | P0 | `tests/test_real_cli_matrix.py` | `test_python_source_and_output_behavior_cases_import_and_call_target_function` | 直接 import/call `calc.add(2,3)`。 | 新增 `behavior_cases`。 |
| 32 | P0 | `tests/test_real_cli_matrix.py` | `test_python_source_and_output_rejects_unreachable_return_only_solution` | 不可达 return 不能通过。 | 防假通过。 |
| 33 | P0 | `tests/test_real_cli_matrix.py` | `test_python_entrypoint_required_calls_ast_rejects_main_rewrite_without_target_call` | 改写 `main.py` 但未调用目标函数失败。 | AST required calls。 |
| 34 | P0 | `tests/test_protocol_docs_contract.py` | `test_prompt_cache_docs_name_controlled_above_minimum_synthesis_exception` | docs 包含 controlled stable-prefix-above-minimum synthesis 例外表述。 | 文档合同对齐。 |
| 35 | P0 | `tests/test_protocol_docs_contract.py` | `test_docs_separate_responses_store_prompt_cache_retention_default_and_previous_response_id` | docs 分开说明 `store`、provider default `prompt_cache_retention`、`previous_response_id`。 | 防止 state/cache retention 混用。 |
| 36 | P0 | `tests/test_protocol_docs_contract.py` | `test_docs_reject_positive_gateway_cache_or_routing_product_surface` | 允许否定式 guardrails；禁止把 `enable cache-aware routing`、`llmup response cache`、`semantic cache`、`sticky routing` 写成产品能力或配置。 | 锁肯定产品能力和配置面。 |
| 37 | P1 | `scripts/fixtures/cli_matrix/smoke/claude_openai_responses_multi_turn_request_shape_contract/task.json` | fixture contract | Responses fixture 断言 `input_types` 与 function call/output pair。 | 补齐 Responses target 真实矩阵证据。 |
| 38 | P1 | `scripts/fixtures/cli_matrix/long_horizon/python_bugfix/task.json` | fixture contract | 增加 `behavior_cases` 和可选 `required_calls`。 | 加固 Python long-horizon fixture。 |

## 实施切入点

- `src/server/proxy.rs`：在 conversation-state bridge expansion 后、`classify_request_processing` / boundary reject 前生成唯一 `PreparedRequestBody`；后续 request classification、boundary、translation、debug trace、hooks 和 warning headers 全部传递这份 prepared outcome。
- `src/server/headers.rs` / upstream header policy：Anthropic target same-wire/native preserve `anthropic-beta` / SDK beta context editing header；OpenAI-family target strip Anthropic-only beta headers。
- `src/translate/internal/assessment.rs`：拆出 context edit schema validation / preparation；assessment 改为接收 `effective_body`。
- `src/translate/internal/context_editing.rs`：提供纯函数式 preparation helper；server 调用一次，translator 只消费结果，不重新 preparation。
- `src/translate/internal.rs`、`src/translate/internal/openai_family.rs`、`src/translate/internal/openai_responses.rs`：translation 入口接收 prepared body/outcome，确保 Anthropic-only `context_management` 不泄露到 OpenAI-family upstream，并让 Anthropic -> Responses stateless request 显式 `store:false`。
- `src/prompt_cache_controls.rs`：收窄 OpenAI-family key synthesis；加入 non-empty stable prefix、cache minimum/cacheability 判定、provider state skipped reason 和 Anthropic cache-control drop reason。
- `src/debug_trace.rs`：增加 context edit outcome、effective request summary、Responses `input_types`、tool-call pair counts、OpenAI Responses `store` 摘要、prompt-cache fingerprint-only routing-hint trace。
- `scripts/real_cli_matrix.py`：修复 duplicate request_id；扩展 trace verifier；扩展 `python_source_and_output`。
- `tests/test_real_cli_matrix.py`：先写 verifier red tests。
- `tests/test_protocol_docs_contract.py`：锁 docs wording 和禁止的肯定产品能力/配置面，允许否定式 guardrail 文案。
- `scripts/fixtures/cli_matrix/smoke/*responses*` 与 `scripts/fixtures/cli_matrix/long_horizon/python_bugfix/task.json`：更新 fixture verifier。

## 验收标准

功能验收：

- Anthropic `context_management.edits` 的合法 `clear_tool_uses_20250919` 请求不会因为 `trigger` / `clear_at_least` 被本地 400。
- 小上下文或无法证明 trigger 满足的请求不会因为 `keep` 被立即清理。
- 只传 `{type: clear_tool_uses_20250919}` 时不会按默认 `keep=3` 本地清理；有 warning/omit，visible history 保留。
- 即使 `trigger.tool_uses` 可满足，只要 `clear_at_least.input_tokens` 需要 token facts 且本地不可证明，也不会执行清理。
- `clear_tool_inputs:true` 保留所有 tool pair id / call_id 和消息结构，只替换 input/result payload，不删除 pair 或产生空消息。
- 显式 `clear_thinking_20251015.keep` 可以 deterministic apply；缺省 `keep` 不硬编码，完整可见 history 下 warning/omit。
- Conversation-state bridge 后、server `classify_request_processing` / boundary 前生成唯一 `PreparedRequestBody`；同一个 `effective_body` 被 server boundary、translator、debug trace、hooks 和 warning headers 使用。
- Anthropic same-wire/native path 可以同时 preserve `context_management` 与合法 native `cache_control`。
- Same-wire Anthropic native preserve/raw forward 不要求 mutation；provider-native context editing 与 cache_control 由 provider 处理。
- Same-wire Anthropic native context editing preserve `anthropic-beta` / SDK betas；cross-provider Anthropic -> OpenAI-family 不转发 Anthropic beta header。
- Anthropic -> OpenAI-family cross-provider path 对 `cache_control` drop+warning，不派生 OpenAI `prompt_cache_key` / `prompt_cache_retention`。
- Anthropic -> OpenAI Responses stateless translated request 显式 `store:false`；`llmup` 永不为 cross-provider request 合成 `store:true`。
- OpenAI-family `prompt_cache_key` 只在非空稳定且明显达到 provider cache minimum 的 static prefix 下合成；无 static prefix、cacheability unknown、prefix below minimum、provider state、Anthropic cache_control、动态 tail-only 请求不合成。
- Prompt-cache trace 只标记 provider routing hint；cache hit 只能由 provider usage cached-token counters 证明。
- `llmup` 不设置/不合成 `prompt_cache_retention`；省略时 trace/docs 标为 provider default may apply，不得写成 disabled。`store:false` 不关闭 provider prompt-cache retention。
- CLI trace verifier 拒绝重复 matching request `request_id`。
- Responses trace/fixture 能证明 `function_call` 与 `function_call_output` 成对，且不依赖动态 `call_id` 字面值。
- Python long-horizon verifier 直接 import/call 目标函数，不能被不可达 return 或入口点绕过。
- docs contract 表述与实现一致；测试允许否定式 guardrail，禁止把 gateway cache、semantic cache、sticky/cache-aware routing 写成 `llmup` 产品能力或配置。

Rust gate：

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --verbose
cargo test --locked --verbose clear_tool_uses
cargo test --locked --verbose clear_thinking
cargo test --locked --verbose prompt_cache_key
cargo test --locked --verbose responses_request_summary
```

Python gate：

```bash
python -m unittest tests.test_real_cli_matrix
python -m unittest tests.test_protocol_docs_contract
python -m unittest discover -s tests
```

CI / repo gate：

```bash
./scripts/check-governance.sh
./scripts/test-and-report.sh
```

真实矩阵 gate：

- 在有对应 provider credentials 的 CI 或 release-prep 环境运行 targeted real CLI matrix。
- 必须覆盖 Chat target 与 Responses target 的 Anthropic client -> OpenAI-family upstream。
- Responses target 报告中必须出现 `upstream_summary.input_types` 和 tool-call pair 摘要断言。
- negative fail-closed suite 仍验证 provider-owned state/resource、opaque-only carrier、非法显式 fields。

## 风险与回滚 / 失败策略

| 风险 | 影响 | 策略 |
| --- | --- | --- |
| 没有 provider 精确 token count，无法等价执行 `trigger.input_tokens` / `clear_at_least`，也无法精确证明 OpenAI cache minimum。 | 可能少清理、少合成 key，但不会错清理或误提示 cacheability。 | 保守 no-op / no-synthesis warning；不把 token-count endpoint 引入本批次依赖；后续若要增强必须单独评审。 |
| 收窄 `prompt_cache_key` synthesis 后 cache hit 下降。 | 成本优化机会减少。 | 正确性优先；显式用户 key 仍可用；紧急回滚可将 synthesis 函数临时改为 always `None`，不新增用户开关；不得声称 provider cache hit。 |
| OpenAI Responses `store:false` 改变部分 translated request 的 provider state retention。 | 少数依赖默认 Responses state 的跨协议请求会失去 provider application state。 | Cross-provider Anthropic -> Responses 的目标是 stateless full input；需要 provider state 的请求必须走 OpenAI-native same-wire 或显式 provider-state path，不能靠默认 store。 |
| Duplicate `request_id` 校验可能暴露既有 fixture 缺陷。 | 短期测试变红。 | 修 fixture，不放松 verifier；重复 ID 是 trace 可靠性 bug。 |
| `behavior_cases` import 执行不安全或污染环境。 | 测试不稳定。 | 只在 fixture workspace 内设置 `sys.path`，限制为纯 Python import/call；必要时 subprocess 隔离并设置 timeout。 |
| Docs contract 测试过窄导致合理 wording 被误伤。 | 文档维护成本上升。 | 测试锁核心合同和禁止项，不锁整段文案；保持 wording 可维护。 |
| Anthropic 官方模型默认继续变化。 | 本地默认表会过期。 | 本批次不维护默认表；缺省 `keep` 保守 warning/omit。 |

失败策略：

- P0 任一项未过，不允许把本批次标记为 pre-GA ready。
- 若 context edit preparation 引发大面积 regression，优先禁用本地 apply，仅保留 schema accept + warning/omit + fail-closed opaque；不得回到按 `keep` 立即清理。
- 若 prompt-cache synthesis regression 难以快速修复，优先关闭 synthesis，保留显式 key/retention preserve；不得扩大到 Anthropic/OpenAI 硬转换，也不得把 routing hint 当 cache hit。
- 若 trace/verifier regression 阻塞 release，先修 verifier/fixture；不得降低 `require_response_for_each_request` 语义。

## 参考资料

Provider 官方文档：

- Anthropic context editing: <https://platform.claude.com/docs/en/build-with-claude/context-editing>
- Anthropic prompt caching: <https://platform.claude.com/docs/en/build-with-claude/prompt-caching>
- OpenAI prompt caching: <https://platform.openai.com/docs/guides/prompt-caching>
- OpenAI data controls / retention: <https://platform.openai.com/docs/guides/your-data/>

同类产品边界参考：

- LiteLLM prompt caching: <https://docs.litellm.ai/docs/completion/prompt_caching>
- LiteLLM proxy caching: <https://docs.litellm.ai/docs/proxy/caching>
- OpenRouter prompt caching: <https://openrouter.ai/docs/guides/best-practices/prompt-caching>
- OpenRouter response caching: <https://openrouter.ai/docs/guides/features/response-caching>
- Cloudflare AI Gateway caching: <https://developers.cloudflare.com/ai-gateway/configuration/caching/>
- Helicone provider prompt caching announcement: <https://www.helicone.ai/changelog/20250214-anthropic-prompt-caching>
- Helicone response / LLM caching: <https://docs.helicone.ai/features/advanced-usage/caching>
- Portkey Anthropic prompt caching: <https://portkey.ai/docs/integrations/llms/anthropic/prompt-caching>
- Portkey simple / semantic gateway cache: <https://portkey.ai/docs/product/ai-gateway/cache-simple-and-semantic>
- Vercel AI Gateway provider options / automatic caching: <https://vercel.com/docs/ai-gateway/provider-options>
- Vercel AI Gateway OpenAI-compatible advanced caching: <https://vercel.com/docs/ai-gateway/sdks-and-apis/openai-compat/advanced>
- Envoy AI Gateway vendor-specific fields: <https://aigateway.envoyproxy.io/docs/capabilities/llm-integrations/vendor-specific-fields/>
- Envoy AI Gateway prompt caching: <https://aigateway.envoyproxy.io/docs/capabilities/llm-integrations/prompt-caching/>
