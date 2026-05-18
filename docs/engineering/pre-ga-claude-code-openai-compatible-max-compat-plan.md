# Pre-GA Claude Code -> OpenAI-Compatible 最大兼容开发计划

- 状态：team-reviewed handoff draft
- 日期：2026-05-18
- 范围：让 Claude Code / Anthropic Messages client 通过 `llmup` 使用 OpenAI-compatible Chat Completions upstream 时尽量可用，同时尽量利用 provider-side prompt cache，并保持 reasoning / thinking 语义的最大安全兼容。
- 非范围：`llmup` response cache、semantic cache、provider cache 资源生命周期管理、持久化会话数据库、完整 Conversations API 模拟、Anthropic server tool / code execution / skills 容器模拟、通用负载均衡或 fallback 产品化。

## 背景

当前 `claude + preset-openai-compatible` 在真实 CLI 矩阵中不是正向通过，而是被标记为 `expected_fail_closed`。用户手动运行 Claude Code 时看到的 400 是 `llmup` 本地 request boundary 在上游调用前拒绝了顶层 `thinking` / `context_management`，不是 OpenAI-compatible provider 返回的错误。

这与新的产品目标不一致：`llmup` 的目标是最大化兼容，Claude Code 应该可以优先以降级兼容方式使用 OpenAI-compatible endpoint。只有真正无法安全表示、且请求缺少可见上下文的 provider-owned state 才应该 fail closed。

## 目标

1. Claude Code 使用 OpenAI-compatible Chat Completions upstream 时，普通对话、streaming、工具循环和编辑工具应能正常工作。
2. Anthropic-only 顶层控制不再一锅 hard reject；按 `map` / `warn+drop` / `hard fail` 三类处理。
3. Prompt cache 支持只使用 provider-side 机制。`llmup` 可以保留、显式映射或在严格条件下合成目标 provider request hint，但不保存、不查找、不复用模型响应。
4. Reasoning / thinking 优先保留安全请求 hint 和 unsigned plain visible thinking text；signed、omitted、redacted 或 opaque carrier 不跨 provider 伪造，Phase 1 继续 hard fail。
5. E2E gate 不再把正向可用性路径藏进 `expected_fail_closed`。
6. 设计和配置保持低心智负担：默认最大兼容，不新增用户可见兼容等级。

## 参考资料

官方资料：

- Anthropic Messages API 是 stateless，客户端应发送完整 conversation history：<https://platform.claude.com/docs/en/build-with-claude/working-with-messages>
- Anthropic context editing 是 server-side 清理 tool results / thinking blocks：<https://platform.claude.com/docs/en/build-with-claude/context-editing>
- Anthropic extended thinking 包含 `thinking` block、`signature` 和 redacted thinking 的 round-trip 约束：<https://platform.claude.com/docs/en/build-with-claude/extended-thinking>
- Anthropic prompt caching 支持 top-level / block-level `cache_control`，thinking blocks 不能直接标记 cache：<https://platform.claude.com/docs/en/build-with-claude/prompt-caching>
- OpenAI prompt caching 自动启用，并可用 `prompt_cache_key` / `prompt_cache_retention` 影响 cache routing / retention：<https://developers.openai.com/api/docs/guides/prompt-caching>
- OpenAI Chat Completions 暴露 `prompt_cache_key`、`prompt_cache_retention`、`reasoning_effort`：<https://platform.openai.com/docs/api-reference/chat/create>
- OpenAI Responses `previous_response_id` / `conversation` 是 provider-managed state：<https://developers.openai.com/api/docs/guides/conversation-state>

开源实践参考：

