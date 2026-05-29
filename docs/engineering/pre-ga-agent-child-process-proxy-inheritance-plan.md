# pre-GA Agent 子代理代理继承最小闭环计划

更新时间：2026-05-19

## 一句话目标

`llmup-codex` 和 `llmup-claude` 启动原生 Codex CLI / Claude Code 后，原生客户端同一 runtime 内自己管理的 subagent / Task 模型调用，默认仍然走 llmup proxy，并尽量不破坏 provider / gateway 的 prompt cache 命中。

这份计划刻意收敛，不做复杂运行时系统。

## 为什么要重写

上一版计划引入了 `AgentRuntimeEnvelope`、session proxy credential、PATH shim、Codex session overlay、proxy lease/TTL 等设计。它们可以解决更多边缘场景，但对 pre-GA 当前目标来说太重，容易把产品心智从“普通用户用 launcher 启动 coding agent”扩大成“llmup 管理整棵进程树”。

当前最小闭环只回答一个问题：

> 用户用 `llmup-codex` / `llmup-claude` 启动原生客户端后，同一 runtime 内的子代理请求是否继续走 llmup？

先把这个闭环做稳，再讨论更复杂的二次启动和后台进程继承。

## V1 范围

### 保证

- `llmup-codex` 启动的 Codex 主会话请求走 llmup proxy。
- Codex 官方 subagent 在同一 Codex runtime 内产生的模型请求走 llmup proxy。
- Codex custom agent 未覆盖 provider/base URL/auth 时，或只显式设置 `model` 时，模型请求仍走 llmup proxy。
- `llmup-claude` 启动的 Claude Code 主会话请求走 llmup proxy。
- Claude Code Task subagent / built-in subagent 的模型请求默认走 llmup proxy。
- Claude Code 默认关闭 attribution header，减少随机 `cch` / version / fingerprint 类字段破坏 prompt cache 稳定前缀。

### 不保证

- 不保证用户在 Bash 里裸跑 `codex` / `claude` 会自动继承当前 llmup proxy。
- 不保证任意 shell 子进程、MCP server、hook、脚本自动拿到 proxy credential。
- 不保证父进程退出后，脱离父进程的后台任务还能继续使用同一个 proxy。
- 不把 Claude agent teams 作为 V1 保证；它是实验功能，放入未来评估和手动 smoke。
- 不保证显式覆盖 provider/base URL/auth 的 Codex custom agent 仍走 llmup。
- 不劫持 `PATH`，不安装 `codex` / `claude` shim。
- 不引入 `AgentRuntimeEnvelope`。
- 不新增 session-scoped proxy credential 体系。
- 不改写用户项目里的 `.codex/`、`.claude/`、`CLAUDE.md`、`AGENTS.md`。
- 不做 llmup 自己的 prompt cache。

这些场景可以作为未来增强，但不进入 V1。

## 设计原则

- **KISS**：只用原生客户端官方支持的配置和环境变量。
- **低心智负担**：普通用户仍只需要 `llmup-config`、`llmup-codex`、`llmup-claude`。
- **最大兼容但不假装全能**：能让原生 subagent / Task 走 proxy 的地方默认做好；不能可靠保证的场景明确说清楚。
- **不偷换边界**：llmup 只代理模型 API，不伪造官方订阅、账号、quota 或 billing 身份。
- **不扩大安全面**：真实 provider key 只留在 proxy 侧，不进入 coding agent 子进程环境。

## Codex 最小实现

当前实现已经做了主要事情：

- 通过 argv 注入 `model_provider="proxy"`。
- 注入 top-level `openai_base_url="<llmup>/openai/v1"` 作为 Codex 官方配置 fallback，避免内部路径落回内置 `openai` provider 时打到官方 endpoint。
- 注入 `model_providers.proxy.base_url="<llmup>/openai/v1"`。
- 注入 `model_providers.proxy.env_key="OPENAI_API_KEY"`。
- 注入 `model_providers.proxy.wire_api="responses"`。
- 注入 `model_catalog_json`。
- 注入 `-m <llmup alias>`。
- 通过 env 设置 `CODEX_HOME`、`OPENAI_API_KEY=<local proxy key>`；当前实现也设置 `OPENAI_BASE_URL=<llmup>/openai/v1` 作为兼容辅助，但 V1 的官方兼容依据是 Codex argv 配置中的 `openai_base_url` fallback 和 `model_providers.proxy.base_url` / `env_key` / `wire_api`，而不是 `OPENAI_BASE_URL`。

