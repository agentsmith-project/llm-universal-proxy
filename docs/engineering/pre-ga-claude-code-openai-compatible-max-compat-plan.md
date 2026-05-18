# Pre-GA Claude Code -> OpenAI-Compatible 最大兼容最终目标计划

- 状态：implementation contract
- 日期：2026-05-18
- 范围：把 Claude Code / Anthropic Messages client -> OpenAI-family provider 作为 `llmup` 的一等正向路径实现，覆盖 OpenAI Chat Completions 与 OpenAI Responses 两类 target adapter，并通过真实 CLI / E2E 证明普通对话、streaming、工具循环、编辑工具、thinking/reasoning 和 provider-side prompt cache hint 都可用。
- Pre-GA 原则：不为旧行为保留兼容负担；允许重构 translation pipeline、assessment、stream reducer、tool bridge、prompt-cache hint 和轻量状态边界。重构必须服务于最终合同，不能把 `llmup` 做成 LiteLLM / OpenRouter 风格的重型网关。

## 背景

当前 `claude + preset-openai-compatible` 在真实 CLI 矩阵中不是正向通过，而是被归类为预期拒绝。用户手动运行 Claude Code 时看到的 400 是 `llmup` 本地 request boundary 在上游调用前拒绝了顶层 `thinking` / `context_management`，不是 OpenAI-compatible provider 返回的错误。

这与最终目标不一致。Claude Code / Anthropic Messages client 使用 OpenAI-compatible upstream 必须是正常路径；只有真正无法安全表示、缺少可见上下文、或依赖 provider-owned opaque state/resource 的请求才应该在上游前拒绝。

## 目标合同

1. `llmup` MUST 把 Claude Code / Anthropic Messages client -> OpenAI-family provider 视为一等正向路径。OpenAI-family 在本文中同时包含 Chat Completions (`openai-completion`) 与 Responses (`openai-responses`) 两种 target adapter。
2. 该路径 MUST 通过真实 CLI / E2E：普通 `hi`、streaming、结构化 tool use/tool result、workspace edit fixture、usage 汇总和负向拒绝用例都要被验证。
3. 在请求满足“完整可见 history”时，translation MUST 优先 `map` / `synthesize` / `warn+drop`，MUST NOT 因 Anthropic-only 顶层 hint 直接 fail closed。
4. `llmup` MAY 合成 provider request 所需的兼容数据，但 MUST NOT 伪造 opaque security carrier、provider-owned resource、provider cache 内容或模型响应。
5. Prompt cache 只能使用 provider-side 机制。`llmup` MAY 生成 OpenAI-compatible request hint，MUST NOT 实现 response cache、result cache、semantic cache、cache-aware routing 或跨 provider cache handle 互转。
6. Thinking / reasoning MUST 尽量保留下游可用能力：可见 unsigned thinking 要进入 reasoning side channel 或普通上下文；top-level `thinking` 要在 surface 或显式覆盖允许时映射为 OpenAI reasoning hint；opaque/signed/redacted carrier 不能伪造。
7. 配置面 MUST 保持小。不得新增用户可见兼容等级或“模式开关”；高风险 provider feature 只能由 model surface、显式 `extra_body` 或 same-wire provider-native request 控制。

## 参考资料

官方资料：

- Anthropic Messages API stateless，客户端应发送 conversation history：<https://platform.claude.com/docs/en/build-with-claude/working-with-messages>
- Anthropic streaming lifecycle：<https://platform.claude.com/docs/en/build-with-claude/streaming>
- Anthropic extended thinking：<https://platform.claude.com/docs/en/build-with-claude/extended-thinking>
- Anthropic context editing：<https://platform.claude.com/docs/en/build-with-claude/context-editing>
- Anthropic prompt caching：<https://platform.claude.com/docs/en/build-with-claude/prompt-caching>
- Claude Code LLM gateway：<https://code.claude.com/docs/en/llm-gateway>
- OpenAI prompt caching：<https://developers.openai.com/api/docs/guides/prompt-caching>
- OpenAI Chat Completions request fields：<https://platform.openai.com/docs/api-reference/chat/create>
- OpenAI Responses request fields：<https://platform.openai.com/docs/api-reference/responses>
- OpenAI reasoning guide：<https://developers.openai.com/api/docs/guides/reasoning>
- OpenAI function calling guide：<https://developers.openai.com/api/docs/guides/function-calling>
- OpenAI provider-managed conversation state：<https://developers.openai.com/api/docs/guides/conversation-state>

