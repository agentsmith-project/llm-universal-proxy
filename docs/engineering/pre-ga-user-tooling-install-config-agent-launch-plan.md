# pre-GA 用户工具层安装、配置与 Coding Agent 启动改进计划

更新时间：2026-05-18

## 背景

当前中文快速开始已经把 `llmup` 讲成“本机中转站”，但实际使用路径仍然偏开发者：

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
- Codex/Claude Code 的原生命令行参数默认可透传。
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

新增三个用户命令：

| 命令 | 面向用户的含义 | 责任边界 |
| --- | --- | --- |
| `llmup-config` | 配置 llmup | 交互式生成配置、保存密钥、检查环境 |
| `llmup-codex` | 用 llmup 启动 Codex CLI | 启动本地代理、注入 Codex 代理参数、透传 Codex 参数 |
| `llmup-claude` | 用 llmup 启动 Claude Code | 启动本地代理、注入 Claude 代理环境、透传 Claude 参数 |

同时保留：

- `llm-universal-proxy --config <config.yaml>`
- `llm-universal-proxy --admin-bootstrap`
- `scripts/run_codex_proxy.sh`
- `scripts/run_claude_proxy.sh`

安装器也创建 `llmup` 入口。`llmup` 的行为必须明确：

- `llmup` 无参数时显示极短下一步：先运行 `llmup-config`，再运行 `llmup-codex` 或 `llmup-claude`。
- `llmup --help` 显示用户工具层帮助。
- `llmup --version` 显示版本。
- `llmup config ...` 等价于 `llmup-config ...`。
- `llmup codex ...` 等价于 `llmup-codex ...`。
- `llmup claude ...` 等价于 `llmup-claude ...`。

推荐实现为同一个 Rust 二进制的多入口分发：

- release 里继续包含 `llm-universal-proxy`。
- 安装器额外创建 `llmup`、`llmup-config`、`llmup-codex`、`llmup-claude` 到同一二进制的 symlink/hardlink。
- 程序根据 `argv[0]` 分发，也支持显式子命令：`llm-universal-proxy config|codex|claude ...`。

这样可以避免继续把用户主入口绑在 Python 测试脚本上，也避免安装后还要求用户有 Python 运行环境。

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

## `llmup-config` 设计

第一版命令：

```bash
llmup-config
llmup-config init
llmup-config doctor
llmup-config show
```

`llmup-config` 无参数时等价于 `llmup-config init`，但如果已经配置过，则进入“查看/修改/重新配置”的简单菜单。

`init` 只问最少问题，提示文案面向普通用户：

1. 你的模型服务接口像哪一种？
   - OpenAI 接口，默认推荐。不确定就直接回车。
   - Claude/Anthropic 接口。
   - OpenAI Responses 接口。仅当服务商明确要求时选择。
2. 模型服务地址，例如 MiniMax 中国站 `https://api.minimaxi.com/v1` 或国际站 `https://api.minimax.io/v1`。
3. 模型名，例如 `MiniMax-M2.7-highspeed`。
4. 本地模型短名字，默认 `default`。
5. API Key，以隐藏输入方式读取。

工程实现再把用户选择映射到内部格式：

| 用户看到的选择 | 内部格式 |
| --- | --- |
| OpenAI 接口 | `openai-completion` |
| Claude/Anthropic 接口 | `anthropic` |
| OpenAI Responses 接口 | `openai-responses` |

生成结果：

- `~/.llmup/config.yaml`
- `~/.llmup/secrets.env`
- 必要时创建 `~/.llmup-codex` 和 `~/.llmup-claude`

`secrets.env` 里保存：

```bash
LLM_UNIVERSAL_PROXY_KEY=<random-local-proxy-key>
LLMUP_PROVIDER_DEFAULT_API_KEY=<user-provider-api-key>
```

`config.yaml` 只引用环境变量，不写明文 API Key：

```yaml
data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY

upstreams:
  DEFAULT:
    provider_key:
      env: LLMUP_PROVIDER_DEFAULT_API_KEY
```

