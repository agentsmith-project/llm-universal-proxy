# Pre-GA Conversation State Bridge 工作计划

- 状态：current-main status update；text-only memory MVP 和 route/config owner hardening 已实现，tool/custom tool replay、stream capture、细粒度 trace metadata 仍待完成
- 日期：2026-05-16
- 范围：为 `llmup` 增加显式配置的纯内存会话状态桥，用于把使用状态型接口的客户端转换到无状态 provider 协议
- 非范围：LLM 响应缓存、语义缓存、跨进程持久化数据库、provider 私有 opaque state 反解、默认无配置保存用户数据、后台任务队列产品化、提示词管理产品

## 计划协同

本计划以 [pre-ga-remove-native-gemini-format-plan.md](./pre-ga-remove-native-gemini-format-plan.md) 为 native Gemini 的范围裁剪依据。当前 main 已移除 active native Gemini runtime support；状态桥不为 Gemini `generateContent`、`thoughtSignature`、`cachedContent` 或 `/google/v1beta/*` 增加新能力。

删除 native Gemini 后，状态桥的 active MVP 已实现目标只包括：

- OpenAI Responses stateful client -> OpenAI Chat upstream。
- OpenAI Responses stateful client -> Anthropic Messages upstream。

如果 Gemini 作为 Google OpenAI-compatible upstream 使用，它在 `llmup` 内部属于 OpenAI Chat wire protocol，不需要 Gemini-specific replay。

## 目标

让 Codex-like、OpenAI Responses-like 的状态型客户端，在显式开启的情况下，也能通过 `llmup` 使用 Anthropic Messages、OpenAI Chat Completions 等无状态或手动 replay 型协议。

核心目标：

- 当客户端使用本地 `previous_response_id` 时，`llmup` 可以从自己维护的内存状态中展开可重放上下文。
- 展开后的上下文继续走现有协议转换器，目标 provider 看到的是完整 transcript，而不是 OpenAI Responses 的状态句柄。
- 状态桥只保存会话重放所需的输入/输出事件，不缓存或复用模型响应。
- 默认行为保持 fail closed；只有显式配置启用状态桥的路由才改变现有边界。
- 状态桥是最大安全兼容策略下的显式 state expansion。它会使请求需要构造/转换，必须在 trace 和 warnings 中可见；它不是独立产品行为或用户可选策略。
- 最大安全兼容性仍是唯一实现目标。

一句话边界：这是 `ConversationStateBridge`，不是 cache。

## MVP 范围

为了保持实现简单快速，MVP 当前只做一个能力：

- `POST /openai/v1/responses` translated route 上的 `previous_response_id` continuation。

当前已实现的 MVP 是非流式 text-only continuation：

- 第一轮保存 visible user input 和 completed assistant text。
- 第二轮用本地 `resp_llmup_*` 展开历史，发送到 OpenAI Chat 或 Anthropic upstream。
- 工具调用、custom tool、reasoning summary、streaming capture 是后续阶段；当前 main 尚未实现这些 replay/capture 能力。

初版不做：

- 本地完整 Conversations API 模拟。
- `GET /responses/{id}` / `DELETE /responses/{id}` / cancel 等 Responses lifecycle 模拟。
- `background` 任务生命周期。
- hosted `prompt` 模板展开。
- `context_management` / compact 本地实现。
- 复杂内存配额、LRU、admin state browser、跨进程恢复。

也就是说，MVP 是一个短期内存 replay buffer：收到第一轮 translated Responses 请求后保存可重放 transcript；第二轮带本地 `resp_llmup_*` 时展开历史并继续调用目标 provider。`llmup` 重启、TTL 到期、ID 未命中时，直接 fail closed。

## 背景与现状

OpenAI Responses 和 Conversations 是状态型接口。官方文档描述了两种主要状态模式：

- `previous_response_id`：用上一个 response ID 串联多轮 response。
- `conversation`：使用 Conversations API 保存并检索 conversation items。

