# Plan: 支持 Codex 跨 Provider 子代理的 `agent_message` 输入（Pullot 反馈）

状态：**handoff-ready**（经两轮对抗 review 修订；round-2 判定 READY，无阻塞 MUST-FIX）
更新时间：2026-08-01
来源：Pullot（部署在 `pullot.com:9998` → 192.168.0.230 的 llmup）使用者反馈

> v2 修订要点（来自 review）：① 加密 payload 的 portability warning 必须在 **assessment 层**发出（翻译层无 warning 通道）；② **V1 不使用 `agent_message`**——本特性只针对 V2/hosted 风格的 `agent_message`（客户 repro 已证明发射方走 V2）；③ 强制专用 `responses_agent_message_text` 抽取器（不得复用 `map_responses_content_to_openai`，否则会把加密 opaque blob 原样塞进 user 消息）；④ `responses_input_item_has_visible_portable_context` 的 `agent_message` arm 从“可选”升级为**必需**（fork_turns 场景下避免被 reasoning.encrypted_content 连续性校验拒绝）；⑤ 明文/加密形态判定改为**发布前置门控**，而非发布后验证。

## 1. 背景

Pullot 已兼容 Codex CLI 的 OpenAI Responses API、流式、shell/tool calls、MCP namespace tools、multi-agent spawn/wait、Responses `model` 字段（v0.3.1，含 namespace bridge）。

新的失败场景是**跨 Provider 子代理**：

```
OpenAI 官方主代理 → spawn_agent → 创建使用 Pullot 模型的自定义子代理
                                  → Pullot 子代理接收任务（失败）
```

主代理把委派任务以 Responses API 的 **`agent_message` 输入项**传给子代理。Pullot（llmup）当前把 Responses 翻译到 Chat Completions 时不识别 `agent_message`，直接拒绝：

```
OpenAI Responses input item type `agent_message`
is outside the portable cross-protocol subset
and cannot be faithfully translated to OpenAI Chat Completions
```

影响 `ds-flash`、`glm-5.2`、`fork_turns="none"`、`fork_turns=1` 等组合。

> 关于“反方向可用”：原反馈称 Pullot 主代理 → OpenAI 子代理可用。本计划**不依赖该说法**，且需注意：若主代理本身由 Pullot 承载，子→父的 `agent_message`（MESSAGE）会作为主代理历史的入站项再次进入 llmup，同样需要本翻译。即本翻译对两个方向都适用；验收 §11 的 #9 之所以“非 llmup”，仅因为验收场景里主代理是 OpenAI 官方（原生处理）。

**与 v0.3.1 namespace bridge 的关系**：无交互。子代理请求会同时带 `multi_agent_v1` namespace 工具组（v0.3.1 已 bridge，扁平到 `<ns>__<child>`，作用于 `tools[]`）和 `agent_message` 输入项（本计划，作用于 `input[]`）。二者作用在请求的不同部分，互不干扰。

## 2. 根因（已通过源码确认）

- llmup 的 Rust 翻译代码**完全没有引用 `agent_message`**（全仓 `grep` 无命中）。它落到通用拒绝路径。
- 拒绝点：`src/translate/internal/assessment.rs` 的 `responses_nonportable_input_item_message`（错误串 ~1037-1039，与客户报错**逐字一致**）。
- 可移植输入项白名单 `responses_portable_input_item_type`（`assessment.rs:888-898`）是单个 `matches!`，不含 `agent_message`；亦不在 `responses_hosted_input_item_type`（`:856-866`）。
- 输入翻译主循环 `responses_to_messages`（`openai_responses.rs:721`，dispatch :786，catch-all `_ => {}` :1012）没有 `agent_message` 分支。

## 3. 范围划分（什么真正属于 llmup）