`doctor` 检查：

- `~/.llmup/config.yaml` 是否存在且可被 Rust 配置加载器解析。
- `~/.llmup/secrets.env` 是否存在、权限是否安全、是否包含必要变量。
- `codex` / `claude` 是否在 `PATH` 中；缺少用户未选择的客户端只显示提醒，不把整体检查标成失败。
- 代理能否用生成配置启动并通过 `/health`。
- 本机端口冲突时是否可以自动换端口。

`show` 只展示脱敏摘要：

- 配置文件路径。
- 模型短名字。
- 模型服务类型。
- 模型服务地址。
- API Key 是否已配置，永不打印明文。
- 下一步命令：`llmup-codex` 或 `llmup-claude`。

非交互初始化用于测试和高级自动化，必须避免把密钥直接写在命令行参数里：

```bash
printf '%s\n' "$MINIMAX_API_KEY" | llmup-config init \
  --non-interactive \
  --interface openai \
  --model-service-url https://api.minimaxi.com/v1 \
  --model-name MiniMax-M2.7-highspeed \
  --model-alias default \
  --api-key-stdin
```

也支持 `--api-key-env MINIMAX_API_KEY`。不提供 `--api-key <value>`，避免密钥进入 shell history 或进程列表。

## Agent 启动器设计

`llmup-codex` 和 `llmup-claude` 默认采用轻量监督模式：

1. 识别这是不是 agent 会话命令。
2. `llmup-codex --help`、`llmup-codex --version`、`llmup-claude --help`、`llmup-claude --version` 显示 llmup launcher 自己的帮助/版本，不要求本机已安装 Codex/Claude Code。
3. 如果用户需要查看客户端自己的帮助，可以用 `llmup-codex -- --help` 或 `llmup-claude -- --help`。
4. 如果是 `claude update`、`claude install`、`claude auth`、`claude mcp`、`claude doctor`、`codex login`、`codex logout`、`codex login status`、`codex update`、`codex mcp` 等客户端本地管理命令，不启动代理、不注入模型，只设置对应持久配置目录并原样执行。
5. 对 agent 会话命令，读取 `~/.llmup/config.yaml` 和 `~/.llmup/secrets.env`。
6. 自动选择本地端口，生成本次运行使用的完整 runtime YAML。
7. 启动一个本次会话专用的 `llm-universal-proxy` 子进程。
8. 等待 `/health` 成功，并确认子进程仍然存活。
9. 设置客户端需要的 base URL、本地 proxy key、配置目录环境变量。
10. 在用户原始工作目录启动 Codex CLI 或 Claude Code。
11. Codex/Claude Code 退出后，停止本次启动的代理子进程，并返回客户端退出码。

这保持了当前“一条命令启动代理和客户端”的低心智负担，同时不需要引入后台 daemon、pid registry、跨进程生命周期管理或系统服务。

可选高级参数：

| 参数 | 含义 |
| --- | --- |
| `--llmup-home <path>` | 覆盖 `LLMUP_HOME` |
| `--llmup-config <path>` | 使用指定 llmup YAML |
| `--llmup-env-file <path>` | 使用指定密钥 env 文件 |
| `--llmup-proxy-base <url>` | 连接已有代理，不启动本地子进程 |
| `--llmup-port <port>` | 指定本地代理端口 |
| `--llmup-keep-proxy` | 调试用，客户端退出后不停止代理 |

`--llmup-proxy-base` 连接已有代理时，启动器默认仍从 `secrets.env` 读取 `LLM_UNIVERSAL_PROXY_KEY` 作为客户端访问本地/外部代理的 key；这个模式不需要加载真实 provider key，也不启动或停止 proxy 进程。

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
- Codex/Claude 子进程不能继承真实 provider key；它们只拿本地 proxy key。
- 客户端环境从当前 shell 复制，但必须移除 `secrets.env` 中的所有 key、常见 provider secret 变量和包含 `API_KEY`、`AUTH_TOKEN`、`SECRET`、`CREDENTIAL` 的 llmup/provider 相关变量，再显式写入本地 proxy key。
- Claude Code 默认设置 `CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1`，减少工具子进程继承敏感环境变量。
- 因为不改 `HOME`，dangerous/yolo 模式仍可能访问用户真实文件和凭据。检测到危险权限参数时，只在本次命令启动前提示一次风险。

