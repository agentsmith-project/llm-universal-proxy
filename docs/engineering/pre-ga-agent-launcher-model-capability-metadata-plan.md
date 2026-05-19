# Pre-GA Agent Launcher 模型能力元信息改进计划

目标：GA 前把 `llmup-config` / `llmup-codex` / `llmup-claude` 的模型 alias、limits、tool surface、Codex catalog、Claude Code gateway 选模行为收敛到一套 Rust 正式实现。测试矩阵已有的合理逻辑迁移为共享投影函数或对齐其输出，不继续维护第二套事实来源。本文以本轮已确认实现和 review 后的最终约束为准。

## 1. 官方依据与本地验证

### Codex

官方依据：

- Codex 配置参考：<https://developers.openai.com/codex/config-reference/>
  - 相关项：`model_catalog_json`、`model_context_window`、`model_auto_compact_token_limit`、`model_reasoning_effort`、`model_reasoning_summary`、`model_supports_reasoning_summaries`、`model_provider`、`model_providers.<id>.wire_api`、`tools.web_search`、`tools.view_image`。
  - 当前官方文档说明 `model_providers.<id>.wire_api` 只支持 `responses`。
- Codex CLI 页面：<https://developers.openai.com/codex/cli>

本地验证：

- 本地版本：`codex-cli 0.131.0`。
- `codex debug models -c model_catalog_json="..."` 在不加 `--bundled` 时能读取自定义 catalog。
- 完整 shape 的 `main` catalog 可返回：`context_window=200000`、`auto_compact_token_limit=61200`、`input_modalities=["text"]`、`supports_search_tool=false`、`apply_patch_tool_type="freeform"`、`supports_parallel_tool_calls=false`。
- 最小 catalog 如果缺 `supported_reasoning_levels` 等完整字段会被 parser 拒绝；因此正式 catalog 不能只写几个字段。
- `codex exec --strict-config -c 'tools.view_image=false'` 在本地 0.131.0 报 unknown configuration field，虽然官方 docs 已列出 `tools.view_image`。默认不注入；只有已确认版本支持时才注入。
- `tools.web_search=false` 和旧写法 `web_search="disabled"` 本地都被接受。正式实现使用官方结构化 `tools.web_search=false`，迁移掉旧写法。

### Claude Code

官方依据：

- Gateway docs：<https://code.claude.com/docs/en/llm-gateway>
  - `ANTHROPIC_BASE_URL` 可指向 Anthropic-compatible gateway。
  - `/v1/models` discovery 由 `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1` opt-in；本计划不默认启用。
- Env vars：<https://code.claude.com/docs/en/env-vars>
  - 关键变量包括 `ANTHROPIC_BASE_URL`、`ANTHROPIC_API_KEY`、`ANTHROPIC_MODEL`、`ANTHROPIC_CUSTOM_MODEL_OPTION`、`ANTHROPIC_CUSTOM_MODEL_OPTION_NAME`、`ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION`、`ANTHROPIC_CUSTOM_MODEL_OPTION_SUPPORTED_CAPABILITIES`、`ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU}_MODEL`、`CLAUDE_CODE_EFFORT_LEVEL`、`CLAUDE_CODE_MAX_OUTPUT_TOKENS`、`CLAUDE_CODE_AUTO_COMPACT_WINDOW`、`CLAUDE_AUTOCOMPACT_PCT_OVERRIDE`、`CLAUDE_CODE_MAX_CONTEXT_TOKENS`、`CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`。
- CLI reference：<https://code.claude.com/docs/en/cli-reference>
  - `--model`、`--effort`、`--resume`、`--continue`、`--permission-mode`、`--dangerously-skip-permissions` 是原生参数；llmup launcher 只透传，不重新解析。
- Model config：<https://code.claude.com/docs/en/model-config>
  - `default` 是特殊值，表示清除模型覆盖并回到 Claude Code 默认模型。`--model <alias|name>`、`ANTHROPIC_MODEL`、`ANTHROPIC_DEFAULT_*_MODEL` 语义按官方。