V1 不做 Codex session overlay，也不把动态端口写入共享 `~/.llmup-codex/config.toml`。

V1 要补的不是大架构，而是验证和小修：

- 用 contract test 固定当前 argv/env 注入。
- 用 fake Codex 子请求模拟确认 custom provider override 会被带到子请求；同时覆盖“落回内置 `openai` provider 时读取 top-level `openai_base_url`”的路径。
- 用 mock server 捕获请求，确认子请求打到 llmup proxy，而不是官方 endpoint。
- 对 Codex custom agent 只显式 `model` 的场景做负向测试：请求仍必须走 proxy；如果模型名无法路由，llmup 返回清晰错误。
- 对 Codex custom agent 显式覆盖 provider/base URL/auth 的场景，只做边界文档和可观测错误，不把它纳入 V1 保证。

如果真实 Codex 版本证明“官方 subagent 仍会退回官方 endpoint”，再重新评估更重的 session config / overlay 方案。不要在没有证据前引入。

## Claude Code 最小实现

当前实现已经注入：

- `CLAUDE_CONFIG_DIR=<llmup managed dir>`
- `ANTHROPIC_AUTH_TOKEN=<local proxy key>`
- `ANTHROPIC_BASE_URL=<llmup>/anthropic`
- `ANTHROPIC_MODEL=<llmup alias>`
- `ANTHROPIC_CUSTOM_MODEL_OPTION=<llmup alias>`
- 默认不注入 `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1`，避免 llmup 意外改写 Claude Code 原生 permission / sandbox 行为；需要该 hardening 的高级用户可自行显式设置。
- 输出 token 和 auto compact 相关 env

V1 增加两个默认 env：

```text
CLAUDE_CODE_SUBAGENT_MODEL=<llmup alias>
CLAUDE_CODE_ATTRIBUTION_HEADER=0
```

含义：

- `CLAUDE_CODE_SUBAGENT_MODEL=<alias>`：让 Claude Code subagent 默认使用 llmup alias。官方文档说明它会用于所有 subagent，并覆盖 per-invocation model 和 subagent frontmatter model。这个行为符合 V1 目标：默认最大化让子代理继续走 llmup alias。
- `CLAUDE_CODE_ATTRIBUTION_HEADER=0`：官方文档说明它会省略 system prompt 开头的 attribution block，并能改善通过 LLM gateway 时的 prompt-cache 命中。Linux.do 帖子提到的 `cch` / 随机 attribution 问题只作为社区线索，确定依据是官方文档。

### Claude agent teams

Claude agent teams 是实验功能，默认关闭，并且官方文档说明 teammate 默认不一定继承 lead 的 `/model`。

V1 不实现 teammate global config 写入，也不把 agent teams 作为硬保证。只保留两个动作：

- 文档中明确边界：`llmup-claude` V1 保证主会话和普通 Task/subagent，不保证 agent teams。
- 在当前 Claude Code 版本可用时做手动 smoke，记录 teammate 请求是否仍经过 llmup。只有真实证据显示该能力稳定且实现足够小，未来再考虑 `teammateDefaultModel` global config。

## 测试计划

### Contract tests

- Codex launcher argv/env 包含 proxy provider、base URL、env key、wire API、model catalog、model alias。
- Claude launcher env 包含 `ANTHROPIC_BASE_URL`、`ANTHROPIC_AUTH_TOKEN`、`ANTHROPIC_MODEL`、`ANTHROPIC_CUSTOM_MODEL_OPTION`。
- Claude launcher env 新增并固定：
  - managed profile projection 开启时，`CLAUDE_CODE_SUBAGENT_MODEL=<alias>`。
  - managed proxy 模式下，`CLAUDE_CODE_ATTRIBUTION_HEADER=0`。