Chat Completions 和 Anthropic Messages 的共同基线是显式 transcript replay。客户端或 SDK 通常需要在每次请求里带上完整历史。

当前 `llmup` 支持 OpenAI Responses 请求转换到 Chat/Anthropic，并已为非流式 text-only `previous_response_id` continuation 增加本地内存展开和 route/config owner hardening。Native Gemini 已不是 active runtime surface；Gemini 品牌只能作为 OpenAI-compatible upstream 走 OpenAI Chat wire protocol。外部 provider `resp_*` / `conv_*` 仍不能导入；未知本地 ID、过期 ID、owner mismatch、route/config drift 继续 fail closed。首轮 `store:false` 请求仍会调用上游，但不保存本地状态；之后如果 client 试图用对应历史继续 replay，会因为没有本地状态而 fail closed。

## 当前 Codebase 判断

当前 main 已具备：

- OpenAI Responses 原生 state/resource 路由透传，包括 `/responses/compact`、`/responses/{id}/input_items`、`/conversations/*`。
- 省略 `model` 的 stateful OpenAI Responses 请求可以在唯一、明确的 native Responses upstream 上路由。
- OpenAI Responses 带完整 `input` 时，可以通过 `responses_to_messages()` 转成 Chat-style messages，再进入现有 Anthropic/OpenAI Chat 转换链。Native Gemini 分支已不是 active target。
- `conversation_state_bridge.mode = off | memory`、`ttl_seconds`、`max_bytes` 配置已存在。
- `ConversationStateBridgeStore` 已挂在 `AppState`，使用内存 HashMap、`resp_llmup_*` ID、TTL、全局 `max_bytes` 和 owner hash。
- 非流式 text-only Responses -> OpenAI Chat / Anthropic continuation 已实现：第一轮保存 user text 和 assistant text，第二轮用本地 `previous_response_id` 展开历史后再进入现有转换链。
- `store:false` 首轮仍调用上游但不保存本地状态；未知/过期/owner mismatch 本地 ID fail closed。
- native OpenAI Responses forwarding 保留 provider ID，不导入本地状态。
- `background` / `store` enabled-semantics alignment / translation-boundary detector unification slice 已完成：`background:false|null` 和 `store:false|null` 不再触发 provider-owned stateful fail-closed，`background:true`、`store:true`、`previous_response_id`、`conversation`、`prompt`、`context_management` 仍 fail closed。
- route/config owner hardening 已完成：StoredBridgeResponse 保存内部 route/config fingerprint；continuation 在 upstream dispatch 前按当前 runtime/fingerprint 重新校验，drift 时 400 fail closed；无 `model` 的 single-upstream replay 在配置未变时仍成功。
- namespace revision 采用保守绑定：配置更新会让旧 local state fail closed。这是安全取舍，不做迁移、持久化或 fallback；fingerprint 只是内部保护，不是产品功能或用户配置。
- 文档和测试已经锁定“provider-owned state 不重建”的现有行为。

仍未完成：

- remaining detector work：如需继续提 detector，只限共享 helper、细粒度 trace metadata、以及其它 consolidation，不再把 enabled-semantics 小切片列为下一步。
- proxy-key / trusted tenant policy 下的 owner 隔离策略仍未产品化；当前 memory bridge 依赖 client-provider-key owner hash。
- tool call、custom tool call、tool output、visible reasoning summary replay 尚未实现。
- streaming response capture 尚未实现；当前 memory bridge 只支持非流式 text-only replay。
- 细粒度 trace metadata 尚未完成；需要补齐 bridge enabled、hit/miss/expired/owner_mismatch、replay item count、memory limit 等不含 prompt 内容的 metadata。

已接受的 pre-GA 方向变化：

- `docs/CONSTITUTION.md` 已记录默认 stateless、provider-owned lifecycle state reconstruction 仍 out of scope，并把 `conversation_state_bridge.mode=memory` 限定为 narrow local replay buffer。
- 本计划不引入持久化数据库，但已引入显式配置的内存状态。宪章措辞应保持：默认仍是无状态；可配置的内存 `ConversationStateBridge` 是最大安全兼容策略下的内部 state expansion 能力，不是另一套兼容策略。

