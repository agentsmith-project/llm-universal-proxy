# 计划：`codex-setup` 子代理配置子命令 —— 收敛单一心智模型 + 弃用既有 client-config 工具

状态：**handoff-ready (2-round review: correctness SHIP-READY + scope/handoff NEEDS-FIX items applied)**
更新时间：2026-08-02
来源：2026-08-02 会话——客户提案 "LLMUP Codex Subagent Setup TUI"（13 节 epic）蒸馏；两条调研流（Codex 可行性核实 + 既有工具盘点）已并入下文。KISS / YAGNI / CONVERGENCE。

> 一句话范围：在既有的**单**二进制 `llm-universal-proxy` 上新增一个 `codex-setup` **子命令**（CLI flags + 基础 stdin 提示，**非**独立 TUI 二进制），取代 `llmup-config` / `llmup-codex` / `llmup-claude` 三个 argv[0] 别名（"一个心智模型"），让用户为 Codex **V1 混合子代理**（官方主代理 + llmup 子代理）一键生成 `~/.codex/` 下的 provider/agent/profile 配置；**不做** catalog-derivation、**不**生成 `fork_turns`、**不**碰 Claude Code 自动配置。本计划同时是既有 client-config 工具的弃用清单（含 file:line 编辑点）。

### 设计决策：subcommand，不做独立 TUI 二进制（也非 MVP 形态的 TUI）

- **单二进制交付**：`Cargo.toml:2` 仅声明一个 `llm-universal-proxy`（无 `[[bin]]`、无 `default-run`）。新增独立二进制违背交付模型；复用既有 argv[0]/子命令分派（`src/main.rs`）零成本。
- **KISS + 可脚本化 + 无头友好**：MVP 默认**非交互**——`codex-setup --base-url ... --model ... --provider-key ...` 一行生成全部文件，CI/批量部署直接用；缺省字段退化为**基础 stdin 提示**（如未给 `--model` 则拉 `/models` 列表让用户选序号）。**不**依赖 ratatui/crossterm。
- **一个心智模型**：用户唯一要记的是 `llmup codex-setup`（友好别名，见 §6.3）或等价的 `llm-universal-proxy codex-setup`。
- **ratatui/crossterm 留在 `Cargo.toml:34-35`，但 MVP 不依赖**；完整 TUI dashboard 体验是 **Phase 2**（§4.2）。本计划文件名保留历史 `...-tui-plan.md`，实现形态以本文为准（CLI-first）。

## 0. 策略与目标

- **目标产物**：`llm-universal-proxy` 上的 `codex-setup` 子命令（CLI flags + 基础 stdin 提示），输出 Codex V1 混合拓扑所需的全部文件：`~/.codex/agents/llmup-<model>.toml`（自定义子代理）、`~/.codex/llmup.config.toml`（profile）、`~/.codex/llmup/state.json`（托管清单，**不含** API key）。Claude Code 自动配置**整体砍掉**。
- **V1 是默认且唯一受支持的混合拓扑**（与 `2026-08-02-max-compat-hardening-plan.md` §0 的 V1-first 立场一致）。V2（`multi_agent_v2`）被 Fernet 服务端密钥根本性阻断，不投入。
- **KISS 取舍**：V1 是 Codex 默认（见 §1），因此 `codex-setup` **不**做 catalog-derivation（脆弱且 YAGNI）；只生成 profile + 自定义 agent 文件，并防御性地确保 profile 不开 V2。
- **弃用**：`llmup-config`（`config_wizard.rs`）、`llmup-codex` launcher（`agent_launcher.rs` 的 Codex 臂）、`llmup-claude`（Claude 臂）三个 argv[0] 别名全部移除；`env_file.rs` 仅被 `agent_launcher.rs:19` 与 `config_wizard.rs:10` 引用（二者均删 → 孤儿死码），随弃用**一并删除**（新 `codex-setup` 生成 Codex 配置，不读 llmup 自有的 `secrets.env`）。**保留** `agent_model_profile.rs`（server `/models` 经 `src/server/models.rs:317` → `build_codex_model_catalog_for_config` 依赖）、server `/models` 端点。

