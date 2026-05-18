# LLM Universal Proxy

[English README](./README.md) · [文档索引](./docs/README.md)

`llmup` 是放在本机的模型 API 代理。它让 Codex CLI 和 Claude Code 通过同一套本地 launcher 连接你的模型 API 或兼容 endpoint，同时把真实 provider key 留在 proxy 侧。

它追求最大安全兼容：能安全转换的请求就在本地转换；不能安全表达的能力会先在本地失败，而不是把不确定的请求硬塞给上游。

> [!IMPORTANT]
> `llmup` 面向模型 API 和兼容 endpoint。它不是把厂商 App 订阅或第一方 CLI 套餐变成第三方 API 的工具，除非厂商明确允许这种接入方式。

## 三步开始

请先安装你要使用的原生客户端。`llmup` 不会自动安装 Codex CLI 或 Claude Code。

安装本地程序和三个用户命令：

```bash
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/agentsmith-project/llm-universal-proxy/releases/latest/download/install.sh | sh
```

配置你的模型服务：

```bash
llmup-config
```

启动你要用的客户端：

```bash
llmup-codex
```

或者：

```bash
llmup-claude
```

真实模型服务 Key 由 `llmup-config` 保存到本机代理配置里，不需要粘到 Codex CLI 或 Claude Code 里。launcher 只把本地代理密码交给客户端，真实 provider key 留在 proxy 侧。

launcher 也会管理本地代理进程、客户端 base URL、默认模型别名，以及 llmup 专用的 Codex/Claude 状态目录。

## 为什么没有独立的 `llmup` 命令

第一版有意不提供独立的 `llmup` 主命令。普通用户只需要记住 `llmup-config`、`llmup-codex` 和 `llmup-claude`。

这样第一次使用不会变成学习一套新 shell：先配置，然后继续启动你本来就要用的原生客户端。`llm-universal-proxy --config` 仍然保留给高级服务端用法。

## 兼容边界

`llmup` 提供稳定的本地协议入口，但不承诺所有模型服务能力都完全等价。

- 最大安全兼容表示尽量保留可安全携带的信息，并拒绝不安全的转换。
- provider 专属状态、不透明 reasoning carrier、不可移植工具语义，可能需要原生路径，或者会先在本地失败。
- `xhigh` 这类 reasoning effort 仍然是客户端或请求参数，不是模型名的一部分。

## 高级用法

- [docs/clients.md](./docs/clients.md)：launcher 管理 Codex CLI 和 Claude Code 的方式
- [docs/advanced-usage.md](./docs/advanced-usage.md)：手动启动 proxy、YAML、手动 Codex/Claude 接线和认证模式
- [docs/container.md](./docs/container.md)：容器镜像用法
- [docs/admin-dynamic-config.md](./docs/admin-dynamic-config.md)：Admin 和动态配置参考
- [docs/ga-readiness-review.md](./docs/ga-readiness-review.md)：GA 范围和兼容边界

## License

MIT License