## 设计原则

1. 默认关闭。未启用状态桥时，所有现有 fail-closed 行为保持不变。
2. 只重放 `llmup` 自己观察并保存过的状态。外部 OpenAI 返回的 `resp_*` / `conv_*` ID 不可凭空使用。
3. 只保存可重放会话事件，不保存可直接返回给用户的响应缓存条目。
4. 不服务缓存响应。每次客户端请求都必须调用目标 provider 生成新响应。
5. 不反解 provider-private state。`encrypted_content`、opaque reasoning、provider compact state 等不能跨协议重建。
6. 不在内部 raw same-protocol forwarding 路径中启用。native OpenAI Responses upstream 继续透传 provider state；状态桥只在需要请求构造/转换的路径上展开本地 state。
7. 明确最小 owner 边界。namespace 和认证主体必须参与状态隔离，避免不同调用方互相读取状态。
8. 只实现简单 TTL 和全局最大内存占用。状态过期、进程重启、状态不存在时直接 fail closed。
9. `store: false` 默认不保存状态。
10. 不通过自然语言判断内容是否可压缩或可省略。任何裁剪、摘要、compaction 都必须是显式后续阶段。

状态类型必须分清：

- `ProviderNativeHandle`：OpenAI provider 的真实 `resp_*` / `conv_*`、Anthropic thinking signature、以及 retired native Gemini 文档里的 `thoughtSignature` / `cachedContent` 等 provider-owned handle。只在 provider-native forwarding 或 retired reference 中保留，不能被状态桥导入。
- `LlmupOwnedTranscript`：`llmup` 自己保存的短期内存 transcript，可跨协议 replay。
- `OpaqueCarrier`：`encrypted_content`、opaque compaction、不可见 reasoning carrier 等。不能跨协议展开。

## 配置草案

初始配置只支持纯内存 store，并且刻意保持小配置面：

```yaml
conversation_state_bridge:
  mode: off          # off | memory
  ttl_seconds: 3600
  max_bytes: 268435456
```

推荐默认：

- `mode: off`
- `mode: memory` 后才捕获和展开 OpenAI Responses 状态。
- `ttl_seconds` 控制状态生命周期。
- `max_bytes` 是全局内存上限，不做 per-tenant/per-conversation 细分。
- `store: false` 优先于 bridge 保存，但这是固定语义，不做成配置项。

`conversation_state_bridge.mode = "memory"` 是现有配置里的 storage/backend selector，也是允许进程内短期保存 prompt/response 事件的数据保留安全门。字段名里的 `mode` 只选择状态桥 backend，不得被复用成兼容能力开关、路由策略或产品分层。

## 请求处理观测

不新增单独主路径或用户可选策略。状态桥启用时，请求处理观测为 `RequestTransformationRequired`，并暴露 `state_bridge` 字段；这表示请求需要显式 state expansion 后再构造/转换，仍属于单一最大安全兼容策略。provider-native prompt-cache 合成仍只是 provider-native request-control support：

```rust
enum RequestProcessing {
    RequestTransformationNotRequired,
    RequestTransformationRequired,
}

enum StateBridgeModifier {
    Off,
    CaptureCandidate,
    Expanded,
}
```

`state_bridge` 字段有两个入口：

`BridgeCaptureCandidate`：

- client format 是 OpenAI Responses。
- target upstream format 不是 OpenAI Responses，或者 route 明确要求翻译。
- `conversation_state_bridge.mode = "memory"`，也就是内存状态桥已配置。
- 请求可以不包含 `previous_response_id`。第一轮也必须经过 bridge preprocessor，用于消费 `store` 并决定 capture/no-save policy。

`BridgeContinuation`：