## 1. 已核实的 Codex 可行性（源码 + 文档，引用到行）

> 下列结论已对照 `reference/codex/` 源码核实。仅记录对决策有载荷的事实。

| 事实 | 引用 | 对本计划的含义 |
|---|---|---|
| `model_catalog_json` 设定后**替换（非合并）**内置 catalog | `reference/codex/codex-rs/core/src/config/mod.rs:3180,3878`（`load_model_catalog` 整体替换内置 catalog；`:964-965` 为字段文档原文） | 任何派生 catalog 必须包含**全部**模型——这是 catalog-derivation 脆弱性的根因之一 |
| `codex debug models --bundled` 输出 `{models:[ModelInfo]}` JSON | `reference/codex/codex-rs/cli/tests/debug_models.rs:13-16`（`args(["debug","models","--bundled"])`） | 可作为"派生 catalog"的输入源——但本计划**不**用它（见 KEY DECISION #1） |
| `--profile <name>` 加载 `~/.codex/<name>.config.toml` 叠加 config.toml | `reference/codex/codex-rs/cli/src/lib.rs:64,120`（"Layer $CODEX_HOME/<name>.config.toml on top of the base user config"）；`reference/codex/codex-rs/core/src/config/mod.rs:3264` | profile 是默认安装模式的基础；可覆盖 model/model_provider/model_catalog_json |
| 自定义 agent 文件 flatten `ConfigToml`，`model_provider` 覆盖父级 | `reference/codex/codex-rs/core/src/agent/role.rs:108`（`let preserve_current_provider = role_layer_toml.get("model_provider").is_none();`）；`:176-178`（`model`/`model_reasoning_effort` 同理） | 生成的 `agents/llmup-<model>.toml` 设 `model_provider = "llmup"` 即可让子代理走 llmup；`developer_instructions`/`model_reasoning_effort`/`model_context_window` 均可写入 |
| `[model_providers.X.auth] command`（命令产出 Bearer token） | `reference/codex/codex-rs/model-provider-info/src/lib.rs:192-193`（`auth.command` 非空校验）、`:426`（`has_command_auth`） | 子命令的 provider 鉴权可选 `env_key` 或 `auth.command`（命令式 token） |
| `request_max_retries` / `stream_max_retries` / `stream_idle_timeout_ms` 是 provider 字段 | `reference/codex/codex-rs/model-provider-info/src/lib.rs:123,125,128` | 生成的 `[model_providers.llmup]` 可按需写入 |
| `fork_turns` 是 **V2 spawn-tool 调用参数**，**非** config 键 | `reference/codex/codex-rs/core/src/tools/handlers/multi_agents_spec.rs:643,647`（出现在 spawn-agent 工具 schema）；`multi_agents_v2/spawn.rs:191` | **不要**在 agent/profile 文件里写 `fork_turns`（见 CORRECTION） |
| `fork_context: bool` 是 V1 spawn 工具参数（缺省=false 即不分叉） | `reference/codex/codex-rs/core/src/tools/handlers/multi_agents/spawn.rs:236`；`multi_agents_spec_tests.rs:128,157` | V1 不分叉是默认；生成文件同样**不**写 `fork_context` |
| `MultiAgentV2` 是运行时 `Feature` 标志，**默认关闭**（即 V1 默认） | `reference/codex/codex-rs/features/src/lib.rs:158`（枚举）、`:1097`（spec 注册）；`reference/codex/codex-rs/core/src/config/edit.rs:882`（`!spec.default_enabled`）；测试均显式 `features.enable(Feature::MultiAgentV2)`（如 `control_tests.rs:650,756,1248`） | **V1 是默认**——这是 KEY DECISION #1 的基石；profile 防御性地显式 `multi_agent_v2 = false` 即可 |
| AGENTS.md：Codex 读它（全局 `~/.codex/AGENTS.md` + 项目级） | Codex 将其原样读入 | 托管块（BEGIN/END 标记）是可行的工具约定——但本计划 Phase 2 才做 |
| 压缩对 llmup 子代理安全（本地压缩，不命中 `/responses/compact`） | `reference/codex/codex-rs/core/src/compact.rs:108-110,241-394`；`reference/codex/codex-rs/model-provider-info/src/lib.rs:422-424`（仅 OpenAI/Azure 命中远程压缩） | 与 max-compat 计划 §8 一致；子命令无需为压缩做任何特殊处理 |
| ratatui + crossterm 已在依赖中 | `Cargo.toml:34-35` | MVP **不**依赖；Phase 2 的 TUI dashboard 直接可用，无需加依赖 |