本地验证：

- 本地版本：Claude Code `2.1.144`。
- 本机 2.1.144 观测样例中，`--model default` 发出的真实请求 model 是 `claude-opus-4-7`，不是 llmup alias `default`。这证明自动追加 `--model default` 的旧实现有风险；不要把该具体模型名写成跨版本事实。
- 设置 `ANTHROPIC_CUSTOM_MODEL_OPTION=main` 且 `ANTHROPIC_MODEL=main`，能让请求发送 `model: main`。
- `CLAUDE_CODE_MAX_OUTPUT_TOKENS` 会进入请求 `max_tokens`。
- `CLAUDE_CODE_EFFORT_LEVEL=xhigh` 会进入请求 `output_config.effort`；`_SUPPORTED_CAPABILITIES` 会影响 effort/thinking。它们本计划只作为原生透传事实，不新增 llmup reasoning/capabilities 产品面。
- `CLAUDE_CODE_MAX_CONTEXT_TOKENS` 需要 `DISABLE_COMPACT=1` 才影响 context window；默认不要依赖它。
- `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1` 是兼容缓解，不保证完全没有 beta header。

## 2. 本轮收敛基线

- Rust 正式实现已加入 `agent_model_profile`，以 `ModelLimits { context_window, max_output_tokens }` 和 `ModelSurface` 为唯一事实来源；不要新增第二套 `codex:` / `claude:` YAML schema。
- `llmup-codex` 已加入 launcher profile projection、Codex model catalog 生成、proxy provider 注入和 tool surface 投影；real CLI matrix 通过 Rust hidden launch-plan JSON 读取正式 argv/env/artifacts，不继续维护 Python catalog/projection helper。
- `llmup-claude` 已加入 custom model env、limits env 和 managed profile env scrub；不再依赖自动追加 `--model default`。
- `llmup-config set-limits` 已作为高级入口配置 alias/upstream limits；普通向导仍不强制询问 limits、不硬编码 limits。
- Anthropic `/v1/models` 和 `/v1/models/{id}` 已补齐顶层 `max_input_tokens` / `max_tokens`，并继续保留 `llmup.surface` / `llmup.limits`。`capabilities` 本轮不输出，避免在没有 canonical schema 的情况下硬投影。
- 本地 `config.yaml` 的 `upstreams` 只支持 map-form。list-form upstreams 必须明确失败，不在本轮扩大配置兼容范围。

## 3. 最终设计

### 3.1 非目标

- 不新增 native 参数 schema，不把 Codex/Claude 的原生参数复制进 llmup 配置。
- 不从模型名猜测 context、max output、reasoning、capabilities 或 tool support。
- 不默认开启 Claude Code gateway discovery。
- 不新增“兼容级别”开关，不维护多套做同一件事的路径。
- 不维护测试专用 YAML 分支；测试需要的能力也用 canonical `limits + surface` 表达。

### 3.2 模型选择规则

提供 `--llmup-model <alias>`，作为 llmup-managed alias 的唯一入口；默认值为 `main`。

托管模式分两种，不能同时生效：

- 默认 managed projection：launcher 根据 `--llmup-model` 选择 alias，生成 Codex catalog/tool hints 或 Claude custom model env/limits。
- 克制的 escape hatch：`--llmup-no-profile-projection`。此模式只做 proxy plumbing 和原生参数透传，不生成 catalog、不注入 Claude custom model option、不注入 limits/tool projection。它用于高级用户直接使用 native model/provider/catalog 配置。

冲突处理必须 fail fast：

