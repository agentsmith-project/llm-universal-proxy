# pre-GA 用户工具层安装、配置与 Coding Agent 启动改进计划

更新时间：2026-05-18

## 背景

当前中文快速开始已经把项目讲成“本机中转站”，但实际使用路径仍然偏开发者：

- 用户需要自己下载 release asset，自己放置二进制。
- 用户需要手写 YAML 和本地密钥文件。
- 用户需要运行 `scripts/run_codex_proxy.sh` 或 `scripts/run_claude_proxy.sh` 这类测试/开发味道很重的脚本名。
- 当前交互脚本每次用临时目录启动 Codex/Claude Code。`scripts/interactive_cli.py` 在运行时创建 `TemporaryDirectory`，再通过 `scripts/real_cli_matrix.py` 把 `HOME`、`CODEX_HOME`、`CLAUDE_CONFIG_DIR` 指到临时目录。这适合测试隔离，但不适合普通用户长期使用：会话、登录状态、resume 信息和用户配置都会跟着进程结束而丢失。
- 当前脚本固定重建 Codex/Claude 命令，不能把 `--resume`、`--yolo`、`--dangerously-skip-permissions`、`--permission-mode` 等真实客户端参数原样交给上游 CLI。

这份计划只处理外围用户工具层，不改变 `llm-universal-proxy --config <config.yaml>` 的服务端配置语义、Admin API、协议转换逻辑或已有测试矩阵入口。

## 设计目标

目标用户是“想把本地 Codex CLI 或 Claude Code 接到自己模型服务上”的普通用户，而不是 llmup 开发者。第一条用户路径必须收敛成“安装、配置、选择一个客户端启动”：

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/agentsmith-project/llm-universal-proxy/releases/latest/download/install.sh | sh
llmup-config
```

然后按用户实际使用的客户端二选一：

```bash
llmup-codex
# 或
llmup-claude
```

如果用户想固定到某个 release，安装脚本本身也应从版本化 release URL 获取：

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/agentsmith-project/llm-universal-proxy/releases/download/v0.2.32/install.sh | sh
```

产品体验目标：

- 用户不需要手写 YAML。
- 用户不需要手动 export API Key。
- 用户不需要知道 `upstream`、`model_aliases`、`provider_key` 等内部概念。
- 用户不需要手动下载二进制和配套脚本。
- Codex/Claude Code 的会话和配置默认持久化在 llmup 专用目录里。
- `llmup-codex <args>` 像 `codex <args>` 一样使用；`llmup-claude <args>` 像 `claude <args>` 一样使用。llmup 只消费第一个 `--` 之前的 `--llmup-*` 控制参数，以及一个可选 routing delimiter `--`。
- 现有高级用户仍然可以继续直接运行 `llm-universal-proxy --config ...`。

## 非目标

为防止产品范围蔓延，本计划明确不做：

- 不做 GUI、系统托盘、后台常驻服务或系统服务安装。
- 不自动安装 Codex CLI 或 Claude Code。缺少客户端时只给出清晰提示。
- 不自动迁移用户已有的 `~/.codex` 或 `~/.claude`。
- 不把 provider 市场、价格表、智能推荐、智能路由纳入 `llmup-config` 第一版。
- 不在用户向导里暴露搜索、图片、并行工具、context window 等高级能力开关。
- 不改变现有 YAML 配置格式、路由语义、协议转换矩阵或容器部署方式。
- 不承诺接入 ChatGPT/Claude App 订阅；这里服务的是模型 API 或兼容 API endpoint。
- 不先做 Windows 原生 PowerShell 完整安装体验。第一版聚焦 macOS、Linux、WSL；Windows 后续单独设计。

## 产品形态

公开用户命令只保留三个：

| 命令 | 面向用户的含义 | 责任边界 |
| --- | --- | --- |
| `llmup-config` | 配置 llmup | 交互式生成配置、保存密钥、检查环境 |
| `llmup-codex` | 用 llmup 启动 Codex CLI | 启动本地代理、注入 Codex 代理参数、透传 Codex 参数 |
| `llmup-claude` | 用 llmup 启动 Claude Code | 启动本地代理、注入 Claude 代理环境、透传 Claude 参数 |

同时保留服务端高级入口和 legacy/test harness：

- `llm-universal-proxy --config <config.yaml>`，作为高级服务端入口。
- `llm-universal-proxy --admin-bootstrap`，作为高级服务端入口。
- `scripts/run_codex_proxy.sh`，仅作为 legacy/test harness 或开发文档入口。
- `scripts/run_claude_proxy.sh`，仅作为 legacy/test harness 或开发文档入口。

第一版不提供独立 `llmup` 命令。它既不是必要用户动作，也容易让用户以为还有一套统一 agent shell 或二级子命令。用户只需要记住三个动作命令；高级用户继续使用 `llm-universal-proxy --config ...`。

推荐实现为同一个 Rust 二进制的多入口分发：

- release 里继续包含 `llm-universal-proxy`。
- 安装器额外创建 `llmup-config`、`llmup-codex`、`llmup-claude` 到同一二进制的 symlink/hardlink。
- 程序根据 `argv[0]` 分发；第一版不设计 `llmup <subcommand>` 入口。

这样可以避免继续把用户主入口绑在 Python 测试脚本上，也避免安装后还要求用户有 Python 运行环境。

### Help / Version 契约

去掉独立 `llmup` 后，安装后的帮助和版本信息必须仍然闭环，并且不能要求用户已经完成配置或安装 Codex/Claude Code：

| 命令 | 行为 |
| --- | --- |
| `llm-universal-proxy --help` | 显示服务端高级用法 |
| `llm-universal-proxy --version` | 显示版本 |
| `llmup-config --help` | 显示设置向导帮助 |
| `llmup-config --version` | 显示版本 |
| `llmup-codex --llmup-help` | 显示 Codex launcher 的 llmup 差异 |
| `llmup-codex --llmup-version` | 显示版本 |
| `llmup-claude --llmup-help` | 显示 Claude launcher 的 llmup 差异 |
| `llmup-claude --llmup-version` | 显示版本 |

不提供 `llmup --help`、`llmup --version` 或 `llmup <subcommand>`。

## 用户配置模型

默认目录：

| 路径 | 用途 |
| --- | --- |
| `~/.llmup` | llmup 工具层配置、密钥文件、运行日志 |
| `~/.llmup/config.yaml` | 生成给 llmup proxy 使用的服务端配置 |
| `~/.llmup/secrets.env` | 本地密钥文件，权限必须是 `0600` 或等价 |
| `~/.llmup-codex` | llmup 管理的 Codex 配置和会话目录 |
| `~/.llmup-claude` | llmup 管理的 Claude Code 配置和会话目录 |

环境变量覆盖：