- clawgate：把 Anthropic client / Claude Code 翻译到任意 OpenAI-compatible backend，主打 streaming 和 tool use：<https://clawgate.org/>
- claude-code-proxy：Claude Code -> OpenAI-compatible API proxy：<https://github.com/fuergaosi233/claude-code-proxy>
- claude2openai-proxy：Anthropic `/v1/messages` -> OpenAI/LiteLLM，支持 content block、tool、SSE 转换：<https://github.com/ziozzang/claude2openai-proxy>
- UniClaudeProxy：支持 Claude Code 到 OpenAI-compatible / Responses / Anthropic passthrough，提供 reasoning block 和工具桥接参考：<https://github.com/vibheksoni/UniClaudeProxy>
- raine/claude-code-proxy：强调 Claude Code streaming fallback、context window 与 prompt cache key/session id 处理：<https://github.com/raine/claude-code-proxy>

这些项目的共同启发：Claude Code -> OpenAI-compatible 应是正向可用路径；实现上普遍采用降级、streaming event 转换和工具 round-trip，而不是遇到 provider-native 顶层字段就整体 400。

不直接照搬的部分：

- 不把 tool call / tool result 默认退化成普通文本；只有结构化转换确实不可能时，才考虑显式 fallback。
- 不引入 ReAct XML、per-model 大量 knobs、agent runtime、guardrail/interceptor 平台、cache-aware routing 或 provider fallback DSL。
- 不硬编码某 provider/model 总是开启 reasoning、总是强制 tool choice、或自动注入 beta header；这些必须来自 explicit request、surface 或 provider-native same-wire handling。

## 当前 Codebase 判断

关键现状：

- `src/translate/internal/assessment.rs` 当前把 Anthropic 顶层 `container`、`thinking`、`context_management` 放在 hard reject 集合里。
- `src/translate/internal.rs` 的 `claude_to_openai()` 已经会构造 OpenAI Chat request；只要 assessment 放行，顶层 Anthropic-only 字段默认不会进入目标 request。
- `tool_choice`、`disable_parallel_tool_use`、普通 `tool_use` / `tool_result` 已有基础映射。
- Anthropic -> OpenAI Chat 当前会生成 system message，但后续 maximum-safe role repair 可能把 system/developer 降成 user 并合并相邻 user string；这可能影响 OpenAI prompt-cache prefix 稳定性，需要在 Phase 3A 审计。
- `ConversationStateBridgeStore` 已存在，但只覆盖 OpenAI Responses `resp_llmup_* previous_response_id` 本地 replay，不覆盖 Anthropic container/context state。
- `src/prompt_cache_controls.rs` 已支持显式 `extra_body.openai.prompt_cache_key` / `prompt_cache_retention` 从 Anthropic request 映射到 OpenAI-family target。
- `scripts/real_cli_matrix.py` 当前把 Claude client 到非 Anthropic upstream 统一归类为 `expected_fail_closed`，导致 gate 没能证明正向可用。

关键缺口：

- `thinking` / `context_management` 过度 hard reject，直接阻塞 Claude Code。
- 没有区分“可降级 provider hint”和“真正需要 provider-owned state 的 opaque handle”。
- Claude Code -> OpenAI-compatible 只能依赖 OpenAI 自动 prompt caching 和现有显式 `extra_body.openai.prompt_cache_key`；对 Claude Code 这种不能自然发送 OpenAI extra fields 的 client，尚无受控的 target-provider cache hint 策略。
- E2E 报告没有区分“正向可用性通过”和“负向 fail-closed 通过”。

## 与现有文档合同的关系 / 文档同步 gate

本计划不是新增产品模式，而是对旧 fail-closed / cache / context-management 边界的下一步有界修订。开发 handoff 必须同步旧文档，避免同一代码库同时存在相反合同。