## 2. 关键决策（KEY DECISIONS，编码进实现）

1. **V1 是默认——不做 catalog-derivation。** `codex-setup` **不**通过 `codex debug models --bundled` + `model_catalog_json` 打 `multi_agent_version` 补丁。它确保 V1 的方式是：在生成的 profile 里**不开** `multi_agent_v2`（V1 在 V2 feature 关闭时即默认，见 §1）。catalog-derivation 机制**显式 OUT OF SCOPE**——脆弱（catalog 替换非合并 + feature-gated + 未公开接口 + V1 既是默认则 YAGNI）。**限制声明**：若用户环境强制开启 V2，子命令无法可靠覆盖（在 doctor 中告警，不尝试修复）。**V1-默认防御是非对称的**：profile 的 `[features] multi_agent_v2 = false` 仅在 `agents_enabled=true`（默认）且 `Feature::Collab` 开启（默认）时才得到 V1；若用户 base config 设 `[agents] enabled=false` 或 `multi_agent=false`，profile 得到的是 `Disabled` 而非 V1。故 `--doctor` 还需对 `Feature::Collab`/`agents_enabled` 被关闭的情形告警。
2. **生成的配置不含 `fork_turns`。** V1 不分叉是默认（`fork_context` 缺省 false）。agent/profile 文件**不**写 `fork_turns`（它不是 config 键），也不写 `fork_context`。
3. **Profile 模式是默认安装方式**（`~/.codex/llmup.config.toml`，经 `--profile llmup` 或 `codex exec --profile llmup` 选中）。全局模式（改写 `~/.codex/config.toml`）为**高级/可选**，带 diff 预览与二次确认。
4. **弃用既有工具**（详见 §6）：`llmup-config`（`src/user_tools/config_wizard.rs`）、`llmup-codex` launcher（`src/user_tools/agent_launcher.rs` Codex 臂）、`llmup-claude`（Claude 臂）。`env_file.rs` 仅被这两个待删文件引用（`agent_launcher.rs:19`、`config_wizard.rs:10`），随弃用**一并删除**。**保留** `agent_model_profile.rs`（server `/models` 依赖，`src/server/models.rs:317`）。新的 `codex-setup` 子命令是唯一入口。
5. **复用**（不重造）：`agent_model_profile.rs`（catalog 构建器，`AgentModelProfile::from_config` / `build_codex_model_catalog_for_config`）、server `/models` 端点（Codex-UA catalog 发现，`src/server/models.rs:316-318`）、`config.rs`（Config / aliases / surface / dialect）、`write_runtime_config_for_port` + `ProxyProcess` 模式（`src/user_tools/agent_launcher.rs:540-575,1178-1234`，**仅当**子命令选择托管代理生命周期时——可选）。

## 3. 对客户方案的修正（明确写出）

- **`fork_turns = "none"` 不是 config 键**（它是 V2 spawn 工具的调用参数，`multi_agents_spec.rs:643,647`）。V1 不分叉是默认 → 生成文件**不写**它。
- **`model_catalog_json` 替换而非合并**（`config/mod.rs:3180,3878` `load_model_catalog`）→ 派生 catalog 须含全部模型。**但本计划不做 catalog-derivation**，故该限制只作为"为何不做"的论据。
- **`multi_agent_version` 是 feature-gated** → 即便写入 catalog 也只在 V2 feature 开启时生效，**不可靠** → 丢弃。
- **"catalog 版本适配器 + Codex 升级陈旧检测"是 YAGNI**（V1 是默认，没有需要适配的 V2）。
- **（本会话追加）"独立 TUI 二进制 / ratatui 向导"非 MVP 形态** → 单二进制 + `codex-setup` CLI 子命令；TUI dashboard 置 Phase 2（见顶部"设计决策"）。

