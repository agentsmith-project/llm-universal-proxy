# LLM Universal Proxy

[English README](./README.md) · [文档索引](./docs/README.md)

Codex CLI 和 Claude Code 很好用，但它们经常只认自己熟悉的接口。
你手上可能有另一个模型服务，比如 MiniMax，给的是“长得像 OpenAI 的接口”。
这时就容易卡住：工具想走一种格式，模型服务又等着另一种格式。

`llmup` 就是放在你电脑上的一个本地中转站。Codex CLI 或 Claude Code 先把请求发给
`llmup`，`llmup` 再尽量转成模型服务能听懂的格式，然后把结果送回客户端。少数服务独有能力如果无法安全转换，`llmup` 会在本地报错，而不是把不确定的请求硬塞给模型服务。

这份中文 README 只讲一件事：不用 Docker，也不用先学完整配置，先把本地二进制跑起来，让 Codex CLI 或 Claude Code 通过 `llmup` 使用 MiniMax 这样的 OpenAI-compatible 服务。

## 你会得到什么

- 给 Codex CLI 接入它原本不好直接使用的 OpenAI-compatible Chat Completions 服务。
- 给 Claude Code 接入 OpenAI-compatible 服务，例如 MiniMax。
- 把真实模型服务 API Key 留在本地中转站里，客户端只拿一个本地密码。
- 给复杂模型名取一个短名字，例如 `minimax`。
- 一条命令启动本地中转站和客户端；退出客户端后，中转站也会自动停掉。

这里的 MiniMax 只是一个例子，不是 `llmup` 绑定或必需的供应商。你可以把地址、模型名、API Key 换成其他长得像 OpenAI 接口的服务。

## 你需要准备什么

先确认你有这些东西：

- 已经安装好的 Codex CLI 或 Claude Code，至少装一个即可。
- Python 3。仓库里的启动脚本需要它。
- Git，或者能下载本仓库 zip 包。
- 一个模型服务账号。下面用 MiniMax 举例。
- 这个模型服务的三样信息：
  - API 地址前缀，例如 `https://api.minimaxi.com/v1`
  - 模型名，例如 `MiniMax-M2.7-highspeed`
  - API Key。不要发给别人，也不要提交到仓库。

下面的命令以 macOS、Linux 或 WSL 的 Bash 终端为例。Windows 原生 PowerShell 后续再单独写，第一次使用建议走 WSL。

## 第一步：拿到本地程序

先把仓库拉下来。这里主要需要里面的启动脚本和示例文件。

```bash
git clone https://github.com/agentsmith-project/llm-universal-proxy.git
cd llm-universal-proxy
```

然后下载 `llmup` 的本地可执行文件。先按你的电脑选择一个文件名：

| 你的电脑 | 文件名 |
| --- | --- |
| Mac，Apple Silicon，例如 M1/M2/M3/M4 | `llm-universal-proxy-macos-aarch64.tar.gz` |
| Mac，Intel 芯片 | `llm-universal-proxy-macos-x86_64.tar.gz` |
| Linux，常见 Intel/AMD 服务器或电脑 | `llm-universal-proxy-linux-x86_64.tar.gz` |
| Linux，ARM64 | `llm-universal-proxy-linux-aarch64.tar.gz` |

如果你在 WSL 里使用，请按 Linux 选择，通常是 `llm-universal-proxy-linux-x86_64.tar.gz`。

例如 Apple Silicon Mac 可以这样下载：

```bash
export LLMUP_ASSET="llm-universal-proxy-macos-aarch64.tar.gz"

mkdir -p .local/bin
curl -L \
  -o /tmp/llmup.tar.gz \
  "https://github.com/agentsmith-project/llm-universal-proxy/releases/latest/download/${LLMUP_ASSET}"
tar -xzf /tmp/llmup.tar.gz -C .local/bin
chmod +x .local/bin/llm-universal-proxy

test -x .local/bin/llm-universal-proxy && echo "llmup 已准备好"
```

如果最后一行能看到 `llmup 已准备好`，说明本地程序已经放好了。