开源实践参考：

- clawgate：Anthropic client / Claude Code -> OpenAI-compatible backend，强调 streaming 和 tool use：<https://clawgate.org/>
- claude-code-proxy：Claude Code -> OpenAI-compatible API proxy：<https://github.com/fuergaosi233/claude-code-proxy>
- claude2openai-proxy：Anthropic `/v1/messages` -> OpenAI / LiteLLM，支持 content block、tool、SSE 转换：<https://github.com/ziozzang/claude2openai-proxy>
- UniClaudeProxy：Claude Code 到 OpenAI-compatible / Responses / Anthropic passthrough，包含 reasoning block 和工具桥接参考：<https://github.com/vibheksoni/UniClaudeProxy>
- raine/claude-code-proxy：Claude Code streaming fallback、context window、prompt cache key/session id 处理：<https://github.com/raine/claude-code-proxy>
- LiteLLM：可作为 provider surface 和 OpenAI-compatible 差异参考，但 `llmup` MUST NOT 复制其重型路由产品形态：<https://github.com/BerriAI/litellm>

共同启发：Claude Code -> OpenAI-compatible 应该正向可用；实现上应做协议降级、streaming event 转换、结构化工具 round-trip 和受控 hint synthesis，而不是遇到 provider-native 字段就整体 400。

## 完整可见 History

“完整可见 history”是机械条件，不是模型判断，也不是新的持久状态层。只有同时满足以下条件，request 才能按最大兼容策略降级：

1. `messages` 非空，且当前轮生成所需的对话内容以 portable content block 或 string 形式出现在请求中。
2. 当前轮依赖的 tool call、tool result、assistant turn、system/developer instruction 和用户可见上下文都在请求中，或已由 `llmup` 自己拥有的 in-memory transcript bridge 展开为普通 messages。
3. 请求不依赖外部 provider-owned opaque handle：包括但不限于 Anthropic provider container runtime、server tool resource、hosted prompt/resource、opaque compact handle、OpenAI `previous_response_id` / `conversation` 等未展开状态。
4. 请求中没有只能靠 opaque carrier 才能理解的 reasoning/thinking 内容。若存在 visible summary/text，可按本文 thinking 合同保留可见部分并丢弃 carrier；若只有 opaque carrier，则不满足本条件。
5. tool schema 必须能被目标协议结构化表达；media/source 优先结构化表达，无法结构化表达但可安全文本降级、且完整可见 history 仍成立时 MAY `fallback-text` 并 warning；无法结构化表达且不能安全降级的 media/source MUST fail closed。

实现 MUST 用确定性 JSON/IR 检查判定上述条件。MUST NOT 通过 LLM、自然语言摘要或猜测来判断 history 是否完整。

## Translation Pipeline

Claude Code / Anthropic Messages -> OpenAI-compatible 的请求 MUST 经过同一条可审计 pipeline：