## 参数透传规则

核心规则：`--llmup-*` 是 llmup 自己的参数，其余参数默认原样交给 Codex CLI 或 Claude Code。

示例：

```bash
llmup-codex resume --last
llmup-codex --yolo
llmup-codex --ask-for-approval never --sandbox workspace-write
llmup-codex --llmup-port 19999 -- --yolo

llmup-claude --resume
llmup-claude --permission-mode bypassPermissions
llmup-claude --dangerously-skip-permissions
llmup-claude --llmup-port 19999 -- --resume my-session
```

实现要求：

- 保留用户参数顺序。
- 不吞掉未知参数。
- 不把 `--resume`、`resume`、`--yolo`、`--dangerously-*` 转成 llmup 中间语义。
- 不再使用 `--dangerous-harness` 这类只服务测试脚本的产品参数。
- `--` 后所有参数无条件视为客户端参数。
- `--help` 和 `--version` 在 `--` 前表示 launcher 自己的帮助/版本；在 `--` 后表示客户端参数。
- Codex/Claude 原生 `--model` 或 `-m` 由客户端接收；启动器可以扫描它来决定是否需要注入默认模型，但不能移除或重排。
- 模型扫描必须覆盖 `--model x`、`--model=x`、`-m x`、Codex `-c model=...`、Codex `--config model=...`。
- `--llmup-port=1234` 和 `--llmup-port 1234` 都要支持；缺值要报清楚错误。
- `--` 后即使出现 `--llmup-*`，也必须作为客户端参数透传。
- 如果用户没有提供模型参数，启动器注入配置里的默认模型短名字。
- 非 agent 会话命令不注入模型。

危险权限参数只做一次性提示，不拦截、不默认开启：

- Codex：`--yolo`、`--dangerously-bypass-approvals-and-sandbox`
- Claude Code：`--dangerously-skip-permissions`、`--allow-dangerously-skip-permissions`、`--permission-mode bypassPermissions`

## Codex 注入策略

Codex 官方文档说明 CLI 参数和 `-c key=value` 会覆盖配置文件。`llmup-codex` 应利用这一点，只注入本次运行必需的 provider 设置。没有 Codex subcommand 时，命令形态为：

```bash
codex \
  -c model_provider=\"proxy\" \
  -c model_providers.proxy.name=\"llmup\" \
  -c model_providers.proxy.base_url=\"http://127.0.0.1:<port>/openai/v1\" \
  -c model_providers.proxy.env_key=\"OPENAI_API_KEY\" \
  -c model_providers.proxy.wire_api=\"responses\" \
  -c model_providers.proxy.supports_websockets=false \
  -m <default-model-if-user-did-not-set-one> \
  <user-args...>
```

Codex 有 subcommand 时，不能假设所有 global flags 放在 `codex` 后都能被该 subcommand 接收。实现必须维护一个已知 agent 会话 subcommand 表，并用真实 Codex CLI smoke 测试验证注入位置。已知会话命令包括：

- `codex resume ...`
- `codex exec ...`
- `codex fork ...`

这类命令的目标形态是：

```bash
codex resume \
  -c model_provider=\"proxy\" \
  -c model_providers.proxy.base_url=\"http://127.0.0.1:<port>/openai/v1\" \
  -m <default-model-if-user-did-not-set-one> \
  --last
```

`codex login`、`codex logout`、`codex login status`、`codex update`、`codex mcp` 这类本地管理命令不启动代理、不注入模型。`llmup-codex --help` 和 `llmup-codex --version` 是 launcher 本地帮助；客户端帮助通过 `llmup-codex -- --help` 透传。

同时设置：