- `--llmup-model` 与 `--llmup-no-profile-projection` 同时出现：报错。
- managed projection 开启时，如果 native argv 里检测到 Claude `--model` / `--model=...`：报错，提示改用 `--llmup-model <alias>`，或显式加 `--llmup-no-profile-projection` 后自行管理 native model。
- managed projection 开启时，Codex native argv 中所有会覆盖 llmup model/provider/catalog 的参数都必须 fail fast：`-m` / `--model` / `--model=...`、`--oss`、`--local-provider`、`--profile` / `--profile=...`，以及 `-c` / `--config` 写入的 `model`、`model_provider`、`openai_base_url`、`model_catalog_json`、`model_providers.*`。
- 不允许 llmup alias projection 与 native model/provider/catalog override 两套同时生效。

### 3.3 唯一模型画像

使用 Rust 薄模块 `src/user_tools/agent_model_profile.rs`。唯一输入是已加载 `Config` 和 `--llmup-model` 选中的 alias，唯一事实来源是：

- `Config::effective_model_limits(alias)`：upstream limits + alias override。
- `Config::effective_model_surface(alias)`：upstream `surface_defaults` + alias `surface` override。

派生规则：

- Codex auto compact：仅当 `context_window` 已知时计算。公式：`floor(0.85 * (context_window - max_output_tokens))`；如果 `max_output_tokens` 未知，则 `floor(0.85 * context_window)`。
- Limits 校验落点：全局 config 仍只校验非零；通用 profile 构造不因 Codex 输入预算失败；Codex catalog/projection preflight 对 `max_output_tokens >= context_window` fail；`llmup-config set-limits` 写入时也校验。
- Claude max output：`max_output_tokens` 已知时注入 `CLAUDE_CODE_MAX_OUTPUT_TOKENS=<n>`。
- Claude auto compact：`context_window` 已知时注入 `CLAUDE_CODE_AUTO_COMPACT_WINDOW=<n>`。默认不注入 `CLAUDE_CODE_MAX_CONTEXT_TOKENS`。
- Tool surface：只从 `ModelSurface.tools` 投影；未知就不注入。

### 3.4 Codex launcher

托管 projection 模式保留官方 provider 注入：

- `model_provider="proxy"`
- `model_providers.proxy.base_url="http://127.0.0.1:{port}/openai/v1"`
- `model_providers.proxy.env_key="OPENAI_API_KEY"`
- `model_providers.proxy.wire_api="responses"`
- `model_providers.proxy.supports_websockets=false`
- `-m <llmup alias>`

正式 catalog 投影：

- 在 llmup 管理的 run/artifact dir 写完整 JSON catalog，并通过 `-c model_catalog_json="<path>"` 注入；managed launcher 使用本次 session run dir，hidden launch-plan 使用传入的 artifact dir，不覆盖用户 `~/.codex`。
- catalog/profile preflight 和 JSON 写入必须在启动 proxy 前完成；任何 catalog 生成、写入或 profile 校验失败都不能先启动 proxy。
- catalog entry 的 `slug` / `display_name` 使用 llmup alias。
- catalog 必须包含 Codex 0.131.0 parser 需要的完整 shape，包括顶层 `description`、`supported_reasoning_levels`、`shell_type`、`visibility`、`supported_in_api`、`base_instructions`、`supports_reasoning_summaries`、`apply_patch_tool_type`、`supports_parallel_tool_calls`、`experimental_supported_tools`。
- 从 canonical profile 填充 `context_window`、`auto_compact_token_limit`、`input_modalities`、`supports_search_tool`、`apply_patch_tool_type`、`supports_parallel_tool_calls`。
- `surface.tools.supports_search == false` 时注入 `-c tools.web_search=false`，不再生成旧写法 `web_search="disabled"`。
- `surface.tools.supports_view_image == false` 且 profile 未声明 modalities 时，catalog 默认 `input_modalities=["text"]`；仍不注入 `tools.view_image=false`。该 config 只有已确认版本支持时才注入。
- Codex `-c model_reasoning_*` 等 reasoning 相关配置只作为 native 透传；llmup 不新增 schema、不自动注入。

### 3.5 Claude launcher

