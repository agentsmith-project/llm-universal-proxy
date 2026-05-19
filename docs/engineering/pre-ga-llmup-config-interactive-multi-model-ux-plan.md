# pre-GA llmup-config 交互式多模型配置改进计划

更新时间：2026-05-19

## 背景

当前 `llmup-config` 已经具备无参数进入交互式配置、`doctor` 检查、`set-limits` 修改模型上下文信息等基础能力，但产品心智仍有几个会卡住普通用户的问题：

- 第一次运行的交互流程仍偏“填字段”，不像一个清晰的设置向导。
- 已有配置时只支持“保留 / 重配 / 检查”，不支持继续添加模型服务和模型别名。
- 默认模型别名叫 `default`，容易和 Claude Code 自己的 Default 模型含义冲突。Claude Code 官方文档明确说明 `default` 会解析到用户账号或部署形态对应的系统默认模型，因此 llmup 不应该把自己的默认 alias 也命名为 `default`。
- 底层 `Config` 已经支持多个 `upstreams` 和多个 `model_aliases`，但 `llmup-config` 没有把这个能力用普通用户能理解的方式暴露出来。
- `llmup-codex` 当前只为选中的单个 alias 生成 Codex model catalog；`llmup-claude` 当前也只投射一个 custom model option。用户配置多个 alias 后，agent 自己的模型选择体验还没有闭环。

这份计划只改进用户工具层和相关文档，不改变代理服务端配置语义、协议转换矩阵、Admin API 或 provider 调用逻辑。

## 官方依据

- Codex 官方配置参考：<https://developers.openai.com/codex/config-reference/>
  - `model_catalog_json` 是启动时加载模型 catalog 的官方配置项。
  - `model_provider` / `model_providers.<id>` 是自定义 provider 的官方配置项。
  - 当前官方文档中 `model_providers.<id>.wire_api` 只支持 `responses`，因此 `llmup-codex` 继续以本地 proxy 的 OpenAI Responses 入口作为 Codex 上游。
- Claude Code 官方模型配置：<https://code.claude.com/docs/en/model-config>
  - `/model`、`--model`、`ANTHROPIC_MODEL`、settings `model` 都是官方模型选择入口。
  - `default` 是 Claude Code 的特殊模型设置，表示按账号或部署形态解析系统默认模型。
  - `ANTHROPIC_DEFAULT_OPUS_MODEL`、`ANTHROPIC_DEFAULT_SONNET_MODEL`、`ANTHROPIC_DEFAULT_HAIKU_MODEL` 可控制 `opus`、`sonnet`、`haiku` 三个家族 alias。
  - `CLAUDE_CODE_SUBAGENT_MODEL` 可控制 subagent 模型。
- Claude Code 官方 LLM gateway 文档：<https://code.claude.com/docs/en/llm-gateway>
  - 当 `ANTHROPIC_BASE_URL` 指向 Anthropic Messages gateway 时，`CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1` 可以让 Claude Code 查询 gateway 的 `/v1/models` 并把模型加入 `/model` picker。
  - discovery 默认关闭，且只会加入 ID 以 `claude` 或 `anthropic` 开头的模型。llmup 不能把它当成任意 alias 都可见的唯一方案。

## 目标

第一目标是降低普通用户心智负担：用户运行 `llmup-config` 后，不需要知道 YAML、upstream、model_aliases、provider_key 这些内部词，也能完成“添加模型服务、给模型起名字、让 Codex/Claude Code 能看到模型”的闭环。

具体目标：