```bash
OPENAI_API_KEY="$LLM_UNIVERSAL_PROXY_KEY"
CODEX_HOME="$LLMUP_CODEX_HOME"
```

`OPENAI_BASE_URL` 可以作为兼容性冗余设置，但正确性必须依赖 `-c model_providers.proxy.base_url=...` 注入，而不是依赖环境变量。

Codex model catalog 的生成仍可复用现有 real CLI matrix 里已经验证过的能力，但文件必须放在 llmup 专属运行目录或 Codex 专属目录下，不能覆盖用户自己的 `~/.codex`。

## Claude Code 注入策略

Claude Code 官方文档说明：

- `--resume` / `-r` 支持恢复会话。
- `--permission-mode`、`--settings`、`--setting-sources` 等运行时参数会覆盖配置。
- `CLAUDE_CONFIG_DIR` 会覆盖默认 `~/.claude`，并保存设置、会话历史和插件；Linux/Windows 凭据文件也随目录移动，macOS 的部分登录凭据可能仍由系统 Keychain 管理。

`llmup-claude` 应设置：

```bash
ANTHROPIC_API_KEY="$LLM_UNIVERSAL_PROXY_KEY"
ANTHROPIC_BASE_URL="http://127.0.0.1:<port>/anthropic"
CLAUDE_CONFIG_DIR="$LLMUP_CLAUDE_CONFIG_DIR"
```

默认命令形态：

```bash
claude \
  --model <default-model-if-user-did-not-set-one> \
  <user-args...>
```

不要默认加入 `--bare`。它是 Claude Code 的最小模式，会跳过 hooks、skills、plugins、MCP servers、auto memory、`CLAUDE.md` 自动发现，并跳过 OAuth/keychain reads。需要干净环境时，用户可以自己传 `--bare`；第一版不增加 `--llmup-bare`。

`claude update`、`claude install`、`claude auth`、`claude mcp`、`claude doctor` 这类本地管理命令不启动代理、不注入模型。它们默认仍使用 `CLAUDE_CONFIG_DIR="$LLMUP_CLAUDE_CONFIG_DIR"`，让用户管理的是 llmup 专用 Claude Code 配置目录。`llmup-claude --help` 和 `llmup-claude --version` 是 launcher 本地帮助；客户端帮助通过 `llmup-claude -- --help` 透传。

## 安装器设计

新增 release asset：

- `install.sh`
- 每个平台 archive 继续带 `.sha256`
- archive 内包含 `llm-universal-proxy`，安装后创建 `llmup`、`llmup-config`、`llmup-codex`、`llmup-claude`

安装命令：

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/agentsmith-project/llm-universal-proxy/releases/latest/download/install.sh | sh
```

支持：

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/agentsmith-project/llm-universal-proxy/releases/download/v0.2.32/install.sh | sh
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/agentsmith-project/llm-universal-proxy/releases/latest/download/install.sh | sh -s -- --asset-version 0.2.32
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/agentsmith-project/llm-universal-proxy/releases/latest/download/install.sh | sh -s -- --bin-dir "$HOME/bin"
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/agentsmith-project/llm-universal-proxy/releases/latest/download/install.sh | sh -s -- --no-modify-path
```

版本语义必须写清楚：

- `releases/download/vX.Y.Z/install.sh | sh` 是安装脚本和下载 asset 都固定到 `vX.Y.Z`。
- `releases/latest/download/install.sh | sh -s -- --asset-version X.Y.Z` 是使用 latest 安装脚本去下载指定版本 asset，只 pin 二进制，不 pin 安装脚本本身。
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
- 安装结束打印下一步：先 `llmup-config`，然后按用户使用的客户端选择 `llmup-codex` 或 `llmup-claude`。

暂不把 GPG/cosign 签名作为本计划的交付硬门槛。官方 Claude Code 和主流工具展示了签名 manifest 的更强做法，llmup 可以在 release 签名基础设施稳定后补充；当前计划的硬要求是 TLS 下载加 SHA-256 完整性校验、无 sudo 默认、版本化 installer URL 和可审查安装脚本。