## 4. 范围划分

### 4.1 MVP（Phase 0/1——本质，CLI 形态）

**命令形态（非交互优先，退化为 stdin 提示）：**

```
llm-universal-proxy codex-setup \
  --base-url <url> \
  --model <alias|id> \
  --provider-key <key|env-name>          # 二选一
  [--provider-key-command "<cmd args>"]  # 命令式 Bearer token，二选一
  [--profile-name llmup] [--agent-name llmup-<model>]
  [--reasoning-effort medium] [--context-window N]
  [--wire-api responses] [--max-retries N] [--stream-idle-timeout-ms N]
  [--status | --doctor | --restore | --uninstall]
```

- 默认即**非交互**（所有必需字段由 flags 提供）→ 可脚本化、无头友好。
- 缺省字段退化为**基础 stdin 提示**（line-oriented，镜像既有 `config_wizard.rs:484-527` 的 `prompt_*` 风格）：未给 `--model` 则拉 `/models` 列表让用户选序号；未给 `--provider-key*` 则提示输入（不回显可后置）。**前置条件**：交互式 `/models` 发现需要 llmup server 正在运行；`--model` flag 在非交互模式下短路该依赖（无需 server）。
- **不**使用 ratatui/crossterm；纯 stdout/stdin。

**流程：**
环境探测（Codex CLI 版本、`CODEX_HOME`、鉴权状态）→ 配置 llmup provider（`base_url` + 鉴权用 `env_key` 或 `auth.command`）→ 发现模型（以 Codex UA 调 llmup `/models` → `{models:[ModelInfo]}`，复用 `src/server/models.rs:316-318`；`--model` 已给则跳过交互选择）→ 生成自定义 agent（`~/.codex/agents/llmup-<model>.toml`：`model_provider="llmup"`、`developer_instructions`、`model_reasoning_effort`、`model_context_window`，**无** `fork_turns`/`fork_context`）→ 生成 profile（`~/.codex/llmup.config.toml`：`[model_providers.llmup]` + 防御性 `[features] multi_agent_v2 = false`）→ 连接测试（命中 `/models`，发一次基础 `/responses`）→ 安全写入 → 打印用法提示（`codex exec --profile llmup`）。

**安全：**
- 写前备份；原子写（temp + rename，复用 `config_wizard.rs:1658-1748` `write_config_file_atomic` 的权限保留/0o600 模式，**该 helper 随弃用迁出后保留**）。
- restore：移除 llmup 托管文件/块。
- 状态文件 `~/.codex/llmup/state.json`：记录托管文件路径 + 写入哈希。**绝不**存 API key。

**命令面（单子命令 + action flags）：**
- `llmup codex-setup`（= `llm-universal-proxy codex-setup`；默认 = 生成配置）
- `llmup codex-setup --status`
- `llmup codex-setup --doctor`
- `llmup codex-setup --restore`
- `llmup codex-setup --uninstall`

**弃用执行：** 见 §6（独立提交/阶段）。

### 4.2 Phase 2（记录但不建）

- **ratatui/crossterm 交互式 dashboard/向导**（模型多选、provider 切换、实时连接测试可视化）——MVP 的 CLI 体验升级形态。
- 可选 catalog-derivation（经 `model_catalog_json` 强制 V1）——针对环境强制开 V2 的用户；**默认不提供**。
- AGENTS.md 托管块路由规则（BEGIN/END 标记）。
- Codex 升级检测（catalog 陈旧检查）。
- `/models` 的 `x_llmup` 能力元数据（服务端；多数字段已由 `llmup` 扩展透出，见 `src/server/models.rs:342-343,383`；rename + streaming/subagent_protocols 需新 config）。
- 真实 Codex E2E 子代理测试（消耗官方父代理配额）。
- 批量/多 profile 脚本编排（MVP 已是非交互一行命令；Phase 2 做多 profile 模板/批处理）。

