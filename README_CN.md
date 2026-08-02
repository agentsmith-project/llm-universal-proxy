# LLM Universal Proxy

[English README](./README.md) · [文档索引](./docs/README.md)

`llmup` 是放在本机的模型 API 代理。它让 Codex CLI 通过同一个本地代理连接你的模型 API 或兼容 endpoint，同时把真实 provider key 留在 proxy 侧。

它追求最大安全兼容：能安全转换的请求就在本地转换；不能安全表达的能力会先在本地失败，而不是把不确定的请求硬塞给上游。

> [!IMPORTANT]
> `llmup` 面向模型 API 和兼容 endpoint。它不是把厂商 App 订阅或第一方 CLI 套餐变成第三方 API 的工具，除非厂商明确允许这种接入方式。

## 快速开始

请先安装你要使用的原生客户端。`llmup` 不会自动安装 Codex CLI 或 Claude Code。

用一行命令安装本地程序和 `llmup` 友好别名：

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/agentsmith-project/llm-universal-proxy/releases/latest/download/install.sh | sh
```

安装脚本会创建一个 `llmup` 别名，指向 `llm-universal-proxy`，所以面向用户的命令只有 `llmup codex-setup`。代理是一个本地服务，请先按你的模型服务配置启动它（静态 YAML 见 [配置指南](./docs/configuration.md)）。然后用 `codex-setup` 生成把子代理指向本代理的 Codex 配置：

```bash
llmup codex-setup --base-url http://127.0.0.1:8080/openai/v1 --model main --provider-key <本地代理密钥>
```

`codex-setup` 会在 `~/.codex` 下写入 `llmup` provider、一个自定义子代理和一个 `llmup` profile，让官方 Codex 主代理继续用自己的凭据，而子代理走本代理。真实 provider key 留在服务端，Codex 只拿到本地代理密钥。

然后用生成的 profile 启动 Codex：

```bash
codex exec --profile llmup
```

当代理配置询问“模型服务类型”时，请按服务商文档里的 API 形态选择：`openai-chat-completions`、`openai-responses` 或 `anthropic-messages`。默认本地模型名是 `main`；除非你配置了别的别名，否则把它传给 `codex-setup --model`。

## 关于 `llmup` 别名

安装脚本会创建一个 `llmup` 别名，指向唯一的 `llm-universal-proxy` 二进制，所以面向用户的命令只有 `llmup codex-setup`。`llm-universal-proxy --config` 仍然保留给高级服务端用法。

## 兼容边界

`llmup` 提供稳定的本地协议入口，但不承诺所有模型服务能力都完全等价。

- 最大安全兼容表示尽量保留可安全携带的信息，并拒绝不安全的转换。
- provider 专属状态、不透明 reasoning carrier、不可移植工具语义，可能需要原生路径，或者会先在本地失败。
- `xhigh` 这类 reasoning effort 仍然是客户端或请求参数，不是模型名的一部分。

## 高级用法

- [docs/clients.md](./docs/clients.md)：`codex-setup` 流程与 Codex V1 混合子代理拓扑
- [docs/advanced-usage.md](./docs/advanced-usage.md)：手动启动 proxy、YAML、手动 Codex/Claude 接线和认证模式
- [docs/container.md](./docs/container.md)：容器镜像用法
- [docs/admin-dynamic-config.md](./docs/admin-dynamic-config.md)：Admin 和动态配置参考

## License

MIT License