1. `assessment`：分类每个字段的 disposition：`map`、`synthesize`、`warn+drop`、`fallback-text`、`fail-closed`。这里必须判断完整可见 history、provider-owned state/resource、non-fallbackable unsupported media/source、invalid tool schema、非法显式 OpenAI fields。
2. `deterministic normalization/edit`：执行无 LLM 的确定性规范化和可支持的 context edit，生成稳定 IR。该步骤 MAY 清理旧 tool results 或 unsigned visible thinking，但 MUST 保持 transcript 顺序和 tool call/result 约束。
3. `request construction`：从 IR 构造 OpenAI-family request。Anthropic-only 顶层字段 MUST NOT 泄露到上游；工具、messages/input、system/developer instructions、response format、stream 参数和 model 参数必须按目标 provider surface 构造。
4. `provider cache hint synthesis`：在 request 构造后、upstream 前处理 `prompt_cache_key` / `prompt_cache_retention`。显式 OpenAI extra fields 优先；没有显式 key 时 MAY 受控合成 OpenAI provider request hint。
5. `upstream`：发送到目标 provider。未知 provider feature MUST 由 surface 或显式 request 控制，不能盲目注入高风险字段。
6. `stream reducer`：把 OpenAI-compatible chunks 聚合为 Anthropic SSE lifecycle，按 block id/index 维护 text、thinking、tool partial JSON、finish reason 和 usage。
7. `response repair/usage telemetry`：修复可安全修复的 response shape，输出 warning/debug trace，保留 upstream raw usage 中可见的 cached tokens、reasoning tokens、prompt/completion tokens。Telemetry MUST NOT 驱动本地 cache 或 routing。

## OpenAI-Family 双 Target 合同

Chat Completions 和 Responses 都是 OpenAI-family target，但它们不是同一个 wire shape。实现 MUST 以同一份 normalized IR 为源，分别由 Chat target adapter 和 Responses target adapter 构造请求与解析响应。

| 维度 | Chat Completions target | Responses target |
| --- | --- | --- |
| 上游 endpoint | `/v1/chat/completions` | `/v1/responses` |
| 输入主结构 | `messages`，包含 `system/developer/user/assistant/tool` role | `instructions` + `input` items/messages |
| 工具调用 | assistant `tool_calls` + `role:"tool"` results | `function_call` / `function_call_output` items；custom tools 走已有 bridge 语义 |
| reasoning request | top-level `reasoning_effort` | nested `reasoning.effort` |
| prompt cache hint | top-level `prompt_cache_key` / `prompt_cache_retention` | top-level `prompt_cache_key` / `prompt_cache_retention` |
| provider state controls | 默认 stateless messages；不生成 Chat provider state | `previous_response_id` / `conversation` / hosted `prompt` 是 provider-managed state，只能 same-wire native preserve 或由 `llmup` 自有 transcript bridge 展开 |
| streaming source | Chat chunks：text/tool delta/finish reason/usage | Responses events/items：output text、reasoning、function_call、function_call_output、usage/state events |

合同要求：

1. Anthropic -> OpenAI-family translation MUST 同时有 Chat 和 Responses target tests。不得因为 Chat target 可用就视为 OpenAI-family 完成。
2. Target adapter MUST 只表达目标 wire shape 差异；assessment、完整可见 history 判断、field disposition、tool id 映射、prompt-cache hint、warning trace 应尽量共享。
3. Responses target SHOULD 优先用 stateless full `input` 构造 Anthropic history 的翻译结果。`llmup` MUST NOT 为 Anthropic client 合成外部 provider `previous_response_id` / `conversation` 来制造 provider durable state。
4. OpenAI Responses client same-wire 使用 Responses upstream 时，provider-native state controls 可以按 native policy 保留。跨协议或跨 upstream 时，只有 `llmup` 自有 in-memory transcript bridge 能把本地 id 展开为完整可见 history；不能展开时 MUST fail closed。
5. Chat target 的 `tool_calls` / `role:"tool"` 与 Responses target 的 `function_call` / `function_call_output` 必须共享同一套 call-id 关联规则，避免同一 Anthropic tool loop 在两个 target 上行为不一致。
6. Reasoning 和 prompt-cache 规则 MUST 同时覆盖两种 target shape：Chat 使用 `reasoning_effort`，Responses 使用 `reasoning.effort`；两者都可接收合法 `prompt_cache_key` / `prompt_cache_retention`。

## Field Disposition Matrix