- `docs/protocol-compatibility-matrix.md` 和 `docs/protocol-baselines/capabilities/state-continuity.md` 当前仍把 Anthropic top-level `container` / `thinking` / `context_management` 写成 cross-provider fail-closed / same-wire only；state-continuity baseline 也把 containers 作为非 portable 合同。本计划将其修订为：请求仍含完整可见 history 时，Phase 1 对 top-level `thinking` / `context_management` 和普通无资源 `container` hint `warn+drop`；provider-owned resource、provider-owned container runtime、opaque-only state、缺失 history 仍 hard fail。Phase 1 行为变更合并前必须同步这两份合同，尤其要把旧 containers 非 portable 口径同步为“普通无资源 container hint 可降级，provider-owned resource/container runtime 仍 hard fail”。
- `docs/engineering/pre-ga-request-processing-prompt-cache-support-plan.md`、`docs/protocol-baselines/capabilities/cache.md` 和 `docs/max-compat-design.md` 仍强调不自动生成 OpenAI `prompt_cache_key`。Phase 3A 继续遵守该合同；Phase 3B 是受控合成 target-provider hint 的有界修订，进入实现前必须先同步这些文档并完成独立 follow-up review。
- `docs/engineering/pre-ga-conversation-state-bridge-plan.md` 和 state-continuity baseline 已定义现有 state bridge 范围。Phase 4 不是 Conversations API 模拟，也不是持久 state bridge 扩容；它只能是无 LLM、无持久状态、输入含完整可见 history 的最小 context editing adapter。进入 Phase 4 前必须先同步边界文档并完成独立 follow-up review。

## 设计原则

1. 默认最大兼容。只要请求含有完整可见 history，就优先转换、降级、警告，而不是 fail closed。
2. Fail closed 留给真正不可替代的 provider-owned state：opaque-only reasoning、Phase 1 内 signed/omitted/redacted thinking、外部 provider state ID、server-side container/tool 资源、hosted prompt/resource 生命周期。
3. 不引入 `llmup` cache。任何 cache 增强都是 provider request field / usage telemetry。
4. 不用 LLM 或自然语言判断内容是否“稳定”。cache hint 只能来自显式 provider-native 字段，或来自结构化、确定性、可解释的 route/static-prefix 指纹。
5. 不为了 cache 命中重排 message/tool 顺序。cache 优化必须服从协议语义。
6. 配置面保持小。新增能力默认内建，只有高风险 provider feature 才通过 model surface 或显式 `extra_body` 启用。
7. TDD 开发。每个行为变更先有失败测试，再实现。

## 字段处理策略

本文中的“完整可见 history”是机械条件，不是新状态层：请求的 `messages` 非空，当前轮所需的对话和工具结果都在 `messages` 中以 portable content 表示，并且没有 provider-owned container、server tool、context resource/compact handle 或其他外部 provider state 引用。任一条件不满足时，不能按“完整可见 history”降级。

### Anthropic -> OpenAI-compatible request controls

| 字段/形态 | 目标行为 | 原因 |
| --- | --- | --- |
| top-level `thinking` 且请求满足完整可见 history | Phase 1 默认 `warn+drop`；Phase 2 在目标 surface 支持时可 map 到 OpenAI `reasoning_effort` | Anthropic token budget 与 OpenAI effort 不等价，但不应阻塞普通请求 |
| top-level `context_management` 且请求满足完整可见 history | Phase 1 `warn+drop`；Phase 4 可做最小本地 context editing | 官方语义是 server-side context edit；有完整 history 时可先降级，避免普通 Claude Code 请求直接 400 |
| `context_management` 依赖 provider compact/resource/opaque state，或请求缺少完整可见 history | hard fail | 目标 provider 无法重建缺失上下文 |
| top-level `container` 为空、disabled/null 或无 server resource 依赖，且请求满足完整可见 history | `warn+drop` | 容器 hint 不应阻塞可见 transcript 请求 |
| provider-owned `container.id`、`container.skills` / code execution / MCP / provider server tools | hard fail | 需要 Anthropic provider-owned runtime |
| unsigned plain visible `thinking` blocks | Phase 1 仅保留/降级这类可见文本为 target reasoning side channel 或普通上下文 | 可见内容是 portability floor |
| signed `thinking`（`signature` present）、omitted/non-string thinking、`redacted_thinking` | Phase 1 hard fail；未来只能进入 Phase 2 或独立 reasoning/matrix follow-up，且先同步 `docs/protocol-compatibility-matrix.md` | 不跨 provider 伪造或静默丢弃 opaque/signed/redacted carrier |
| Anthropic `cache_control` | 到 OpenAI target 默认 `warn+drop`，除非显式 `extra_body.openai` | 与 OpenAI `prompt_cache_key` 语义不同 |
| `extra_body.openai.prompt_cache_key` | 显式映射到 OpenAI-family top-level | 当前已支持，应保留 |
| `extra_body.openai.prompt_cache_retention` | 显式映射，合法值 `in_memory` / `24h` | 当前已支持，应保留 |