- client format 是 OpenAI Responses。
- target upstream format 不是 OpenAI Responses，或者 route 明确要求翻译。
- 请求包含本地 `resp_llmup_*` 形式的 `previous_response_id`。
- `conversation_state_bridge.mode = "memory"`，也就是内存状态桥已配置。
- 状态 ID 属于当前 namespace / auth subject。

如果目标是 native OpenAI Responses，则继续原生透传，不走状态桥。

插入点要求：

- 状态展开/lookup 必须发生在 `resolve_requested_model_or_error()` 和 `original_body` 进入 boundary assessment 之前。否则 model-less `previous_response_id` 会先被现有 stateful routing 逻辑拒绝。
- Bridge preprocessor 成功后，要消费并移除本地 `previous_response_id` 和 `store` 控制，把历史和当前 `input` 合成显式 `input`，再进入现有 `assess_request_translation_with_surface()` 和 `translate_request_with_policy()`。
- 第一轮保存必须记录 resolved route owner；第二轮省略 `model` 时用 store owner 恢复路由，第二轮显式 `model` 与 store owner 冲突时 fail closed。
- 状态 store 挂在 `AppState`，不要塞进 `RuntimeState` 快照，避免每次认证上下文 clone 大状态。

## 内部状态模型

状态只保存在内存中，挂在 `AppState` 下，例如：

```rust
struct ConversationStateStore {
    responses: HashMap<String, BridgeResponse>,
    ttl: Duration,
    max_bytes: usize,
    current_bytes: usize,
}

struct StateOwner {
    namespace: String,
    auth_subject_hash: String,
}

struct BridgeResponse {
    id: String,
    owner: StateOwner,
    upstream_name: String,
    upstream_format: UpstreamFormat,
    upstream_model: String,
    translation_contract_revision: Option<String>,
    surface_revision: Option<String>,
    namespace_revision: Option<String>,
    route_config_hash: String,
    parent_response_id: Option<String>,
    request_items: Vec<BridgeItem>,
    output_items: Vec<BridgeItem>,
    status: BridgeResponseStatus,
    created_at_ms: i64,
    expires_at_ms: i64,
}
```

保存内容：

- 可重放 OpenAI Responses input items。
- assistant output items 中可转换的 message / function_call / custom_tool_call / reasoning summary。
- tool call 与 tool output 的 `call_id` 关联。
- 当前请求的 `tools`、tool choice、parallel tool policy、response format 等必要 controls。
- resolved upstream name、target format、target model、translation contract/surface 信息，以及 namespace revision 或 route config hash，用于 continuation route 绑定。
- 当前 response 的状态、完成时间、截断/不完整原因。

不保存：

- provider credentials、downstream Authorization header、proxy key。
- 原始 response body 的“可直接返回副本”。
- provider-private opaque state。
- debug trace / hook payload中的未脱敏副本。
- `store: false` 请求对应的 response state。

## ID 策略

状态桥必须生成 `llmup` 自己拥有的 ID：

- response：`resp_llmup_<opaque>`

规则：

- translated route 上返回给 OpenAI Responses client 的 ID 必须是 `llmup` ID，不能冒充 provider 真实 `resp_*`。
- native OpenAI Responses forwarding 保留 provider ID，不导入本地状态。
- 如果客户端传入的 `previous_response_id` 不是本地已知 ID，fail closed。
- ID 不编码 owner、prompt、模型或 provider 信息。

## 请求展开规则

### `previous_response_id`

流程：

1. 查找本地 `BridgeResponse`。
2. 验证 namespace / auth subject / route policy。
3. 沿 parent chain 展开历史 request/output items。
4. 追加当前请求 `input`。
5. 使用当前请求的 `instructions`，不自动继承上一轮 `instructions`。
6. 构造完整 Responses input，再交给 `responses_to_messages()` 和现有目标协议转换。

重要语义：

- OpenAI 官方文档说明 `previous_response_id` 与 `instructions` 一起使用时，上一轮 instructions 不会自动带到下一轮。因此状态桥也不能盲目重放旧 instructions。
- 如果历史里只有 opaque reasoning / compaction carrier，而没有可见 summary 或 transcript，fail closed。

