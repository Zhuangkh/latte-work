# Provider 与 Code Agent

Provider 是 App 独立管理的模型服务配置，包含名称、协议、Base URL、模型 ID 和凭据。
设置页的「连接与 Agent」和「Provider」是并列分类，切换 Tab 会保留未保存的 Provider 表单。
Provider 页面没有主机选择，也不会因为保存配置而自动关联 Agent。
支持保存 Anthropic Messages、OpenAI Chat Completions、OpenAI Responses 三类配置。

Code Agent 是主机上的执行程序。在「设置 → 连接与 Agent」中选择运行主机，
为 Agent 关联一个协议兼容的 Provider。当前 Claude Code Adapter 只接受
Anthropic Messages；两种 OpenAI 配置可保存，当前没有能使用它们的 Agent Adapter。
不会自动把 OpenAI 转换成 Anthropic。前端过滤选项，server 与 Adapter 同时校验。
未关联 Provider 时沿用 Agent CLI 自己的配置。

## 使用

1. 在「设置 → Provider」添加模型服务。
2. 在「连接与 Agent」Tab 选择主机，再选择 Provider。
3. 点击「保存关联」。本机和远程都在下次发送消息时使用最新配置，
   已在执行的进程持有旧快照，不会被修改。

Anthropic Base URL 通常为 `https://api.example.com`；Claude CLI 追加 `/v1/messages`。
OpenAI 配置按服务提供的 API base 填写，常见为 `https://api.example.com/v1`。
模型 ID 原样传入 CLI。Bearer Token 使用 `Authorization: Bearer …`；API Key 使用
`x-api-key`，标准 OpenAI 的 key 应使用 Bearer Token。

## App 配置与每轮执行

Provider 的增删改查直接调用桌面 Rust 层，读写 App 配置目录，不连接本机或远程 Server，
也不探测已安装的 Agent。定义可离线保存；在「连接与 Agent」中为主机 / Agent 选择时，
按该主机 Hello 返回的协议能力校验。关联也是 App 配置，不修改任何 Agent 的原生设置。

本机与 SSH 主机使用同一执行路径。模型菜单向所选主机发送 Provider 元数据；
发送消息时先从该主机取得会话的 Agent，再读取 App 中最新的关联及凭据，
附在本轮请求中。未关联时显式选择 CLI 配置，模型名称、默认值和档位只来自所选主机，
不会读取其他环境的 Agent 配置。本机 Server 不可用不影响远程模型加载或下一轮发送。

系统 SSH 配置、身份文件、密码认证和自定义端口使用同一条通道。
已运行的任务保持原快照，SSH 断开不停止已接受任务；Agent 直接连接模型服务。
目标 Server 在启动前验证快照、模型和档位，不把本轮 Provider 写入长期目录。
请求 ID 同时绑定配置摘要；重复 ID 的配置变化会被拒绝，连接失败不会自动重发。
App 配置损坏或读取失败会明确报错，不静默改用另一套服务。

删除 Provider 需在界面中二次确认，并原子解除本机及远程主机中指向它的 Agent 关联；其他 Provider 及关联保持不变。更改协议前仍需先解除所有关联。移除 SSH 主机时解除对应
App 关联。未关联的环境显式沿用 CLI，忽略目标 Server 中的旧 Provider 副本，旧文件不删除。

## 模块

- `latte-work-config`：App Provider 定义、每主机关联、只读迁移、文件锁与原子持久化；无 Agent 注册表或 Server 依赖。
- 桌面 `providers.rs`：App 配置命令、目标主机能力校验；凭据不返回 WebView。
- `latte-work-client::request_with_app_provider`：只通过目标连接查询会话、模型及发送任务。
- Server `providers.rs`：执行快照校验、临时启动配置及旧协议管理接口的兼容实现。
- `agents/mod.rs` / `agents/claude.rs`：Agent 能力、原生配置读取及 CLI 参数映射。
- `ProviderSettings` / `AgentSettings`：App 配置和每主机选择界面。

## 凭据与迁移