### 4.3 Phase 3+（长期——显式 OUT OF SCOPE）

- Windows、keychain、multi-provider、marketplace、auto-negotiation、latency stats。

## 5. 架构（brief）

- 新模块：`src/user_tools/codex_setup.rs`（**纯 CLI**：arg 解析 + stdin 提示 + 文件生成；**非** TUI）。子命令分发挂在 `src/main.rs`（见 §6.3，复用既有 argv 分派风格，新增 `codex-setup` 子命令识别）。Phase 2 的 TUI 再加 `src/user_tools/codex_setup_tui.rs`（或 `src/tui/`）。
- 复用：`agent_model_profile` + `/models` + `config`。
- 写入范围：`~/.codex/agents/`、`~/.codex/llmup.config.toml`（profile）、`~/.codex/llmup/state.json`。
- **不**改 `~/.codex/config.toml`（全局模式为 opt-in 高级，带 diff 预览）。
- **不**碰 `~/.codex/auth.json`（官方鉴权保持原样）。
- 可选：若子命令托管代理生命周期，复用 `write_runtime_config_for_port` + `ProxyProcess`（`agent_launcher.rs:540-575,1178-1234`）；MVP 也可只让用户自行跑 server，子命令仅做配置生成 + 连接测试。实现期定。

## 6. 弃用既有工具（编辑点已定位到行）

> 治理门 `scripts/check-governance.sh` 把工具名硬编码为 README/docs/install.sh/release.yml 的**必需内容**（见 §6.2），故移除名字时**必须**同步更新这些断言，否则门禁失败。建议作为**独立提交/阶段**。

### 6.1 源码与入口（src/）

| 目的 | 文件:行 | 动作 |
|---|---|---|
| argv[0] 分派总入口 | `src/main.rs:85-128`（L86 `LLMUP_FORCE_SERVER` 旁路；L93-101 Config；L102-109 Codex 臂；L110-117 Claude 臂） | 移除 Config/Codex/Claude 三臂；新增 `codex-setup` 子命令分派（见 §6.3） |
| server 模式 arg 解析（未知 arg 即拒绝） | `src/main.rs:130-140`（`parse_args` L12-53，未知 token 报 `"unknown argument"` L36） | 新增 `codex-setup` 子命令识别点（在 server arg 解析前 peek argv[1]） |
| `user_tools` 模块导出 | `src/lib.rs:25`（`pub mod user_tools;`） | 保留（承载新子命令 + 保留模块） |
| 入口枚举 + argv[0] 匹配 | `src/user_tools/mod.rs:6,8-9,12-16,18-28` | 移除 `agent_launcher`/`config_wizard`/`env_file`（L9）模块声明与 `UserToolEntrypoint`/`entrypoint_from_argv0`；**保留** `agent_model_profile`；新增 `codex_setup` 模块声明 |
| 弃用：config 向导 | `src/user_tools/config_wizard.rs`（全文 1967 行） | **删除**；但先把 `write_config_file_atomic`（L1658-1748）、`write_secret_file`/`safe_secret_permissions` 等通用安全写 helper 迁出到独立模块（如 `src/user_tools/safe_write.rs`）供新子命令复用 |
| 弃用：launcher（Codex + Claude 臂） | `src/user_tools/agent_launcher.rs`（全文 1264 行） | **删除整文件**；如新子命令需 `write_runtime_config_for_port`/`ProxyProcess`（L540-575,1178-1234），迁出后再删 |
| 弃用：env_file 解析 | `src/user_tools/env_file.rs`（全文 127 行） | **删除整文件**；孤儿死码——仅被 `agent_launcher.rs:19` 与 `config_wizard.rs:10` 引用（二者均删）。server 与新 `codex-setup` 均不依赖（子命令生成 Codex 配置，不读 llmup 自有的 `secrets.env`） |

### 6.2 安装器 / CI / 治理 / 文档（repo 根）