| 反馈要求 | 归类 | 说明 |
|---|---|---|
| 1. 接受 `agent_message` 输入项 | **llmup 核心** | 加白名单 + 加翻译分支 |
| 2. 翻译到 Chat Completions | **llmup 核心** | 忠实映射为 user 消息 |
| 3. 返回 Responses 兼容结果 | 已具备（验证） | 响应合成与输入项类型无关 |
| 4. 支持流式 | 已具备（验证） | 入站翻译 stream/非流式共享 |
| 5. 子代理工具调用不被移除 | 已具备（验证） | tools 与输入项独立；namespace bridge/apply_patch/function 照常 |
| 验收 1/2（OpenAI 主代创建 ds-flash/glm-5.2 子代理） | **llmup 核心** |
| 验收 3/4（fork_turns="none"/1） | **llmup**（含 §7 必需的 visible-context arm） |
| 验收 5（两子代理并行） | 已具备（验证） | llmup 每请求无状态 |
| 验收 6（子代理收到完整任务文本） | **llmup 核心 + 关键风险** | 取决于明文/加密（§6） |
| 验收 7（子代理调用 shell 工具并返回） | 已具备（验证） |
| 验收 8（流式正常完成） | 已具备（验证） |
| 验收 9（主代理经 wait 得到子代理结果） | **本场景非 llmup**（见 §11 注意） |
| 验收 10（不再出现 "outside portable subset"） | **llmup 核心** |

**结论：llmup 的实质改动只有一条线 —— 接受并忠实翻译入站 `agent_message`。** 其余是既有能力组合的验证 + 明确排除的部署侧事项。

### 明确排除（部署侧 / 非 llmup）

- `pullot.com` 域名映射、`[model_providers.pullot.auth]` auth-helper、客户的 Codex `agents/*.toml`、官方 OpenAI 主代理行为、模型是否真去执行任务。

## 4. `agent_message` 规范（Codex 源码确认）

来源：`reference/codex/codex-rs/protocol/src/models.rs:836-846`、`protocol/src/protocol.rs:817-850`、`core/src/context_manager/history.rs:795-805`。

**on-wire**（enum tag `type`，snake_case；**无 `role` 字段**）：

```json
{
  "type": "agent_message",
  "id": "msg_...",
  "author": "/root",                 // 发送方 agent 路径（必填）
  "recipient": "/root/worker",       // 接收方 agent 路径（必填）
  "content": [ /* InputText | EncryptedContent */ ],
  "internal_chat_message_metadata_passthrough": {}
}
```

- `content` 元素：`{"type":"input_text","text":"..."}` 或 `{"type":"encrypted_content","encrypted_content":"..."}`。
- **语义**：跨 agent 的**回合边界**。`is_user_turn_boundary` 对它返回 `true`（与“开启新一轮的 user 消息”等价）；`is_model_generated_item` 返回 `false`（非模型自身产出）。
- 双向：parent→child 是 `NEW_TASK`；child→parent 是 `MESSAGE`。
- Codex 对 `agent_message` **原样**发给 `/responses`，不转 user/developer/system。
- `wait_agent` **不携带内容**（只返回状态串）；子代理结果经 `send_message`→`agent_message` 回到主代理历史。

### 4.1 V1 vs V2（review 修正：V1 不涉及）

- **V1（`multi_agent`，Codex 0.146.0 默认）不构造 `agent_message`**。V1 的 `spawn_agent`（`core/src/tools/handlers/multi_agents/spawn.rs`）把任务作为**普通 user 消息**（`UserInput`→`SpawnInitialInput::UserInput`）交给子代理，早已被 llmup 正常翻译。
- **`agent_message` 仅 V2（`multi_agents_v2`）产生**，由 `InterAgentCommunication::to_model_input_item`（`protocol.rs:817-850`）经 `communication_from_tool_message`（`core/src/tools/handlers/multi_agents_v2.rs:57-84`）构造。
- `Feature::MultiAgentV2` 默认关闭（`features/src/lib.rs:1100`）。
- **客户 repro 命中 `agent_message` 拒绝，证明发射方（官方 OpenAI 主代理）走 V2/hosted-agent 语义**。本地子代理 Codex 的 V1/V2 设置与此正交（发射方是主代理，不是子代理）。
- 因此本计划只针对 V2 风格 `agent_message`；V1 无需改动（其任务以普通 user 消息传递）。