主方案唯一：`ANTHROPIC_CUSTOM_MODEL_OPTION=<alias>` + `ANTHROPIC_MODEL=<alias>`。

托管 projection 模式必须：

- 移除自动追加的 `--model default`。
- 注入 `ANTHROPIC_API_KEY=<proxy_key>`、`ANTHROPIC_BASE_URL=http://127.0.0.1:{port}/anthropic`、`CLAUDE_CONFIG_DIR=<managed dir>`、`CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1`。
- 注入 `ANTHROPIC_CUSTOM_MODEL_OPTION=<alias>`、`ANTHROPIC_MODEL=<alias>`、`ANTHROPIC_CUSTOM_MODEL_OPTION_NAME=<alias>`、`ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION=llmup proxy model <alias>`。
- 注入 managed profile env 前清理会干扰选模的调用者环境变量，包括 `ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU}_MODEL` 及其 `_NAME` / `_DESCRIPTION` / `_SUPPORTED_CAPABILITIES` 等后缀变量，以及 `ANTHROPIC_SMALL_FAST_MODEL` 系列变量。
- 按 limits 注入 `CLAUDE_CODE_MAX_OUTPUT_TOKENS` 和 `CLAUDE_CODE_AUTO_COMPACT_WINDOW`。
- 不自动注入 `ANTHROPIC_CUSTOM_MODEL_OPTION_SUPPORTED_CAPABILITIES`。`--effort`、`CLAUDE_CODE_EFFORT_LEVEL`、Claude Code capabilities、Codex `model_reasoning_*` 都只作为原生透传。未来若要成为 llmup 产品面，必须先扩 canonical schema 并补测试。
- 不默认启用 `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1`。
- 不从 provider 模型名猜测 family alias，也不做 family alias fallback。若配置中显式存在 `haiku`、`sonnet`、`opus` alias，可按官方 env 投射 `ANTHROPIC_DEFAULT_HAIKU_MODEL`、`ANTHROPIC_DEFAULT_SONNET_MODEL`、`ANTHROPIC_DEFAULT_OPUS_MODEL`；若 custom model option 的本地 smoke 不通过，fail fast 并提示升级/更换 Claude Code。

### 3.6 Probe / doctor / preflight

Probe 是实现细节，不在普通 launcher 启动路径运行真实 agent probe，也不应触网。

- CI、`doctor` 或显式 preflight 使用 temp home，优先 parse-only / debug 命令。
- Codex catalog 用 `codex debug models -c model_catalog_json="<generated>"` 验证。
- `tools.view_image=false` 默认不注入；只有版本 allowlist 或离线 probe artifact 确认支持时才注入。
- `tools.web_search=false` 作为 CI/doctor 兼容检查。
- Claude custom model option 用本地 stub smoke 验证；失败则报错，不启用双轨 fallback。

### 3.7 `/models` API

- OpenAI `/v1/models` 保持官方兼容字段 + `llmup` metadata。Codex 能力通过 catalog 传递，不走 OpenAI models object 旁路。
- Anthropic `/v1/models` 和 `/v1/models/{id}` 保留 `llmup.limits` / `llmup.surface`，并补齐顶层可选字段：
  - `max_input_tokens = context_window`，仅当 `context_window` 已知时输出。
  - `max_tokens = max_output_tokens`，仅当 `max_output_tokens` 已知时输出。
- 本轮不输出 Anthropic Models API `capabilities`。不得把 `apply_patch_transport`、Claude Code `_SUPPORTED_CAPABILITIES`、native reasoning / effort 配置投影成 Anthropic Models API `capabilities`；未知能力必须省略。后续若要支持，必须先确认官方 schema，并扩展 canonical schema。
- Codex auto compact 的输入预算只用于 Codex catalog，不投到 Anthropic 官方字段；不要用 `context_window - max_output_tokens` 填 `max_input_tokens`。
- Claude Code discovery 文档不承诺消费这些字段；验收只验证 API 响应兼容和字段同源。