### `conversation`

MVP 不支持本地 `conversation` bridge。

行为：

- native OpenAI Responses forwarding 保持现状。
- translated route 上继续 fail closed。
- 后续如果需要支持，只做本地 `conv_llmup_*`，不导入外部 OpenAI `conv_*`。
- `previous_response_id` 和 `conversation` 不能同时使用；这个官方限制需要继续保留。

### `store`

规则：

- Bridge preprocessor 必须先消费 `store`，再进入跨协议 stateful-control assessment。
- `store: false`：首轮请求仍继续调用上游，但不保存 response state；如果之后 client 用对应历史 ID 继续 `previous_response_id` replay，会因为本地 store 没有可展开状态而 fail closed。
- `store: true` 或省略：当内存状态桥已配置且 route 允许时保存。显式 `store: true` 不应继续触发现有 translated route 的 provider-owned state fail-closed 规则，因为它在内存状态桥下表达的是 `llmup` 本地短期 state。
- 如果未来引入 route-level no-store/ZDR policy，它必须禁用 bridge 保存；初版只需要尊重请求级 `store: false`。

### `background`

初始版本不支持 `background: true` 的跨协议状态桥。

原因：

- background 是异步 lifecycle 语义，不只是上下文 replay。
- 纯内存 store 无法在进程重启后保留任务状态。
- 需要独立任务队列、polling state、cancel 行为和生命周期语义。

行为：当内存状态桥已配置时仍 fail closed，并在错误中说明当前不支持 background lifecycle emulation。

### `prompt`

初始版本不支持 OpenAI hosted prompt template 跨协议展开。

行为：

- 如果 `prompt` 出现在 translated route 上，fail closed。
- 后续可通过本地 prompt-template registry 显式支持，但不属于本计划初版。

### `context_management` / `/responses/compact`

初始版本不做自动 compaction。

行为：

- native OpenAI Responses forwarding 保持现状。
- translated route 上继续 fail closed，除非未来实现显式本地 compact adapter。
- request-side compaction item 只有在已有可见 summary/text 可重放时，才沿用现有 degrade 规则。

## 响应捕获规则

### 非流式

1. 上游成功返回后，先完成现有 response translation。
2. 如果客户端协议是 OpenAI Responses 且内存状态桥已启用：
   - 预生成一个候选本地 `resp_llmup_*`。
   - 尝试把请求 input items 和转换后的 output items 提交到 store。
   - commit 成功后，把客户端可见 response `id` 替换为候选本地 ID。
   - commit 因 `max_bytes` 失败时，当前响应仍可返回，但不承诺后续 continuation；trace/warning 记录 `state_bridge_memory_limit`。
3. 上游失败或转换失败不写入状态。

### 流式（未实现，Post-MVP）

1. 在 response.created 阶段预分配本地 response ID。
2. streaming sink 收集可重放 output items。
3. 只有收到 completed / incomplete terminal event 后提交状态。
4. 客户端断连、上游错误、stream parse fatal 时不提交 completed 状态；可选记录 aborted metadata，但不能用于 replay。
5. 流式事件中客户端可见 ID 必须与最终 store ID 一致。

## 分阶段覆盖范围

MVP 支持：

- OpenAI Responses client -> OpenAI Chat upstream。
- OpenAI Responses client -> Anthropic Messages upstream。
- text message replay。
- assistant text replay。

Post-MVP 支持：

- function call / function call output replay。
- custom tool call 在现有最大兼容翻译能力范围内 replay。
- visible reasoning summary replay。

初始不支持：

- 本地 Conversations API bridge。
- OpenAI hosted tools 的 provider-side state。
- web search / file search / computer use / code interpreter 的 provider-private state。
- `background: true`。
- hosted `prompt`。
- opaque-only reasoning encrypted content。
- opaque-only compaction。
- 外部 OpenAI provider ID 导入。
- 跨进程恢复。