### 4.2 content 的两种形态（决定能否完全翻译）

来自 `to_model_input_item`（`protocol.rs:817-850`）与 `communication_from_tool_message`（`multi_agents_v2.rs:57-84`）：

- **明文**：`content=[{input_text, text}]`，text 已是完整信封（`InterAgentMessage::render()` 预渲染）：
  ```
  Message Type: NEW_TASK
  Task name: <recipient>
  Sender: <author>
  Payload:
  <payload>
  ```
  可完全翻译。
- **加密**：`content=[{input_text, "Message Type: …\nPayload:\n"}, {encrypted_content, <blob>}]`。信封头可见，真实 payload 在 opaque blob。这是 `source != DirectPlantextMessage` 时的常见分支。

`plaintext_agent_message_content(content)`（`models.rs:738-750`）：无 `encrypted_content` 且拼接文本非空时返回文本，否则 `None`。

> ⚠ §6：加密形态是本特性的关键风险——opaque payload 跨 provider 不可恢复。

## 5. 忠实翻译设计

**映射**：`agent_message` → Chat **`user` 角色消息**（与 `is_user_turn_boundary` 语义一致；镜像 llmup 已有 `compaction`→user 先例 `openai_responses.rs:998-1011`）。

- **正文抽取**：**强制使用专用 helper `responses_agent_message_text`**（见 §7）：
  - 明文 → 返回 `plaintext` 拼接文本（已含信封，llmup **不再额外加**边界标记；Codex 信封本身就是边界）。
  - 加密 → 仅取 `input_text` 信封头；`encrypted_content` 丢弃（§6 的 assessment warning 配合）。
  - **不得复用 `map_responses_content_to_openai`**——它没有 `encrypted_content` arm，会把 opaque blob 原样塞进 user 消息正文（回归）。
  - **空内容**（`content:[]` 或全空白 → helper 返回 None）：跳过本次 `messages.push`（不产生空 user 消息），保证防御性。
- **顺序**：在输入数组中该项位置 `flush_assistant` + `flush_deferred_user_after_tool_results` 后 `messages.push(...)`（镜像 compaction）。不合并进相邻 developer。
- **不得折叠进 system/developer**：`instructions`→首条 system（`:756-760`）、`developer`→`system` 归一（`internal.rs:650-661` 只动 `type:"message"`）独立机制；`agent_message` 走自己 push，互不干扰。
- **`internal_chat_message_metadata_passthrough`**：llmup 直接忽略（防御性；实际子代理的 Codex client 在发给非 OpenAI 上游前已清除 `client.rs:848-857`，llmup 通常观测不到）。
- **入站专用**：`agent_message` 不出现在出站 `messages_to_responses`（`:1455` 只产 reasoning/message/tool 项）；流式与非流式共享入站翻译。
- **覆盖面**：单个 dispatch arm 自动覆盖 Chat 与 Anthropic 两种上游（Responses→Anthropic 也走 `client_to_openai_completion`→`responses_to_messages`）。同格式 Responses→Responses 直通不触发评估/翻译（`assessment.rs:2421-2423` 早返回），`agent_message` 原样通过，无需改动。

## 6. 加密 payload：关键风险与处理

加密形态下真实任务正文在 opaque `encrypted_content`（为同 provider 往返设计，类似 `reasoning.encrypted_content`），**非 OpenAI 上游无法解密**。

- 命中**明文** → 特性完全可用（验收 6 成立）。
- 命中**加密** → 仅信封头可见，payload 丢失 → 子代理拿不到任务正文，特性不完整。协议层硬限制，非 llmup bug。