| 字段/形态 | 目标 disposition | 合同 |
| --- | --- | --- |
| top-level `thinking` | `map` 或 `warn+drop` | 若显式 `extra_body.openai.reasoning_effort` 合法，MUST 使用显式值；若 target surface 明确支持 Chat `reasoning_effort` 或 Responses `reasoning.effort`，SHOULD 将 Anthropic thinking hint 映射为目标 shape 的合法 effort；否则在完整可见 history 下 MUST warning 后丢弃顶层 hint。 |
| top-level `context_management` | deterministic edit、`warn+drop` 或 `fail-closed` | 支持的 context edit MAY 在 normalization 中执行；未知但不影响完整可见 history 的 hint SHOULD warning+drop；依赖 provider compact/resource/opaque state 或缺少完整可见 history 时 MUST fail closed。 |
| top-level `container` | `warn+drop` 或 `fail-closed` | 空、disabled、普通 hint、无 server resource 依赖且 history 完整时 SHOULD warning+drop；provider-owned `container.id`、skills、code execution、MCP、server tools/resource runtime MUST fail closed，除非该请求已由 `llmup` 自己拥有的内存 transcript 完全展开。 |
| Anthropic `cache_control` | same-wire preserve 或 cross-provider `warn+drop` | Anthropic target 可按 same-wire 透传；OpenAI-compatible target MUST NOT 与 OpenAI `prompt_cache_key` / retention 硬互转。可用于 trace 说明“已丢弃 Anthropic cache hint”，但不能派生 OpenAI key。 |
| `extra_body.openai.prompt_cache_key` | explicit `map` | OpenAI-family target MUST 保留并校验显式 key。显式 key 优先于合成 key；非法值 MUST fail closed。 |
| `extra_body.openai.prompt_cache_retention` | explicit `map` | 仅允许 provider 支持的合法值。不得因为合成 key 自动设置 retention；非法显式值 MUST fail closed。 |
| `tool_use` / `tool_result` | structured `map` | assistant `tool_use` MUST 在 Chat target 转为 `tool_calls`，在 Responses target 转为 `function_call` / custom-tool bridge；user `tool_result` MUST 在 Chat target 转为 `role:"tool"`，在 Responses target 转为 `function_call_output`。必须保留或合成稳定 call id，按 id/block index 关联，不能默认降级成普通文本。 |
| partial tool JSON | stream-aware `map` | Streaming 中 MUST 支持 partial JSON delta，维护 block id/index 和 arguments buffer。结束时必须生成目标 Anthropic lifecycle 事件和一致的 final content block。 |
| unsigned plain visible thinking | reasoning side channel 或 context `map` | 可见文本 SHOULD 保留到目标 provider 支持的 reasoning side channel；不支持时 MAY 放入普通 assistant context，并记录降级 warning。 |
| signed `thinking` with visible text | visible text `map` + carrier `warn+drop` | MUST NOT 伪造 signature。若 history 完整且 block 内有普通可见 thinking text，MAY 保留该文本并 warning/drop signature carrier；若后续 round-trip 依赖 signature 才能继续，MUST fail closed。 |
| omitted / redacted / encrypted / opaque thinking | visible summary `map` 或 `fail-closed` | MUST NOT 伪造 redacted/encrypted/opaque carrier。若同一请求提供独立可见摘要，可保留摘要并 warning/drop carrier；opaque-only 或缺少可见 history 时 MUST fail closed。 |
| media/source | structured `map`、`fallback-text` 或 `fail-closed` | 目标协议可表达时 MUST 结构化映射；无法结构化表达但可安全文本降级、且完整可见 history 仍成立时 MAY `fallback-text` 并 warning；无法结构化表达且不能安全降级的 non-fallbackable unsupported media/source MUST fail closed。 |
| provider-owned state/resource | `fail-closed` | 外部 provider state id、hosted prompt/resource、server-side tool resource、provider container runtime、未展开 `previous_response_id` / `conversation` MUST fail closed。只有 `llmup` 自己拥有的内存 transcript bridge 能展开成完整可见 messages。 |
| OpenAI upstream reasoning output | Anthropic visible thinking `map` | 可见 `reasoning_content` / reasoning delta SHOULD 映射为 Anthropic thinking block/delta；encrypted或 opaque reasoning MUST NOT 被伪造成可见 thinking。 |
| finish reason / stop reason | deterministic `map` | `stop`、`length`、`tool_calls`、content filter 和 provider-specific finish reason MUST 映射到 Anthropic `stop_reason` 或 warning trace；未知值不得破坏 SSE lifecycle。 |

