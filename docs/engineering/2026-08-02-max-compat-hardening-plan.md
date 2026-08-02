# 计划：最大兼容硬化 —— 收敛第 4 个 encrypted 站点 + 消除静默丢弃

状态：**handoff-ready for stable release**（V1-first 策略；压缩安全已核实；稳定发布验收标准已列）
更新时间：2026-08-02
来源：2026-08-01 会话 opaque-state DROP 审计（CONVERGENCE 计划，非新建能力）

> 一句话范围：把 `function_call_output` / `custom_tool_call_output` 输出数组中的 `EncryptedContent` part 的处置从**翻译层 `Err`** 迁到 **assessment 层**（与 `reasoning` / `agent_message` / `compaction` 其余 3 个 encrypted 站点一致），并让**既无 `type` 又无 `role` 的畸形输入项不再静默消失**。不含任何新协议层、新治理文档、新证据记录。KISS/DRY/YAGNI。

## 0. 策略与目标

- **目标**：实现本计划后，多代理兼容功能进入**稳定发布**状态。`agent_message` 已在 `main`（commit `4b1ea7d` + v0.3.1 namespace-tool bridge），叠加本计划的两处审计 gap 修复 + 退化路径一致性补丁，即达成稳定发布条件，按既有 release-identity 流程发 **v0.3.2**。
- **V1 是受支持的混合拓扑**：官方主代理（ChatGPT 登录 + 官方模型）+ llmup 子代理（本地模型），经实测可用——V1 派活、干活、回结果、压缩均通过（`DEEPSEEK_CHILD_OK`）。本计划把 V1 做扎实并稳定发布。
- **V2（`multi_agent_v2`）明确不追求**：它是已知硬限制——OpenAI 用服务端密钥（Fernet 令牌）加密跨代理任务，非 OpenAI 子代理无法解密、本地状态也无法伪造，故 V2 混合不可行（Codex issue #33551）。本计划**不投入 V2 支持**，仅维持 V2 的 `agent_message` 加密形态的现有正确行为："接受 + 告警 + 丢弃"（drop blob + warn，因 Fernet 不可解）。
- **一句话**：把 V1 做扎实并稳定发布；把 V2 的限制讲清楚，不在它上面投入。

## 1. 背景（审计综合，不重新调研）

- portability warning **只在 assessment 层**发出：`src/translate/internal/assessment.rs::TranslationAssessment::warning` → `AllowWithWarnings` → `src/server/errors.rs` 注入 `x-llmup-portability-warning` 头。翻译层 `responses_to_messages` 等返回 `Result<(), String>`（`Err` = 拒绝），**没有 warning 通道**。
- 已发货：`agent_message` 输入翻译（commit `4b1ea7d`）+ namespace-tool bridge（v0.3.1），均已验证。
- 本会话对每个 opaque / non-portable state DROP 做了权威审计：整体姿态 **CONSISTENT**（warn 或 reject，非静默）。仅以下两处是真实 gap。
- **V2 multi-agent 实证（承载性结论，不可推翻，不要再提"抽取 encrypted_content"）**：子代理 `agent_message.encrypted_content` 是 **Fernet token**（`gAAAAAB…`，OpenAI 服务端持有密钥）。任务正文在 Fernet 加密内，llmup 无密钥、本地 state 无法伪造/解密 → opaque。故 `agent_message` 的 drop-blob + warn 是正确姿态。混合部署（官方主代理 + llmup/非 OpenAI 子代理）**只在 V1 多代理可用**（任务以普通 user 消息传递，已实测 `DEEPSEEK_CHILD_OK`）；**V2 被根本性阻断**（Codex issue #33551）。

## 2. 范围划分（什么真正属于本次 CONVERGENCE）