## 与 Prompt Cache 支持的关系

状态桥与 provider-native prompt-cache request-control support 是相邻但不同的能力：

- 状态桥负责把缺失的 conversation context 展开成完整 target prompt。
- provider-native prompt-cache request-control support 可以在 target prompt 构造完成之后，再添加目标 provider 的 cache request controls。
- 状态桥本身不决定哪些内容应该被 provider cache。
- 状态展开后的稳定 prefix 可以作为 `prompt_cache_key` 或 Anthropic breakpoint 策略的输入，但必须通过前一份 prompt-cache plan 的策略和 trace 规则。

执行顺序：

1. Conversation state 展开。
2. Source -> target protocol translation。
3. Provider-native prompt-cache request-control support。
4. Upstream request。

## 安全与隔离

当前已实现的隔离边界：

- State owner 至少包含 namespace 和认证主体 hash。
- client-provider-key 模式下，认证主体可以由下游 provider key 的安全 hash 派生。
- continuation owner 绑定当前 runtime 中的 route/config fingerprint 与 namespace revision；配置 drift 在 upstream dispatch 前 400 fail closed。
- 无 `model` 的 single-upstream continuation 在 route/config 未变时可继续 replay。
- store lookup 只有四种结果：命中、未找到、过期、owner mismatch。除命中外都 fail closed。
- debug trace / hook 不应记录 prompt 内容。

仍需补强：

- proxy-key / trusted tenant policy 仍不产品化，本轮不新增用户配置。
- route/config fingerprint 不对外暴露为产品功能；配置更新即旧 local state fail closed，不做迁移、持久化或 fallback。
- debug trace 需要补齐状态 ID、展开条数和 fail reason 等细粒度 metadata。

内存保护：

- 初版只实现 TTL 清理和一个全局 `max_bytes`，不实现 LRU、per-tenant 配额、per-conversation 配额、跨进程恢复或后台压缩。
- 过期清理可以是请求路径上的惰性清理，也可以是轻量周期任务；选择实现最简单的一种。
- 写入新状态前先清理过期项；如果仍超过 `max_bytes`，当前 response 不提交可 replay 状态，并在 trace/warning 中说明 `state_bridge_memory_limit`。
- 初版可以用一把简单 mutex/RwLock 串行化 store 写入，不引入版本协议。

隐私保护：

- 纯内存不等于无数据保留。文档必须明确：开启状态桥会在 `llmup` 进程内保存 prompt 和模型输出，直到 TTL 或进程退出。
- `store: false` 不保存。
- 不允许 hook/debug 输出状态内容。

## 开发阶段

Current-main delivery status:

- Delivered: Phase 0/1/2 的配置、内存 store、`resp_llmup_*`、TTL/max_bytes、owner hash、非流式 text-only capture/replay。
- Delivered slice: Phase 5 中的 `background` / `store` enabled-semantics alignment / translation-boundary detector unification slice。
- Delivered slice: route/config owner hardening，包括内部 route/config fingerprint、当前 runtime 复校验、drift pre-dispatch 400 fail closed，以及未变配置下的 no-model single-upstream replay。
- Pending next: remaining prompt-cache translated/block-level review；OpenAI-family -> Anthropic 顶层 `extra_body.anthropic.cache_control` 显式映射、Anthropic -> OpenAI-family `extra_body.openai.prompt_cache_key` / `prompt_cache_retention` 显式映射已交付，未来 reviewed synthesis 不新增用户/运营配置面。
- Later: Phase 3 tool/custom tool replay、Phase 4 stream capture、reasoning summary replay，以及 shared detector helper / 细粒度 trace metadata consolidation。

### Phase 0：合同冻结与文档更新

交付：

- 更新 `CONSTITUTION.md`：默认 stateless；可选纯内存 `ConversationStateBridge` 是最大安全兼容策略下明确配置的 state expansion 能力。
- 更新 state-continuity docs：区分 provider-owned state、llmup-owned bridge state、cache。
- 新增配置 schema 文档和默认关闭说明。