## Proxy 可合成数据边界

`llmup` MAY 合成以下数据，因为它们是目标协议兼容 shim 或 request-local bookkeeping：

- OpenAI `prompt_cache_key`，前提是使用稳定、确定性、不可逆、不含敏感原文的 canonical digest，且显式用户 key 优先。
- OpenAI-family reasoning hint：Chat `reasoning_effort` 或 Responses `reasoning.effort`，前提是 target surface 明确支持，或用户通过 `extra_body.openai.reasoning_effort` 显式覆盖。
- Synthetic message/content block ids、tool-call ids、block index shim、stream reducer buffer id。
- Provider-compatible request fields，例如 Chat `tool_choice` / `response_format` shim、Responses `tools` / `reasoning` / `input` item shape、function schema 包装。
- Request/stream 内部 bookkeeping，例如 partial JSON buffer、usage accumulator、warning disposition trace。
- 现有 `llmup`-owned in-memory transcript bridge 展开的 messages，用于把 provider-state-style client request 转成完整可见 history。

`llmup` MUST NOT 合成或伪造以下数据：

- Anthropic thinking `signature`、redacted thinking payload、encrypted reasoning payload、opaque reasoning token。
- Provider-owned container/resource contents、server tool results、hosted prompt/resource 内容、外部 provider conversation/container state。
- 模型响应、response cache hit、semantic cache hit、provider KV cache 内容或跨 provider cache handle。
- 会让用户误以为 provider 保留了状态的持久 id；重启后丢失的内存状态不能伪装成 provider durable state。

## Thinking / Reasoning 合同

1. Top-level Anthropic `thinking` MUST 被视为能力 hint，而不是必须同语义转发的 opaque state。
2. 显式 `extra_body.openai.reasoning_effort` MUST 优先，且非法值 MUST fail closed。
3. 若 model surface 声明支持 OpenAI Chat `reasoning_effort` 或 Responses `reasoning.effort`，translator SHOULD 将 Anthropic `thinking` 映射为目标 shape 的合法 effort。`budget_tokens` 只能用确定性、保守的区间启发式映射；不得生成目标不支持的值。
4. 未声明 reasoning support 的 OpenAI-compatible target MUST NOT 被盲目注入 reasoning field；在完整可见 history 下应 warning+drop 顶层 hint。
5. Visible unsigned thinking MUST 尽量保留。优先进入目标 provider 的 reasoning side channel；没有 side channel 时可作为普通 assistant context 保留，并记录 trace。
6. Signed/redacted/opaque carrier MUST NOT 被伪造。若请求仍有完整可见 history 且存在独立可见文本/摘要，translator MAY 保留可见部分并 warning/drop carrier；若只有 opaque carrier、或 provider 要求 signature round-trip 才能继续，MUST fail closed。
7. Response 侧 SHOULD 将 OpenAI-compatible 可见 reasoning delta 映射为 Anthropic thinking delta/content block，让 Claude Code 的 thinking UI 尽量正常。不可见、加密、或 provider-private reasoning 只能作为 usage/trace 呈现。

## Provider-Side Prompt Cache 合同