- managed projection 下覆盖/清理父环境里的 `CLAUDE_CODE_SUBAGENT_MODEL`，固定为 llmup alias。
- `--llmup-no-profile-projection` 时，不注入 `CLAUDE_CODE_SUBAGENT_MODEL`；用户自管模型，文档说明风险。`CLAUDE_CODE_ATTRIBUTION_HEADER=0` 仍可随 managed proxy 注入，因为它不是模型锁定。
- `--llmup-no-proxy` 时，不注入 proxy env、subagent model env 或 attribution env，不启动 proxy。
- 更新 `docs/clients.md`，增加一小节 launcher 继承边界：不保证 agent teams、裸 `codex` / `claude`、任意 shell 子进程、父进程退出后的后台任务继承 proxy。

### Mock e2e

- fake Codex 主会话请求命中 llmup mock server。
- fake Codex 子请求模拟命中 llmup mock server，其中包含读取 top-level `openai_base_url` 的内置 `openai` provider fallback 路径。它只证明 launcher 注入被模拟子请求继承，不等同于证明官方 Codex runtime 行为。
- fake Codex custom agent 只显式 model 时，请求仍命中 llmup mock server；未知 model 返回明确错误。
- fake Claude 主会话请求命中 llmup mock server。
- fake Claude Task 子请求模拟命中 llmup mock server，并使用 llmup alias。它只证明 launcher env 设计，不等同于证明官方 Claude runtime 行为。

### Cache stability tests

- 启动 Claude 时 env 中存在 `CLAUDE_CODE_ATTRIBUTION_HEADER=0`。
- V1 硬 gate 只固定这个 env；真实 request body 捕获和前缀稳定性检查作为 smoke/诊断，不作为默认硬 gate。
- 如做诊断，可观察 system 前缀是否仍出现已知社区线索字段，例如 `cch`、`cc_version`、`cc_entrypoint`、`x-anthropic-billing-header`。
- 如果未来 Claude Code 改名或官方行为变化，以官方文档为准更新测试，不把社区帖子当作唯一依据。

### Manual smoke

- `llmup-codex` 默认启动并正常对话。
- Codex subagent / custom agent 触发方式在当前版本可用时手动验证，并标注客户端版本。
- `llmup-claude` 默认启动并正常对话。
- Claude Task subagent 手动验证，并标注客户端版本。
- Claude agent teams 仅作为未来/实验 smoke：当前版本可用时手动观察 teammate 请求是否经过 llmup，不作为 V1 gate。

## 验收标准

- 主会话和最小子请求模拟在 mock server 中都能看到经过 llmup。
- 当前版本 Codex subagent 和 Claude Task subagent 的真实 smoke 有记录，且不与 contract/mock 结论冲突。
- Claude subagent 默认使用 llmup alias。
- Claude 默认关闭 attribution header，减少 gateway prompt-cache 前缀污染。
- 真实 upstream provider key 不进入 Codex/Claude 子进程环境。
- 没有引入 `AgentRuntimeEnvelope`、PATH shim、proxy lease、Codex overlay 等重设计。
- 文档明确说明 V1 不保证 agent teams、裸 `codex` / `claude` 二次启动、任意 shell 子进程、父进程退出后的后台任务继承 proxy。

## 未来再评估

只有当真实测试证明 V1 不能覆盖核心场景时，才重新评估这些增强：

- session-scoped proxy credential
- AgentRuntimeEnvelope
- session-local PATH shim
- Codex session `CODEX_HOME` overlay/root
- proxy idle TTL / lease
- nested launcher 复用已有 proxy
- Claude agent teams teammate global config

这些能力不是否定项，只是不该作为最小闭环的起点。

## Sources

- OpenAI Codex subagents: <https://developers.openai.com/codex/subagents>
- OpenAI Codex config basics: <https://developers.openai.com/codex/config-basic>
- OpenAI Codex config reference: <https://developers.openai.com/codex/config-reference>
- Claude Code environment variables: <https://code.claude.com/docs/en/env-vars>
- Claude Code model configuration: <https://code.claude.com/docs/en/model-config>
- Claude Code LLM gateway: <https://code.claude.com/docs/en/llm-gateway>
- Claude Code settings: <https://code.claude.com/docs/en/settings>
- Claude Code agent teams: <https://code.claude.com/docs/en/agent-teams>
- Linux.do community thread, cache attribution clue only: <https://linux.do/t/topic/1613608>