### 3.8 `llmup-config`

- 普通向导继续默认生成 text-only `surface_defaults`：text input/output、search false、view image false、apply patch freeform、parallel calls false。
- 普通向导不强制询问 limits，不硬编码 limits。
- 高级入口：`llmup-config set-limits --alias <name> --context-window <n> --max-output-tokens <n>` 或 `llmup-config set-limits --upstream <name> --context-window <n> --max-output-tokens <n>`。
- `--alias` 与 `--upstream` 必须二选一；未知目标报错；已是相同值时幂等。
- 命令只写 `config.yaml`，不碰 secrets。
- 写 alias limits 时，如果当前 alias 是字符串形式，升级为 structured alias 并保留原 target。
- 写入时校验 `context_window > 0`、`max_output_tokens > 0`、`max_output_tokens < context_window`。
- 原子写 `config.yaml` 时，临时文件必须在写入内容前收紧权限；优先沿用原文件 mode，原文件缺失时至少使用 `0600`。
- 本地 `config.yaml` 只支持 map-form `upstreams`；如果发现 list-form upstreams，`set-limits` 必须明确失败。

## 4. TDD 与测试矩阵

测试覆盖要求：

- Model selection：`--llmup-model` 默认 `main`；native model flag 与 managed projection 冲突 fail fast；`--llmup-no-profile-projection` 禁用所有 profile projection；两者同时出现报错。
- Codex conflict detection：managed projection 拒绝 `-m` / `--model` / `--model=...`、`--oss`、`--local-provider`、`--profile` / `--profile=...`，以及 `-c` / `--config` 写入的 `model`、`model_provider`、`openai_base_url`、`model_catalog_json`、`model_providers.*`。
- Profile merge：upstream limits/surface 与 alias override 合并正确。
- Limits：全局 config 只校验非零；通用 profile 构造不因 Codex 输入预算失败；Codex catalog/projection preflight 对 `max_output_tokens >= context_window` fail；`set-limits` 写入时 fail。
- Codex catalog：生成完整 shape；包含 parser 必需字段；`context_window=200000`、`max_output_tokens=128000` 时 auto compact 为 `61200`；catalog/projection preflight 和写入失败发生在 proxy 启动前。
- Codex tools：search false 生成 `tools.web_search=false`；不再生成 `web_search="disabled"`；view image false 在未确认支持时不生成 `tools.view_image=false`；当 view image false 且未声明 modalities 时默认 `input_modalities=["text"]`。
- Claude launch plan：默认 alias `main` 时 env 有 `ANTHROPIC_CUSTOM_MODEL_OPTION=main` 和 `ANTHROPIC_MODEL=main`，argv 不包含自动 `--model default`。
- Claude env scrub：managed profile 清理 `ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU}_MODEL` 及其 `_NAME` / `_DESCRIPTION` / `_SUPPORTED_CAPABILITIES` 等后缀变量，并清理 `ANTHROPIC_SMALL_FAST_MODEL` 系列变量。
- Claude reasoning/capabilities：不自动生成 `_SUPPORTED_CAPABILITIES`；`--effort` / `CLAUDE_CODE_EFFORT_LEVEL` 只作为 native passthrough 验证。
- Claude limits：`max_output_tokens` 投影到 `CLAUDE_CODE_MAX_OUTPUT_TOKENS`；`context_window` 投影到 `CLAUDE_CODE_AUTO_COMPACT_WINDOW`；默认不生成 `CLAUDE_CODE_MAX_CONTEXT_TOKENS`。
- `/models`：Anthropic top-level `max_input_tokens == context_window`、`max_tokens == max_output_tokens`，并与 `llmup.limits` 同源。
- `/models capabilities`：默认省略；不得从 `apply_patch_transport`、Claude Code `_SUPPORTED_CAPABILITIES` 或 native reasoning / effort 配置误生成 Anthropic Models API `capabilities`。
- `llmup-config set-limits`：`--alias` / `--upstream` 二选一、未知目标、冲突参数、字符串 alias 升级 structured alias、幂等写入、原子写权限、list-form upstreams 明确失败都要覆盖。

