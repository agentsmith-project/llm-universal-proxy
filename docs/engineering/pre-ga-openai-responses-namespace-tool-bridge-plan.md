# pre-GA OpenAI Responses Namespace Tool Bridge 研究记录

更新时间：2026-05-29

## 背景

Codex 0.134.0 在 `features.multi_agent=true` 时会暴露 Responses
namespace tool。当前已见到的失败形态：

```json
{
  "error": {
    "code": "invalid_request_error",
    "message": "OpenAI Responses namespace tool `multi_agent_v1` cannot be faithfully translated to OpenAI Chat Completions",
    "type": "invalid_request_error"
  }
}
```

这不是上游模型 key 或路由错误，而是协议表达能力不一致：

- OpenAI Responses 支持 namespace tool。
- OpenAI Chat Completions 的 `tools` 是扁平 function tool 列表。
- `multi_agent_v1` 的语义不是普通函数集合，而是一组带命名空间的 agent lifecycle 工具。

当前产品决策是先在 `llmup-codex` 默认关闭 Codex multi-agent 功能，让客户的主聊天路径尽快可用。后续如果要恢复，需要作为显式 bridge 能力实现，而不是静默假装完全等价。

## 外部事实

调研来源：

- Codex subagents 文档：https://developers.openai.com/codex/subagents
- Codex `multi_agent_v1` 源码：https://raw.githubusercontent.com/openai/codex/main/codex-rs/core/src/tools/handlers/multi_agents_spec.rs
- OpenAI Chat Completions API reference：https://platform.openai.com/docs/api-reference/chat/create
- 相关社区兼容性问题：
  - https://github.com/openai/codex/issues/14242
  - https://github.com/ollama/ollama/issues/15921

Codex 源码里定义了 `MULTI_AGENT_V1_NAMESPACE = "multi_agent_v1"`，并在该 namespace 下挂载类似 `spawn_agent`、`send_input`、`resume_agent`、`wait_agent`、`close_agent` 的工具。Chat Completions 没有 namespace 字段，只能表达一个扁平 `function.name`。

## 当前实现选择

已落地的短期修复：

- `llmup-codex` 仍用 Responses wire API 连接本地 proxy。
- 当选中 alias 的上游不是原生 `openai-responses` 时，launcher 自动追加：

```text
-c features.multi_agent=false
```

- 原生 `openai-responses` 上游不关闭该功能。

这样保留了最强路径的能力，同时让 `openai-chat-completions`、Anthropic 等需要翻译的上游不再遇到 namespace tool 无法翻译的硬失败。

## 是否能通过中间翻译支持

可以实现有限 bridge，但不能称为 faithful general translation。

可行方案是把 namespace tool 暂时扁平化给 Chat Completions：

```text
multi_agent_v1.spawn_agent -> multi_agent_v1__spawn_agent
multi_agent_v1.wait_agent  -> multi_agent_v1__wait_agent
multi_agent_v1.close_agent -> multi_agent_v1__close_agent
```

响应回来时再把扁平名字反向恢复成 Responses item：

```json
{
  "type": "function_call",
  "namespace": "multi_agent_v1",
  "name": "spawn_agent",
  "arguments": "...",
  "call_id": "..."
}
```

这个 bridge 只能在 request scope 内维护映射，不能依赖模型自己稳定记住名字规则。也不能默认开放给所有 namespace tool，否则真实 function 名和扁平化名字会冲突。

## 实现边界

首版若做，建议只允许 `multi_agent_v1`：

- 未知 namespace 继续 fail closed。
- 如果请求里已有同名扁平 function，例如 `multi_agent_v1__spawn_agent`，必须拒绝，避免碰撞。
- bridge 必须覆盖非流式、流式、历史重放和 `tool_choice`，缺一不可。
- 配置上应作为 experimental opt-in，不改变默认安全行为。

需要改动的主要区域：

- `src/translate/internal/tools.rs`
  - 保留 namespace 子工具定义，不再只记录 namespace 名。
  - 新增 namespace bridge context，类似现有 custom tool bridge context。
  - request side 把 whitelist namespace children 展开成 Chat function tools。
  - response side 把扁平 tool call 恢复为带 `namespace` 的 Responses function call。
- `src/translate/internal/openai_responses.rs`
  - history replay 中的 namespace function call 要映射回扁平 Chat tool call。
  - `tool_choice` / allowed tools 要做同样映射。
- `src/translate/internal/assessment.rs`
  - bridge 开启且 namespace 被 whitelist 时不再走当前 non-portable tool fail-closed 分支。
  - 未开启 bridge或未知 namespace 时保持 fail-closed。
- `src/streaming/openai_sink.rs` 和 streaming state
  - streaming tool call event 要携带 namespace。
  - `response.output_item.added`、arguments delta、done、terminal `response.output` 都要恢复 namespace 字段。

## 测试要求

实现前必须补齐以下 contract tests：

- Responses namespace `multi_agent_v1` request -> Chat Completions 扁平 function tools。
- Chat 扁平 tool call -> Responses `function_call`，包含原始 `namespace`。
- streaming Chat tool call -> Responses SSE 全路径包含 namespace。
- Responses input history 中的 namespaced tool call -> Chat history 扁平 tool call。
- `tool_choice` / allowed tools 的 namespaced selector -> 扁平 selector。
- 碰撞测试：请求里同时存在 `multi_agent_v1__spawn_agent` 普通 function 时拒绝。
- 未知 namespace 在未显式支持时继续 fail closed。
- 真实 Codex + fake upstream 的回归 smoke，确认 Codex 不再报 namespace translation error。

## 风险

- Chat 模型可能输出 `multi_agent_v1.spawn_agent`、`multi_agent_v1__spawn_agent`、`spawn_agent` 等不同名字，bridge 只能可靠处理 request scope 中登记过的名字。
- 上游模型不一定理解 Codex multi-agent lifecycle 的调度意图，即使协议层能传过去，效果也未必等同原生 Responses 模型。
- 流式路径漏掉 namespace 会导致 Codex 客户端状态机和最终 history 不一致。
- 该能力会扩大协议桥的语义承诺，默认开启前需要真实客户端版本矩阵验证。

## 结论

短期默认关闭 Codex multi-agent 是正确取舍。后续可以实现 `multi_agent_v1` 专用 namespace bridge，但应作为实验能力、白名单启用、完整覆盖 unary/streaming/history/tool_choice，并保留未知 namespace 的 fail-closed 行为。