Provider 和关联在 Unix 保存在 Tauri `app_config_dir()/providers.json`，macOS 默认路径为
`~/Library/Application Support/co.latte.work/providers.json`。Unix 目录权限 700、文件权限 600。
Windows 保存在 `app_config_dir()/private-providers/providers.json`，仅专属子目录设为受保护的
当前用户 SID 独占 DACL（不因令牌提权而拒绝）；混存其他 App 文件的根目录权限不变。现存文件先验证同一打开句柄的
owner、DACL 和非 reparse 属性，再读取；祖先路径拒绝 junction/reparse，验证期间固定目录句柄。
写入前重新验证私有目录及现存目标，临时文件通过私有权限校验后才写入凭据并原子替换。
内容未加密，主机用户可读。通过有界文件锁串行化多个 App 进程的修改，每次操作重新读取，
使用临时文件和原子替换保存，配置大小上限 128 KiB。

新 App 文件不存在时，Windows 优先从旧 `app_config_dir()/providers.json` 迁移；
没有旧 App 文件才读取 `~/.local/share/latte-work/providers.json`。Unix 直接从该 Server 路径迁移。
Windows 的旧文件也必须在读出凭据前通过私有权限及路径检查；不安全对象显式失败，不自动修复，
不读取、删除或覆盖原文件，也不创建空配置。用户可保留备份后自行恢复当前用户 owner 和
仅该用户 FullControl 的 DACL，再重试；若目录本身含 junction，则需先恢复普通本地目录路径。
迁移将 Provider 定义、
凭据、本机关联和 SSH 主机关联一次性迁移到 App 配置。迁移不启动旧 Server，也不修改原文件；
schema 1 的全局选择仅在指向现有 Anthropic Provider 时保留为本机 Claude 关联；
OpenAI Provider 或已失效的选择沿用旧版行为，不创建关联，保留各主机的 CLI 默认配置。
Provider 定义及凭据仍完整迁移。无旧配置时创建空 App 配置，之后不重复导入。
新文件一旦存在就具有权威性；修改或删除 Provider 不会从保留的旧文件重新导入。
无效旧文件会报错并保留原文件，不写空配置覆盖它。后续旧版 App 对 Server 配置的修改不会
自动同步回新版 App。

Windows Server 的兼容持久配置使用 `%LOCALAPPDATA%/latte-work/private/providers.json`，
临时 Claude 启动配置位于 `private/launch/`，附件位于独立私有 `paste-tmp/`。
普通 `state.sqlite` 沿用父目录 ACL；自定义 `--state-dir` 不自动获得默认用户目录的隐私保证。
Server 顶层旧 `providers.json` 显式报迁移错误，不从未经验证的位置自动读入或移动；
这与 App 上述只读导入规则不同。恢复前保留原文件，按错误指明的位置处理 owner、唯一用户
FullControl 与非 reparse 路径，再重试；不要为排错放宽整个状态目录或删除数据库。
Unix Server 保留原有顶层文件和 0700/0600 权限契约。

列表仅返回凭据是否存在，编辑留空保留原值；改地址或认证方式需重新填写凭据。
凭据只经过原生层及认证通道，不进入 WebView、事件或请求账本；运行中的目标 CLI 可读取
临时 settings，正常结束后清理。Server 被强制杀死可能遗留私有临时文件。

此改动复用已有的 `ModelsForProvider` / `TurnProvider` 协议，不新增 wire 字段。
目标 Server 需支持显式 CLI / Snapshot；旧版接口仍供独立 Server 客户端使用，App 不再调用。
升级 Server 前仍需等任务结束并关闭终端；App 配置管理无需升级或启动 Server。

## 验证

`make ci` 包含离线 CRUD、schema 1 / 2 迁移、凭据隐藏、跨 App 写入、主机隔离、
目标 Server 的原生模型及自定义快照执行、续聊、请求摘要与错误拒绝测试。
前端测试覆盖本机连接失败时的 Provider 管理及远程 Agent 关联设置。
实际 Server + 确定性 CLI fixture 与真实 SSH / Claude 验证分别记录。

可选真实 Claude CLI 与本地模拟上游验证：

```sh
python3 scripts/smoke-providers.py --binary target/debug/latte-work-server --claude /absolute/path/to/claude
```

## 会话模型选择