1. `llmup` MUST NOT 实现 response cache、semantic cache、result cache、provider KV cache 保存、cache lookup、cache-aware routing 或 fallback routing。
2. OpenAI prompt caching 是 provider-side 能力。Translator MUST 保持静态 prefix 稳定：system/developer 内容、tool 定义、schema 序列化、message/tool 顺序不得混入 request id、trace id、时间戳、随机数、`previous_response_id` 或 `resp_llmup_*`。
3. 显式 `extra_body.openai.prompt_cache_key` / `prompt_cache_retention` MUST 保留、校验并映射到 Chat Completions 和 Responses target。
4. 当 target 是 OpenAI-family Chat 或 Responses、没有显式 key、且 translator 能构造稳定 canonical static-prefix digest 时，`llmup` SHOULD 合成 OpenAI `prompt_cache_key`。推荐格式：`llmup:v1:<namespace-fp>:<upstream-fp>:<model-fp>:<static-prefix-fp>`。
5. Canonical static-prefix digest 的输入 MUST 只包含 canonicalized target upstream static prefix：resolved namespace/upstream/model/protocol/version、system/developer instructions、tool definitions/schema、stable response-format/static config。
6. Digest 输入和合成 key MUST NOT 包含最后用户消息、dynamic transcript tail、request id、timestamp、conversation/container ids、provider credentials、trace ids、random、raw prompt text 明文、Anthropic `cache_control`、`previous_response_id` 或 `resp_llmup_*`。
7. 合成 key 不得自动设置 `prompt_cache_retention`。Retention 只能来自合法显式请求或 provider-native same-wire request。
8. Anthropic `cache_control` 与 OpenAI `prompt_cache_key` / retention MUST NOT 硬互转。OpenAI key 可由稳定 prefix digest 合成，但不能声称等价于 Anthropic block-level TTL。
9. Usage telemetry SHOULD 暴露 provider raw usage 中的 cached tokens、reasoning tokens、prompt/completion tokens。Telemetry 只能用于可观测性，MUST NOT 驱动本地缓存或路由决策。
10. 若字段长度、字符集、隐私、稳定性或 target support 无法确认，translator MUST omit synthesized key 并 warning，不得让普通请求因此失败。

## Tool / SSE 合同

1. Tool conversion MUST 是结构化路径。`tool_use`、`tool_result`、function call、tool output 的 id/name/arguments/result 必须优先进入目标协议原生字段。
2. Tool call id 缺失时 MAY 合成稳定 shim；已有 id MUST 保留。并行或交错工具调用必须按 id 和 block index 关联。
3. Partial JSON MUST 被 stream reducer 支持。OpenAI-compatible arguments delta 要聚合并映射为 Anthropic `input_json_delta` / final tool block；不得因 chunk 边界不同生成非法 JSON。
4. Anthropic SSE lifecycle MUST 完整：`message_start`、`content_block_start`、`content_block_delta`、`content_block_stop`、`message_delta`、`message_stop`。
5. Finish reason MUST 稳定映射：工具调用结束为 Anthropic `tool_use`，普通停止为 `end_turn`，长度为 `max_tokens`，stop sequence 为 `stop_sequence`，未知 provider reason 进入 warning trace。
6. Usage MUST 在 stream 结束时尽量补齐。若 upstream 分片缺 usage，translator 可输出未知/缺省 telemetry，但不得伪造 token count。
7. 只有目标协议完全无法结构化表达、且请求仍满足完整可见 history 时，MAY warning 后把工具相关内容降级为普通文本。这是最后兜底，不是默认行为。

## 轻量状态边界

1. `llmup` MAY 使用纯内存、进程内、TTL/max-bytes 限制的 transcript bridge，只为把 provider-state-style client 请求展开成完整可见 history。
2. 该 bridge MUST 是 `llmup` 自己拥有的可见 transcript，不得导入外部 provider opaque state 并假装可理解。
3. 缺失、过期、超出 max bytes、进程重启后不存在的 state MUST 直接 fail closed。不得尝试摘要、猜测、远程恢复或创建持久占位。
4. MUST NOT 引入厚重数据库、跨进程恢复、admin 会话浏览、provider resource 生命周期管理或 Conversations API 完整模拟。
5. Context editing 只能是确定性、无 LLM、输入 history 完整的本地 edit。支持项必须显式列出；未知 edit、compact、opaque state 或 provider resource 依赖必须 fail closed。

## Codebase 重构点

实现 MAY 大幅重构，但 SHOULD 保持模块边界清晰：