**处理（best-effort + 透明，符合 CONSTITUTION 最大兼容与 Invariant #9）**：
- 翻译分支：明文用 helper 全文；加密时取信封头、丢弃 blob（不拒绝、不崩溃）。
- **warning 必须在 assessment 层发出**（翻译层 `responses_to_messages` 返回 `Result<(), String>`，**无 warning 通道**；`x-llmup-portability-warning` 由 assessment 的 `AllowWithWarnings` 经 `server/errors.rs` 注入头）。新增一个 assessment 侧 warn 谓词：请求 input 含带 `encrypted_content` 的 `agent_message` 时 `assessment.warning(...)`（镜像 `responses_has_warning_only_nonportable_tool_definitions`）。`agent_message` 仍在 portable 白名单（不拒绝），让请求继续。
- 服务端 `warn!` 日志可同时记录。

**形态判定 = 发布前置门控（见 §12）**：用客户 repro 抓子代理首请求真实 `agent_message`，判定明文/加密。这决定特性“完全可用”还是“仅信封头可见”。**不得先发布再发现。**

## 7. 改动点（llmup，已定位到行）

| 目的 | 文件:行 | 改动 | 必需? |
|---|---|---|---|
| 白名单（解除拒绝） | `src/translate/internal/assessment.rs:888-898` `responses_portable_input_item_type` | `matches!` 增加 `"agent_message"` | 必需 |
| **可见上下文判定** | `src/translate/internal/assessment.rs:944-953` `responses_input_item_has_visible_portable_context` | 增加 `Some("agent_message")` arm，使 agent_message 计入可见可移植上下文 | **必需**（防御性，非 fork_turns 继承而设——fork 会剥离继承的 `Reasoning`；真实场景：多轮子代理自身累积的 `reasoning.encrypted_content` 与新入站 `agent_message` 同请求共存时，避免触发 `responses_input_reasoning_encrypted_content_requires_native_continuity` 拒绝，见 §8） |
| **加密 warning（assessment 侧）** | `src/translate/internal/assessment.rs`（镜像 `responses_has_warning_only_nonportable_tool_definitions` ~777，`assessment.warning` 先例 ~2468/2482/2489） | 新增 warn 谓词：input 含带 `encrypted_content` 的 agent_message → `assessment.warning(...)`，从而经 `AllowWithWarnings` 发出 `x-llmup-portability-warning` | 必需 |
| 翻译 dispatch 分支 | `src/translate/internal/openai_responses.rs:1012`（`_ => {}` 之前） | 新增 `"agent_message" => { flush_assistant; flush_deferred_user; if let Some(t)=helper(...){ messages.push({role:"user",content:t}) } }`，镜像 compaction | 必需 |
| **专用正文抽取** | `src/translate/internal/openai_responses.rs`（~117-202，其它 `responses_*_summary_text` 旁） | 新增 `responses_agent_message_text`：明文取拼接文本；加密仅取 input_text 信封头；空/全空白返回 None。**不复用** `map_responses_content_to_openai` | 必需 |
| 类型检测 | `openai_responses.rs:170-174` | 无需改（`{type:"agent_message"}` 已识别） | — |
| 出站合成 / 流式 | `openai_responses.rs:1455` / — | 无需改（入站专用 / 共享） | — |
| 测试 | `src/translate/internal/tests/mod.rs`（~4062 compaction 旁） | 见 §9 | 必需 |

实现边界：KISS/DRY，镜像 compaction 既有先例；不加新治理层。

## 8. 边界场景