| 目的 | 文件:行 | 动作 |
|---|---|---|
| install.sh 别名创建 | `install.sh:33-34`（头注释）、`:265`（冲突检查循环）、`:275,290-298`（别名 symlink/hardlink + `mv`）、`:305`（manifest `aliases=...`）、`:335-337`（"next steps" 提示） | 别名表改为只装主二进制 + 单个新友好别名 `llmup`（指向同一二进制，用于 `llmup codex-setup`）；移除三个旧别名 |
| release.yml 安装器冒烟 | `.github/workflows/release.yml:210-215`（`llmup-config --help/--version`、`llmup-codex --llmup-help/--llmup-version`、`llmup-claude ...`） | 改为新子命令冒烟（如 `llmup codex-setup --doctor`、`llmup codex-setup --help`） |
| 治理断言（必需内容） | `scripts/check-governance.sh:129`（README ×2 含工具名）、`:154-163`（`docs/clients.md` 必含三工具名 + 两条 `--llmup-no-proxy --` 文案）、`:177-179`（`docs/configuration.md` "Ordinary user path:"）、`:775-777`（install.sh 必含三工具名）、`:798-803`（release.yml 必含 6 条冒烟行） | **同步更新**断言为新命令/新名（`codex-setup`、`llmup` 别名）；否则门禁红 |
| README | `README.md:25,31,37`（三命令 quickstart）、`:40,44,48,50`（L48 标题 "Why There Is No `llmup` Command" + L50 正文——本计划**引入** `llmup` 作为唯一友好别名，标题与正文均需改写） | 改写为单一 `llmup codex-setup`；`README_CN.md` 同步（同行号镜像） |
| 客户端文档 | `docs/clients.md:9,15,17,20,33,61,65,79,81` | **近乎全文重写**（通篇 launcher-centric，非行级 edit）：重写为 Codex V1 混合拓扑 + `codex-setup` 流程；Claude Code 自动配置段落移除 |
| 高级用法 / 配置 / 项目地图 | `docs/advanced-usage.md:3,68`；`docs/configuration.md:3`；`docs/PROJECT.md:49-50,143,145,148,198-199` | 更新命令引用与文件树/测试清单 |

### 6.3 `codex-setup` 子命令分派（KISS，复用既有模式）

- 既有分派是**纯 argv[0]**（无 clap/`Subcommand`，grep 核实）；二进制单名 `llm-universal-proxy`（`Cargo.toml:2`，无 `[[bin]]`）。
- 新增：在 `src/main.rs` 进入 server arg 解析（`parse_args`）**之前** peek `argv[1]`：若为 `codex-setup` → 路由进 `user_tools::codex_setup::run_cli(argv[2..])`。该 peek 置于 `LLMUP_FORCE_SERVER` 守卫**之外**（始终执行——`codex-setup` 是用户工具而非 server 模式，不应被强制 server 旁路影响）；`LLMUP_FORCE_SERVER=1` 旁路与 server 路径保持不变。
- 调用形态：`llm-universal-proxy codex-setup ...`。install.sh 额外装一个 `llmup` 友好别名（`ln -s llm-universal-proxy`），让 `llmup codex-setup` 字面可用——非必需（`llm-universal-proxy codex-setup` 已等价），但满足"一个心智模型"。
- 解析风格：沿用既有手写 flag 解析（镜像 `config_wizard.rs:1254-1381` `parse_add_model_args` 的 inline/`--flag value` 双形态），不引入 clap（保持单二进制 + 零新依赖）。

## 7. 测试（TDD，核心先行）

> 现有测试盘点（决定改/删/留）：
> - **删/重写**：`tests/user_tools_launcher.rs`（launcher 全量）、`tests/user_tools_config.rs`（config_wizard 全量）、`tests/user_tools_entrypoints.rs`（argv[0] 三别名 E2E，含 L452-457/668-671/784-787/954-957/1049-1050/1240-1243/1370-1371/1506-1509 的 symlink 构造）。**删除量级**：3 个文件合计 **5,280 行**（1,617 + 2,120 + 1,543），评审/排期勿低估。
> - **保留/保持绿**：`tests/user_tools_agent_model_profile.rs`（catalog/profile，被 server `/models` 与新子命令共用）。