| 变量 | 默认值 | 用途 |
| --- | --- | --- |
| `LLMUP_HOME` | `~/.llmup` | 覆盖 llmup 工具层目录 |
| `LLMUP_CODEX_HOME` | `~/.llmup-codex` | 覆盖 Codex 专用目录 |
| `LLMUP_CLAUDE_CONFIG_DIR` | `~/.llmup-claude` | 覆盖 Claude Code 专用目录 |

Codex 启动时设置：

```bash
CODEX_HOME="$LLMUP_CODEX_HOME"
```

Claude Code 启动时设置：

```bash
CLAUDE_CONFIG_DIR="$LLMUP_CLAUDE_CONFIG_DIR"
```

不要默认改写 `HOME`。Codex 有独立 `CODEX_HOME`；Claude Code 官方文档也明确 `CLAUDE_CONFIG_DIR` 会覆盖默认 `~/.claude`，并保存设置、会话历史和插件。保留用户真实 `HOME` 可以减少 git、ssh、npm、pnpm、uv、cargo 等工具在 agent 内运行时找不到用户凭据和缓存的风险。

Claude Code 凭据隔离要按平台描述清楚：`CLAUDE_CONFIG_DIR` 覆盖 Claude Code 的配置目录、会话历史和插件目录；Linux/Windows 上凭据文件也随该目录移动；macOS 上部分登录凭据可能在系统 Keychain 中，不能把 `CLAUDE_CONFIG_DIR` 宣传为“完全隔离所有登录态”。

`~/.llmup-codex` 和 `~/.llmup-claude` 是 llmup 管理的原生客户端 home，不是 llmup 自己的新会话系统。用户遇到 resume、settings、MCP、plugin、skills、memory、`CLAUDE.md` 等行为时，仍按 Codex CLI 或 Claude Code 原生心智理解。

## `llmup-config` 设计

普通用户只需要记一个命令：

```bash
llmup-config
```

`llmup-config` 无参数时进入单一交互流程：

- 第一次运行：创建配置、保存密钥、做一次基础检查，然后打印下一步命令。
- 已配置过：显示脱敏摘要，并提供“直接完成 / 重新配置 / 运行检查”三个选择。

第一版 README 和普通帮助不展示 `init` 或 `show` 这类第二套配置入口，避免用户产生“到底该运行哪个”的疑问。工程实现也不保留隐藏的非交互初始化命令；CI 和自动化测试应喂入真实交互入口，或在 Rust 单元测试里直接调用内部生成函数。

交互流程默认只问最少问题，提示文案面向普通用户：

首次配置示例必须保持 provider-neutral。MiniMax 只能作为可替换的 OpenAI Chat Completions 兼容服务示例，不能写得像默认 provider。

1. 协议格式，可直接回车使用默认 `openai-chat-completions`。
2. 模型服务地址，例如 `https://api.example.com/v1`。
3. 模型名，例如 `provider-model-id`。
4. API Key，以隐藏输入方式读取。

默认按 OpenAI Chat Completions (`/v1/chat/completions`) 服务生成配置，本地模型名固定为 `main`，不要在第一版向导里询问。协议格式用一个带默认值的可选问题处理：用户直接回车就是 `openai-chat-completions`；只有服务商明确要求 OpenAI Responses 或 Anthropic Messages 时，才输入对应值。

- OpenAI Chat Completions 接口，默认推荐。不确定就直接回车。
- OpenAI Responses 接口。仅当服务商明确要求 `/v1/responses` 时选择。
- Anthropic Messages 接口。仅当服务商明确要求 `/v1/messages` 时选择。

工程实现按用户选择生成清晰配置值；配置工具只接受完整协议名，不再接受容易误读的历史短名：

| 用户看到的选择 | 生成的配置值 |
| --- | --- |
| `openai-chat-completions` | `openai-chat-completions` |
| `openai-responses` | `openai-responses` |
| `anthropic-messages` | `anthropic-messages` |

生成结果：

- `~/.llmup/config.yaml`
- `~/.llmup/secrets.env`
- 必要时创建 `~/.llmup-codex` 和 `~/.llmup-claude`

`secrets.env` 里保存：

```bash
LLM_UNIVERSAL_PROXY_KEY=<random-local-proxy-key>
LLMUP_PROVIDER_MAIN_API_KEY=<user-provider-api-key>
```

`config.yaml` 只引用环境变量，不写明文 API Key。第一版生成完整可运行模板，不能只写局部片段：

```yaml
listen: 127.0.0.1:8080
upstream_timeout_secs: 120

data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY

upstreams:
  main:
    api_root: <model-service-url>
    format: <mapped-format>
    provider_key:
      env: LLMUP_PROVIDER_MAIN_API_KEY
    surface_defaults:
      modalities:
        input: ["text"]
        output: ["text"]
      tools:
        supports_search: false
        supports_view_image: false
        apply_patch_transport: freeform
        supports_parallel_calls: false

model_aliases:
  main: "main:<model-name>"
```

第一版不在向导里暴露 `limits`、tooling surface、搜索、图片或并行工具等能力开关。后续如果要开放，也必须从真实 provider 能力发现或明确高级配置开始，不进入普通用户首次路径。

检查项：

- `~/.llmup/config.yaml` 是否存在且可被 Rust 配置加载器解析。
- `~/.llmup/secrets.env` 是否存在、权限是否安全、是否包含必要变量。
- `codex` / `claude` 是否在 `PATH` 中；缺少任一客户端只显示提醒，不把配置本身标成失败。真正启动 `llmup-codex` 或 `llmup-claude` 时，如果对应客户端不存在，再给出阻塞错误和安装提示。
- 代理能否用生成配置启动并通过 `/health`。
- 本机端口冲突时是否可以自动换端口。

脱敏摘要只展示：

- 配置文件路径。
- 本地模型名。
- 模型服务类型。
- 模型服务地址。
- API Key 是否已配置，永不打印明文。
- 下一步命令：按用户使用的客户端运行 `llmup-codex` 或 `llmup-claude`。

不提供隐藏的 `llmup-config init --non-interactive`。测试需要生成配置时，优先通过真实交互入口喂入 stdin；Rust 单元测试可以直接调用内部生成函数。这样用户和测试只维护同一条产品路径，避免 hidden command 变成第二套心智。

## Agent 启动器设计

`llmup-codex` 和 `llmup-claude` 默认采用轻量监督模式，但不解析 Codex/Claude 的具体子命令语义。它们只做三件事：

1. 解析 llmup 自己的控制参数，形成 `LauncherControl`。
2. 把剩余参数作为 `NativeArgv`，保持顺序；managed profile projection 会对覆盖 llmup model/provider/catalog 的原生参数做 fail-fast preflight。
3. 根据 `LauncherControl` 选择 managed proxy 或 no-proxy passthrough，然后启动原生 CLI。

实现不得新增 Codex/Claude 子命令表或参数修复器；只保留 llmup profile projection 必需的 native model/provider/catalog 冲突 preflight。Codex `--profile` / `--profile=...` 可能间接加载原生 model/provider/catalog override，managed projection 下也按冲突处理。

