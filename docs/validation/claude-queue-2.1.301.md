# Claude CLI 排队交付契约验证

2026-10-16，在 macOS arm64 上使用真实安装的 **Claude Code 2.1.301** 验证。
Server、项目、CLI 配置与会话均放在临时目录，结束后关闭测试 Server。
Python 只提供本地 Anthropic API 响应，没有仿真 Claude 的 stream-json 输出；无付费模型调用。

## 结果

| 检查 | 实际结果 |
| --- | --- |
| `--replay-user-messages` | 真实 CLI 接受，初始化并执行成功 |
| 同一执行期间交付两个不同请求 ID、相同正文的输入 | 两条均获得 `isReplay: true` 与不同原生 UUID |
| 原生完成归因 | 一条真实 `result` 的 `user_message_uuids` 同时包含两个输入 UUID |
| Server 持久状态 | 两条队列记录均为 `dispatched`；会话 `completed`、`agent_session_busy: false` |
| 重复执行检查 | 仅一个执行起始事件、两条交付气泡记录、一次原生完成；可正常关闭会话 |
| 本地上游请求 | 两次：一次 Read 工具响应，一次最终文本响应 |

原生字段摘录与二进制/源码 SHA-256 保存在
[原始验证结果](claude-queue-2.1.301.json)。验证在 `0ecf42a` 后的工作区修复上运行，
因此结果明确记录 `source_dirty: true`，并以 `source_files_sha256` 绑定实际验证的源码；
不能把基准提交号当作修复后源码版本。

## 复现

先构建当前 Server，再运行：

```sh
cargo build -p latte-work-server --locked
python3 scripts/smoke-claude-queue.py \
  --binary target/debug/latte-work-server \
  --claude ~/.local/bin/claude \
  --output target/real-queue.json
```

脚本通过真实 Server 协议入队与立即交付，并从真实 CLI 的标准输出捕获回执。
它不生成用户回显或 UUID 归因；缺少所需行为、未结清、重复起轮或异常退出均使验证失败。
脚本有就绪、HTTP 响应释放、轮询及清理时限，输出中不包含登录凭据或真实项目内容。

这是 **2.1.301 的真实 CLI + 本地模拟上游** 证据，独立于 Python CLI 的最终二进制 E2E，
也不是对其他 CLI 版本、真实模型响应质量或所有队列交错情况的验证。