| 项 | 归类 | 说明 |
|---|---|---|
| 1. 第 4 个 encrypted 站点归 assessment | **核心** | `function_call_output`/`custom_tool_call_output` 输出数组中的 `EncryptedContent` part：从翻译层 `Err`（`tools.rs:189-221` 的 `Some(other) => Err` 臂）迁到 assessment 层 warn/reject，镜像 `agent_message`/`reasoning`/`compaction` |
| 2. 畸形 no-type/no-role 输入项 | **次要（小）** | 当前两层都静默跳过（`openai_responses.rs:817-820` `continue` + `assessment.rs:1034` `?`-skip）；加 assessment reject 使其浮现 |
| 3. 测试（TDD） | **核心** | 见 §5；契约测试先行（红）→ 实现 → 绿 |
| 4. 隔离混合 E2E 回归脚本 | **核心（轻量）** | mock-父 + llmup-子 脚本/测试：断言 V1 明文路径通过 + 记录 V2 encrypted `agent_message` drop+warn 限制。**不是重型 harness** |
| 5. V1/V2 客户指引 | **小** | 附录 A 内容；建议实现期在 `docs/clients.md` 多代理章节新建一段（edit site，本计划只写 md） |

### 审计已确认**不是 gap**（不在本次范围，勿"修复"）

- `_ => {}` catch-all（`openai_responses.rs` 输入 dispatch 末尾）：**fail-closed 已生效**——assessment 先于翻译拒绝未知 `type`（`assessment.rs:1072-1074` 的 "outside the portable cross-protocol subset"）。
- 响应侧良性丢弃（`system_fingerprint`、未知 message content part）：非 opaque，不在此计划。
- hosted tool 定义：warn-and-omit（已正确）。
- hosted call/output 项 + 有状态控制面（`store` 等）：reject（已正确）。
- 采样类控制（temperature 等）：warn（已正确）。
- `agent_message` drop-blob + warn：正确，不动。

## 3. 关键 max-compat 决策（第 4 个 encrypted 站点：warn-and-drop vs reject）

**推荐：warn-and-drop（当输出同时含文本 part 时）；reject/fail-closed（当输出 encrypted-only、无任何可见文本结果时）。**

依据（与 CONSTITUTION Invariant #9 "no safe representation" 完全对齐）：

- **有文本幸存（text + encrypted）**：丢弃 encrypted blob、保留文本 part，请求继续 + assessment warning（经 `x-llmup-portability-warning` 头透出）。文本即"安全可移植表示"，符合 warn-and-omit > hard-reject。镜像 `agent_message`（信封头幸存 → warn+drop blob）。
- **encrypted-only（无文本）**：整个工具结果都在 opaque blob 里。**空串工具结果不是忠实表示**——模型会以为工具返回空，进而重试/幻觉/误判工具失败，**静默腐蚀 agent 循环**。这正是 reasoning/compaction 既有的 "opaque-only fails closed" 先例（`responses_input_reasoning_encrypted_content_requires_native_continuity`：无可见上下文时拒绝）。故 fail-closed + 评估层具名 reject 是**更安全的 max-compat 选择**（把问题显式暴露，而非静默丢结果）。
- **非一致性风险（若选纯 warn）**：encrypted-only-no-text 的工具结果若只 warn 不 reject，将把"翻译层 Err → 静默 drop"的 fragile 反模式换成"评估层 warn → 静默空结果"，并未真正修复"第 4 站点无 warning 时静默"的根因。fail-closed 才让该站点与其余 3 个的姿态自洽。

> 该决策使 4 个 encrypted 站点全部由 assessment 拥有，但**姿态分两类**（非"同一规则"）：
> - **tool-output 站点镜像 `reasoning` + `compaction`**：有可见文本幸存 → warn + drop blob；encrypted-only 无可见表示 → hard-reject。
> - **`agent_message` 为 warn-only**：只要出现 `encrypted_content` 就 warn（它天生承载路由信封，永远有最小可见上下文），**不 reject**。本计划**不改 `agent_message`**，仍属 out-of-scope（见 §2/§7）。
>
> 净效果：无任何 encrypted drop 是静默的或埋在翻译层。

## 4. 改动点（已定位到行）

### 4.1 第 4 个 encrypted 站点（主修复）