验收：

- 未配置状态桥时，现有 fail-closed 测试全部保持。
- 文档明确“不做 response cache”。

### Phase 1：内存 Store 骨架

交付：

- 在 `AppState` 加入 `ConversationStateStore`。
- 增加 `conversation_state_bridge` 配置解析和有效配置解析。
- 实现 ID minting、StateOwner、TTL、全局 `max_bytes`、基本 get/put/delete。
- 使用简单 mutex/RwLock 保护内存 HashMap。

验收：

- store 单元测试覆盖 create/get/expire/max_bytes/owner mismatch/restart-miss 语义。
- 默认配置不创建 store 或 store disabled。

### Phase 2：非流式 `previous_response_id` Replay

交付：

- translated OpenAI Responses 非流式响应生成 `resp_llmup_*`。
- 成功响应后保存 request/output items。
- 后续 `previous_response_id` 查本地状态并展开为完整 input。
- 展开后复用现有 `responses_to_messages()` 和目标协议转换。

验收：

- Responses -> Anthropic 第一轮返回本地 response ID。
- 第二轮带 `previous_response_id`，Anthropic upstream 捕获到第一轮 user、第一轮 assistant、新 user input。
- 未知/过期/owner mismatch response ID fail closed。
- `store: false` 后续 replay fail closed。

### Phase 3：工具调用 Replay

交付：

- 保存 assistant function_call / custom_tool_call output items。
- 保存 client 后续 function_call_output / custom_tool_call_output input items。
- 用现有 tool bridge 规则转成 Chat/Anthropic 可接受的历史。
- 处理 pending tool call 状态。

验收：

- 第一轮模型返回 tool call，第二轮 client 提交 tool output + `previous_response_id`，目标 upstream 收到完整 assistant tool call + tool result 历史。
- call_id 缺失、重复、跨 parent chain mismatch fail closed。
- custom tool 在单一最大兼容翻译合同下遵循现有 capability surface，并在无法安全 replay 时 fail closed。

### Phase 4：流式响应捕获

交付：

- streaming response 预分配本地 response ID。
- streaming sink 收集 output deltas 并还原可重放 output items。
- terminal event 后提交状态。
- abort/error 不提交可 replay 状态。

验收：

- 流式第一轮完成后，第二轮 `previous_response_id` 可 replay。
- 客户端断连后 response ID 不可 replay 或明确标记 incomplete。
- stream 中所有可见 response ID 一致。

### Phase 5：轻量清理与观测

交付：

- 实现 TTL 惰性清理或轻量周期清理。
- 实现全局 `max_bytes` 检查。
- 在 debug trace 中记录 bridge enabled、state hit/miss/expired/owner_mismatch、replay item count。
- 确认 hook/debug 不包含状态内容。
- 已完成 `background` / `store` enabled-semantics alignment：`background:false|null` 和 `store:false|null` 不触发 provider-owned stateful fail-closed；`background:true`、`store:true`、`previous_response_id`、`conversation`、`prompt`、`context_management` 仍 fail closed。剩余 detector 工作只包括共享 helper、细粒度 trace metadata、以及其它 consolidation。

验收：

- TTL 到期后 replay fail closed。
- 超过 `max_bytes` 时不提交 replay 状态，并记录 warning/trace。
- debug trace 只含 metadata，不含 prompt 内容。
- `store: false` 不保存。
- Responses stateful controls 在 native routing resolver、bridge preprocessor 和 translation boundary 上行为一致。

### Phase 6：可选增强

仅在核心稳定后考虑：

- 本地 Conversations API bridge。
- 本地 compaction adapter。
- 本地 prompt template registry。
- 持久化 store 后端。
- 外部 OpenAI state import adapter。
- 容量配额、LRU、admin state browser、跨进程恢复、分布式状态同步。

这些增强必须单独评审，不属于初版。

## 测试矩阵

MVP 必须覆盖：