- **fork_turns="none"**：子代理首请求只有 `agent_message`（+ 注入 developer/tool 上下文）→ 单条 user 消息。
- **fork_turns=1 / "all"**：子代理继承父代理截断/全部 rollout，但 fork 过滤会剥离多项——`core/src/agent/control/spawn.rs:47-81` `keep_forked_rollout_item` 对 `AgentMessage`（`:57`）、**`Reasoning`（`:58`）**、`RolloutItem::InterAgentCommunication`（`:73-74`）均返回 false；`:698-701` 闭包二次剥离 AgentMessage 与 usage 提示。故继承的是普通 user/assistant 轮次（已支持）+ 子代理自己的 agent_message。**继承的 reasoning（含 encrypted_content）不会到达子代理请求。**
  - **reasoning.encrypted_content 交互（visible-context arm 的真实场景）**：fork 不传入 reasoning，但**多轮子代理自身**会在前几轮累积带 `encrypted_content` 的 reasoning；当后续收到一条新 `agent_message`（MESSAGE）时，二者在同一请求共存。评估 `responses_input_reasoning_encrypted_content_requires_native_continuity`（`assessment.rs:1055-1070`）在“有 reasoning.encrypted_content 且无可见可移植上下文”时拒绝。§7 的 visible-context arm 让 agent_message 计入可见上下文 → 请求不被拒绝，加密 reasoning 走既有 drop/warn 逻辑。该 arm 是**必需的防御措施**（非为 fork_turns 继承而设）。
- **并行子代理**：每子代理独立请求，llmup 每请求无状态。
- **多条 agent_message**：按序各译一条 user 消息。
- **与 developer_instructions 共存**：Codex 把 `subagent_developer_instructions` 作为独立 developer 项，与 agent_message 同请求共存（`multi_agent_v2_developer_instructions.rs:208-230` 保证分离非空）。llmup 各自独立 push。
- **历史重放**：映射幂等（同项同映射）。
- **空内容 agent_message**：helper 返回 None → 跳过 push（不产空消息）。
- **同格式 Responses→Responses 直通**：`agent_message` 原样通过，不触发本翻译。

## 9. 测试（契约，镜像 compaction 先例）

在 `src/translate/internal/tests/mod.rs` 加：

1. 明文 agent_message → `messages[]` 恰好 +1 条 `role:"user"`，正文为信封全文；不污染 system/developer；顺序正确。
2. 明文 agent_message + 同请求 developer 消息 → 两条独立消息，不合并。
3. agent_message 后跟 tool_call / tool_output → 不被误当 tool 输出，顺序/role 正确。
4. 加密 agent_message → 译为 user 消息（仅信封头）+ **assessment 发出 portability warning**（断言 `AllowWithWarnings` 含相关告警）。
5. **agent_message + 同请求的 reasoning.encrypted_content**（合成：模拟多轮子代理自身累积的加密 reasoning 与新入站 agent_message 共存）→ **不被拒绝**（验证 §7 visible-context arm）；加密 reasoning 按既有规则 drop/warn。
6. 多条 agent_message → 多条 user 消息，顺序保持。
7. `stream:true` 与 `false` 翻译一致（入站共享）。
8. 空内容 agent_message → 不产生空 user 消息。
9. （回归）不再出现 "outside the portable cross-protocol subset" 拒绝。
10. （回归）与 v0.3.1 namespace bridge 同请求共存：`tools[]` 含 `multi_agent_v1` namespace + `input[]` 含 agent_message → 各自正确处理，互不干扰。

## 10. 开放问题（handoff 前需澄清/实证）

1. **明文 vs 加密（最高优先，发布前置门控）**：用客户 repro 抓真实 `agent_message` 判定形态。决定“完全可用”还是“受限于加密”。
2. ~~V1 vs V2~~：**已澄清**——V1 不使用 `agent_message`（任务以普通 user 消息传递，已可用）；本特性只针对 V2 风格 `agent_message`，客户 repro 已证明发射方走 V2。
3. **加密时是否有 Codex 侧明文开关**：`DirectPlaintextMessage` 触发条件（`tools/router.rs:40-54`：`collaboration` namespace 工具 + `encrypted_function_args.is_empty()`）。父侧（OpenAI hosted）是否发明文属服务端行为，不在本仓——靠 §10.1 实证。

## 11. 验收标准映射