新增（TDD 红→绿）：

1. **flag 解析**：`--base-url/--model/--provider-key`（含 inline `--flag=value` 与 env-name 形态）、`--provider-key-command`、action flags `--status/--doctor/--restore/--uninstall` 互斥；必需字段缺失时报错清晰。
2. **Provider TOML 生成**：`[model_providers.llmup]` 含 `base_url`、`wire_api="responses"`、`env_key` **或** `[auth] command`（参见 §1 引用）；可选 `request_max_retries`/`stream_max_retries`/`stream_idle_timeout_ms`。
3. **Agent TOML 生成**：`~/.codex/agents/llmup-<model>.toml` 含 `model_provider="llmup"`、`developer_instructions`、`model_reasoning_effort`、`model_context_window`；**断言不含** `fork_turns`、`fork_context`。
4. **Profile TOML 生成**：`~/.codex/llmup.config.toml` 含 `[features] multi_agent_v2 = false`（防御）；**断言**为合法 TOML、可被 mock ConfigToml 解析器加载。
5. **模型发现**：以 Codex UA 解析 `/models` 响应 → 正确模型列表（复用 `agent_model_profile`/`server/models.rs` 的 catalog 形状）。
6. **安全写入**：备份 + 原子写 + restore（删除托管文件后 state 与磁盘一致）；复用迁出后的 `write_config_file_atomic`。
7. **状态文件**：`~/.codex/llmup/state.json` 记录托管文件 + 哈希；**断言**不含任何 API key 字样。
8. **端到端（mock）**：`codex-setup --base-url ... --model ... --provider-key ...` 生成配置 → mock Codex config 解析器加载成功（覆盖 agent + profile）。

## 8. 验证计划