- `src/translate/internal/assessment.rs`：从 hard reject 列表改为 field disposition classifier，输出 `map/synthesize/warn+drop/fallback-text/fail-closed` 和完整可见 history 判断结果。
- `src/translate/internal.rs`：让 Anthropic -> OpenAI-family construction 消费规范化 IR，保证 Anthropic-only 字段不会泄露到上游，并把 Chat / Responses target adapter 分开。
- `src/translate/internal/openai_responses.rs`：把 Responses-specific input item、function_call/function_call_output、reasoning item、custom-tool bridge 和 Responses SSE/response repair 作为一等 target adapter 维护。
- Content block IR：统一 text、thinking、tool_use、tool_result、partial JSON、media/source 和 block id/index，降低 request 与 stream 两边的重复逻辑。
- Reasoning surface：增加最小 provider capability，例如是否支持 Chat `reasoning_effort`、Responses `reasoning.effort`、允许 effort 集合、是否支持 reasoning content delta；不得引入用户可见兼容等级。
- Prompt cache controls：把显式 mapping、controlled key synthesis、prefix stability check、usage telemetry 放在 provider-side hint 模块中；继续禁止 `llmup` cache。
- Stream reducer：集中处理 Chat chunks 与 Responses events -> Anthropic SSE lifecycle，维护 block id/index、tool argument buffer、reasoning delta、finish reason 和 usage accumulator。
- Warning/debug trace：每个 drop/fallback/synthesis MUST 有机器可读 disposition reason；普通日志不得输出完整 synthesized cache key 或敏感 prompt。
- Real CLI matrix：Claude -> OpenAI-compatible MUST 进入 positive usable suite；negative fail-closed suite 只验证真正不可兼容的 provider-owned state/resource 和非法显式字段。

## TDD / 验收测试矩阵

Focused Rust tests MUST 覆盖：

- assessment 对 top-level `thinking` / `context_management` / 普通无资源 `container` 给出 warning disposition，而不是 hard reject。
- translation 后 OpenAI-family request 不含 Anthropic-only 顶层字段。Chat target 必须保留并正确构造 `model/messages/tools/tool_choice/stream`；Responses target 必须保留并正确构造 `model/instructions/input/tools/tool_choice/stream`。
- provider-owned container/server tool/resource、opaque-only state、non-fallbackable unsupported media/source、invalid tool schema 仍在上游前 fail closed。
- top-level `thinking` 在 Chat surface 支持或显式 override 时映射为合法 `reasoning_effort`；在 Responses surface 支持时映射为合法 `reasoning.effort`；未知 target warning+drop。
- visible unsigned thinking 被保留到 reasoning side channel 或普通上下文。
- signed thinking visible text 可保留且 signature carrier 被 warning/drop；opaque-only、redacted-only、encrypted-only thinking fail closed。
- Anthropic `cache_control` 不会变成 OpenAI `prompt_cache_key`；显式 OpenAI key/retention 被保留并校验。
- 合成 `prompt_cache_key` 对同一 namespace/upstream/model/static-prefix 稳定，对 system/tools 改变敏感；digest 输入只含 canonicalized static prefix，不含最后用户消息、dynamic transcript tail、raw prompt text 明文、request id、timestamp、trace ids、conversation/container ids、provider credentials 或 `resp_llmup_*`。
- tool_use/tool_result 结构化 round-trip、并行工具 id 关联、partial JSON streaming、finish reason 映射和 usage telemetry。
- Anthropic `tool_use` / `tool_result` 必须分别覆盖 Chat `tool_calls` / `role:"tool"` 和 Responses `function_call` / `function_call_output` 两种 target shape。
- Responses target 必须覆盖 stateless full `input` 翻译；OpenAI Responses provider-native `previous_response_id` / `conversation` 只能 same-wire preserve 或由 `llmup` 自有 bridge 展开，不能为 Anthropic client 伪造 provider state id。
- in-memory transcript bridge 只能展开 `llmup` 自有可见 transcript；TTL/max bytes/restart missing 直接 fail closed。

Python / script tests MUST 覆盖：