默认是 managed proxy 模式：

1. 读取 `~/.llmup/config.yaml` 和 `~/.llmup/secrets.env`。
2. 自动选择本地端口，生成本次运行使用的完整 runtime YAML。
3. 启动一个本次会话专用的 `llm-universal-proxy` 子进程。
4. 等待 `/health` 成功，并确认子进程仍然存活。
5. 设置客户端需要的 base URL、本地 proxy key、配置目录环境变量。
6. 为目标客户端生成 managed profile projection 注入；显式关闭 projection 时只生成 proxy plumbing。
7. 在用户原始工作目录执行：`client_binary + InjectionPrelude + NativeArgv`。
8. Codex/Claude Code 退出后，停止本次启动的代理子进程，并返回客户端退出码。

no-proxy passthrough 模式只在用户显式传 `--llmup-no-proxy` 时启用：

1. 不读取 provider key。
2. 不启动 proxy。
3. 不注入 base URL、API key、模型、profile projection 或客户端配置目录。
4. 在用户原始工作目录执行：`client_binary + NativeArgv`。

这保持了当前“一条命令启动代理和客户端”的低心智负担，同时不需要引入后台 daemon、pid registry、跨进程生命周期管理或系统服务。

第一版普通帮助只保留最少集合：

| 参数 | 含义 |
| --- | --- |
| `--llmup-help` | 显示 launcher 自己的帮助 |
| `--llmup-version` | 显示 launcher / llmup 版本 |

`--llmup-no-proxy` 是 troubleshooting / advanced escape hatch，不进入 README 第一屏，但应出现在 `--llmup-help` 的 `Advanced / troubleshooting` 小节。它的含义必须用白话解释：只打开原来的 Codex/Claude 命令，不经过 llmup 代理；用于登录、原生帮助、原生配置、MCP 管理等不想经过 proxy 的命令。文档和 launcher help 都要给出结构化用法：`llmup-codex --llmup-no-proxy -- <native args>` 或 `llmup-claude --llmup-no-proxy -- <native args>`。不要让 launcher 自动识别子命令。

`--llmup-no-profile-projection` 是 managed proxy 模式下的 profile projection escape hatch：仍启动本地 proxy 并注入 base URL / API key，但不生成 Codex catalog/tool hints，不注入 Claude custom model option 或 limits，让高级用户自行管理 native model/provider/catalog 参数。

工程和测试可以保留隐藏参数，例如 `--llmup-port`、`--llmup-config`、`--llmup-env-file`，但第一版不写入 README，也不进入普通帮助。覆盖 home 目录优先使用 `LLMUP_HOME`、`LLMUP_CODEX_HOME`、`LLMUP_CLAUDE_CONFIG_DIR` 环境变量，不再额外设计 `--llmup-home`。

`--llmup-proxy-base` 和 `--llmup-keep-proxy` 暂不进入第一版。它们会把“一条命令启动本次会话代理”的主心智拆成“连接外部代理 / 复用代理 / 保留代理”等多种模式，收益不足以抵消复杂度。高级用户继续直接运行 `llm-universal-proxy --config ...`。

`--llmup-profile` 不进入第一版公开帮助。第一版只支持一个默认配置，避免制造“profile 已完整支持”的假心智；多 profile 后续单独设计。

不做后台 daemon 的原因：

- 用户关切主要是 Codex/Claude Code 的配置和会话持久化，不是 proxy 本身常驻。
- 每次启动一个本地子进程足够简单、可观察、可清理。
- 避免 pre-GA 阶段引入 stale pid、端口占用、配置 fingerprint、跨 shell 状态同步等额外复杂度。

runtime YAML 生成要求：

- 不直接把用户的 `~/.llmup/config.yaml` 原地改写。
- 每次 agent 会话在 `~/.llmup/run/<session-id>/config.yaml` 写入完整 YAML。
- runtime YAML 覆盖 `listen` 为本次选择的 `127.0.0.1:<port>`。
- 当前 `RuntimeConfigPayload` 不包含 `data_auth`，因此不能用它来回写 runtime YAML；必须保留完整 `data_auth.proxy_key` 配置。
- 端口被占用时重试有限次数；重试仍失败时给出清楚错误，不要求普通用户自己排查端口。
- 只停止自己启动的子进程；不按端口杀进程。

环境隔离要求：

- proxy 子进程加载 `secrets.env`，拿到真实 provider key 和本地 proxy key。
- Codex/Claude 子进程不能继承 `secrets.env` 里声明的真实 provider key；它们只拿本地 proxy key。
- 客户端环境从当前 shell 复制，但必须先移除 `secrets.env` 中声明的所有变量名；如能安全实现，也移除值等于 `secrets.env` 中 secret value 的父环境变量。随后显式覆盖注入客户端需要的本地 proxy key。第一版不要承诺清理用户 shell 里所有 unrelated provider secrets，避免误删用户环境且难以测试。
- Codex 客户端环境必须显式设置 `OPENAI_API_KEY="$LLM_UNIVERSAL_PROXY_KEY"`，并确保同名父环境值不会覆盖它。
- Claude Code 客户端环境必须显式设置 `ANTHROPIC_AUTH_TOKEN="$LLM_UNIVERSAL_PROXY_KEY"` 和 `ANTHROPIC_BASE_URL="http://127.0.0.1:<port>/anthropic"`，并确保同名父环境值不会覆盖它。
- Claude Code 客户端环境还必须移除可能绕过 `ANTHROPIC_BASE_URL` 的 provider routing 和认证 helper 变量，再显式写入 llmup 的 `ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_BASE_URL`。至少包括父环境中的 `ANTHROPIC_API_KEY`、`ANTHROPIC_AUTH_TOKEN`、`ANTHROPIC_BEDROCK_*`、`ANTHROPIC_VERTEX_*`、`ANTHROPIC_FOUNDRY_*`、`ANTHROPIC_AWS_*`、`ANTHROPIC_WORKSPACE_ID`、`AWS_*`、`GOOGLE_APPLICATION_CREDENTIALS`、`GCLOUD_PROJECT`、`GOOGLE_CLOUD_PROJECT`、`CLAUDE_CODE_USE_BEDROCK`、`CLAUDE_CODE_USE_VERTEX`、`CLAUDE_CODE_USE_FOUNDRY`、`CLAUDE_CODE_USE_MANTLE`、`CLAUDE_CODE_USE_ANTHROPIC_AWS`、`CLAUDE_CODE_SKIP_BEDROCK_AUTH`、`CLAUDE_CODE_SKIP_VERTEX_AUTH`、`CLAUDE_CODE_SKIP_FOUNDRY_AUTH`、`CLAUDE_CODE_SKIP_MANTLE_AUTH`、`CLAUDE_CODE_SKIP_ANTHROPIC_AWS_AUTH`。
- Claude Code 默认不设置 `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1`，避免 llmup 意外改变原生 permission / sandbox 行为；需要该 hardening 的高级用户可在外层环境中自行显式设置。
- 因为不改 `HOME`，dangerous/yolo 这类原生权限参数仍可能访问用户真实文件和凭据。llmup 不解析这些参数，用户需要按 Codex/Claude 原生文档自行理解风险。