工具转换补充：

- assistant `tool_use` 必须优先转为 OpenAI Chat `tool_calls` 或 Responses `function_call`，保留 `id/name/arguments`。
- user `tool_result` 必须优先转为 OpenAI Chat `role:"tool"` 或 Responses `function_call_output`，保留 call id 和结构化输出。
- 并行或交错工具调用要按 id / block index 关联，不能依赖出现顺序猜测。
- 只有在目标协议确实无法表示且请求仍有可见上下文时，才允许显式 warning 后退化为普通文本；这不是默认路径。

### OpenAI prompt cache hint

OpenAI prompt caching 已自动启用，命中依赖稳定的 prompt prefix；官方最佳实践要求静态或重复内容放在开头，并一致使用 `prompt_cache_key`。本计划分两步处理，避免把目标 provider hint 误写成 `llmup` cache。

Phase 3A 先做无争议的稳定性和显式映射：

- 保持 Anthropic -> OpenAI-family 翻译后的 prefix 稳定：system/developer 内容、tool 定义、schema 序列化、message/tool 顺序不得引入 request id、trace id、时间戳、随机数或 `previous_response_id`。
- 保留已有显式 `extra_body.openai.prompt_cache_key` / `prompt_cache_retention` 映射。
- 不把 Anthropic `cache_control`、TTL、`max_tokens: 0`、conversation id、container id 或 `resp_llmup_*` 转成 OpenAI `prompt_cache_key`。
- 不自动设置 `prompt_cache_retention`；只有用户显式提供时才传给 OpenAI-family target。
- 评估 OpenAI Chat role repair 对 prefix 稳定性的影响。若 role repair 把 system/developer 合并为 user，必须确认这对目标 provider 是必要兼容 shim，否则不要为了 shim 破坏稳定前缀。

Phase 3B 只有在 Phase 3A 后仍无法让 Claude Code 场景稳定获得 OpenAI cache routing benefit 时才进入，并且必须先同步修订 prompt-cache 计划：

- 仅在 target 是 OpenAI-family 且 request 没有显式 `prompt_cache_key` 时考虑。
- 初始只面向 Anthropic/Claude Code -> OpenAI-compatible translation。
- 生成的是 OpenAI provider request field，不是 `llmup` cache key，不代表缓存内容可由 `llmup` 查询或复用。
- 不设置 `prompt_cache_retention`，除非用户显式提供。
- 不从最后用户消息、request id、时间戳、随机数、provider credential、Anthropic `cache_control`、conversation/container id、`previous_response_id` 或 `resp_llmup_*` 派生。
- 允许的 key source 只包括 namespace/upstream/model/protocol/static-prefix 的 canonical digest，以及 wrapper 或请求显式提供的 stable project/session hint。
- digest 不写入普通日志；debug trace 只记录 `target_prompt_cache_hint_synthesized: true`、source component names、短 fingerprint，不记录完整 key。
- 如果无法满足字段长度、字符集、隐私和稳定性要求，必须 omit+warning，不得 fail ordinary requests。

Phase 3B 是对旧 prompt-cache 计划“永不生成 OpenAI `prompt_cache_key`”规则的有界修订；它不得引入 `llmup` cache、cache lookup、response reuse、语义 cache、cache-aware routing、fallback routing 或跨 provider cache handle 互转。