- `tests/test_real_cli_matrix.py` 不再把 Claude non-Anthropic upstream blanket 标为预期拒绝。
- Positive usable suite 和 negative fail-closed suite 分开展示；positive 中出现预期拒绝必须失败。
- Report markdown/json 明确展示真实可用 pass、真实失败、负向拒绝 pass。
- Real CLI matrix MUST 有 OpenAI-family 双 target 覆盖：一个 `openai-completion` / Chat Completions upstream，一个 `openai-responses` / Responses upstream。若现有 fixture 只有 `preset-openai-compatible`，必须补一个 Responses preset 或等价 fixture。

真实 CLI / E2E MUST 覆盖：

- `scripts/run_claude_proxy.sh --model <openai-chat-compatible-preset> --proxy-port <free-port>` 后 Claude Code 普通 `hi` 成功返回。
- `scripts/run_claude_proxy.sh --model <openai-responses-compatible-preset> --proxy-port <free-port>` 后 Claude Code 普通 `hi` 成功返回。
- Claude Code streaming tool-use fixture。
- Claude Code public editing tool workspace edit fixture。
- Claude native Anthropic-compatible baseline 继续通过。
- Codex -> OpenAI-compatible baseline 继续通过，避免修 Claude 路径时破坏现有 OpenAI-compatible 路径。

负向 E2E MUST 覆盖：

- provider-owned container/server tool/resource。
- opaque-only signed/redacted/encrypted thinking。
- non-fallbackable unsupported media/source。
- invalid explicit `extra_body.openai` cache/reasoning field。
- 缺失或过期的 `llmup` in-memory transcript bridge state。

## 旧文档同步要求

因为项目仍处于 Pre-GA，本合同是最终行为口径。相关旧文档 MUST 作为同一开发交付物同步，不能继续作为相反约束、额外准入条件或独立实施顺序存在：

- `docs/protocol-compatibility-matrix.md` 和 `docs/protocol-baselines/capabilities/state-continuity.md` MUST 把 Anthropic top-level `thinking` / `context_management` / 普通无资源 `container` 从 blanket cross-provider fail-closed 改为“完整可见 history 下 map 或 warn+drop；provider-owned state/resource 仍 fail closed”。
- `docs/engineering/pre-ga-request-processing-prompt-cache-support-plan.md`、`docs/protocol-baselines/capabilities/cache.md` 和 `docs/max-compat-design.md` MUST 同步为：允许受控合成 OpenAI provider-side `prompt_cache_key`，但不做 `llmup` response/semantic cache，不把 Anthropic `cache_control` 与 OpenAI key/retention 硬互转。
- `docs/engineering/pre-ga-conversation-state-bridge-plan.md` 和 state-continuity baseline MUST 同步为：只允许纯内存、TTL/max-bytes、`llmup` 自有可见 transcript 展开；不做持久化、provider-owned state 重建或完整 Conversations API 模拟。
- 所有旧 fail-closed/cache/state 文档 MUST 使用本合同的字段 disposition、完整可见 history 定义、合成数据边界、thinking/reasoning 规则和 prompt-cache 规则。

## 非目标与边界

- MUST NOT 实现 `llmup` response/result cache。
- MUST NOT 实现 semantic cache。
- MUST NOT 保存 provider KV cache、provider prompt cache 内容或 provider cache resource。
- MUST NOT 导入外部 `resp_*` / `conv_*` / Anthropic container id 作为可理解的本地状态。
- MUST NOT 做持久化、跨进程恢复、admin 会话浏览、完整 Conversations API 模拟。
- MUST NOT 模拟 Anthropic code execution、skills、MCP、server tools 或 provider container runtime。
- MUST NOT 为了 cache 命中重排 message/tool 顺序、修改 tool schema 语义或注入随机前缀。
- MUST NOT 引入用户可见兼容等级。
- MUST NOT 把 `llmup` 扩展成 LiteLLM / OpenRouter / Portkey 风格的通用路由、fallback、guardrail 或 agent runtime 产品。