## 实施落点

建议新增 Rust 模块：

- `src/user_tools/mod.rs`
- `src/user_tools/config_wizard.rs`
- `src/user_tools/agent_launcher.rs`
- `src/user_tools/install_metadata.rs`
- `src/user_tools/env_file.rs`

`src/main.rs` 调整为：

- 先根据 `argv[0]` 或第一个子命令判断是否进入用户工具层。
- 如果是旧服务端参数，保持当前 `--config` / `--admin-bootstrap` 行为。
- 用户工具层的 parser 必须保留未知参数，以便传给 Codex/Claude。

配置生成应复用 `src/config.rs` 的 serde 加载和 validate 路径，不要在产品代码里复制 Python harness 的手写 YAML 解析器。

现有 Python 脚本继续作为测试矩阵入口。后续 README 中文主路径改成新命令后，可以把旧脚本标注为开发/测试工具。

release workflow 需要同步调整：

- Unix archive 中仍包含主二进制，安装器负责创建 symlink/hardlink。
- Windows archive 后续可复制多份或提供 `.cmd` launcher；第一版文档不承诺 Windows 原生安装。
- release job 上传 `install.sh`。
- build 或 release gate 增加安装器 smoke：在临时 `HOME` 和临时 `bin-dir` 运行安装脚本，再执行 `llmup --version`、`llmup-config --help`、`llmup-codex --help`、`llmup-claude --help`。
- CI 固定 toolchain/target，产物可校验；bit-for-bit reproducible builds 不作为本计划硬门槛。

## TDD 任务清单

先写失败测试，再实现。

Rust 单元/集成测试：

- `llm-universal-proxy --config proxy.yaml` 旧行为不变。
- `llm-universal-proxy config`、`llmup config`、`llmup codex`、`llmup claude`、`llmup-config`、`llmup-codex`、`llmup-claude` 分发正确。
- `llmup-config init --non-interactive ...` 生成的 YAML 能被 `Config::from_yaml_str` 解析并 validate。
- `llmup-config` 不覆盖已有配置，除非显式 `--force`。
- `secrets.env` 写入权限为 `0600` 或平台等价。
- `show` / `doctor` 不输出明文 API Key。
- env file parser 支持安全的 `KEY=value` 子集，拒绝 shell 展开、命令替换和非法 key。
- `doctor` 对未安装且未选择的客户端只给 warning，不把整体状态打成失败。
- runtime YAML 生成保留完整 `data_auth`，覆盖 `listen`，不修改用户原始配置。

Launcher 测试：

- fake `codex` 接收到 `--resume`、`resume --last`、`--yolo`、`--ask-for-approval never` 等原始参数。
- fake `claude` 接收到 `--resume`、`--permission-mode bypassPermissions`、`--dangerously-skip-permissions` 等原始参数。
- `--llmup-*` 参数不传给客户端。
- `--` 后参数全部传给客户端。
- 用户传 `--model`、`--model=...`、`-m`、Codex `-c model=...`、Codex `--config model=...` 时不重复注入默认模型。
- 用户未传模型时注入默认模型短名字。
- 连续两次运行使用同一个 `CODEX_HOME=~/.llmup-codex`。
- 连续两次运行使用同一个 `CLAUDE_CONFIG_DIR=~/.llmup-claude`。
- 客户端退出码被原样返回。
- SIGINT/SIGTERM 时代理子进程被清理，客户端退出码/信号语义尽量保留。
- `--llmup-proxy-base` 模式不启动本地代理、不停止外部代理。
- `codex resume`、`codex exec`、`codex fork` 的注入位置通过真实 CLI smoke 覆盖。
- launcher 自己的 `--help`、`--version` 不要求客户端存在；`claude update`、`claude install`、`claude auth`、`claude mcp`、`claude doctor`、`codex login`、`codex logout`、`codex login status`、`codex update`、`codex mcp` 不启动代理、不注入模型。
- 客户端子进程不继承真实 provider key，只收到本地 proxy key。
- 父环境中的 `OPENAI_API_KEY`、`ANTHROPIC_API_KEY`、`MINIMAX_API_KEY`、`*_AUTH_TOKEN` 不覆盖 llmup 注入给客户端的本地 proxy key。