### Reasoning / thinking

Phase 1 目标是可用性：

- 请求满足完整可见 history 时，top-level Anthropic `thinking` 先 `warn+drop`。
- 只有 unsigned plain visible thinking text 可在 Phase 1 尽量保留为 OpenAI-compatible side channel，例如已有的 `reasoning_content`，或在目标不支持时作为普通 assistant context。
- signed thinking（`signature` present）、omitted/non-string thinking、`redacted_thinking` 和 opaque-only thinking 在 Phase 1 仍 fail closed，不做 warn+drop 降级。
- SSE translation 继续保留 Anthropic event lifecycle：`message_start`、`content_block_start/delta/stop`、`message_delta`、`message_stop`；OpenAI-compatible streaming 侧要按 block id/index 跟踪 text/thinking/tool，避免并行工具调用串线。

Phase 2 目标是经济和能力：

- 增加最小 `ModelSurface.reasoning`，只表达 target 是否支持 OpenAI Chat `reasoning_effort` / Responses `reasoning.effort`。
- 如果 surface 明确支持，将 Anthropic `thinking.type=adaptive` / `enabled budget_tokens` 启发式映射为 `reasoning_effort`。
- 默认不向未知 OpenAI-compatible provider 注入 reasoning field，避免上游 400。
- 支持显式 `extra_body.openai.reasoning_effort` 覆盖，非法值 fail closed。
- signed/omitted/redacted thinking 的任何放宽都必须作为 Phase 2 子任务或独立 reasoning/matrix follow-up，先同步 `docs/protocol-compatibility-matrix.md` 并明确 round-trip / 降级语义；未完成前沿用 Phase 1 hard fail。

## 开发阶段

### Phase 0: TDD 锁定当前 bug 和 gate 口径

目标：先证明当前行为不满足产品目标。

任务：

- 固化文档同步 gate：Phase 1 必须同步 compatibility/state-continuity 合同，覆盖 top-level `thinking` / `context_management` / 普通无资源 `container` hint；Phase 3B 和 Phase 4 必须先走独立 follow-up review。
- 新增 Rust test：Anthropic request 带 top-level `thinking` / `context_management` / 普通无资源 `container` 转 OpenAI Chat 不应被 assessment reject。
- 新增 translation test：同一 request 翻译后不含 `thinking` / `context_management` / `container`，但保留 messages/tools/model。
- 新增 proxy-level test：`/anthropic/v1/messages` 选择 OpenAI-compatible alias 时不返回本地 400。
- 修改 Python matrix test：Claude -> `preset-openai-compatible` 不再 blanket `expected_fail_closed`。
- 把负向 fail-closed case 移到单独 negative suite 或显式 `must_fail_closed` expectation。

验收：

- 当前实现先红，修复后绿。
- 矩阵报告能明确区分 positive usable pass 和 negative fail-closed pass。
- 初始开发 handoff scope 明确为 Phase 0 + Phase 1 + Phase 5；Phase 3B / Phase 4 不混入首批实现。

### Phase 1: 降级放行 Claude Code 常见 Anthropic controls

目标：让 `scripts/run_claude_proxy.sh --model preset-openai-compatible` 的普通 `hi` 不再 400。

任务：

- 同步 `docs/protocol-compatibility-matrix.md` 和 `docs/protocol-baselines/capabilities/state-continuity.md`，把 Anthropic top-level `thinking` / `context_management` / 普通无资源 `container` hint 从 blanket fail-closed 改成“完整可见 history 下 warn+drop”；把 provider-owned resource/container runtime、opaque/missing-history 继续标为 hard fail。旧 state-continuity baseline 中 containers 非 portable 合同必须同步为“普通无资源 container hint 可降级，provider-owned resource/container runtime 仍 hard fail”。
- 将 top-level `thinking` / `context_management` 从 `anthropic_nonportable_request_controls_for_translate()` 移到 warning-only 分类。
- 将 `container` 拆分为：
  - `warn+drop`：请求满足完整可见 history，且无 skills/code-execution/MCP/server resource 依赖的普通 container hint。
  - hard fail：明确需要 provider-owned runtime 的 container/server tool。