## 原生 CLI 薄包装契约

`llmup-codex` / `llmup-claude` 不是新的 coding agent 客户端，而是带 llmup 本地代理能力的原生 CLI launcher。用户心智必须保持为“把原命令前缀换成 `llmup-codex` 或 `llmup-claude`”：

- `llmup-codex <native-args>` 等价于 `codex <native-args>`，只额外做 llmup 代理启动、配置目录设置、provider/base URL/API key 注入和 managed profile projection。
- `llmup-claude <native-args>` 等价于 `claude <native-args>`，只额外做 llmup 代理启动、配置目录设置、base URL/API key 注入和 managed profile projection。
- 不提供 `llmup` 主入口，避免长出另一套 agent 管理语义。
- launcher 只消费第一个 `--` 之前的 `--llmup-*` 控制参数，以及一个可选 routing delimiter `--`。其余参数属于原生 Codex/Claude CLI。
- 除 managed profile projection 的 model/provider/catalog 冲突 preflight 外，launcher 不归一化、不重排、不替用户解释原生 CLI 参数。未知参数、未知子命令、原生 help/version、原生管理命令都默认交给原生 CLI。
- `--` 是结构化传参边界：它只告诉 llmup “后面全部是原生 argv”。第一个 `--` 不传给客户端；如果用户确实需要给客户端传一个字面量 `--`，使用 `-- --`。
- 禁止发明跨客户端统一原生行为参数或语义，例如 `--llmup-resume`、`--provider`、`--api-key`、统一 permission mode、统一 sandbox mode。`resume`、`--model`、`--permission-mode`、`--sandbox`、`--bare` 等都是原生客户端语义。
- launcher 不默认注入原生行为参数：不默认加 Codex `-C`、sandbox、yolo 或 permission bypass；不默认加 Claude `--bare`、`--add-dir`、permission mode。工作目录使用用户启动命令时的 cwd，原生工作区/权限/最小模式参数由用户自己传。
- 缺少 llmup 配置时，launcher 不自动进入配置向导，只给出清晰下一步：运行 `llmup-config`。配置教育属于 `llmup-config`，不是 agent launcher。
- managed profile projection 拥有 llmup model/provider/catalog 选择；相关 native override 会 fail fast。需要自行管理 native model/provider/catalog 时使用 `--llmup-no-profile-projection`，需要完全原生命令时使用 `--llmup-no-proxy`。其他权限、sandbox、最小模式等原生参数仍由用户按原生文档负责。
- 终端默认属于 Codex/Claude。llmup 只在缺配置、缺客户端、proxy 启动失败、`--llmup-*` 参数错误等 preflight 场景输出；proxy stdout/stderr 默认写入 `~/.llmup/run/<session-id>/`，不刷屏。
- stdin/stdout/stderr、TTY、窗口 resize、Ctrl-C、SIGINT/SIGTERM 和退出码都要尽量保持原生 CLI 体验。launcher 只负责清理自己启动的 proxy 子进程，并原样返回客户端退出码；信号语义无法完全保留时必须有测试记录。
- README 和帮助文案不要复制 Codex/Claude 参数表，只说明 llmup 会消费第一个 `--` 之前的 `--llmup-*` 控制参数和可选 routing delimiter `--`，其余进入 `NativeArgv`，并链接官方文档。复制原生参数会快速过期，也会暗示 llmup 拥有这些语义。

## 结构化传参模型

核心规则：`llmup-*` launcher 不解析 Codex/Claude 子命令，只做结构化 argv routing。

输入被拆成三块：

```text
LauncherControl  = 第一个 -- 之前的已知 --llmup-* 参数
NativeArgv       = 所有非 --llmup-* 参数，加上第一个 -- 之后的全部参数
InjectionPrelude = managed proxy 模式下由 client adapter 生成的注入参数；默认包含 profile projection
```

最终执行：

```text
managed: client_binary + InjectionPrelude + NativeArgv
no-proxy: client_binary + NativeArgv
```

`--` 的唯一作用是停止 llmup 参数解析。第一个 `--` 不进入 `NativeArgv`；它后面的内容全部进入 `NativeArgv`，包括 `--llmup-*`。

解析算法必须保持简单：

1. 从左到右扫描 argv，遇到第一个字面量 `--` 立即停止 llmup 参数解析。
2. 第一个 `--` 之前，只消费已知 `--llmup-*` 参数；未知 `--llmup-*` 报清楚错误。
3. 第一个 `--` 之前的其他参数逐个进入 `NativeArgv`，不因看起来像子命令而改变规则。
4. 第一个 `--` 之后的所有参数逐个进入 `NativeArgv`，包括第二个 `--` 和任何 `--llmup-*`。
5. 如果用户确实要把 `--llmup-*` 当成原生客户端参数，必须放到第一个 `--` 之后。

示例：

```bash
llmup-codex resume --last
llmup-codex --yolo
llmup-codex --ask-for-approval never --sandbox workspace-write
llmup-codex --llmup-no-proxy -- mcp list
llmup-codex -- --help

llmup-claude --resume
llmup-claude --permission-mode bypassPermissions
llmup-claude --dangerously-skip-permissions
llmup-claude --llmup-no-proxy -- auth
llmup-claude -- --resume my-session
```

实现要求：

- 第一阶段只抽取第一个 `--` 之前的已知 `--llmup-*` 及其值；抽取后的客户端参数必须以 `Vec<OsString>` 或等价结构保存，并通过 `Command.args(...)` 交给原生 CLI。禁止拼接 shell 字符串，禁止 UTF-8 重编码后再切分。
- 保留用户参数顺序。
- 不吞掉未知参数。
- 不把 `--resume`、`resume`、`--yolo`、`--dangerously-*` 转成 llmup 中间语义。
- 不再使用 `--dangerous-harness` 这类只服务测试脚本的产品参数。
- 第一个 `--` 后所有参数无条件视为客户端参数，包括 `--llmup-*`、`--help` 和 `--version`；第一个 `--` 本身不传给客户端。
- `--llmup-help` / `--llmup-version` 是 launcher 自己的帮助/版本，且不要求本机已安装 Codex/Claude Code。原生 `--help` / `-h` / `--version` / `-v` 必须交给客户端，`resume --help`、`doctor --help`、`-- --help` 也必须交给客户端。
- 隐藏工程参数如果保留，例如 `--llmup-port=1234` 和 `--llmup-port 1234`，缺值要报清楚错误，但不能出现在第一版 README 和普通帮助里。
- 第一个 `--` 之前的未知 `--llmup-*` 必须报清楚错误；非 `--llmup-*` 的未知参数必须透传。
- `--` 后即使出现 `--llmup-*`，也必须作为客户端参数透传。
- managed proxy 模式生成必要的 proxy plumbing；默认 managed profile projection 还会投射所选 llmup alias。`NativeArgv` 如包含覆盖 llmup model/provider/catalog 的原生参数，launcher 在启动前 fail fast；显式传 `--llmup-no-profile-projection` 时跳过 profile projection，并把这些原生参数交给客户端。
- no-proxy passthrough 模式总是不生成 `InjectionPrelude`。