| 目的 | 文件:行 | 改动 | 必需? |
|---|---|---|---|
| 新增 warn 谓词（assessment 侧） | `src/translate/internal/assessment.rs`（紧邻 `responses_has_warning_only_encrypted_agent_message` `:798-816`） | 新增 `responses_has_warning_only_encrypted_tool_output(body)`：当某 `function_call_output`/`custom_tool_call_output` 的 `output` 数组**同时含 `encrypted_content` part 与至少一个 text part** 时返回 true | 必需 |
| warn 分派 | `src/translate/internal/assessment.rs:2521-2525`（`agent_message` warn 分派旁） | 追加：`if responses_has_warning_only_encrypted_tool_output(body) { assessment.warning(format!(...encrypted tool output payload opaque to {upstream_format}; text parts kept, encrypted payload dropped...)) }` | 必需 |
| 新增 reject 谓词（assessment 侧，owns 具名处置） | `src/translate/internal/assessment.rs`（紧邻 `responses_nonportable_input_item_message` `:1025-1076`） | 新增 `responses_nonportable_tool_output_message(body, target_format) -> Option<String>`：遍历 `function_call_output`/`custom_tool_call_output` 的 `output` 数组 part：(a) encrypted-only 无 text part → reject（"encrypted-only tool output has no portable representation"）；(b) 其它非 text 非 encrypted typed part（如 `input_image`）→ reject（**沿用现有措辞** "OpenAI Responses tool output arrays containing `{other}` cannot be faithfully translated to {target}; only text arrays are portable." 以保持现有 media-reject 测试绿）；(c) 无 type part → reject | 必需 |
| reject 分派 | `src/translate/internal/assessment.rs:2511-2513`（`responses_nonportable_input_item_message` reject 分派旁） | 追加：`if let Some(message) = responses_nonportable_tool_output_message(body, upstream_format) { assessment.reject(message); }` | 必需 |
| 翻译层：drop encrypted（不再 Err） | `src/translate/internal/tools.rs:189-221` `responses_tool_output_to_openai_tool_content` | 在数组迭代 match 增加 `Some("encrypted_content") => continue,`（丢弃该 part，text part 保留）；`Some(other)`（非 text 非 encrypted）与 `None` 臂**保留 `Err`** 作 defense-in-depth（assessment 应已先拒绝；若被绕过，翻译层仍 fail-closed） | 必需 |
| 翻译层：degraded 路径同向 drop encrypted（一致性补丁，闭合第 2 条 custom-tool-output 路径） | `src/translate/internal/openai_responses.rs:1377-1403` `responses_tool_output_partial_replay_text`（"marked"/降级 custom-tool 回放路径） | 当 ALL text part 为空、且数组含 `encrypted_content` part 时，`:1390` 的 fallback 当前做 `serde_json::to_string(&Value::Array(items.clone()))`，重序列化**整个数组**、泄漏 opaque blob（与 "encrypted payload dropped" warning 文案矛盾——warning 仍触发故非静默，但确有泄漏）。**修复**：在 fallback 重序列化**之前**先过滤掉 `type == "encrypted_content"` 的 part（filter 在 `:1383-1388`，fallback 在 `:1390`），与主路径同一 "drop encrypted_content" 处置，确保任何路径都不泄漏 blob。属结构性一致性修复，非新补丁 | 必需 |
| 白名单 | `src/translate/internal/assessment.rs:915-926` `responses_portable_input_item_type` | **无需改**（`function_call_output`/`custom_tool_call_output` 已在白名单；encrypted 处置走新的 warn/reject 谓词，不改 item 级 portable 判定） | — |

> 注意（回归保护）：现有 `translate_request_responses_tool_output_media_arrays_to_openai_rejects`（`tests/mod.rs:1783-1816`）断言 `err.contains("tool output") && err.contains("input_image")`。reject 迁到 assessment 后，`translate_request` 会把 assessment reject message 作为 `Err` 返回——**保持现有措辞**即可让该测试继续绿；另新增一个断言"该 reject 发生在 assessment 层"的测试（见 §5）。

> **实现不变量（implementation notes，约束 §4.1 谓词与翻译层改动）：**
> - **match 臂顺序（tools.rs）**：新增的 `Some("encrypted_content") => continue` 臂**必须排在** `Some(other) => Err` 臂**之前**，否则 Rust 把 `encrypted_content` 路由进 reject 臂。
> - **非数组 output 短路（assessment 新谓词）**：新增 warn/reject 谓词须对非数组 `output`（缺省 / `None` / 裸 string / object）经 `.and_then(Value::as_array)` 短路返回——`function_call_output` 合法地有 `output: None`（`tools.rs:194`）或裸 string（`tools.rs:219`）。镜像 `responses_has_warning_only_encrypted_agent_message` 模板（`assessment.rs:798-816`）。
> - **混合数组的 reject 谓词优先级**：若 tool-output 数组混排（如 `[encrypted_content, input_image]`，encrypted + 非 text、无 text），按 part 迭代、依序返回首个 reject：无 type → media(`other`) → encrypted-only-no-text。reject 即 reject；混合场景会先命中 media 臂，可接受。