真实 CLI smoke：

- `codex --version`、`codex debug models -c model_catalog_json="<generated>"`。
- `tools.view_image=false` 只作为 CI/doctor probe；0.131.0 预期 reject。
- `tools.web_search=false` 作为 CI/doctor 兼容检查。
- `claude --version`。
- 用 Anthropic stub 抓包验证 custom model option 下 alias `main` 发送 `model: main`。
- 抓包验证 `CLAUDE_CODE_MAX_OUTPUT_TOKENS` 进入 `max_tokens`。`CLAUDE_CODE_EFFORT_LEVEL=xhigh` 只在 native passthrough smoke 中验证。

矩阵迁移：

- 将 Codex catalog builder、auto compact 公式、tool hint projection、Claude env projection 做成 Rust 共享模块/可测试函数。
- Python matrix 通过正式 binary 的 Rust hidden launch-plan JSON 获取 argv/env/artifacts，并以该输出作为 projection 事实来源；不要在 matrix 里继续维护 Python catalog builder、auto compact 公式、tool hint projection 或 Claude env projection helper。
- 不要求整个 matrix 改成直接跑 managed launcher，以免和矩阵自身 proxy 生命周期形成双 proxy。
- 可新增一个单独的正式 launcher smoke，覆盖 `llmup-codex` / `llmup-claude` 端到端启动；它不替换现有 matrix 生命周期。

## 5. 验收标准

- `llmup-codex --llmup-model main` 在 limits/surface 已知时生成并注入完整 Codex catalog；`codex debug models` 能看到期望的 context、auto compact、modalities 和 tool surface。
- managed projection 开启时，native model flag 和会覆盖 Codex llmup model/provider/catalog 的 native config 冲突 fail fast；escape hatch 模式不生成 profile projection。
- `llmup-codex` 在 proxy 启动前完成 catalog/profile preflight 和 catalog 写入；失败时不启动 proxy。
- `llmup-codex` 使用 `tools.web_search=false`；不默认注入 `tools.view_image=false`。
- `llmup-codex` 在 `supports_view_image=false` 且未声明 modalities 时生成 `input_modalities=["text"]`，但不注入 `tools.view_image=false`。
- `llmup-claude` 不再自动追加 `--model default`。
- `llmup-claude --llmup-model main` 通过 custom model option 让 Claude Code 发往 proxy 的请求 model 为 `main`。
- `llmup-claude` managed profile 清理会覆盖 llmup 选模的 Claude 原生默认模型环境变量，包括 default model family 和 small-fast model 系列。
- `llmup-claude` 不自动注入 `_SUPPORTED_CAPABILITIES`，只在配置中存在 `haiku` / `sonnet` / `opus` alias 时投射官方 family alias env。
- `CLAUDE_CODE_MAX_OUTPUT_TOKENS`、`CLAUDE_CODE_AUTO_COMPACT_WINDOW` 按 configured limits 注入；默认不依赖 `CLAUDE_CODE_MAX_CONTEXT_TOKENS`。
- Anthropic `/models` 本轮只补齐 `max_input_tokens=context_window` / `max_tokens=max_output_tokens`，并保留 `llmup.limits` / `llmup.surface`；不输出 `capabilities`。
- `llmup-config set-limits --alias ...` / `--upstream ...` 可用，只写 `config.yaml`，并覆盖冲突、未知目标、幂等、字符串 alias 升级、原子写权限和 list-form upstreams 明确失败测试。
- 没有新增 `codex:` / `claude:` YAML schema；没有 provider-wide 硬编码 limits；没有默认开启 Claude Code gateway discovery；没有测试专用 YAML 分支。
- Rust 单元测试、真实 launcher smoke、real CLI matrix 均对齐同一套 Rust projection。