- 确保 `claude_to_openai()` 构造 request 时不会泄露 Anthropic-only 顶层字段。
- warning headers/debug trace 记录字段被 drop 的原因。
- 保留 hard fail：signed thinking（`signature` present）、omitted/non-string thinking、`redacted_thinking`、opaque-only thinking、provider-owned container/server tools、unsupported media/source、invalid tool schema。

验收：

- Claude Code -> OpenAI-compatible smoke 可返回。
- 带 top-level `thinking` / `context_management` / 普通无资源 `container` hint 的 request 有 warning，没有本地 400。
- 带 provider-owned container/server tool 的 request 仍在上游前 fail closed。
- 带 signed/omitted/redacted/opaque-only thinking 的 request 仍在上游前 fail closed。
- 旧合同文档不再与 Phase 1 行为相反。

### Phase 2: Reasoning 最大安全映射

目标：在不破坏 OpenAI-compatible 普适性的前提下，尽量保留 reasoning 能力。

任务：

- 设计最小 `ModelSurface.reasoning` 字段，不引入兼容等级：
  - `supports_openai_reasoning_effort: bool`
  - 可选 `allowed_efforts`
- 支持显式 `extra_body.openai.reasoning_effort`。
- 当 surface 明确支持时，把 Anthropic top-level `thinking` 映射为 OpenAI reasoning hint：
  - `adaptive` + effort -> 对应 effort。
  - `enabled budget_tokens` -> bounded heuristic：小预算 `low`，中等 `medium`，大预算 `high`；不生成 `xhigh`，除非显式请求且 surface 允许。
  - `disabled` -> `none` / omit，取决于 surface 允许值。
- 对未知 target 默认继续 `warn+drop`，不注入 reasoning field。
- unsigned plain visible thinking blocks 继续保留；signature/omitted/redacted/opaque carrier 未完成矩阵同步前仍不跨 provider。

验收：

- 未声明 reasoning support 的 OpenAI-compatible target 不多出 reasoning field。
- 声明 support 的 target 收到合法 `reasoning_effort`。
- 非法 explicit effort fail closed。
- signed/omitted/redacted/opaque-only reasoning request 在未完成矩阵同步前仍 fail closed。

### Phase 3: OpenAI provider prompt-cache hint

目标：提升 Claude Code -> OpenAI-compatible 的 provider-side prompt cache 命中概率，且不实现 `llmup` cache。

任务：

- Phase 3A：先实现 prefix 稳定性审计和显式 provider field 路径。
  - 保留显式 `extra_body.openai.prompt_cache_key` 优先级。
  - 不自动设置 `prompt_cache_retention: "24h"`。
  - 确认翻译后的 tools/system/message 顺序稳定。
  - 为 translated route 增加 OpenAI `cached_tokens` 可见性检查；`provider_cache_usage` 仍只在能拿到真实 raw upstream usage 时输出。
- Phase 3B：如进入目标 provider hint synthesis，再实现 deterministic static-prefix hint。
  - 先同步 `docs/engineering/pre-ga-request-processing-prompt-cache-support-plan.md`、`docs/protocol-baselines/capabilities/cache.md`、`docs/max-compat-design.md`，并完成独立 follow-up review；未完成则不得实现 Phase 3B。
  - 在 Anthropic -> OpenAI-family translation 完成后、upstream request 前，如果没有显式 `prompt_cache_key`，生成目标 provider hint。
  - 初版只对 system prompt + tools 做 canonical digest，不包含最后用户消息。
  - key 格式使用稳定短前缀，例如 `llmup:v1:<namespace-fp>:<upstream-fp>:<model-fp>:<static-prefix-fp>`。
  - key 长度或字符集不满足目标 provider 要求时 omit+warning，不让普通请求 fail。
  - Debug trace/hook 增加新 disposition，例如 `target_hint_synthesized`，但不记录完整 key。