## Codex 注入策略

Codex 官方文档说明 CLI 参数和 `-c key=value` 会覆盖本次 invocation 的配置。`llmup-codex` 应利用这一点，只注入本次运行必需的 provider 设置，不修改用户 `~/.codex/config.toml`。custom provider id 使用 `proxy`，不能占用官方或常见内置 id，例如 `openai`、`ollama`、`lmstudio`。

managed profile projection 模式下生成 `InjectionPrelude`：

```bash
codex \
  -c model_provider=\"proxy\" \
  -c model_providers.proxy.name=\"llmup\" \
  -c model_providers.proxy.base_url=\"http://127.0.0.1:<port>/openai/v1\" \
  -c model_providers.proxy.env_key=\"OPENAI_API_KEY\" \
  -c model_providers.proxy.wire_api=\"responses\" \
  -c model_providers.proxy.supports_websockets=false \
  -c model_catalog_json=\"<llmup-run-or-artifact-dir>/codex/model-catalog.json\" \
  -m <alias> \
  <NativeArgv...>
```

这条规则对所有 `NativeArgv` 一致，包括 `resume`、`exec`、`review`、`mcp`、`login`、`--help` 等。llmup 不区分哪些是 agent 会话命令，哪些是 Codex 本地管理命令。用户需要 no-proxy Codex 原生命令时使用 `--llmup-no-proxy`。

同时设置：

```bash
OPENAI_API_KEY="$LLM_UNIVERSAL_PROXY_KEY"
CODEX_HOME="$LLMUP_CODEX_HOME"
```

`OPENAI_BASE_URL` 可以作为兼容性冗余设置，但正确性必须依赖 `-c model_providers.proxy.base_url=...` 注入，而不是依赖环境变量。

这是 managed profile projection：`llmup-codex` 把 Codex 指到本地 proxy，并把所选 llmup alias 投射为 Codex 可见的模型画像。launcher 会根据配置里的 limits / `llmup.surface` 生成 Codex model catalog 和 supported tool hints；profile 声明不支持 web search 时注入 `-c tools.web_search=false`。协议转换和请求执行仍由 proxy 配置和服务端转换承担。

`-m <alias>` 由 managed projection 注入选中的 llmup alias，默认 alias 是 `main`。managed projection 开启时，如果 `NativeArgv` 包含会覆盖 llmup model/provider/catalog 的原生参数，例如 `-m`、`--model`、`--oss`、`--local-provider`、`--profile` / `--profile=...`、`-c model=...`、`-c model_provider=...`、`-c model_catalog_json=...` 或 `-c model_providers.*=...`，launcher 必须在启动前 fail fast，提示用户改用 `--llmup-model <alias>` 或显式加 `--llmup-no-profile-projection` 后自行管理 native 参数。需要完全不经过 llmup 时使用 `--llmup-no-proxy`。

由于 Codex 对 option/subcommand 位置有自己的解析规则，真实 CLI smoke 必须覆盖最常见的 managed 命令形态，例如 `llmup-codex`、`llmup-codex resume --last`、`llmup-codex exec ...`。如果某个 Codex 版本不接受 managed projection 注入，不能引入子命令表作为 workaround，应优先改用位置无关的配置注入方式，例如 llmup 专用 `CODEX_HOME` 下的配置文件，或把该行为记录为当前版本限制。

Codex model catalog/tool hints 文件必须放在 llmup 管理的 run/artifact dir 中；managed launcher 使用本次 session run dir，hidden launch-plan 使用传入的 artifact dir，不能覆盖用户自己的 `~/.codex`。

## Claude Code 注入策略

Claude Code 官方文档说明：

- `--resume` / `-r` 支持恢复会话。
- `--permission-mode`、`--settings`、`--setting-sources` 等运行时参数会覆盖配置。
- `CLAUDE_CONFIG_DIR` 会覆盖默认 `~/.claude`，并保存设置、会话历史和插件；Linux/Windows 凭据文件也随目录移动，macOS 的部分登录凭据可能仍由系统 Keychain 管理。

`llmup-claude` 应设置：

```bash
ANTHROPIC_AUTH_TOKEN="$LLM_UNIVERSAL_PROXY_KEY"
ANTHROPIC_BASE_URL="http://127.0.0.1:<port>/anthropic"
CLAUDE_CONFIG_DIR="$LLMUP_CLAUDE_CONFIG_DIR"
ANTHROPIC_CUSTOM_MODEL_OPTION="<alias>"
ANTHROPIC_MODEL="<alias>"
ANTHROPIC_CUSTOM_MODEL_OPTION_NAME="<alias>"
ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION="llmup proxy model <alias>"
```

默认命令形态不追加 native model 参数：

```bash
claude \
  <NativeArgv...>
```

不要默认加入 `--bare`。它是 Claude Code 的最小模式，会跳过 hooks、skills、plugins、MCP servers、auto memory、`CLAUDE.md` 自动发现，并跳过 OAuth/keychain reads。需要干净环境时，用户可以自己传 `--bare`；第一版不增加 `--llmup-bare`。

这条规则对所有 `NativeArgv` 一致，包括 `--resume`、`auth`、`mcp`、`doctor`、`--help` 等。llmup 不区分哪些是 agent 会话命令，哪些是 Claude Code 本地管理命令。用户需要 no-proxy Claude Code 原生命令时使用 `--llmup-no-proxy`。

Claude Code 的模型选择通过 `ANTHROPIC_CUSTOM_MODEL_OPTION=<alias>` 和 `ANTHROPIC_MODEL=<alias>` 投射。managed projection 开启时，如果 `NativeArgv` 包含 Claude `--model` / `--model=...`，launcher 必须在启动前 fail fast，提示用户改用 `--llmup-model <alias>` 或显式加 `--llmup-no-profile-projection` 后自行管理 native 参数。

与 Codex 相同，Codex 和 Claude Code 的 managed projection 注入都必须由真实 CLI smoke 保护。如果不可行，优先改用位置无关的配置注入方式，不维护子命令表。

## 安装器设计

新增 release asset：

- `install.sh`
- 每个平台 archive 继续带 `.sha256`
- archive 内包含 `llm-universal-proxy`，安装后创建 `llmup-config`、`llmup-codex`、`llmup-claude`