- `llmup-config` 无参数永远是默认入口，进入清晰的交互式配置界面。
- 新配置默认 alias 改为 `main`，不再生成 `default` alias。
- 新配置默认 upstream 名也使用 `main`，不再生成 `DEFAULT`。
- 支持在交互式界面里添加多个模型服务，给每个模型服务下的具体 provider model 命名多个 alias。
- 支持继续使用现有 `set-limits` 能力配置上下文长度和最大输出长度，但在普通向导中只作为“可选优化”，不是首次配置必填项。
- `llmup-codex` 生成的 Codex model catalog 包含配置中的所有 alias，并仍用 `--llmup-model <alias>` 决定启动默认模型。
- `llmup-claude` 至少稳定支持 `main` 作为启动模型，并对 `haiku`、`sonnet`、`opus` 这三个官方家族 alias 做简单映射。任意多 alias 进入 Claude `/model` picker 只通过官方 gateway discovery 或后续验证后的方案处理，不发明自己的 picker。
- README 主路径更新为新的低心智负担用法；高级配置放到独立文档。

## 非目标

- 不新增 GUI、TUI 全屏界面、后台 daemon 或系统服务。
- 不自动安装 Codex CLI 或 Claude Code。缺少客户端时只给出清晰提示。
- 不新增独立 `llmup` 主命令或 agent 管理壳。
- 不做 provider 市场、价格表、智能推荐或自动测速。
- 不从模型名猜测上下文长度、最大输出、thinking/reasoning 能力或工具能力。
- 不让普通向导暴露所有 YAML 字段。
- 不新增第二套配置格式；所有改动都写回现有 `config.yaml` 的 `upstreams`、`model_aliases`、`limits`、`surface_defaults` / `surface`。
- 不再把 `OpenAI-compatible` 作为用户可选协议名。必须明确写成 `openai-chat-completions` 或 `openai-responses`。
- 不为 Codex/Claude 的原生参数设计统一抽象。`llmup-codex` / `llmup-claude` 仍然只透传原生参数。

## 设计原则

- KISS：一个普通用户主路径，一个高级逃生口，不设计多套等价方式。
- 复用现有概念：模型服务对应现有 upstream，模型名字对应现有 alias。
- 明确命名：用户看到“模型服务”和“本地模型名”，工程里仍落到 upstream 和 alias。
- 显式不猜：limits/surface 只来自用户配置或未来明确的 provider 能力发现，不从字符串推断。
- 先让 Codex catalog 多模型闭环，Claude 采用官方确认的最小闭环。

## Team Review 决策

本计划经过三类 review：本地实现审阅、普通用户 UX 审阅、Codex/Claude 官方行为核对。收敛后的决策如下：

- 默认 alias 必须从 `default` 改为 `main`。`default` 在 Claude Code 中是特殊模型设置，不适合作为 llmup 新用户默认模型名。
- 首次 `llmup-config` 只做一个模型服务的闭环，不在第一屏暴露多模型服务、limits、surface 或 YAML。
- 多模型管理进入“已有配置后的二级菜单”和少量自动化子命令，不放进 README 第一屏。
- Codex 多模型使用官方 `model_catalog_json` 入口，但 catalog entry 的完整 schema 需要继续用 `codex debug models -c model_catalog_json=...` 和 fake-server smoke 保护。
- Claude Code 不承诺任意 llmup alias 都自动出现在 `/model` picker。稳定路径是 selected alias + custom model option；多模型只映射官方 `haiku` / `sonnet` / `opus` 家族 alias。Gateway discovery 先不默认启用。
- 不设计 llmup 自己的模型切换器。用户可以通过 `llmup-codex --llmup-model <alias>` / `llmup-claude --llmup-model <alias>` 选择本次启动模型；原生 `/model` 行为仍归 Codex/Claude 自己负责。

## 用户体验设计

### 首次运行

`llmup-config` 第一次运行时展示一段很短的说明：

```text
llmup 设置向导

我会帮你把一个模型服务接到本机，让 llmup-codex 和 llmup-claude 可以使用它。
不需要手写配置文件，API Key 只保存在本机。
```

然后只问必要问题：

1. 模型服务类型，默认 `openai-chat-completions`。
2. 模型服务地址，例如 `https://api.example.com/v1`。
3. 服务商真实模型名，例如 `MiniMax-M2.7-highspeed`。
4. API Key，保存到本机密钥文件。

本地模型名第一版默认就是 `main`，首次向导不主动询问。这样能减少普通用户第一次配置时的选择压力；需要多个模型时，在已有配置菜单里添加。