1. 单元/契约：§7 全绿。
2. 门控全绿：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`、`python3 -m unittest discover -s tests -p 'test*.py'`、`bash scripts/check-governance.sh`（**注意**：治理断言随 §6.2 同步更新）。
3. **回归守卫**：`tests/user_tools_agent_model_profile.rs` 与 server `/models`（Codex UA catalog）行为零变化。
4. 弃用核对：`grep -rn` 确认 `llmup-config`/`llmup-codex`/`llmup-claude`/`UserToolEntrypoint`/`AgentKind`/`env_file`(模块) 在 `src/`、`install.sh`、`.github/`、`docs/`（除历史 engineering 归档）零残留（`CHANGELOG.md` 历史发布条目合法提及上述名字，同样豁免）。
5. 本轮范围**不**发新治理/证据/报告 md（本计划除外）。

## 9. 风险

- **Codex 配置 schema 跨版本变化**（agent TOML 字段、profile 机制）。缓解：子命令按**探测到的** Codex 版本生成；doctor 对未测试版本告警。
- **V1-默认假设可能失效**（未来 Codex 把默认翻成 V2）。缓解：profile 显式 `[features] multi_agent_v2 = false`（防御性，KEY DECISION #1）。
- **弃用爆炸半径**：5 个 Rust 文件（`config_wizard.rs`、`agent_launcher.rs`、`env_file.rs` 删；`mod.rs`、`main.rs` 改）+ 3 个测试文件删/重写（合计 **5,280 行**：`user_tools_launcher.rs` 1,617 + `user_tools_config.rs` 2,120 + `user_tools_entrypoints.rs` 1,543）+ `install.sh` + `release.yml` + `check-governance.sh`（5 段断言）+ README×2 + `docs/{clients,advanced-usage,configuration,PROJECT}.md`。**作为独立提交/阶段**，先删源码+测试 → 再改安装/CI/治理 → 再改文档。
- **CLI flag 面膨胀**：MVP flag 已较多（§4.1）。缓解：绝大多数可选字段给合理默认（`profile-name=llmup`、`wire-api=responses`、`reasoning-effort=medium`），仅 `--base-url/--model/--provider-key*` 必填；doctor 能从现有 state 反推。
- **TOML 保留注释**：MVP 整文件生成不保留用户注释；state.json + 托管块边界（Phase 2 AGENTS.md 同款）作为后续提升。`~/.codex/llmup.config.toml` 是子命令全权拥有的文件（profile 模式默认），故无注释保留问题；仅全局模式（opt-in）改 `config.toml` 时才有此风险，靠 diff 预览兜底。

## 10. 非目标（显式 OUT OF SCOPE）

- **不做 catalog-derivation**（`codex debug models --bundled` + `model_catalog_json` 打 `multi_agent_version` 补丁）——脆弱 + YAGNI。
- **不在生成配置里写 `fork_turns` 或 `fork_context`**（前者非 config 键；后者默认 false 即不分叉）。
- **不做 V2 混合支持**（Fernet 阻断，与 max-compat 计划一致）。
- **不做 Claude Code 自动配置**（整体砍掉；`llmup-claude` 别名删除）。
- **不碰 `~/.codex/config.toml`（默认）、`~/.codex/auth.json`**。
- **MVP 不做 ratatui/crossterm 仪表盘**（CLI 子命令足矣；TUI 是 Phase 2）。
- **不引入 clap**（保持单二进制 + 零新依赖；手写 flag 解析）。
- **不做** Phase 2/3 条目（§4.2/§4.3）：ratatui dashboard、AGENTS.md 托管块、Codex 升级检测、`x_llmup` 元数据重命名、真实 E2E、批量多 profile、Windows/keychain/multi-provider/marketplace。

## 验收标准

> 以下全部满足即达"Phase 1 MVP 完成"。开发团队按此核对。状态已转 `handoff-ready`（2 轮评审：correctness SHIP-READY + scope/handoff NEEDS-FIX 已应用）；实现期逐项勾选。

- [ ] **单一子命令**：`llmup codex-setup`（`llm-universal-proxy codex-setup`）默认非交互跑通：给定 `--base-url/--model/--provider-key*` 一行生成 `agents/llmup-<model>.toml` + `llmup.config.toml` + `llmup/state.json`；缺省字段退为基础 stdin 提示。
- [ ] **V1 默认、无 fork_turns**：生成文件**不含** `fork_turns`/`fork_context`；profile 含 `[features] multi_agent_v2 = false`；**不**做 catalog-derivation。
- [ ] **可脚本化/无头**：全部必需字段经 flags 提供，零交互；CI 可直接调用（Phase 2 的批量编排除外）。
- [ ] **不依赖 ratatui**：MVP 编译/运行不触发 ratatui/crossterm 代码路径（依赖留在 `Cargo.toml`，Phase 2 才用）。
- [ ] **安全写入**：写前备份 + 原子写 + restore/uninstall 可清回干净状态；`state.json` 经断言**不含** API key。
- [ ] **复用而非重造**：`agent_model_profile.rs`（server `/models` 经 `src/server/models.rs:317` 依赖）、server `/models`（Codex UA catalog）保持原状且测试绿。
- [ ] **弃用完成**：`llmup-config`/`llmup-codex`/`llmup-claude`（及孤儿 `env_file.rs`，随 `agent_launcher.rs`/`config_wizard.rs` 一并删除）在 `src/`、`install.sh`、`.github/`、`docs/`（非历史归档）零残留（`CHANGELOG.md` 历史发布条目豁免）；旧 launcher/wizard/entrypoint 测试已删；`tests/user_tools_agent_model_profile.rs` 保持绿。
- [ ] **治理门同步**：`scripts/check-governance.sh` 断言更新为新命令/新名（`codex-setup`、`llmup` 别名）；`bash scripts/check-governance.sh` 绿。
- [ ] **门禁全绿**：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`、`python3 -m unittest discover -s tests -p 'test*.py'`。
- [ ] **CHANGELOG**：新增条目记录 (a) `codex-setup` 子命令引入（V1 混合子代理配置，CLI-first）与 (b) `llmup-config`/`llmup-codex`/`llmup-claude` 弃用（含 install/CI/治理/文档同步）。