验收：

- Phase 3A：显式 key 不被覆盖，Anthropic `cache_control` 仍不会被伪装成 OpenAI key。
- Phase 3A：翻译不引入随机前缀扰动；同一输入的 upstream tools/system/message 前缀 byte-stable 或 canonical-stable。
- Phase 3B：同一 alias、同一 system/tools、不同 user prompt 生成相同 `prompt_cache_key`。
- Phase 3B：system/tools 改变时 key 改变。
- Phase 3B：key 不含原文、provider key、request id、timestamp、conversation/container id、`resp_llmup_*`。
- Phase 3B：旧 prompt-cache 合同已同步为有界修订，不再同时声称“永不生成”。
- `provider_cache_usage` 仍只是 usage telemetry，不驱动 routing/cache。

### Phase 4: 最小 Anthropic context editing adapter

目标：只在需要时实现轻量本地 context editing，不做 LLM summarization。

触发条件：

- Phase 1/2 后真实 Claude Code 长上下文或工具循环仍因 `context_management` 降级过度导致明显失败。
- 已先同步 `docs/engineering/pre-ga-conversation-state-bridge-plan.md`、`docs/protocol-baselines/capabilities/state-continuity.md` 和 compatibility matrix，并完成独立 follow-up review。

任务：

- 明确 Phase 4 与现有 state bridge 的边界：不保存跨请求状态，不消费 provider state handle，不模拟 Conversations API，不做摘要/compaction 产品化。
- 只支持可确定的 edit：
  - `clear_tool_uses_20250919`：按 Anthropic 官方策略清理旧 tool results，替换为可见 placeholder。
  - `clear_thinking_20251015`：只删除旧 unsigned plain visible thinking blocks，保留最近 N turn。
- 不实现 server-side compaction、SDK compaction、摘要生成、任务预算、跨请求容器状态。
- signed/omitted/redacted thinking 仍不由 Phase 4 顺手放宽；如需处理，必须先走 Phase 2 或独立 reasoning/matrix follow-up。
- 输入必须满足完整可见 history；否则 fail closed。
- 所有 edit 都在 translation 前发生，并记录 warning/trace。

验收：

- 只有支持的 edit type 被执行。
- 未知 edit type、compact、opaque-only state fail closed。
- 编辑后 transcript 顺序仍满足 OpenAI Chat tool call/result 约束。
- state bridge / context-management 文档已同步，不把 Phase 4 误写成持久状态桥或 provider-owned state 重建。

### Phase 5: Real CLI / E2E 矩阵收敛

目标：让 gate 证明真实可用，而不是证明“按预期拒绝”。

正向 required cases：

- `claude__preset-openai-compatible__smoke_pong`
- `claude__preset-openai-compatible__tool_identity_public_contract`
- `claude__preset-openai-compatible__public_editing_tool_workspace_edit_contract`
- 保留 `claude__preset-anthropic-compatible__*` 作为 native baseline。
- 保留 Codex -> OpenAI-compatible baseline，避免修 Claude 时破坏 Codex。

负向 cases：

- provider-owned container/server tool。
- signed/omitted/redacted/opaque-only thinking。
- unsupported media/source。
- invalid explicit `extra_body.openai` cache/reasoning field。

验收：

- positive suite 中 `expected_fail_closed > 0` 时整体失败。
- negative suite 中 fail-closed 不计入 positive pass。
- 发布说明不能再写“E2E 通过”而不区分 positive/negative。

## 测试清单

Rust focused tests：

Phase 0/1 初始 handoff：