生成结果：

```yaml
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
  main: "main:<provider-model-name>"
```

`secrets.env` 保存：

```bash
LLM_UNIVERSAL_PROXY_KEY=<random-local-proxy-key>
LLMUP_PROVIDER_MAIN_API_KEY=<user-provider-api-key>
```

### 已有配置

再次运行 `llmup-config` 时，不直接退出，也不要求用户记命令。显示脱敏摘要后进入简单菜单：

```text
当前配置

模型服务:
  main  openai-chat-completions  https://api.example.com/v1

本地模型:
  main -> main:MiniMax-M2.7-highspeed

API Key: 已配置

你想做什么？
1. 完成，直接使用 llmup-codex 或 llmup-claude
2. 添加或修改本地模型
3. 运行检查
4. 重新配置
```

其中“添加或修改本地模型”再进入二级菜单：

- 添加一个新的模型服务：询问模型服务类型、地址、真实模型名、本地模型名和 API Key。
- 给已有模型服务添加一个本地模型名：从已有服务中选择，再输入真实模型名和本地模型名。
- 修改模型上下文长度：复用现有 `set-limits` 逻辑，提示用户选择本地模型名或模型服务，然后输入 `context_window` 和 `max_output_tokens`。

不要同时提供“先添加模型服务再添加模型名”和“添加模型名时顺便建模型服务”两套普通用户路径。交互菜单中的“添加一个模型服务”就是最完整路径；高级命令行子命令可复用内部函数，但不作为 README 主路径。

### 协议格式文案

用户必须看到完整名字：

```text
模型服务类型 [openai-chat-completions]
1. openai-chat-completions  常见兼容接口，路径通常是 /v1/chat/completions
2. openai-responses         OpenAI Responses 接口，路径通常是 /v1/responses
3. anthropic-messages       Anthropic Messages 接口，路径通常是 /v1/messages
```

不再出现单独的 `openai`、`completion`、`compatible` 作为新文案。解析层也只接受这三个完整名字，避免用户在“兼容接口”与具体协议形态之间产生误解。

## 命令设计

普通用户只需要：

```bash
llmup-config
llmup-codex
llmup-claude
```

保留并完善少量自动化子命令：

```bash
llmup-config doctor
llmup-config list
llmup-config set-limits (--alias <name> | --upstream <name>) --context-window <n> --max-output-tokens <n>
```

新增子命令只作为测试和高级自动化入口，不进入 README 主路径：

```bash
llmup-config add-model --new-service --service-name <name> --interface <format> --url <url> --model <provider-model> --alias <alias> --api-key-stdin
llmup-config add-model --service <name> --model <provider-model> --alias <alias>
```

其中：

- `add-model --new-service` 创建 upstream、保存对应 provider key，并创建第一个 alias。
- `add-model --service` 不接触密钥，只给已有 upstream 增加 alias。
- `list` 输出脱敏摘要，供用户和测试确认当前配置。
- 非交互命令必须复用交互式向导同一套校验、写文件和 secret env 命名函数。

## 默认命名规则

新配置默认：

- upstream name：`main`
- model alias：`main`
- provider key env：`LLMUP_PROVIDER_MAIN_API_KEY`

新增模型服务时，默认 service name 从本地模型名派生，但必须规范化为小写字母、数字和连字符；冲突时提示用户换名字，不自动生成 `main-2` 这类用户看不懂的名字。

禁止新生成：

- model alias `default`
- upstream name `DEFAULT`

已有配置处理：

- 如果用户已有 `default` alias，配置继续可读，但新 launcher 默认不会再选择它。
- `llmup-config` 摘要中提示：`default` 容易和 Claude Code 默认模型混淆，建议重新命名为 `main`。
- 提供一个显式交互动作“把 default 改名为 main”。只有当 `main` 不存在且 `default` 存在时显示。
- 如果用户坚持使用旧 alias，需要显式传 `--llmup-model default`；Claude Code 路径不推荐这样做，文档应引导改名。
- 不做 legacy fallback：无参数 launcher 只默认选择 `main`。如果 `main` 不存在但 `default` 存在，必须 fail fast，并提示运行 `llmup-config` 执行改名，避免在 Claude Code 中误触发 `default` 特殊模型语义。
- pre-GA 可以在后续版本移除新生成 `default` 的测试期望，但不需要实现复杂的自动迁移器。