### 4.2 畸形 no-type/no-role 输入项（次要）

| 目的 | 文件:行 | 改动 | 必需? |
|---|---|---|---|
| assessment 侧浮现 | `src/translate/internal/assessment.rs:1033-1034`（`responses_nonportable_input_item_message` 内 `let item_type = responses_input_item_type(item)?;`） | 把 `?`-skip 改为显式判定：`responses_input_item_type(item)` 返回 `None`（即既无 `type` 又无 `role`，见 `openai_responses.rs:170-174` 的派生规则）时，返回 reject message（如 "OpenAI Responses input item lacks both `type` and `role` and cannot be faithfully translated to {target_label}"） | 必需 |
| 翻译层 | `src/translate/internal/openai_responses.rs:817-820` | **保持 `continue`**（防御性；assessment 已先拒绝整个请求）。仅在 assessment 被绕过时不崩溃。无需改 | — |

> 取舍：选 reject（非 warn）——畸形（无 `type`/`role`）输入项无任何可识别语义，"no safe representation"，与 Invariant #9 的 fail-closed 一致。最小改动。

### 4.3 隔离混合 E2E 回归脚本（轻量）

- **目标**：V1 路径的回归守卫 + V2 限制的捕获记录。**不是**跨官方主代理的重型 E2E。
- **落点**（二选一，实现期定）：
  - 优选：`src/translate/internal/tests/mod.rs` 新增一组契约测试，构造 **mock-父输入**（`agent_message` 明文项 = V1 风格 → 子代理收到完整任务文本；`agent_message` 加密项 = V2 风格 → 断言 drop blob + assessment warning）。这与既有 `translate_request_responses_encrypted_agent_message_warns_and_keeps_header_only`（`tests/mod.rs:4231-4286`）同构，零外部依赖、CI 友好。
  - 备选：`scripts/hybrid-multi-agent-regression.sh`（若想端到端跑本地 llmup 子代理 + mock 父请求）。仅在契约测试不足以覆盖时启用；默认不进 CI 必跑。
- **断言**：V1 明文 → 子代理侧 `messages[]` 含完整任务 user 消息（通过）；V2 加密 → assessment `AllowWithWarnings` 且 warning 含 `encrypted_content` + `dropped`，翻译后 body 不含 opaque blob。
- **不构造**官方 OpenAI 主代理（重型、凭证依赖、flaky）。

### 4.4 V1/V2 客户指引（小）

- 本计划**附录 A** 已写入指引全文。
- 实现期 edit site（**本计划只写 md，不在此处改**）：在 `docs/clients.md` 的 Codex 章节下**新建** "Hybrid multi-agent topology (V1)" 子小节（该文件目前无 multi-agent/hybrid 拓扑章节，grep `multi_agent`/`hybrid` 无命中，故无现存段落可"追加"），写入附录 A 的精简版（1 段）：混合部署用 V1（默认），勿开 `[features] multi_agent_v2`。

## 5. 测试（TDD，契约先行）

在 `src/translate/internal/tests/mod.rs` 加（镜像 §4 中既有 `agent_message` encrypted 测试 `:4231-4286` 的结构）：