安装器测试：

- 在临时 `HOME` 下安装到 `~/.local/bin`。
- OS/arch 映射到正确 asset 名。
- checksum mismatch fail closed。
- `--bin-dir` 和 `--no-modify-path` 生效。
- shell profile marker 幂等。
- 安装后 `llmup --help`、`llmup-config --help`、`llmup-codex --help`、`llmup-claude --help` smoke 通过，且不要求本机已安装 Codex/Claude Code。
- 版本化 installer URL 固定安装脚本和 asset；latest installer 加 `--asset-version` 只固定 asset，并在输出中说明。
- archive path traversal、已有 symlink 覆盖、路径含空格、unsupported OS/arch 都有 fail-closed 覆盖。

文档测试：

- 中文 README 第一屏只保留安装、配置、启动三个动作。
- README 不再要求普通用户 clone repo、手写 YAML 或手动下载 asset。
- README 不出现真实 API Key 示例。

## 验收标准

面向用户：

- 全新 macOS/Linux/WSL 环境里，用户可以通过在线脚本安装 llmup 和三个友好命令。
- 用户运行 `llmup-config` 后，可以不手写 YAML、不 export 环境变量完成 MiniMax 这类 OpenAI-compatible 服务配置。
- 用户运行 `llmup-codex` 后，Codex CLI 通过本地 llmup 使用配置的模型服务。
- 用户运行 `llmup-claude` 后，Claude Code 通过本地 llmup 使用配置的模型服务。
- `llmup-codex resume --last`、`llmup-codex --yolo`、`llmup-claude --resume`、`llmup-claude --dangerously-skip-permissions` 不被 llmup 拒绝或吞参。
- Codex 和 Claude Code 的会话、配置、resume 数据跨进程保留。
- `llmup --help`、`llmup config`、`llmup codex`、`llmup claude` 可用。
- 本地管理命令例如 `llmup-claude update` 或 `llmup-codex mcp` 不会被代理启动和模型注入干扰。

面向工程：

- 旧的服务端入口、YAML 配置格式和测试脚本不被破坏。
- 新入口不依赖 Python。
- 用户工具层只占用 `--llmup-*` 参数命名空间。
- API Key 不出现在日志、doctor、show、错误信息或生成的 README 示例中。
- release archive 和安装器产物可校验，release workflow 覆盖安装器 smoke。

## 风险与处理

| 风险 | 处理 |
| --- | --- |
| Codex/Claude CLI 参数未来变化 | 默认透传未知参数，只解析 `--llmup-*` |
| 临时目录问题再次出现 | 产品 launcher 禁止使用 `TemporaryDirectory` 作为客户端配置目录 |
| 改写 `HOME` 影响用户工具 | 默认不改 `HOME`，只设置 `CODEX_HOME` / `CLAUDE_CONFIG_DIR` |
| provider key 泄露给客户端或工具子进程 | proxy 与客户端分开构造 env，客户端只拿本地 proxy key |
| 用户误用危险权限 | 检测到危险参数时提示一次，但不代替用户决策 |
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

## Team Review 结果

- 产品/用户体验 review：要求把首次向导里的协议术语藏到工程映射里，明确安装后是 Codex/Claude 二选一启动，定义 `llmup` 本体行为，并避免公开第一版不完整的 profile 概念。本文已修订。
- 代码架构 review：要求补齐 runtime YAML、端口重试、`data_auth` 保留、`--llmup-*` 解析、release workflow 和 installer gate。本文已修订。
- 生态/安装器 review：要求收窄 SHA-256 的安全表述、补版本化 installer URL、补 Codex subcommand 注入规则、补 Claude macOS Keychain caveat 和客户端 env 隔离。本文已修订。