## Agent 模型可见性

### Codex

Codex 多模型能力走官方 `model_catalog_json`。llmup 不在 Codex 内部发明自己的模型切换器；模型是否在 Codex UI 中如何展示，以 Codex 对 model catalog 的官方和当前版本行为为准。

当前 `AgentModelProfile::from_config(config, alias)` 只构造单 alias profile，`build_codex_model_catalog(profile)` 只输出一个模型。下一步改成：

- 新增 `AgentModelCatalog::from_config(config, selected_alias)`。
- catalog 中包含 `config.model_aliases` 的所有 alias。
- 每个 alias 继续使用现有 limits/surface 合并逻辑生成 entry。
- `selected_alias` 只用于启动时追加 `-m <selected_alias>`，不限制 catalog 内容。
- `--llmup-model` 默认值从 `default` 改为 `main`。
- 如果 selected alias 不存在，启动前 fail fast，并打印 `llmup-config list` 可看到的 alias。

验收：

- `codex debug models -c model_catalog_json=<generated>` 能解析所有 configured alias。
- 真实 Codex smoke 尽量确认 `/model` 或等价模型选择界面能看到这些 alias；如果某个 Codex 版本 UI 不展示 catalog 中的全部 alias，只要 `-m <selected_alias>` 能稳定使用 selected alias，本轮不为此增加 llmup 自己的模型切换 UI。
- Codex 不再出现 `Model metadata for default not found`，因为默认启动模型为 `main`，且 catalog 中包含 `main`。

### Claude Code

Claude Code 多模型能力分为两层。

稳定默认层：

- `ANTHROPIC_MODEL=<selected_alias>` 继续控制启动模型。
- `ANTHROPIC_CUSTOM_MODEL_OPTION=<selected_alias>` 继续确保 selected alias 至少可见。
- `--llmup-model` 默认值从 `default` 改为 `main`。
- `CLAUDE_CODE_SUBAGENT_MODEL=<selected_alias>` 继续让 subagent 使用同一个 llmup alias，避免子代理退回官方 endpoint 或系统默认模型。

官方家族 alias 层：

- 如果配置中存在 `haiku` alias，则设置 `ANTHROPIC_DEFAULT_HAIKU_MODEL=haiku`。
- 如果配置中存在 `sonnet` alias，则设置 `ANTHROPIC_DEFAULT_SONNET_MODEL=sonnet`。
- 如果配置中存在 `opus` alias，则设置 `ANTHROPIC_DEFAULT_OPUS_MODEL=opus`。
- 对应 `_NAME` / `_DESCRIPTION` 可使用 alias 原名和 `llmup proxy model <alias>`，但不从模型名猜测 capabilities。
- 这些值不是 provider 原始模型名，而是 Claude Code 发给 llmup gateway 的 provider-equivalent model ID。llmup 必须能把 `haiku` / `sonnet` / `opus` 当作自己的 model alias 解析到真实 upstream model。
- 必须有 fake Claude smoke：选择 `haiku`、`sonnet` 或 `opus` 后，代理收到的请求体 model 是对应 alias，并解析到 llmup 配置的真实模型。

Gateway discovery 层：

- 不默认开启 `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY=1`。
- 后续可以提供高级开关，例如 `--llmup-enable-claude-model-discovery` 或配置项，但本计划不实现。原因是官方 discovery 只接受部分模型 ID 前缀，且结果会写入 Claude Code 自己的 cache，普通用户第一版不需要理解这层。

边界：