安装命令：

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/agentsmith-project/llm-universal-proxy/releases/latest/download/install.sh | sh
```

支持：

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/agentsmith-project/llm-universal-proxy/releases/download/v0.2.32/install.sh | sh
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/agentsmith-project/llm-universal-proxy/releases/latest/download/install.sh | sh -s -- --bin-dir "$HOME/bin"
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/agentsmith-project/llm-universal-proxy/releases/latest/download/install.sh | sh -s -- --no-modify-path
```

版本语义必须写清楚：

- `releases/download/vX.Y.Z/install.sh | sh` 是安装脚本和下载 asset 都固定到 `vX.Y.Z`。
- 第一版不提供 `--asset-version`。固定版本只使用版本化 release URL，避免“脚本版本”和“二进制版本”分离带来的解释成本。
- README 给普通用户展示 latest 路径；工程文档和 release notes 同时提供版本化路径。

安装器要求：

- `install.sh` 必须 POSIX `sh` 兼容，因为文档用 `| sh`。
- 默认安装到 `~/.local/bin`。
- 不默认使用 `sudo`。
- 自动识别 macOS/Linux/WSL 和 x86_64/aarch64。
- 下载对应 release archive 和 `.sha256`。
- SHA-256 校验失败必须停止安装。
- 明确 SHA-256 只是同一 release 下的完整性校验，用来发现传输损坏或 asset mismatch；它不是完整供应链认证，不能替代签名、发布者身份校验或 GitHub 账户安全。
- 解包前检查 archive entry，拒绝绝对路径、`..` path traversal、非预期文件名和多余可执行文件。
- 下载和解包在临时目录完成，安装用 atomic rename。
- 已有文件不静默覆盖，除非是同一安装器管理的旧版本或用户显式确认。
- 目标目录不可写时失败并提示，不自动 sudo。
- 路径含空格要可用。
- 修改 shell profile 必须幂等，带清晰 marker，并支持 `--no-modify-path`。
- 安装器必须检测目标安装目录是否在当前 `PATH` 中。因为 `curl ... | sh` 不能修改当前 shell 的环境，如果目录已在当前 `PATH`，安装结束打印短命令：`llmup-config`、`llmup-codex`、`llmup-claude`。如果目录不在当前 `PATH`，安装结束必须打印当前终端可直接复制运行的绝对路径，例如 `$HOME/.local/bin/llmup-config`，并说明重新打开终端后可使用短命令。
- 安装结束打印极短命令表：`llmup-config` 设置模型服务，`llmup-codex` 启动 Codex CLI，`llmup-claude` 启动 Claude Code，`llm-universal-proxy --help` 查看高级服务端用法；当目录不在当前 `PATH` 时，命令表使用绝对路径。

暂不把 GPG/cosign 签名作为本计划的交付硬门槛。官方 Claude Code 和主流工具展示了签名 manifest 的更强做法，llmup 可以在 release 签名基础设施稳定后补充；当前计划的硬要求是 TLS 下载加 SHA-256 完整性校验、无 sudo 默认、版本化 installer URL 和可审查安装脚本。

## 实施落点

建议新增 Rust 模块：

- `src/user_tools/mod.rs`
- `src/user_tools/config_wizard.rs`
- `src/user_tools/agent_launcher.rs`
- `src/user_tools/install_metadata.rs`
- `src/user_tools/env_file.rs`

`src/main.rs` 调整为：

- 先根据 `argv[0]` 判断是否进入用户工具层。
- 如果是旧服务端参数，保持当前 `--config` / `--admin-bootstrap` 行为。
- 用户工具层的 parser 必须保留未知参数，以便传给 Codex/Claude。

配置生成应复用 `src/config.rs` 的 serde 加载和 validate 路径，不要在产品代码里复制 Python harness 的手写 YAML 解析器。

现有 Python 脚本只作为开发/测试 harness 保留，不进入 README、普通帮助、安装后下一步或 release notes 主路径。它们当前会使用临时 home，并可能注入 Codex `-C` / sandbox、Claude `--bare` / `--add-dir` 等测试参数，因此不能作为新产品 launcher 的兼容基线。测试矩阵需要 launcher projection 时，必须通过带环境门禁的 Rust hidden launch-plan 读取正式生成的 argv/env/artifacts；不要在 Python harness 里维护 Codex catalog、tool hint 或 Claude env projection helper。

release workflow 需要同步调整：

- Unix archive 中仍包含主二进制，安装器负责创建 symlink/hardlink。
- Windows archive 后续可复制多份或提供 `.cmd` launcher；第一版文档不承诺 Windows 原生安装。
- release job 必须把 `install.sh` 作为单独 release asset 上传，不能只上传平台 archive。workflow 的 artifact download / upload pattern 必须覆盖 `install.sh`。
- 平台 archive 内不需要包含 `install.sh`；在线安装路径依赖 release asset 里的单独 `install.sh`。
- build 或 release gate 增加安装器 smoke：在临时 `HOME` 和临时 `bin-dir` 运行安装脚本，再执行 `llm-universal-proxy --help`、`llm-universal-proxy --version`、`llmup-config --help`、`llmup-config --version`、`llmup-codex --llmup-help`、`llmup-codex --llmup-version`、`llmup-claude --llmup-help`、`llmup-claude --llmup-version`。
- CI 固定 toolchain/target，产物可校验；bit-for-bit reproducible builds 不作为本计划硬门槛。

## TDD 任务清单

先写失败测试，再实现。

优先级：

- P0：配置生成、argv routing、env isolation、proxy lifecycle、安装器 smoke、fake full-flow E2E。
- P1：真实 Codex/Claude CLI smoke、PTY/window resize、信号边界和平台差异。

Rust 单元/集成测试：

- `llm-universal-proxy --config proxy.yaml` 旧行为不变。
- `llmup-config`、`llmup-codex`、`llmup-claude` 分发正确；用户帮助快照中不得出现独立 `llmup` 入口或 `llmup <subcommand>` 用法。
- `llmup-config` 交互式生成的 YAML 能被 `Config::from_yaml_str` 解析并 validate。
- `llmup-config` 不覆盖已有配置，除非用户在交互式流程里显式选择 reconfigure。
- `secrets.env` 写入权限为 `0600` 或平台等价。
- `llmup-config` 的脱敏摘要和检查输出不包含明文 API Key。
- env file parser 支持安全的 `KEY=value` 子集，拒绝 shell 展开、命令替换和非法 key。
- `llmup-config` 内置检查对未安装的 Codex/Claude 只给 warning，不把配置生成结果标成失败；对应 launcher 真正启动时再阻塞。
- runtime YAML 生成保留完整 `data_auth`，覆盖 `listen`，不修改用户原始配置。
- P0 full-flow E2E：临时 `HOME` 下运行真实 `llmup-config` 交互入口并通过 stdin 输入配置，生成 `config.yaml` / `secrets.env`；`llmup-codex` 和 `llmup-claude` 分别启动本地 proxy；fake client 使用注入的 base URL 发请求；mock upstream 断言请求到达、model alias 解析为真实模型、provider auth header 正确；fake client 环境断言没有 `secrets.env` 中声明的真实 provider key，只有本地 proxy key；用户原始 `~/.llmup/config.yaml` 未被 launcher 原地改写，只有 runtime YAML 覆盖本次 `listen`。