1. **`function_call_output` 含 `encrypted_content` + text part → 请求继续 + portability warning**：assessment 为 `AllowWithWarnings`，warning 含 `encrypted_content` 与 `dropped`；翻译后 tool content 仅保留 text part，opaque blob 不泄漏（`!serialized.contains("OPAQUE_...")`）。
2. **`function_call_output` encrypted-only（无 text）→ fail-closed reject**：assessment 为 `Reject`，message 明确命名 encrypted-only / 无可移植表示（验证 §3 决策）。
3. **`function_call_output` 含其它非 text 非 encrypted typed part（如 `input_image`）→ assessment 层 reject**（消息含 `tool output` + `input_image`；**回归**现有 `:1783` 测试仍绿，且新增断言该 reject 来自 assessment）。
4. **`custom_tool_call_output` 同上 1/2 行为**（对称覆盖两个 item type）。**定位注记**：该测试须 (a) 指向 **Chat Completions** 上游（非 Anthropic），且 (b) 前置一个 `custom_tool_call` item —— 因为独立、非 degraded 的 `custom_tool_call_output` 发往非 Chat 目标会先被 `custom_tools_not_portable_message`（`openai_responses.rs:976-979`）拒绝，到不了被 patch 的函数；而 "marked" 的 custom 输出走 §4.1 第二行的 degraded 路径。该配对 + Chat 目标镜像既有 text-only 测试 `tests/mod.rs:1742-1780`。
5. **畸形输入项（无 `type` 无 `role`）→ 不再静默**：assessment `Reject`，message 命名 `type`/`role` 缺失。
6. **（回归）text-only `function_call_output`/`custom_tool_call_output` 行为不变**：现有 `translate_request_responses_tool_output_text_arrays_to_openai_text_parts`（`:1742-1780`）及同类 custom 用例继续通过、无 warning、无 reject。
7. **（回归，混合 E2E，§4.3 契约版）** V1 明文 `agent_message` → 子代理收完整任务；V2 加密 `agent_message` → drop blob + warn。

> TDD 顺序：1→2→3→4→5 先红，再按 §4 实现，再绿；6/7 作回归。

## 6. 验证计划

1. 单元/契约：§5 全绿。
2. 门控全绿：
   - `cargo fmt --check`
   - `cargo clippy --all-targets -- -D warnings`
   - `cargo test`
   - `python3 -m unittest discover -s tests -p 'test*.py'`
   - `bash scripts/check-governance.sh`
3. **回归守卫**：`function_call_output`/`custom_tool_call_output` 的 text-only 翻译行为零变化（现有测试不动 + 新增回归测试通过）；media-array reject 测试保持绿（措辞保留）。
4. 不发布本轮范围外的任何文件（无新治理/证据/报告 md）。

## 7. 非目标（明确 OUT OF SCOPE）

- **不抽取/解密 `encrypted_content`**（Fernet + OpenAI 服务端密钥——不可能）。
- **不改 `agent_message` drop-blob+warn**（已正确）。
- **不实现 `/responses/compact` 端点**（服务端 opaque state；既有决策 no）。
- **不改全局姿态**（不转 body-level warning 渲染、不把 warn 改硬拒绝或反之——更大的产品决策，YAGNI）。
- **不加通用 inter-agent 协议层 / 新治理层 / 新证据/报告文档**。
- **不动 `_ => {}` catch-all 与响应侧良性丢弃**（审计确认非 gap）。
- **不修改 `responses_portable_input_item_type` 白名单语义**（encrypted 处置由新谓词承载，不删/加 item type）。

## 8. 压缩（compaction）行为与安全性

> 核实结论（2026-08-01 会话，基于 Codex 源码）：压缩对 V1 llmup 子代理是安全的，**不构成发布风险**。

- **远程压缩端点对 llmup 永不被调用**：Codex 仅对 OpenAI/Azure provider 调用 `/responses/compact`（由 `supports_remote_compaction()` 判定）。llmup 这类自定义 provider 不满足该判定，**从不命中**该端点。
- **llmup 走本地压缩**：撞上下文上限时，Codex 给 llmup 发一个**普通 Responses 请求**（内置 `SUMMARIZATION_PROMPT`），让模型总结历史，再用摘要替换旧历史。llmup 只需像处理任何普通请求一样应答这次"总结"请求即可——**不会 404、不会失败**。
- **超长总结请求的退路**：若总结请求本身超长，Codex 逐条丢弃最旧历史条目再重试，不会卡死。长跑子代理撞上下文上限 → 本地压缩 → 正常工作。
- **触发时机**：由 llmup `/models` catalog 的 `context_window` / `auto_compact_token_limit` 决定（llmup P2 catalog 已提供）。
- **llmup 不实现 `/responses/compact`**（既有决策：该端点依赖 OpenAI 加密的压缩状态，见 §7 非目标）——**这不会造成问题**，因为该端点对 llmup 永不被调用。
- **引用**：`reference/codex/codex-rs/core/src/compact.rs:108-110, 241-394`、`reference/codex/codex-rs/model-provider-info/src/lib.rs:422-424`。