- `assess_request_translation_*claude_to_openai*_thinking_context_management_container_warns`
- `translate_request_claude_to_openai_drops_top_level_controls`
- `translate_request_claude_to_openai_rejects_provider_owned_container`
- `translate_request_claude_to_openai_preserves_unsigned_plain_visible_thinking`
- `translate_request_claude_to_openai_rejects_signed_thinking_phase1`
- `translate_request_claude_to_openai_rejects_omitted_or_redacted_thinking_phase1`
- `translate_request_claude_to_openai_rejects_opaque_only_thinking`

Phase 2 follow-up only：

- `translate_request_claude_to_openai_reasoning_effort_surface_gate`
- signed/omitted/redacted thinking 的任何新降级测试只能在 protocol compatibility matrix 同步后加入。

Phase 3A follow-up only：

- `translate_request_claude_to_openai_preserves_prefix_stability_for_prompt_cache`
- `translate_request_claude_to_openai_preserves_explicit_prompt_cache_key`

Phase 3B follow-up only：

- `translate_request_claude_to_openai_synthesizes_prompt_cache_key`

Python / script tests：

Phase 0/5 初始 handoff：

- `tests/test_real_cli_matrix.py` 移除 Claude non-Anthropic blanket expected-fail。
- positive/negative suite 汇总逻辑测试。
- report markdown/json 明确展示 usable pass 与 expected fail-closed。

Manual / real CLI：

Phase 5 初始 handoff：

- `scripts/run_claude_proxy.sh --model preset-openai-compatible --proxy-port <free-port>` 后发送 `hi`。
- Claude Code workspace edit fixture。
- Streaming tool-use fixture。

Gate 原则：

- 每次只跑相关小范围测试。
- 行为稳定后再跑全量 `cargo test` / Python suite / clippy。

## 风险与边界

- 降级 `context_management` 可能增加 token 使用或导致长上下文失败；这是比普通请求直接 400 更符合最大兼容的初始取舍。
- 合成 `prompt_cache_key` 可能改变 OpenAI routing bucket；Phase 3B 必须保持稳定、短、不可包含敏感原文，并允许显式 key 覆盖。Phase 3A 不合成 key。
- 对未知 OpenAI-compatible provider 注入 reasoning field 可能导致上游 400；因此必须由 surface 或显式 extra_body 控制。
- 不模拟 Anthropic code execution / skills / MCP / server tools；这些需要 provider runtime。
- 不为了 cache 命中而改变 tool order、message order 或 schema 内容。

## Handoff 顺序

1. 初始开发 handoff 推荐只做 Phase 0 + Phase 1 + Phase 5：修复当前 Claude Code -> OpenAI-compatible 400，并让 gate 证明正向可用。
2. Phase 1 合并前必须同步 compatibility matrix 和 state-continuity baseline，覆盖 top-level `thinking` / `context_management` / 普通无资源 `container` hint，避免旧文档继续声明 blanket fail-closed / same-wire only 或 containers 一律非 portable；signed/omitted/redacted thinking 仍保持 hard fail。
3. Phase 3A cache prefix stability 和显式 key 路径是后续经济性增强；它仍不自动生成 `prompt_cache_key`。
4. Phase 3B synthesized target hint 不属于初始 handoff。必须先同步 prompt-cache 计划、cache baseline、max-compat design，并通过独立 follow-up review。
5. Phase 4 context editing 不属于初始 handoff。必须先同步 state bridge / state-continuity / compatibility 文档，并通过独立 follow-up review；只有真实 Claude Code 行为证明需要时才做。
6. Phase 2 reasoning mapping 由真实 provider capability 驱动，不阻塞首批可用性修复，也不要和 Phase 3B / Phase 4 绑成大包。

## 非目标

- 不实现 `llmup` response/result cache。
- 不实现 semantic cache。
- 不保存 provider KV cache 或 provider cache resource。
- 不导入外部 `resp_*` / `conv_*` / Anthropic container IDs 作为本地状态。
- 不做持久化、跨进程恢复、admin 会话浏览。
- 不引入用户可见兼容等级或模式开关。
- 不把 `llmup` 发展成 LiteLLM/OpenRouter/Portkey 风格的完整路由产品。