| 区域 | 覆盖要求 |
| --- | --- |
| 默认行为 | bridge off 时，现有 stateful controls 跨协议 fail closed |
| 非流式 text replay | Responses -> Chat/Anthropic 的 `previous_response_id` 多轮上下文展开 |
| 隔离 | namespace/auth subject mismatch fail closed |
| Route owner | model-less continuation 使用保存的 upstream/model/config owner；显式 model 或 config revision/hash 冲突 fail closed |
| TTL | expired state fail closed 且有 trace reason |
| max_bytes | 超过全局内存上限时不提交 state，且有 warning/trace |
| store:false | 首轮仍调用上游但不保存；后续使用对应历史 replay 时因无本地状态 fail closed；不泄露内容 |
| detector enabled-semantics | `background:false|null` / `store:false|null` 不触发 provider-owned stateful fail-closed；enabled/present controls 仍 fail closed |
| Native forwarding | OpenAI Responses native routes 不被本地 bridge 改写 |
| Prompt cache 顺序 | state 展开先于 provider-native prompt-cache request-control support |

Post-MVP 覆盖：

| 区域 | 覆盖要求 |
| --- | --- |
| 工具调用 | function_call/custom_tool_call + tool output replay |
| 流式 | completed 后可 replay，abort/error 不可 replay |
| Reasoning summary | 只 replay visible summary；opaque-only carrier fail closed |

## Handoff 任务顺序

推荐下一步顺序：

1. Remaining prompt-cache translated/block-level review next：在已交付的状态展开、route/config owner 绑定、OpenAI-family -> Anthropic 顶层 `extra_body.anthropic.cache_control` 显式映射，以及 Anthropic -> OpenAI-family `extra_body.openai.prompt_cache_key` / `prompt_cache_retention` 显式映射上，继续评审剩余 translated/block-level 支持；未来 reviewed synthesis 不新增用户/运营配置面。
2. Tool/custom tool replay later：之后再评估 function_call/custom_tool_call、tool output、reasoning summary replay。
3. Stream capture later：最后再评估 streaming response capture 和本地 Conversations API bridge。

主要代码区域：

- [src/config.rs](../../src/config.rs)
- [src/server/proxy.rs](../../src/server/proxy.rs)
- [src/server/responses_resources.rs](../../src/server/responses_resources.rs)
- [src/translate/internal/openai_responses.rs](../../src/translate/internal/openai_responses.rs)
- [src/translate/internal.rs](../../src/translate/internal.rs)
- [src/streaming/stream.rs](../../src/streaming/stream.rs)
- [src/streaming/openai_sink.rs](../../src/streaming/openai_sink.rs)
- [src/streaming/state.rs](../../src/streaming/state.rs)
- [src/telemetry.rs](../../src/telemetry.rs)
- [tests/integration_test.rs](../../tests/integration_test.rs)

## 明确非目标

- 不做 LLM 响应缓存。
- 不做语义缓存。
- 不把 provider 私有状态转换成通用状态。
- 不默认保存任何 prompt / response。
- 不支持外部 provider response ID 自动导入。
- 不在初版支持 background lifecycle。
- 不在初版支持 hosted prompt template。
- 不在初版支持自动 compaction。
- 不引入数据库或外部服务。

## 参考资料

官方参考：

- OpenAI Conversation state guide: <https://developers.openai.com/api/docs/guides/conversation-state>
- OpenAI Responses create reference: <https://developers.openai.com/api/reference/resources/responses/methods/create>
- OpenAI Conversations reference: <https://developers.openai.com/api/reference/resources/conversations>
- OpenAI Background mode guide: <https://developers.openai.com/api/docs/guides/background>

本地参考：

- [docs/protocol-baselines/capabilities/state-continuity.md](../protocol-baselines/capabilities/state-continuity.md)
- [docs/protocol-compatibility-matrix.md](../protocol-compatibility-matrix.md)
- [Request processing and provider-native prompt-cache request-control plan](./pre-ga-request-processing-prompt-cache-support-plan.md)