## 稳定发布验收标准

> 以下全部满足即达"稳定发布"（v0.3.2）。开发团队按此核对。

- [ ] **审计 gap 关闭**：`function_call_output` / `custom_tool_call_output` 输出数组中的 `EncryptedContent` part 处置收归 assessment 层（text 幸存 → warn-and-drop；encrypted-only → reject）；畸形 no-type/no-role 输入项 assessment 层 reject（不再静默）。
- [ ] **退化路径一致性修复**：degraded custom-tool 回放路径（`responses_tool_output_partial_replay_text` fallback）在重序列化前过滤 `encrypted_content` part，与主路径同向 drop，任何路径都不泄漏 opaque blob。
- [ ] **TDD 契约测试全绿**：含 text-only 回归、encrypted warn-and-drop、encrypted-only reject、非 text 非 encrypted typed part（如 `input_image`）reject、畸形输入 reject、`custom_tool_call_output` 配对测试（Chat 上游 + 前置 `custom_tool_call`）。
- [ ] **V1 混合 E2E 回归守卫通过**：mock/官方主 + llmup 子，V1 纯文本派活 + 结果回传断言通过（§4.3 契约版）。
- [ ] **全门禁绿**：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`、`python3 -m unittest discover -s tests -p 'test*.py'`、`bash scripts/check-governance.sh`。
- [ ] **`agent_message` 加密形态行为已验证正确**：drop blob + warn（因 Fernet 不可解），不硬拒绝整个请求；行为不变，由测试覆盖。
- [ ] **V1-first 客户指引 + V2 限制已写明**：本计划附录 A（实现期同步到 `docs/clients.md` 多代理/Codex 章节）。
- [ ] **客户原始报错消除**：`agent_message` "outside portable subset" 不再出现（现在接受 + 告警，而非拒绝）。
- [ ] CHANGELOG v0.3.2 条目定稿 —— 记录 (a) `agent_message` 跨 provider 子代理翻译（已在 main, commit 4b1ea7d，消除客户 "outside the portable cross-protocol subset" 报错）与 (b) max-compat 加固（第 4 个 encrypted 站点收归 assessment、退化路径 blob 泄漏修复、畸形输入项显式化）。须在 tag v0.3.2 之前作为 release-identity 流程的一步完成（v0.3.1 release commit 358d46b 先例已将 CHANGELOG 定稿列为独立步骤）。
- [ ] **发布**：按既有 release-identity 流程发 v0.3.2（tag → CI → GHCR → 部署 pullot.com:9998/230）。

## 附录 A：V1-first 客户指引（混合部署：官方主代理 + llmup 子代理）

**推荐配置（混合编排）：**

- **主代理**：官方模型 + ChatGPT 登录，`model_provider = "openai"`。
- **子代理**：`agents/<name>.toml` 里 `model_provider = "pullot"`（指向 llmup），配齐 `base_url` / `wire_api = "responses"` / `env_key`。

**关键约束：**

- **不要启用 `[features] multi_agent_v2`**（用默认 V1）。原因：V2 用 OpenAI 服务端 Fernet 密钥加密跨代理任务，非 OpenAI 子代理解不开 → 子代理收不到任务正文；V1 用纯 user 消息派活，正常工作（实测 `DEEPSEEK_CHILD_OK`）。
- **用"全新上下文"子代理**（不要 full-history fork），避免分叉历史约束。
- **压缩安全（本地）**：长跑子代理撞上下文上限走本地压缩——Codex 发普通总结请求，llmup 正常应答即可（详见"压缩（compaction）行为与安全性"一节），可放心长跑。

**V2 是已知限制（不在本发布投入）：** V2 的跨代理任务以 `agent_message` 输入项传递，其 `content` 含 `encrypted_content`——一个 Fernet token（`gAAAAAB…`），加密密钥由 OpenAI 服务端持有。非 OpenAI 子代理（经 llmup）无法解密，真实任务正文不可恢复；llmup 只能保留可见信封头 + drop blob + 发 portability warning。V2 混合被根本性阻断（Codex issue #33551），本计划不投入 V2 支持。