如果下载地址返回 404，可以打开 [Releases](https://github.com/agentsmith-project/llm-universal-proxy/releases) 页面，手动下载同名文件。

## 第二步：写一个 MiniMax 配置

先写一个只包含 MiniMax 的最小配置文件：

MiniMax 常见有两个 API 地址：

- 国际站账号通常用 `https://api.minimax.io/v1`
- 中国站账号通常用 `https://api.minimaxi.com/v1`

下面示例先用中国站地址。如果你的账号在国际站，把 `api_root` 改成 `https://api.minimax.io/v1`。

```bash
cat > llmup-minimax.yaml <<'YAML'
listen: 127.0.0.1:18888
upstream_timeout_secs: 120

data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY

upstreams:
  MINIMAX:
    api_root: https://api.minimaxi.com/v1
    format: openai-completion
    provider_key:
      env: MINIMAX_API_KEY
    limits:
      context_window: 200000
      max_output_tokens: 128000
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
  minimax: "MINIMAX:MiniMax-M2.7-highspeed"
YAML
```

这段配置里最重要的是三行：

- `api_root`：模型服务的 API 地址前缀。MiniMax 国际站通常是 `https://api.minimax.io/v1`，中国站通常是 `https://api.minimaxi.com/v1`。
- `provider_key.env`：真实 API Key 放在哪个环境变量里。这里写的是变量名 `MINIMAX_API_KEY`，不是 API Key 本身。
- `minimax`：你给模型取的本地短名字。后面 Codex CLI 和 Claude Code 都用这个名字。

这个 YAML 不放真实 API Key。真实 API Key 放在下一步的 `.env.llmup.local` 里。

如果你的 MiniMax 模型名不是 `MiniMax-M2.7-highspeed`，把最后一行里的模型名换成你账号里可用的模型名。

再写一个本地密钥文件：

```bash
cat > .env.llmup.local <<'ENV'
MINIMAX_API_KEY=REPLACE_WITH_YOUR_MINIMAX_API_KEY
LLM_UNIVERSAL_PROXY_KEY=local-dev-key
ENV
```

`MINIMAX_API_KEY` 是真实模型服务的 Key。`LLM_UNIVERSAL_PROXY_KEY` 是给本机中转站用的本地密码，可以自己换一个更长的随机字符串。

`.env.llmup.local` 已经被 `.gitignore` 覆盖，不应该提交到仓库。也建议你不要把真实 API Key 写进聊天、截图或公开文档。

## 第三步：启动 Codex CLI

如果你想让 Codex CLI 使用 MiniMax，运行：

```bash
bash scripts/run_codex_proxy.sh \
  --binary "$PWD/.local/bin/llm-universal-proxy" \
  --config-source llmup-minimax.yaml \
  --env-file .env.llmup.local \
  --workspace "$PWD" \
  --model minimax
```

这条命令会自动做几件事：

- 启动 `llmup`。
- 等 `llmup` 准备好。
- 把 Codex CLI 指到本地 `llmup`。
- 告诉 Codex CLI 使用你配置的 `minimax` 模型短名字。
- 你退出 Codex CLI 后，自动停止这次启动的 `llmup`。

你可以先在 Codex CLI 里问一句很短的问题，例如：

```text
hi
```

能正常回答，就说明链路通了。

## 第四步：启动 Claude Code

如果你想让 Claude Code 也使用同一个 MiniMax OpenAI-compatible 服务，运行：

```bash
bash scripts/run_claude_proxy.sh \
  --binary "$PWD/.local/bin/llm-universal-proxy" \
  --config-source llmup-minimax.yaml \
  --env-file .env.llmup.local \
  --workspace "$PWD" \
  --model minimax
```

Claude Code 还是按它熟悉的 Claude Messages 方式发请求；`llmup` 会在本地把请求尽量转成 MiniMax 这类 OpenAI-compatible 服务能接收的请求。无法安全转换的少数能力会在本地报错。

## 如果你的模型服务是 Anthropic Messages

有些服务提供的是“长得像 Claude `/v1/messages` 的接口”。这种情况下，可以新建一个单独的配置文件：

```bash
cat > llmup-anthropic-like.yaml <<'YAML'
listen: 127.0.0.1:18888
upstream_timeout_secs: 120

data_auth:
  mode: proxy_key
  proxy_key:
    env: LLM_UNIVERSAL_PROXY_KEY

upstreams:
  MY_ANTHROPIC_LIKE_SERVICE:
    api_root: https://anthropic-compatible.example/v1
    format: anthropic
    provider_key:
      env: MY_ANTHROPIC_LIKE_API_KEY
    limits:
      context_window: 200000
      max_output_tokens: 128000
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
  my-claude-like-model: "MY_ANTHROPIC_LIKE_SERVICE:provider-model-name"
YAML
```

把 `https://anthropic-compatible.example/v1` 换成你的模型服务地址，把 `provider-model-name` 换成真实模型名。

再写本地密钥文件：

```bash
cat > .env.llmup.local <<'ENV'
MY_ANTHROPIC_LIKE_API_KEY=REPLACE_WITH_YOUR_PROVIDER_API_KEY
LLM_UNIVERSAL_PROXY_KEY=local-dev-key
ENV
```

启动 Codex CLI：

```bash
bash scripts/run_codex_proxy.sh \
  --binary "$PWD/.local/bin/llm-universal-proxy" \
  --config-source llmup-anthropic-like.yaml \
  --env-file .env.llmup.local \
  --workspace "$PWD" \
  --model my-claude-like-model
```

启动 Claude Code：

```bash
bash scripts/run_claude_proxy.sh \
  --binary "$PWD/.local/bin/llm-universal-proxy" \
  --config-source llmup-anthropic-like.yaml \
  --env-file .env.llmup.local \
  --workspace "$PWD" \
  --model my-claude-like-model
```

## 常见问题

**提示找不到 `codex` 或 `claude` 命令**

说明你的电脑还没有安装对应客户端，或者命令不在 `PATH` 里。先在同一个终端里确认：

```bash
codex --version
claude --version
```

你只需要安装自己要用的那个客户端。

**提示找不到 `llm-universal-proxy`**

确认你下载后执行过：

```bash
chmod +x .local/bin/llm-universal-proxy
test -x .local/bin/llm-universal-proxy && echo "llmup 已准备好"
```

也确认启动命令里的 `--binary "$PWD/.local/bin/llm-universal-proxy"` 没有写错。

**提示 401、unauthorized 或 API key 无效**

通常是 `.env.llmup.local` 里的真实模型服务 Key 填错了，或者 Key 没有这个模型的权限。注意：客户端拿到的是 `LLM_UNIVERSAL_PROXY_KEY`，真正发给 MiniMax 的是 `MINIMAX_API_KEY`。

**提示 `MINIMAX_API_KEY`、`MY_ANTHROPIC_LIKE_API_KEY` 或 `LLM_UNIVERSAL_PROXY_KEY` 缺失**

通常是 `.env.llmup.local` 没写好，或者启动命令漏了：

```bash
--env-file .env.llmup.local
```

也检查一下你有没有把 `REPLACE_WITH_...` 这种占位文字替换成真实值。

**提示 404、not found 或 model not found**

通常是 `api_root` 或模型名填错了。`api_root` 写到版本前缀即可，例如：

```yaml
api_root: https://api.minimaxi.com/v1
```

不要写成完整的 `/chat/completions` 地址。模型名也要换成供应商后台实际可用的名字。

**提示端口被占用**

wrapper 默认会临时使用 `18888` 端口。换一个端口即可：

```bash
bash scripts/run_codex_proxy.sh \
  --proxy-port 19999 \
  --binary "$PWD/.local/bin/llm-universal-proxy" \
  --config-source llmup-minimax.yaml \
  --env-file .env.llmup.local \
  --workspace "$PWD" \
  --model minimax
```

Claude Code 同理，把脚本名换成 `scripts/run_claude_proxy.sh`。

## 继续阅读

第一次使用只看本页就够了。后面如果你想了解更多，可以继续看：

- [docs/clients.md](./docs/clients.md)：Codex CLI 和 Claude Code 更细的接法
- [docs/configuration.md](./docs/configuration.md)：完整 YAML 配置说明
- [docs/README.md](./docs/README.md)：完整文档索引