Launcher 测试：

- fake `codex` 接收到 `--resume`、`resume --last`、`--yolo`、`--ask-for-approval never` 等原始参数。
- fake `claude` 接收到 `--resume`、`--permission-mode bypassPermissions`、`--dangerously-skip-permissions` 等原始参数。
- fake client 逐参数记录 argv；未知参数、带空格路径、`--foo=x`、短参组合、多个 `--`、`--` 后的 `--llmup-port` 都逐参数保留。
- 第一个 `--` 之前的已知 `--llmup-*` 参数进入 `LauncherControl`，不传给客户端。
- 第一个 `--` 本身不传给客户端；之后的参数全部传给客户端，包括 `--llmup-*`。
- `-- --` 会给客户端传入一个字面量 `--`。
- 非 `--llmup-*` 未知参数透传；未知 `--llmup-*` 报清楚错误。
- `--llmup-no-proxy` 不启动代理、不注入模型、不设置 llmup 客户端配置目录，只执行完全原生客户端命令。
- managed proxy 模式生成 proxy plumbing，并在默认 managed profile projection 下投射所选 llmup alias。
- managed profile projection 下，`NativeArgv` 中出现会覆盖 llmup model/provider/catalog 的原生参数时 fail fast；Codex `--profile` / `--profile=...` 也属于该类冲突；`auth`、`mcp`、`doctor`、`--help` 等普通原生命令或参数继续透传。
- `--llmup-no-profile-projection` 下不生成 Codex catalog/tool hints，不注入 Claude custom model env/limits，并允许用户自行传 native model/provider/catalog 参数。
- no-proxy passthrough 模式不生成 `InjectionPrelude`。
- managed 模式连续两次运行使用同一个 `CODEX_HOME=~/.llmup-codex`。
- managed 模式连续两次运行使用同一个 `CLAUDE_CONFIG_DIR=~/.llmup-claude`。
- `HOME`、`XDG_*`、`TMPDIR` 默认保持父进程原值，不被 launcher 改写。
- 客户端退出码被原样返回。
- stdin/stdout/stderr 默认透给客户端；proxy 日志默认写入运行目录，不污染客户端 TUI。
- P1 PTY smoke 覆盖窗口 resize 和 SIGINT/SIGTERM；这类测试只验证关键契约，不要求在所有平台上穷尽信号语义。
- 隐藏工程参数不出现在普通帮助；内部 launch-plan flags、`--llmup-proxy-base` 和 `--llmup-keep-proxy` 不作为第一版普通用户测试目标。
- 真实 CLI smoke 验证 managed projection 注入能被当前 Codex/Claude 接受，至少覆盖默认启动、Codex `resume --last`、Codex `exec ...`、Claude `--resume`、原生 `--help`。不为每个 Codex/Claude 子命令建兼容矩阵；如果当前注入方式在常见命令上不可行，优先改用位置无关的配置注入方式。
- launcher 自己的 `--llmup-help`、`--llmup-version` 不要求客户端存在；原生 `--help` 走 `NativeArgv`，是否需要 proxy 由 managed/no-proxy 模式统一决定。
- 客户端子进程不继承 `secrets.env` 中声明的真实 provider key，只收到本地 proxy key。
- 父环境中的 `OPENAI_API_KEY`、`ANTHROPIC_API_KEY`、`ANTHROPIC_AUTH_TOKEN` 不覆盖 llmup 注入给客户端的本地 proxy key；unrelated parent secrets 不作为第一版清理承诺。
- 父环境中的 Claude provider selector 和 gateway/auth helper 变量不会绕过 llmup：`CLAUDE_CODE_USE_BEDROCK`、`CLAUDE_CODE_USE_VERTEX`、`CLAUDE_CODE_USE_FOUNDRY`、`CLAUDE_CODE_USE_MANTLE`、`ANTHROPIC_BEDROCK_*`、`ANTHROPIC_VERTEX_*`、`ANTHROPIC_FOUNDRY_*`、`ANTHROPIC_AWS_*`、`GOOGLE_APPLICATION_CREDENTIALS` 等被 scrub 后再显式注入 llmup 的 `ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_BASE_URL`。

安装器测试：

- 在临时 `HOME` 下安装到 `~/.local/bin`。
- OS/arch 映射到正确 asset 名。
- checksum mismatch fail closed。
- `--bin-dir` 和 `--no-modify-path` 生效。
- shell profile marker 幂等。
- 临时 `HOME` 且安装目录不在当前 `PATH` 时，安装器输出可直接运行的绝对路径下一步命令；测试用该绝对路径执行 `llmup-config --help`。
- 安装后 `llm-universal-proxy --help`、`llm-universal-proxy --version`、`llmup-config --help`、`llmup-config --version`、`llmup-codex --llmup-help`、`llmup-codex --llmup-version`、`llmup-claude --llmup-help`、`llmup-claude --llmup-version` smoke 通过，且不要求本机已安装 Codex/Claude Code。
- 版本化 installer URL 固定安装脚本和 asset；第一版不提供 `--asset-version`。
- archive path traversal、已有 symlink 覆盖、路径含空格、unsupported OS/arch 都有 fail-closed 覆盖。

文档测试：

- `README.md` 和 `README_CN.md` 第一屏只保留安装、设置、启动三个动作。
- README 可以说明“没有独立 `llmup` 主命令是刻意设计”，避免用户自然尝试 `llmup --help` 后误判安装失败。
- 普通用户文档不再要求 clone repo、`cargo build`、手写 YAML、手动下载 asset、手动 export API Key。
- 普通用户文档不得把 `scripts/run_codex_proxy.sh`、`scripts/run_claude_proxy.sh`、`--dangerous-harness`、`--config-source`、`--env-file`、`--proxy-base` 当作用户路径展示；开发文档可以通过 allowlist 保留。
- README 不出现真实 API Key 示例。

## 验收标准

面向用户：