在 Provider 中填写默认模型 ID，以及可选模型 ID（每行一个，最多 64 个）。默认模型自动包含在列表中。
输入框底部的模型菜单读取 App 为该项目主机、Agent 选择的 Provider，不混用其他 Provider 的模型。
关闭设置后重新读取最新列表；发送时仍由远端根据本轮快照校验。

模型随下一条消息提交，服务端在启动前校验列表，并只覆盖本轮启动快照；不修改 Provider 默认值或其他会话。
选择记录保存在会话中，重连可恢复；重复请求 ID 也校验模型，避免换模型后错误复用已接受请求。
“默认”在绑定 Provider 时使用其默认模型；未绑定时沿用 CLI，新一轮恢复会话时显式清除上轮模型覆盖。
原有单模型 Provider 自动兼容，可选模型列表默认为空。

未关联 Provider 时提供 Claude 的模型别名，实际可用性仍由 CLI 和账号决定。启动传参依据
[Claude Code 官方模型配置文档](https://code.claude.com/docs/en/model-config)，通过 `--model` 选择，
不修改用户的 CLI 设置文件。

## 思考强度

思考档位属于 Code Agent 的能力，不属于 Provider 协议的通用属性。每个 Agent
适配器负责可选档位、模型限制、自动行为和原生参数映射；公共注册表只按 Agent
分发，未实现的 Agent 不继承 Claude 档位。前端只展示 Host 返回的 `effort_levels`，
发送时由服务端再次使用同一能力规则校验。当前只有 Claude 适配器已实现；下列
档位和自动行为仅描述 Claude，不代表其他 Code Agent 的能力。

模型选择器的 `Reasoning effort` 子菜单使用 Agent 原生英文档位；Claude 为 `auto / low / medium / high / xhigh / max`，按模型能力筛选。选择随下一条消息提交并保存在会话中，
不会修改 Provider 默认模型或用户的 Claude 配置文件。已知不支持 effort 的模型禁用调整，
已知模型版本只提供支持的档位；自定义模型 ID 的实际支持情况和组织限制由 CLI / 网关决定。

Claude adapter 在每轮启动时设置 `--effort` 和临时配置中的 `CLAUDE_CODE_EFFORT_LEVEL`。
“自动”用环境值 `auto` 清除之前的覆盖；不强制固定为 high。恢复会话时也重新应用本轮选择。
这不是关闭思考的开关，也不保证模型内部一定消耗某个 token 数。

依据 [Claude Code effort 配置](https://code.claude.com/docs/en/model-config#adjust-effort-level)。
`scripts/smoke-providers.py` 使用真实 CLI 和本地模拟 Anthropic 服务验证高、低及自动恢复，
检查实际发出的 `output_config.effort`，不调用付费模型接口。

## Claude 原生模型显示名称

未绑定 Latte Work Provider 时，模型目录额外返回按别名索引的 `model_labels`。
Claude 适配器读取执行 Host 的 `ANTHROPIC_DEFAULT_{FAMILY}_MODEL_NAME`；未设置名称时
回退到对应 `_MODEL`，没有覆盖时仍显示原生别名。菜单中自定义名称为主文字，别名
以浅灰色辅助显示；输入框只显示主名称。选择值仍是 `sonnet` 等原生别名，名称不会
写入会话模型参数，也不修改 Claude 配置或 Provider。

元数据读取范围：Host 进程模型变量、`CLAUDE_CONFIG_DIR/settings.json`（缺省为
`~/.claude/settings.json`）、已注册项目的 `.claude/settings.json` 和
`.claude/settings.local.json`、系统 `managed-settings.json`；按此顺序覆盖。
每个文件限 1 MiB，字段白名单只包含四个模型族的 ID 和显示名称。凭据与其他配置
不进入响应。无效或不可读文件跳过，名称最长 256 字符且不得包含控制字符。
MDM、云端托管策略、managed-settings.d 及自定义 modelPicker 行不在此显示元数据
读取范围内，执行行为和最终模型仍由 Claude 决定。绑定 Provider 时完全使用 Provider
模型列表，不叠加原生别名显示名称。

依据：[Claude 自定义模型显示名称](https://code.claude.com/docs/en/model-config#customize-pinned-model-display-and-capabilities)
与 [设置优先级](https://code.claude.com/docs/en/settings#settings-precedence)。