| 验收 | 落点 |
|---|---|
| 1/2 官方主代理创建 ds-flash/glm-5.2 子代理 | §7 解除拒绝 + 翻译 → 通过 |
| 3/4 fork_turns="none"/1 | §7（含 visible-context arm）+ §8 → 通过 |
| 5 两子代理并行 | 已具备，验证 |
| 6 子代理收到完整任务文本 | 明文通过；加密受 §6 限制（**须先 §12 形态判定**） |
| 7 子代理调用 shell 工具并返回 | 已具备，验证 |
| 8 流式完成 | 已具备（入站共享），验证 |
| 9 主代理经 wait 得到结果 | 本场景非 llmup（主代理 OpenAI 原生处理 child→parent agent_message）；**注意**：若主代理也由 Pullot 承载，则 child→parent agent_message 进入 llmup，本翻译同样适用（即 #9 在“Pullot 主代理”场景下回到 llmup 范围） |
| 10 不再出现 "outside portable subset" | §7 直接结果 |

最小验收（明文前提下）：`官方主代理 → {pullot_deepseek→DEEPSEEK_CHILD_OK, pullot_glm→GLM_CHILD_OK} → OFFICIAL_PARENT_OK`。

## 12. 验证计划（形态判定是发布前置门控）

1. 单元/契约：§9 全绿；`cargo fmt`/`clippy --all-targets -- -D warnings`/`cargo test` + `python3 -m unittest discover -s tests` + `bash scripts/check-governance.sh` 全绿。
2. **形态判定（发布前必做）**：在 Pullot（230/pullot.com）或本机隔离 llmup，用客户 repro 触发“官方主代理 spawn Pullot 子代理”，抓子代理首请求的 `agent_message`：
   - 明文 → 验收 6 成立，进入 §13 发布。
   - 加密 → 验收 6 仅信封头；**不得直接发布**。需先与客户对齐预期，并按“明文完全可用 / 加密 best-effort + 文档化 payload 丢失限制”明确发布范围，或调研 §10.3 明文开关后再定。
3. 本机隔离 E2E：CODEX_HOME 隔离 + 本地 llmup，ds-flash/glm-5.2 作子代理 provider，构造官方主代理 spawn Pullot 子代理，验证不拒绝且子代理收到任务（明文前提下完整链路 `OFFICIAL_PARENT_OK`）。

> 关键：单元测试无论明文/加密都会绿（#4 测 best-effort 路径）。**“测试绿” ≠ “特性对该客户可用”**。§12.2 形态判定是防止“过测试却生产失效”的硬门控。

## 13. 发布与回滚

- 改动小（白名单 + 1 翻译分支 + 专用 helper + 1 assessment warn 谓词 + visible-context arm + 测试）。作为下一个 patch 发布（release-identity 当前为 `v0.3.2`，槽位空）。遵循既有 release-identity 流程（CHANGELOG、container-image.json、tag 触发 release.yml），再部署到 230/pullot.com。
- **回滚**：改动集中在一个 commit、无 schema/迁移；回滚 = revert 该 commit + 重新发布上一版镜像。blast-radius 仅限“跨 Provider V2 子代理”路径，不影响普通 Codex 单代理/同 provider 用法。

## 14. 安全与信任边界

`agent_message` 内容跨 agent，被译为上游 user 消息——**与 Codex 原生发给 OpenAI 的语义一致**（Codex 本就把 agent_message 作 user 回合边界）。内容逐字保留（明文）或仅信封头（加密），llmup 不注入额外指令、不冒充角色，**无新的冒充/越权面**。`internal_chat_message_metadata_passthrough` 忽略（不外泄到上游）。信任边界与既有 user 消息翻译相同。

## 15. 非目标

- 不实现 child→parent 方向的额外处理（主代理 OpenAI 原生支持；若主代理是 Pullot 则由本翻译同一机制覆盖，无需额外开发）。
- 不尝试解密 `encrypted_content`（不可能；opaque）。
- 不改部署侧（域名、auth-helper、客户 Codex 配置）。
- 不加新治理层；不扩大为通用 inter-agent 协议层。