- 全新 macOS/Linux/WSL 环境里，用户可以通过在线脚本安装主二进制和三个友好命令。
- 用户运行 `llmup-config` 后，可以不手写 YAML、不 export 环境变量完成 OpenAI Chat Completions、OpenAI Responses 或 Anthropic Messages 服务配置；MiniMax 只能作为可替换示例出现。
- 用户运行 `llmup-codex` 后，Codex CLI 请求经本地 llmup 到达 mock upstream，认证头是 provider key，模型名由 `main` alias 解析为真实模型，客户端只持有本地 proxy key，退出码透传。
- 用户运行 `llmup-claude` 后，Claude Code 请求经本地 llmup 到达 mock upstream，认证头是 provider key，模型名由 `main` alias 解析为真实模型，客户端只持有本地 proxy key，退出码透传。
- `llmup-codex resume --last`、`llmup-codex --yolo`、`llmup-claude --resume`、`llmup-claude --dangerously-skip-permissions` 不被 llmup 拒绝或吞参。
- 除第一个 `--` 之前的 `--llmup-*` 和可选 routing delimiter `--` 外，用户可以继续使用 Codex/Claude 官方文档里的原生命令、子命令和参数；llmup 不要求用户学习另一套 agent 参数。
- 用户需要执行 no-proxy 命令时，例如登录、更新、MCP 管理、查看原生帮助，可以使用 `--llmup-no-proxy`。
- Codex 和 Claude Code 的会话、配置、resume 数据跨进程保留。
- `llmup-config`、`llmup-codex`、`llmup-claude` 可用；README 不提供独立 `llmup` 入口或第二套 `llmup <subcommand>` 路径。
- `llmup-codex --llmup-help` / `llmup-claude --llmup-help` 显示 launcher 帮助；原生帮助属于用户传给 Codex/Claude 的 `NativeArgv`。

面向工程：

- 旧的服务端入口、YAML 配置格式和测试脚本不被破坏。
- 新入口不依赖 Python。
- 用户工具层只消费第一个 `--` 之前的 `--llmup-*` 控制参数和可选 routing delimiter `--`。
- 参数透传有 argv golden tests 保护，确认只多出必要注入项，`NativeArgv` 顺序和内容不被重写。
- API Key 不出现在日志、配置摘要、检查输出、错误信息或生成的 README 示例中。
- release archive 和安装器产物可校验，release workflow 覆盖安装器 smoke。

## 风险与处理

| 风险 | 处理 |
| --- | --- |
| Codex/Claude CLI 参数未来变化 | 默认透传未知参数，只解析第一个 `--` 前的 `--llmup-*`，并用 `--` 作为可选 routing delimiter |
| Codex/Claude 新增或改变子命令 | llmup 不维护子命令表；所有子命令都是 `NativeArgv`；需要 no-proxy 执行时用 `--llmup-no-proxy` |
| launcher 慢慢变成假 Codex/Claude CLI | 薄包装契约写成硬约束；帮助和 README 只解释 llmup 差异，不复制原生 CLI 参数表 |
| 用户用原生参数绕过 projection 或改变模型/provider/catalog | managed projection 下 fail fast；需要自行管理 native model/provider/catalog 时用 `--llmup-no-profile-projection`；需要完全原生命令时用 `--llmup-no-proxy` |
| 临时目录问题再次出现 | 产品 launcher 禁止使用 `TemporaryDirectory` 作为客户端配置目录 |
| 改写 `HOME` 影响用户工具 | 默认不改 `HOME`；managed 模式只设置 `CODEX_HOME` / `CLAUDE_CONFIG_DIR` |
| provider key 泄露给客户端或工具子进程 | proxy 与客户端分开构造 env，客户端只拿本地 proxy key |
| 用户误用危险权限 | 危险权限属于原生 CLI 行为；README 提醒用户阅读 Codex/Claude 原生文档，llmup 不解析或代替用户决策 |
| API Key 明文落盘 | 第一版用 `0600` env 文件加全链路脱敏；系统 keychain 留作后续增强 |
| 安装脚本供应链风险 | TLS、SHA-256 完整性校验、无 sudo 默认、版本化 installer URL、脚本可审查；签名作为后续增强 |
| 后台代理生命周期变复杂 | 第一版不做 daemon，只做当前会话子进程监督 |

## 外部参考

本计划在 2026-05-18 重新检查了以下资料：

- OpenAI Codex CLI 安装与官方仓库：https://github.com/openai/codex
- OpenAI Codex CLI 参数参考：https://developers.openai.com/codex/cli/reference
- OpenAI Codex 配置基础：https://developers.openai.com/codex/config-basic
- OpenAI Codex 配置参考：https://developers.openai.com/codex/config-reference
- Claude Code 安装文档：https://code.claude.com/docs/en/setup
- Claude Code CLI 参数参考：https://code.claude.com/docs/en/cli-usage
- Claude Code settings：https://code.claude.com/docs/en/settings
- Claude Code `.claude` 目录说明：https://code.claude.com/docs/en/claude-directory
- Claude Code 环境变量参考：https://code.claude.com/docs/en/env-vars
- Claude Code 身份与凭据参考：https://code.claude.com/docs/en/iam
- Rust 安装器参考：https://rust-lang.org/tools/install/
- uv 安装器参考：https://github.com/astral-sh/uv/blob/main/docs/getting-started/installation.md
- Homebrew 安装器参考：https://docs.brew.sh/Installation.html
- GitHub release asset 直链规则：https://docs.github.com/en/repositories/releasing-projects-on-github/linking-to-releases
- rustup proxy/委托模型参考：https://rust-lang.github.io/rustup/concepts/proxies.html
- uv `run` 参数分隔参考：https://docs.astral.sh/uv/reference/cli/#uv-run

## Team Review 结果

- 产品/用户体验 review：要求把首次向导里的协议术语藏到工程映射里，明确安装后是 Codex/Claude 二选一启动，避免公开第一版不完整的 profile 概念。后续 KISS review 进一步确认第一版不需要独立 `llmup` 命令，本文已修订。
- 代码架构 review：要求补齐 runtime YAML、端口重试、`data_auth` 保留、`--llmup-*` 解析、release workflow 和 installer gate。本文已修订。
- 生态/安装器 review：要求收窄 SHA-256 的安全表述、补版本化 installer URL、补 Codex subcommand 注入规则、补 Claude macOS Keychain caveat 和客户端 env 隔离。本文已修订。
- 薄包装契约追加 review：产品、CLI 架构和官方文档/生态实践 review 均确认 `llmup-codex` / `llmup-claude` 应设计为原生 CLI 薄包装。后续根据产品收敛要求，本文进一步简化为结构化 argv routing：只解析第一个 `--` 前的 `--llmup-*` 和可选 routing delimiter `--`，不维护 Codex/Claude 子命令表，managed 模式统一启动 proxy 并执行 managed profile projection，no-proxy passthrough 由 `--llmup-no-proxy` 显式选择。
- 提交后产品经理 team 复审：要求继续降低普通用户心智，补齐去掉独立 `llmup` 后的 help/version 闭环，减少 `llmup-config` 首次问题，补完整可运行 config 模板、P0 fake full-flow E2E、精确 env scrub、release `install.sh` asset 链路和用户文档泄漏检查。第二轮复审又补出当前 shell `PATH` 不生效、managed projection 与 native override 冲突、env scrub 合同不一致等问题。本文已修订。