- 不承诺任意 alias 都出现在 Claude `/model` picker。
- 如果用户希望 Claude `/model` 中有多个官方家族选项，推荐把 alias 命名为 `haiku`、`sonnet`、`opus`。
- `main` 作为普通启动模型，适合大多数用户；家族 alias 是可选增强。

## 实施落点

### `src/user_tools/config_wizard.rs`

需要结构化重构，但保持轻量：

- 将“读取输入”和“配置写入”分离，方便测试。
- 新增 `ConfigSummary`，供交互式摘要、`list`、`doctor` 复用。
- 新增 `AddModelOptions`，内部区分 new service 和 existing service，复用 `InitOptions` 的校验和写入路径。
- 把 `generated_config_yaml` 从只生成一个 `DEFAULT/default`，改为使用传入 service name、alias name 和 provider key env。
- provider key env 由 service name 派生：`LLMUP_PROVIDER_<UPPER_SNAKE_NAME>_API_KEY`。
- 写 `secrets.env` 时保留已有 key；对应 service key 缺失时新增，已存在时保留不覆盖；添加模型服务不覆盖 `LLM_UNIVERSAL_PROXY_KEY`。stale duplicate secret 更新不属于本轮最小路径。
- `doctor` 检查所有 upstream 所需 secret 是否存在。
- `set-limits` 保持现有命令，但交互式菜单可调用同一逻辑。

### `src/user_tools/agent_model_profile.rs`

- 保留单 profile 函数用于局部测试。
- 新增多模型 catalog builder，输入 `Config` 和 selected alias，输出完整 Codex catalog。
- 把 auto compact、modalities、tool surface entry 生成逻辑抽成 per-alias helper，避免复制。
- selected alias 不影响 catalog entry 列表，只影响 launcher `-m`。

### `src/user_tools/agent_launcher.rs`

- 默认 alias 从 `default` 改为 `main`。
- `ProfileProjection` 从单 `AgentModelProfile` 扩展为 selected profile + all profiles/catalog，或者新增 `AgentModelProjection` 包装，避免字段含义混乱。
- Codex profile projection 写包含所有 alias 的 catalog。
- Claude profile projection 注入 selected alias，同时按配置中存在的 `haiku` / `sonnet` / `opus` alias 注入官方家族 env。
- 错误提示中推荐 `llmup-config list`，而不是让用户手改 YAML。

### 文档

必须同步更新：

- `README.md`：主推 `llmup-config` 交互式配置，默认模型名写 `main`，不要再出现 `default` 作为新用户示例。
- `docs/clients.md`：说明 `--llmup-model` 默认是 `main`，Codex catalog 包含所有 llmup alias 且 selected alias 可启动；Codex UI 是否展示全部 alias 以当前 Codex 行为为准。Claude 默认启动 selected alias，`haiku` / `sonnet` / `opus` 是官方家族映射。
- `docs/advanced-usage.md`：保留手动 YAML、多模型服务、多 alias、limits、手动启动 proxy 的高级说明。
- 旧工程计划中提到“默认 alias 是 default”的地方必须改成“历史实现/旧计划”，或直接更新为 `main`，避免交付给开发团队时互相冲突。

## TDD 任务清单

先写失败测试，再实现。

配置向导：

- `llmup-config` 无参数在无配置时生成 `main/main`，不生成 `DEFAULT/default`。
- 生成的 `config.yaml` 可被 `Config::from_yaml_str` 解析并 validate。
- `secrets.env` 包含 `LLM_UNIVERSAL_PROXY_KEY` 和 `LLMUP_PROVIDER_MAIN_API_KEY`，权限安全，且不打印明文 key。
- 交互式已有配置摘要能列出多个 upstream 和多个 alias。
- `llmup-config list` 输出脱敏摘要，不触网。
- `add-model --new-service` 增加 upstream、alias；对应 service secret 缺失时新增，已存在时保留不覆盖。
- `add-model --service` 只改 `model_aliases`，不改 secrets。
- service name / alias name 校验覆盖空值、空格、冒号、重复、大小写碰撞、规范化碰撞、`foo_bar` / `foo.bar` 这类容易混淆的输入，以及所有新建路径禁用新建 `default`。
- 旧 `default` alias 可读；当 `default` 存在且 `main` 不存在时摘要显示改名建议，launcher 默认 `main` 不存在时要给出清晰错误和 `llmup-config` 修复提示。
- `doctor` 对多个 upstream 的 provider key env 全量检查。

Codex launcher：

- 默认 selected alias 是 `main`。
- `--llmup-model <alias>` 选择启动模型，但 catalog 仍包含所有 alias。
- catalog 中每个 alias 都有完整 entry shape、limits、auto compact、modalities、tool surface。
- unknown selected alias fail fast，错误里列出可用 alias 或提示运行 `llmup-config list`。
- `codex debug models -c model_catalog_json=<generated>` 能解析多 alias catalog；真实 Codex smoke 尽量覆盖 selected alias 能启动。
- 真实或 fake Codex smoke 确认不会再因 `default` 缺 metadata 报警。

Claude launcher：

- 默认 selected alias 是 `main`。
- env 包含 `ANTHROPIC_MODEL=main`、`ANTHROPIC_CUSTOM_MODEL_OPTION=main`、`CLAUDE_CODE_SUBAGENT_MODEL=main`。
- 配置存在 `haiku`、`sonnet`、`opus` alias 时，分别注入 `ANTHROPIC_DEFAULT_HAIKU_MODEL`、`ANTHROPIC_DEFAULT_SONNET_MODEL`、`ANTHROPIC_DEFAULT_OPUS_MODEL`。
- fake Claude smoke 覆盖家族 alias：选择 `haiku` / `sonnet` / `opus` 后，请求体 model 是该 alias，proxy 按 llmup alias 解析到目标 upstream model。
- 不存在家族 alias 时不注入对应 env。
- 不默认开启 `CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY`。
- native `--model` 冲突处理仍保持现有 fail-fast 逻辑。

文档契约：

- README 不再把 `default` 作为新用户模型名。
- README 不再使用含混的 `OpenAI-compatible` 来指协议格式。
- 普通用户路径只出现 `llmup-config`、`llmup-codex`、`llmup-claude`。
- 高级文档包含手动多模型服务、多 alias、limits 示例。

## 验收标准

- 新用户按 README 运行 `llmup-config`，默认生成 `main` 模型，并能直接运行 `llmup-codex` / `llmup-claude`。
- `llmup-config` 已有配置界面可以添加第二个模型服务和第二个 alias，不需要手写 YAML。
- `llmup-codex` 的模型 catalog 包含所有 alias，启动默认模型为 `main`；selected alias 可通过 Codex 当前版本稳定发请求。
- `llmup-claude` 默认不再碰 `default`，并能用 `main` 作为 selected alias；配置 `haiku` / `sonnet` / `opus` 时按官方 env 映射。
- 旧配置不会被静默改写；如果只有旧 `default` alias，无参数 launcher 明确失败并提示通过 `llmup-config` 改名为 `main`。新生成配置、新文档、新测试都不再依赖 `default`。
- 所有改动复用现有 YAML schema，不新增 provider marketplace、智能推荐、复杂 profile 系统或第二套模型能力 schema。

## 开发顺序建议

这不是分阶段发布计划，而是为了 TDD 落地时减少返工的实现顺序：

1. 先改测试期望和文档契约，确认 `main` 是新默认名。
2. 重构 `config_wizard` 的生成函数，支持 service name、alias name、secret env name 参数化。
3. 增加 `list`、`add-model --new-service`、`add-model --service` 的非交互测试入口。
4. 改交互式菜单，让无参数 `llmup-config` 覆盖首次配置和已有配置管理。
5. 改 Codex catalog builder 为全 alias catalog。
6. 改 launcher 默认 alias 和 Claude 家族 alias env 投影。
7. 更新 README、clients、高级文档和旧工程计划中的冲突描述。
8. 跑小范围 gate：`cargo fmt --check`、相关 user_tools 测试、docs contract 测试、Codex/Claude fake launcher smoke。
