//! Claude Code CLI bidirectional stream-json/control protocol.
//! Compatibility source: anthropics/claude-agent-sdk-python internal query/transport.
use super::{Action, AgentAdapter, AgentCommand, Input};
use crate::providers::LaunchConfig;
use anyhow::{Context, Result, bail};
use latte_work_protocol::{
    ContextUsage, EventKind, ExecutionPhase, SubagentStatus, SubagentUpdate, TurnUsage,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    path::PathBuf,
    process::Stdio,
    time::Duration,
};

pub const MODEL_ALIASES: &[&str] = &["sonnet", "opus", "haiku", "fable"];

/// Claude CLI/model capability policy, independent of other agent adapters.
pub fn effort_levels(model: Option<&str>) -> &'static [latte_work_protocol::Effort] {
    use latte_work_protocol::Effort::{High, Low, Max, Medium, Xhigh};
    let model = model.unwrap_or_default().to_ascii_lowercase();
    // CLI aliases can be mapped by a host or gateway. Full known model IDs allow
    // tighter choices; unknown custom IDs are delegated to the CLI/provider.
    if model == "haiku" || model.contains("claude-haiku-") {
        return &[];
    }
    if model.contains("claude-sonnet-4-6") || model.contains("claude-opus-4-6") {
        return &[Low, Medium, High, Max];
    }
    if ["claude-sonnet-4", "claude-opus-4"]
        .iter()
        .any(|prefix| model.contains(prefix))
        && !["claude-opus-4-7", "claude-opus-4-8"]
            .iter()
            .any(|prefix| model.contains(prefix))
    {
        return &[];
    }
    &[Low, Medium, High, Xhigh, Max]
}

/// Discover CLI-supported flags without starting a session or touching user settings.
pub(super) async fn permission_modes(
    binary: &str,
) -> Result<Vec<latte_work_protocol::AgentPermissionMode>> {
    let (success, help, _) = permission_output(binary, &["--help"]).await?;
    if !success {
        bail!("Agent 权限能力读取失败");
    }
    if let Ok(modes) = modes_from_help(&help) {
        return Ok(modes);
    }
    // Modern Claude hides some supported flags from --help. An invalid value
    // makes its argument parser disclose the enum before help/session startup.
    // Never infer support from an exit code or substitute a permission mode.
    let flag = format!("--permission-mode={PERMISSION_PROBE}");
    let (success, _, diagnostic) = permission_output(binary, &[&flag, "--help"]).await?;
    if success {
        bail!("无法确认此 CLI 支持的权限模式");
    }
    modes_from_diagnostic(&diagnostic)
}

const PERMISSION_PROBE: &str = "latteWorkPermissionProbe";

async fn permission_output(binary: &str, args: &[&str]) -> Result<(bool, String, String)> {
    use tokio::io::AsyncReadExt;
    const LIMIT: u64 = 128 * 1024;
    let mut child = crate::agent_environment::command(binary)
        .args(args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("无法读取 Agent 权限选项")?;
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let mut stdout = child
            .stdout
            .take()
            .context("Agent 未提供帮助输出")?
            .take(LIMIT + 1);
        let mut stderr = child
            .stderr
            .take()
            .context("Agent 未提供参数诊断")?
            .take(LIMIT + 1);
        let (mut bytes, mut diagnostic) = (Vec::new(), Vec::new());
        tokio::try_join!(
            stdout.read_to_end(&mut bytes),
            stderr.read_to_end(&mut diagnostic)
        )?;
        if bytes.len() + diagnostic.len() > LIMIT as usize {
            bail!("Agent 帮助输出超出限制");
        }
        Ok((
            child.wait().await?.success(),
            String::from_utf8(bytes)?,
            String::from_utf8(diagnostic)?,
        ))
    })
    .await
    .context("读取 Agent 权限选项超时")
    .and_then(|result| result);
    // Also reap on oversized output, protocol errors and timeout.
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result
}

fn modes_from_help(help: &str) -> Result<Vec<latte_work_protocol::AgentPermissionMode>> {
    let option = help
        .split("--permission-mode")
        .nth(1)
        .and_then(|part| part.split("\n  --").next())
        .context("此 CLI 未公布可设置的权限模式，请更新 Agent")?;
    let choices = option
        .split("choices:")
        .nth(1)
        .and_then(|part| part.split(')').next())
        .context("此 CLI 未公布权限模式列表")?;
    modes_from_choices(choices)
}

fn modes_from_diagnostic(
    diagnostic: &str,
) -> Result<Vec<latte_work_protocol::AgentPermissionMode>> {
    let choices = diagnostic
        .lines()
        .find(|line| {
            line.contains("--permission-mode <mode>")
                && line.contains(PERMISSION_PROBE)
                && line.contains("is invalid.")
        })
        .and_then(|line| line.split("Allowed choices are ").nth(1))
        .context("无法确认此 CLI 支持的权限模式；请检查 Agent 安装或稍后重试")?;
    modes_from_choices(choices.trim_end_matches('.'))
}

fn modes_from_choices(choices: &str) -> Result<Vec<latte_work_protocol::AgentPermissionMode>> {
    let ids: Vec<_> = choices
        .split(',')
        .map(|value| value.trim().trim_matches('"').trim_matches(char::from(39)))
        .collect();
    let manual = if ids.contains(&"manual") {
        "manual"
    } else {
        "default"
    };
    let known = [
        (
            manual,
            "请求批准",
            "按 Agent 的权限规则，对需要确认的操作请求批准。",
            false,
        ),
        (
            "acceptEdits",
            "自动接受编辑",
            "自动批准文件编辑；其他操作仍按 Agent 规则确认。",
            false,
        ),
        (
            "plan",
            "计划模式",
            "分析和规划任务，不直接修改项目文件。",
            false,
        ),
        (
            "auto",
            "自动判断",
            "由 Agent 的安全检查决定是否批准；需账号和策略支持。",
            false,
        ),
        (
            "dontAsk",
            "不询问",
            "只执行已获许可的操作，拒绝其他需要批准的操作。",
            false,
        ),
        (
            "bypassPermissions",
            "完全访问权限",
            "跳过常规权限确认，可执行命令和修改文件；仍受主机策略约束。",
            true,
        ),
    ];
    let modes: Vec<_> = known
        .into_iter()
        .filter(|(id, ..)| ids.contains(id))
        .map(
            |(id, label, description, elevated)| latte_work_protocol::AgentPermissionMode {
                id: id.into(),
                label: label.into(),
                description: description.into(),
                elevated,
            },
        )
        .collect();
    if modes.is_empty() {
        bail!("此 CLI 没有已适配的权限模式");
    }
    Ok(modes)
}

// JSON counters must fit JavaScript's exact integer range on the shared wire.
fn count(value: &Value, key: &str) -> Option<f64> {
    value[key]
        .as_u64()
        .filter(|v| *v <= 9_007_199_254_740_991)
        .map(|v| v as f64)
}

#[derive(Default)]
pub struct Claude {
    initial_title: Option<String>,
    native_session_id: Option<String>,
    renaming: Option<(String, String)>,
    rename_result_title: Option<String>,
    streamed: bool,
    streamed_children: HashSet<String>,
    child_stream_overflow: bool,
    emitted_text: bool,
    context: Option<ContextUsage>,
    accounting_model: Option<String>,
    progress: Option<ExecutionPhase>,
    stream_model: Option<String>,
    stream_usage: Value,
    live_totals: [f64; 4],
    live_steps: f64,
    phase: Phase,
    commands: HashSet<String>,
    tasks: HashSet<String>,
    agent_tasks: HashMap<String, Option<String>>,
    foreground_agents: HashSet<String>,
    session_state: Option<String>,
    delegated: bool,
    last_result: Option<String>,
}
// Gateways may return a provider-prefixed model while CLI accounting uses its
// bare name plus a context suffix. Only match that same name, never any model.
fn model_key(model: &str) -> &str {
    model
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .split('[')
        .next()
        .unwrap_or(model)
}
fn model_usage<'a>(models: &'a Value, model: &str) -> Option<&'a Value> {
    if let Some(exact) = models.get(model) {
        return Some(exact);
    }
    let mut matching = models
        .as_object()?
        .iter()
        .filter(|(name, _)| model_key(name) == model_key(model));
    let (_, usage) = matching.next()?;
    matching.next().is_none().then_some(usage)
}
impl Claude {
    fn set_accounting_model(&mut self, model: Option<String>) {
        if self.accounting_model != model
            && let Some(context) = &mut self.context
        {
            context.window_tokens = None;
        }
        self.accounting_model = model;
    }
    fn update_context(&mut self, model: &str, usage: &Value) {
        if let Some(input) = count(usage, "input_tokens") {
            let used = input
                + count(usage, "cache_read_input_tokens").unwrap_or(0.0)
                + count(usage, "cache_creation_input_tokens").unwrap_or(0.0);
            // Some gateways emit zero placeholders until message_delta.
            if used > 0.0 {
                self.context = Some(ContextUsage {
                    model: model.into(),
                    used_tokens: used,
                    window_tokens: self
                        .context
                        .as_ref()
                        .filter(|c| c.model == model)
                        .and_then(|c| c.window_tokens),
                });
            }
        }
    }
}
#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Initializing(String),
    Opening,
    Discovering,
    Running,
    BetweenTurns,
    Finished,
}

pub(super) async fn discover() -> (String, latte_work_protocol::AgentInfo) {
    let binary = binary();
    let probe = tokio::time::timeout(
        Duration::from_secs(5),
        crate::agent_environment::command(&binary)
            .arg("--version")
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let (available, detail) = match probe {
        Ok(Ok(result)) if result.status.success() => (
            true,
            String::from_utf8_lossy(&result.stdout).trim().to_owned(),
        ),
        _ => (
            false,
            "未找到 Claude Code；在此 Host 安装并登录后重启 Server。".into(),
        ),
    };
    (
        binary,
        latte_work_protocol::AgentInfo {
            id: "claude".into(),
            name: "Claude Code".into(),
            available,
            provider_protocols: super::provider_protocols("claude").unwrap().to_vec(),
            detail,
        },
    )
}
fn binary() -> String {
    if let Ok(value) = std::env::var("LATTE_WORK_CLAUDE") {
        return value;
    }
    if let Some(home) = std::env::var_os("HOME") {
        let path = PathBuf::from(home).join(".local/bin/claude");
        if path.is_file() {
            return path.to_string_lossy().into_owned();
        }
    }
    "claude".into()
}
fn write_action(value: Value) -> Result<Action> {
    let mut bytes = serde_json::to_vec(&value)?;
    bytes.push(b'\n');
    Ok(Action::Write(bytes))
}

impl AgentAdapter for Claude {
    fn set_initial_title(&mut self, title: Option<String>) {
        self.initial_title = title;
    }
    fn has_background_work(&self) -> bool {
        !self.tasks.is_empty() || !self.foreground_agents.is_empty()
    }
    fn knows_subagent(&self, native_id: &str) -> bool {
        self.agent_tasks.contains_key(native_id)
    }
    fn can_reconfigure(&self) -> bool {
        matches!(self.phase, Phase::BetweenTurns)
            && self.renaming.is_none()
            && self.tasks.is_empty()
            && self.foreground_agents.is_empty()
            && (self.session_state.as_deref() == Some("idle")
                || (self.session_state.is_none() && !self.delegated))
    }
    fn command(
        &mut self,
        binary: &str,
        cwd: &str,
        resume: Option<&str>,
        config: &LaunchConfig,
    ) -> Result<AgentCommand> {
        self.native_session_id = resume.map(str::to_owned);
        self.set_accounting_model(
            config
                .model
                .clone()
                .or_else(|| config.provider.as_ref().map(|p| p.metadata.model.clone())),
        );
        let mut command = crate::agent_environment::command(binary);
        if let Some(title) = &self.initial_title {
            command.arg(format!("--name={title}"));
        }
        command.args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
        ]);
        if matches!(self.phase, Phase::Discovering) {
            command.arg("--no-session-persistence");
        }
        if let Some(id) = resume {
            command.arg(format!("--resume={id}"));
        }
        command
            .current_dir(cwd)
            .env_remove("CLAUDECODE")
            .env("CLAUDE_CODE_SDK_READS_SESSION_STATE", "1");
        if let Some(mode) = &config.permission_mode {
            command.arg(format!("--permission-mode={mode}"));
        }
        if let Some(level) = config.effort {
            command.env("CLAUDE_CODE_EFFORT_LEVEL", level.as_str());
            command.arg(format!("--effort={}", level.as_str()));
        }
        let mut settings = if let Some(provider) = &config.provider {
            let metadata = &provider.metadata;
            let model = config.model.as_deref().unwrap_or(&metadata.model);
            let mut env = json!({
                "ANTHROPIC_BASE_URL": metadata.base_url,
                "ANTHROPIC_MODEL": model,
                "ANTHROPIC_DEFAULT_MODEL": model,
                "ANTHROPIC_DEFAULT_HAIKU_MODEL": model,
                "ANTHROPIC_DEFAULT_SONNET_MODEL": model,
                "ANTHROPIC_DEFAULT_OPUS_MODEL": model,
                "ANTHROPIC_DEFAULT_FABLE_MODEL": model,
                "ANTHROPIC_SMALL_FAST_MODEL": model,
                "CLAUDE_CODE_SUBAGENT_MODEL": model,
                "ANTHROPIC_AUTH_TOKEN": "",
                "ANTHROPIC_API_KEY": "",
                "CLAUDE_CODE_OAUTH_TOKEN": "",
                "ANTHROPIC_CUSTOM_HEADERS": "",
                "CLAUDE_CODE_USE_BEDROCK": "",
                "CLAUDE_CODE_USE_VERTEX": "",
                "CLAUDE_CODE_USE_FOUNDRY": ""
            });
            if metadata.protocol != latte_work_protocol::ProviderProtocol::AnthropicMessages {
                bail!("Claude Code 只支持 Anthropic Messages Provider");
            }
            let (key, credential) = match metadata.auth {
                latte_work_protocol::ProviderAuth::Bearer => {
                    ("ANTHROPIC_AUTH_TOKEN", provider.credential.as_str())
                }
                latte_work_protocol::ProviderAuth::ApiKey => {
                    ("ANTHROPIC_API_KEY", provider.credential.as_str())
                }
                // Claude requires a nonempty credential even for unauthenticated gateways.
                latte_work_protocol::ProviderAuth::None => {
                    ("ANTHROPIC_AUTH_TOKEN", "latte-work-no-auth")
                }
            };
            env[key] = json!(credential);
            for (key, value) in env.as_object().expect("env object") {
                command.env(key, value.as_str().expect("env string"));
            }
            command.arg(format!("--model={model}"));
            json!({"env":env,"model":model,"apiKeyHelper":""})
        } else {
            if let Some(model) = &config.model {
                command.arg(format!("--model={model}"));
            } else if resume.is_some() {
                command.arg("--model=default");
            }
            json!({"env":{}})
        };
        if let Some(level) = config.effort {
            settings["env"]["CLAUDE_CODE_EFFORT_LEVEL"] = json!(level.as_str());
        }
        // An absent override leaves native environment/settings resolution intact.
        // Settings env overrides inherited env. Keep turn-specific effort and
        // credentials in a private file; never change the user's CLI settings.
        let mut file = tempfile::Builder::new()
            .prefix("claude-launch-")
            .suffix(".json")
            .tempfile_in(&config.settings_dir)?;
        file.write_all(&serde_json::to_vec(&settings)?)?;
        file.flush()?;
        command.arg("--settings").arg(file.path());
        Ok(AgentCommand {
            command,
            _settings: Some(file),
        })
    }
    fn advance(&mut self, input: Input<'_>) -> Result<Vec<Action>> {
        match input {
            Input::RenameTitle { id, title } => {
                if !matches!(self.phase, Phase::Running | Phase::BetweenTurns) {
                    bail!("原生会话尚未就绪，标题未修改");
                }
                if self.renaming.is_some() {
                    bail!("原生标题修改结果待确认，请稍后重试");
                }
                self.renaming = Some((id.into(), title.into()));
                self.rename_result_title = None;
                Ok(vec![write_action(
                    json!({"type":"control_request","request_id":id,"request":{"subtype":"rename_session","title":title,"source":"host"}}),
                )?])
            }
            Input::Open => {
                if !matches!(self.phase, Phase::Idle) {
                    bail!("Claude 已经启动");
                }
                self.phase = Phase::Opening;
                Ok(vec![write_action(self.initialize())?])
            }
            Input::Interrupt => {
                if !matches!(self.phase, Phase::Running) {
                    return Ok(vec![Action::Interrupted]);
                }
                Ok(vec![write_action(
                    json!({"type":"control_request","request_id":"latte-interrupt","request":{"subtype":"interrupt"}}),
                )?])
            }
            Input::DiscoverCommands => {
                if !matches!(self.phase, Phase::Idle) {
                    bail!("Claude 已经启动");
                }
                self.phase = Phase::Discovering;
                Ok(vec![write_action(self.initialize())?])
            }

            Input::Start { prompt } => {
                if matches!(self.phase, Phase::BetweenTurns) {
                    self.check_command(prompt)?;
                    self.reset_turn();
                    self.phase = Phase::Running;
                    return Ok(vec![write_action(self.prompt(prompt))?]);
                }
                if !matches!(self.phase, Phase::Idle) {
                    bail!("Claude 本轮已经启动");
                }
                self.phase = Phase::Initializing(prompt.to_owned());
                Ok(vec![write_action(self.initialize())?])
            }
            Input::Message(line) => {
                if matches!(self.phase, Phase::Idle | Phase::Finished) {
                    bail!("Claude 本轮未启动或已结束");
                }
                self.decode(serde_json::from_str(line).context("Claude 返回无效 JSON")?)
            }
            Input::QuestionAnswer {
                id,
                mut input,
                questions,
                answers,
            } => {
                if !matches!(self.phase, Phase::Running | Phase::BetweenTurns) {
                    bail!("Claude 当前不可处理回答");
                }
                let allow = answers.is_some();
                if let Some(answers) = answers {
                    crate::questions::validate(&questions, &answers)?;
                    let mut values = serde_json::Map::new();
                    for (question, native) in questions
                        .iter()
                        .zip(input["questions"].as_array().context("问题参数不完整")?)
                    {
                        let answer = answers
                            .iter()
                            .find(|a| a.question_id == question.id)
                            .context("问题没有回答")?;
                        let mut selections: Vec<_> = answer
                            .option_indices
                            .iter()
                            .map(|i| question.options[*i as usize].label.clone())
                            .collect();
                        if let Some(text) = &answer.text
                            && !text.trim().is_empty()
                        {
                            selections.push(text.trim().into());
                        }
                        values.insert(
                            native["question"].as_str().context("问题文本缺失")?.into(),
                            json!(selections.join(", ")),
                        );
                    }
                    input["answers"] = Value::Object(values);
                }
                let response = if allow {
                    self.approval(id, input, true)
                } else {
                    json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":{"behavior":"deny","message":"用户跳过了本次问题，请勿假定用户已回答"}}})
                };
                Ok(vec![write_action(response)?])
            }
            Input::Approval { id, input, allow } => {
                if !matches!(self.phase, Phase::Running | Phase::BetweenTurns) {
                    bail!("Claude 当前不可处理审批");
                }
                Ok(vec![write_action(self.approval(id, input, allow))?])
            }
        }
    }
}
fn parse_questions(input: &Value) -> Result<Vec<latte_work_protocol::UserQuestion>> {
    use latte_work_protocol::{QuestionOption, UserQuestion};
    let rows = input["questions"].as_array().context("问题参数缺失")?;
    if rows.is_empty() || rows.len() > 4 {
        bail!("问题数量无效");
    }
    let mut seen = std::collections::HashSet::new();
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let text = row["question"]
                .as_str()
                .filter(|s| !s.trim().is_empty() && s.len() <= 32 * 1024)
                .context("问题文本无效")?;
            if !seen.insert(text) {
                bail!("问题文本重复");
            }
            let header = row["header"].as_str().unwrap_or("");
            if header.len() > 256 {
                bail!("问题标题过长");
            }
            let options = row["options"]
                .as_array()
                .filter(|a| !a.is_empty() && a.len() <= 8)
                .context("问题选项无效")?;
            let options = options
                .iter()
                .map(|option| {
                    let label = option["label"]
                        .as_str()
                        .filter(|s| !s.trim().is_empty() && s.len() <= 1024)
                        .context("选项标签无效")?;
                    let description = option["description"].as_str().unwrap_or("");
                    if description.len() > 16 * 1024 {
                        bail!("选项说明过长");
                    }
                    Ok(QuestionOption {
                        label: label.into(),
                        description: description.into(),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(UserQuestion {
                id: format!("q{index}"),
                header: header.into(),
                text: text.into(),
                options,
                multi_select: row["multiSelect"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}
impl Claude {
    fn reset_turn(&mut self) {
        if !self.has_background_work() && self.streamed_children.is_empty() {
            self.child_stream_overflow = false;
        }
        self.streamed = false;
        self.emitted_text = false;
        self.progress = None;
        self.stream_model = None;
        self.stream_usage = Value::Null;
        self.live_totals = [0.0; 4];
        self.live_steps = 0.0;
    }
    fn check_command(&self, prompt: &str) -> Result<()> {
        if let Some(name) = prompt
            .trim_start()
            .strip_prefix('/')
            .and_then(|s| s.split_whitespace().next())
            && !self.commands.contains(name)
        {
            bail!("当前 Claude 不支持命令 /{name}；输入 / 查看可用命令");
        }
        Ok(())
    }
    fn initialize(&self) -> Value {
        json!({"type":"control_request","request_id":"latte-init","request":{"subtype":"initialize"}})
    }
    fn prompt(&self, text: &str) -> Value {
        json!({"type":"user","message":{"role":"user","content":text},"parent_tool_use_id":null,"session_id":""})
    }
    fn approval(&self, id: &str, input: Value, allow: bool) -> Value {
        let response = if allow {
            json!({"behavior":"allow","updatedInput":input})
        } else {
            json!({"behavior":"deny","message":"用户拒绝了本次工具调用"})
        };
        json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":response}})
    }
    fn child_message(&mut self, m: &Value, parent: &str) -> Result<Vec<Action>> {
        let native = self
            .agent_tasks
            .iter()
            .find(|(_, tool)| tool.as_deref() == Some(parent))
            .map(|(id, _)| id.clone());
        let known = native.is_some() || self.foreground_agents.contains(parent);
        let capture = |event| Action::SubagentEvent {
            tool_use_id: parent.into(),
            event,
        };
        let mut output = Vec::new();
        match m["type"].as_str() {
            Some("stream_event") if known => {
                let event = &m["event"];
                if event["type"] == "content_block_delta"
                    && event["delta"]["type"] == "text_delta"
                    && let Some(text) = event["delta"]["text"].as_str()
                {
                    if !self.streamed_children.contains(parent) {
                        if self.streamed_children.len() >= 512 {
                            self.child_stream_overflow = true;
                        }
                        if self.child_stream_overflow {
                            // Keep the parent alive and wait for the complete
                            // assistant message/native history. Do not resume
                            // halfway through a skipped stream after a slot frees.
                            return Ok(output);
                        }
                    }
                    self.streamed_children.insert(parent.into());
                    output.push(capture(EventKind::Text { text: text.into() }));
                }
            }
            Some("assistant") => {
                let streamed = self.streamed_children.remove(parent);
                if let Some(blocks) = m["message"]["content"].as_array() {
                    for block in blocks {
                        match block["type"].as_str() {
                            Some("text") if known && !streamed => {
                                if let Some(text) = block["text"].as_str() {
                                    output.push(capture(EventKind::Text { text: text.into() }));
                                }
                            }
                            Some("tool_use") => {
                                if let (Some(id), Some(name)) =
                                    (block["id"].as_str(), block["name"].as_str())
                                {
                                    output.push(Action::SourceUsed {
                                        id: format!("{parent}:{id}"),
                                        name: name.into(),
                                    });
                                    if known {
                                        output.push(subagent_action(
                                            native.as_deref().unwrap_or(&format!("tool:{parent}")),
                                            Some(parent.into()),
                                            &json!({"last_tool_name":name}),
                                            None,
                                        ));
                                        output.push(capture(EventKind::Tool {
                                            id: id.into(),
                                            name: name.into(),
                                            input: block["input"].clone(),
                                        }));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Some("user") if known => {
                if let Some(blocks) = m["message"]["content"].as_array() {
                    for block in blocks {
                        if block["type"] == "tool_result"
                            && let Some(id) = block["tool_use_id"].as_str()
                        {
                            output.push(capture(EventKind::ToolResult {
                                id: id.into(),
                                content: block["content"].clone(),
                                is_error: block["is_error"].as_bool().unwrap_or(false),
                            }));
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(output)
    }
    fn decode(&mut self, m: Value) -> Result<Vec<Action>> {
        let mut output = Vec::new();
        if let Some(parent) = m["parent_tool_use_id"].as_str()
            && matches!(
                m["type"].as_str(),
                Some("assistant" | "stream_event" | "user")
            )
        {
            return self.child_message(&m, parent);
        }
        if matches!(self.phase, Phase::BetweenTurns)
            && matches!(m["type"].as_str(), Some("assistant" | "stream_event"))
            && m["parent_tool_use_id"].is_null()
        {
            self.reset_turn();
            self.phase = Phase::Running;
            output.push(Action::TurnStarted);
        }
        if m["type"] == "control_response"
            && self
                .renaming
                .as_ref()
                .is_some_and(|(id, _)| m["response"]["request_id"] == *id)
        {
            let (_, requested) = self.renaming.take().expect("checked rename");
            let result = if m["response"]["subtype"] == "success" {
                Ok(self.rename_result_title.take().unwrap_or(requested))
            } else {
                Err(format!(
                    "原生会话标题修改失败：{}",
                    m["response"]["error"]
                        .as_str()
                        .unwrap_or("当前 Agent 不支持此操作")
                ))
            };
            return Ok(vec![Action::TitleRenamed(result)]);
        }
        if m["type"] == "system"
            && m["subtype"] == "session_title_changed"
            && self.native_session_id.as_ref().is_some_and(|id| {
                m["session_id"]
                    .as_str()
                    .is_some_and(|incoming| incoming != id)
            })
        {
            return Ok(output);
        }
        if m["type"] == "system"
            && m["subtype"] == "session_title_changed"
            && m["parent_tool_use_id"].is_null()
        {
            if let Some(title) = bounded_field(&m, "title", 800) {
                if self.renaming.is_some() {
                    self.rename_result_title = Some(title);
                } else {
                    output.push(Action::Title(title));
                }
            }
            return Ok(output);
        }
        match m["type"].as_str().unwrap_or_default() {
            "control_response" if m["response"]["request_id"] == "latte-interrupt" => {
                if m["response"]["subtype"] != "success" {
                    bail!("Claude 停止请求失败");
                }
                output.push(Action::Interrupted);
            }
            "control_response" if m["response"]["request_id"] == "latte-init" => {
                if m["response"]["subtype"] != "success" {
                    bail!("Claude 初始化失败: {}", m["response"]["error"]);
                }
                if matches!(self.phase, Phase::Discovering) {
                    let commands = parse_commands(&m["response"]["response"]["commands"])?;
                    self.phase = Phase::Finished;
                    return Ok(vec![Action::Commands(commands)]);
                }
                if matches!(self.phase, Phase::Initializing(_) | Phase::Opening) {
                    let phase = std::mem::replace(&mut self.phase, Phase::Running);
                    let prompt = match phase {
                        Phase::Initializing(prompt) => Some(prompt),
                        Phase::Opening => None,
                        _ => unreachable!(),
                    };
                    if prompt
                        .as_deref()
                        .is_some_and(|p| p.trim_start().starts_with('/'))
                    {
                        let commands = parse_commands(&m["response"]["response"]["commands"])?;
                        self.commands.extend(commands.into_iter().map(|c| c.name));
                    }
                    // Cache the catalog for subsequent turns; old CLIs without a
                    // catalog can still accept ordinary prompts, never unknown commands.
                    if let Some(commands) = m["response"]["response"]["commands"].as_array() {
                        for command in commands {
                            if let Some(name) = command["name"].as_str() {
                                self.commands.insert(name.into());
                            }
                            if let Some(aliases) = command["aliases"].as_array() {
                                self.commands.extend(
                                    aliases.iter().filter_map(|a| a.as_str().map(str::to_owned)),
                                );
                            }
                        }
                    }
                    output.push(Action::Ready);
                    if let Some(prompt) = prompt {
                        self.check_command(&prompt)?;
                        output.push(write_action(self.prompt(&prompt))?);
                    } else {
                        self.phase = Phase::BetweenTurns;
                    }
                }
            }
            "system" if m["subtype"] == "init" && m["parent_tool_use_id"].is_null() => {
                if let Some(id) = m["session_id"].as_str() {
                    self.native_session_id = Some(id.into());
                    output.push(Action::NativeSession(id.into()));
                }
                if let Some(model) = bounded_field(&m, "model", 256) {
                    self.set_accounting_model(Some(model));
                }
            }
            "system" if m["subtype"] == "session_state_changed" => {
                self.session_state = m["state"].as_str().map(str::to_owned);
            }
            "system" if m["subtype"] == "task_started" => {
                let id = native_task_id(&m)?;
                if self.tasks.len() >= 256 && !self.tasks.contains(id) {
                    bail!("Claude 后台任务数量超出限制");
                }
                self.tasks.insert(id.into());
                let tool = bounded_field(&m, "tool_use_id", 256);
                if m["task_type"] == "local_agent"
                    || tool
                        .as_ref()
                        .is_some_and(|id| self.foreground_agents.contains(id))
                {
                    if let Some(tool) = &tool {
                        self.foreground_agents.remove(tool);
                    }
                    if self.agent_tasks.len() >= 256 && !self.agent_tasks.contains_key(id) {
                        if let Some(old) = self
                            .agent_tasks
                            .keys()
                            .find(|key| !self.tasks.contains(*key))
                            .cloned()
                        {
                            self.agent_tasks.remove(&old);
                        } else {
                            bail!("Claude 子智能体数量超出限制");
                        }
                    }
                    self.agent_tasks.insert(id.into(), tool.clone());
                    output.push(subagent_action(id, tool, &m, Some(SubagentStatus::Running)));
                }
            }
            "system"
                if matches!(
                    m["subtype"].as_str(),
                    Some("task_notification" | "task_updated" | "task_progress")
                ) =>
            {
                let id = native_task_id(&m)?;
                let fields = if m["subtype"] == "task_updated" && m["patch"].is_object() {
                    &m["patch"]
                } else {
                    &m
                };
                let raw_status = fields["status"].as_str().or(m["status"].as_str());
                let status = match raw_status {
                    Some("running" | "pending") => Some(SubagentStatus::Running),
                    Some("paused") => Some(SubagentStatus::Paused),
                    Some("completed") => Some(SubagentStatus::Completed),
                    Some("failed") => Some(SubagentStatus::Failed),
                    Some("stopped" | "killed") => Some(SubagentStatus::Stopped),
                    _ => None,
                };
                if let Some(tool) = self.agent_tasks.get(id) {
                    let mut action = subagent_action(id, tool.clone(), fields, status);
                    // Native task_progress.description describes the current
                    // activity (e.g. "Reading ..."), not the delegated task title.
                    if m["subtype"] == "task_progress"
                        && let Action::Event(EventKind::Subagent { update }) = &mut action
                    {
                        update.title = None;
                    }
                    output.push(action);
                }
                if matches!(
                    raw_status,
                    Some("completed" | "failed" | "stopped" | "killed")
                ) {
                    self.tasks.remove(id);
                }
            }
            "system" if m["subtype"] == "compact_boundary" => {
                self.context = None;
                output.push(Action::Event(EventKind::Usage {
                    context: None,
                    totals: None,
                }));
            }
            "conversation_reset" => {
                self.context = None;
                output.push(Action::Event(EventKind::Usage {
                    context: None,
                    totals: None,
                }));
            }
            "control_request" => {
                let id = m["request_id"].as_str().unwrap_or_default().to_owned();
                let request = &m["request"];
                if request["subtype"] == "can_use_tool" {
                    if id.is_empty() || !request["input"].is_object() {
                        bail!("无效的 Claude 审批请求");
                    }
                    if request["tool_name"] == "AskUserQuestion" {
                        match parse_questions(&request["input"]) {
                            Ok(questions) => output.push(Action::Question { id, tool_use_id: request["tool_use_id"].as_str().map(str::to_owned), input: request["input"].clone(), questions }),
                            Err(_) => output.push(write_action(json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":{"behavior":"deny","message":"Latte Work 无法读取问题参数；请重新提出有效的问题"}}}))?),
                        }
                    } else {
                        output.push(Action::Approval {
                            id,
                            tool_use_id: request["tool_use_id"].as_str().map(str::to_owned),
                            tool: request["tool_name"].as_str().unwrap_or("Tool").into(),
                            input: request["input"].clone(),
                        });
                    }
                } else {
                    output.push(write_action(json!({"type":"control_response","response":{"subtype":"error","request_id":id,"error":"Unsupported control request in Latte Work v0.1"}}))?);
                }
            }
            "stream_event" => {
                let event = &m["event"];
                if m["parent_tool_use_id"].is_null() {
                    match event["type"].as_str() {
                        Some("message_start") => {
                            self.stream_model =
                                event["message"]["model"].as_str().map(str::to_owned);
                            self.stream_usage = event["message"]["usage"].clone();
                        }
                        Some("message_delta") => {
                            if let Some(model) = self.stream_model.clone()
                                && let Some(delta) = event["usage"].as_object()
                            {
                                if !self.stream_usage.is_object() {
                                    self.stream_usage = json!({});
                                }
                                self.stream_usage
                                    .as_object_mut()
                                    .unwrap()
                                    .extend(delta.clone());
                                let usage = self.stream_usage.clone();
                                self.update_context(&model, &usage);
                                let keys = [
                                    "input_tokens",
                                    "cache_read_input_tokens",
                                    "cache_creation_input_tokens",
                                    "output_tokens",
                                ];
                                let current = keys.map(|key| count(&usage, key).unwrap_or(0.0));
                                if current[0] + current[1] + current[2] > 0.0 {
                                    output.push(Action::Event(EventKind::Usage {
                                        context: self.context.clone(),
                                        totals: Some(TurnUsage {
                                            input_tokens: Some(self.live_totals[0] + current[0]),
                                            cache_read_tokens: Some(
                                                self.live_totals[1] + current[1],
                                            ),
                                            cache_write_tokens: Some(
                                                self.live_totals[2] + current[2],
                                            ),
                                            output_tokens: count(&usage, "output_tokens")
                                                .map(|v| self.live_totals[3] + v),
                                            model_time_ms: None,
                                            steps: Some(self.live_steps + 1.0),
                                        }),
                                    }));
                                }
                            }
                        }
                        Some("message_stop") if self.stream_model.take().is_some() => {
                            for (i, key) in [
                                "input_tokens",
                                "cache_read_input_tokens",
                                "cache_creation_input_tokens",
                                "output_tokens",
                            ]
                            .iter()
                            .enumerate()
                            {
                                self.live_totals[i] +=
                                    count(&self.stream_usage, key).unwrap_or(0.0);
                            }
                            self.live_steps += 1.0;
                        }
                        _ => {}
                    }
                    let phase = match event["type"].as_str() {
                        Some("content_block_start") => {
                            match event["content_block"]["type"].as_str() {
                                Some("thinking" | "redacted_thinking") => {
                                    Some(ExecutionPhase::Thinking)
                                }
                                Some("text") => Some(ExecutionPhase::Replying),
                                _ => Some(ExecutionPhase::Waiting),
                            }
                        }
                        Some("content_block_delta") => match event["delta"]["type"].as_str() {
                            Some("thinking_delta") => Some(ExecutionPhase::Thinking),
                            // Existing Text events already signal reply streaming.
                            _ => None,
                        },
                        Some("message_start" | "message_stop" | "content_block_stop") => {
                            Some(ExecutionPhase::Waiting)
                        }
                        _ => None,
                    };
                    if let Some(phase) = phase
                        && self.progress != Some(phase)
                    {
                        self.progress = Some(phase);
                        output.push(Action::Event(EventKind::Progress { phase }));
                    }
                }
                if m["parent_tool_use_id"].is_null()
                    && event["type"] == "content_block_delta"
                    && event["delta"]["type"] == "text_delta"
                    && let Some(text) = event["delta"]["text"].as_str()
                {
                    self.progress = Some(ExecutionPhase::Replying);
                    self.streamed = true;
                    self.emitted_text = true;
                    output.push(Action::Event(EventKind::Text { text: text.into() }));
                }
            }
            "assistant" => {
                if m["parent_tool_use_id"].is_null() {
                    let message = &m["message"];
                    let usage = &message["usage"];
                    let previous = self.context.clone();
                    if let Some(model) = message["model"].as_str() {
                        self.update_context(model, usage);
                    }
                    if self.context != previous {
                        output.push(Action::Event(EventKind::Usage {
                            context: self.context.clone(),
                            totals: None,
                        }));
                    }
                }
                if let Some(blocks) = m["message"]["content"].as_array() {
                    for block in blocks {
                        match block["type"].as_str().unwrap_or_default() {
                            "text" if !self.streamed && m["parent_tool_use_id"].is_null() => {
                                if let Some(text) = block["text"].as_str() {
                                    self.emitted_text = true;
                                    output
                                        .push(Action::Event(EventKind::Text { text: text.into() }));
                                }
                            }
                            "tool_use" => {
                                if matches!(block["name"].as_str(), Some("Agent" | "Task")) {
                                    self.delegated = true;
                                    if m["parent_tool_use_id"].is_null() {
                                        let id = block["id"]
                                            .as_str()
                                            .filter(|id| !id.is_empty() && id.len() <= 256)
                                            .context("无效的 Claude Agent 工具标识")?;
                                        if self.foreground_agents.len() >= 256
                                            && !self.foreground_agents.contains(id)
                                        {
                                            bail!("Claude 子智能体数量超出限制");
                                        }
                                        self.foreground_agents.insert(id.into());
                                        output.push(subagent_action(
                                            &format!("tool:{id}"),
                                            Some(id.into()),
                                            &block["input"],
                                            Some(SubagentStatus::Running),
                                        ));
                                    }
                                }
                                output.push(Action::Event(EventKind::Tool {
                                    id: block["id"].as_str().unwrap_or_default().into(),
                                    name: block["name"].as_str().unwrap_or("Tool").into(),
                                    input: block["input"].clone(),
                                }));
                            }
                            _ => {}
                        }
                    }
                }
                self.streamed = false;
            }
            "user" => {
                if let Some(blocks) = m["message"]["content"].as_array() {
                    for block in blocks {
                        if block["type"] == "tool_result" && m["parent_tool_use_id"].is_null() {
                            if let Some(id) = block["tool_use_id"].as_str()
                                && self.foreground_agents.remove(id)
                            {
                                let summary = match &block["content"] {
                                    Value::String(text) => text.clone(),
                                    value => value.to_string(),
                                };
                                output.push(subagent_action(
                                    &format!("tool:{id}"),
                                    Some(id.into()),
                                    &json!({"summary":summary}),
                                    Some(if block["is_error"] == true {
                                        SubagentStatus::Failed
                                    } else {
                                        SubagentStatus::Completed
                                    }),
                                ));
                            }
                            output.push(Action::Event(EventKind::ToolResult {
                                id: block["tool_use_id"].as_str().unwrap_or_default().into(),
                                content: block["content"].clone(),
                                is_error: block["is_error"].as_bool().unwrap_or(false),
                            }));
                        }
                    }
                }
            }
            "result" => {
                let failed = m["is_error"].as_bool().unwrap_or(false)
                    || m["subtype"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("error"));
                // Child completions must not finish (or restart) the parent turn.
                if let Some(parent) = m["parent_tool_use_id"].as_str().filter(|id| !id.is_empty()) {
                    self.streamed_children.remove(parent);
                    let native = self
                        .agent_tasks
                        .iter()
                        .find(|(_, tool)| tool.as_deref() == Some(parent))
                        .map(|(id, _)| id.clone());
                    if native.is_some() || self.foreground_agents.remove(parent) {
                        let id = native.unwrap_or_else(|| format!("tool:{parent}"));
                        self.tasks.remove(&id);
                        output.push(subagent_action(
                            &id,
                            Some(parent.into()),
                            &json!({"summary":m["result"]}),
                            Some(if failed {
                                SubagentStatus::Failed
                            } else {
                                SubagentStatus::Completed
                            }),
                        ));
                    }
                    return Ok(output);
                }
                if matches!(self.phase, Phase::BetweenTurns) {
                    let text = m["result"].as_str().filter(|text| !text.is_empty());
                    // The CLI can deliver trailing result envelopes after a
                    // parent turn has completed. Empty/duplicate success is idle
                    // telemetry, not an initialization failure or another turn.
                    if !failed && (text.is_none() || text == self.last_result.as_deref()) {
                        return Ok(output);
                    }
                    self.reset_turn();
                    self.phase = Phase::Running;
                    output.push(Action::TurnStarted);
                }
                if !matches!(self.phase, Phase::Running) {
                    bail!("Claude 在初始化完成前返回了任务结果");
                }
                self.phase = Phase::BetweenTurns;
                if let Some(context) = &mut self.context {
                    // CLI accounting uses the requested model; gateways may
                    // return an entirely different model name in API messages.
                    // Use the explicit launch/init identity, never another entry
                    // chosen from cumulative (including subagent) billing usage.
                    let models = &m["modelUsage"];
                    if !models.is_null() {
                        context.window_tokens = models
                            .get(&context.model)
                            .or_else(|| {
                                self.accounting_model
                                    .as_deref()
                                    .and_then(|model| models.get(model))
                            })
                            .or_else(|| model_usage(models, &context.model))
                            .or_else(|| {
                                self.accounting_model
                                    .as_deref()
                                    .and_then(|model| model_usage(models, model))
                            })
                            .and_then(|usage| count(usage, "contextWindow"))
                            .filter(|v| *v > 0.0);
                    }
                }
                let usage = &m["usage"];
                if self.context.is_some() || usage.is_object() {
                    output.push(Action::Event(EventKind::Usage {
                        context: self.context.clone(),
                        totals: Some(TurnUsage {
                            input_tokens: count(usage, "input_tokens"),
                            cache_read_tokens: count(usage, "cache_read_input_tokens"),
                            cache_write_tokens: count(usage, "cache_creation_input_tokens"),
                            output_tokens: count(usage, "output_tokens"),
                            model_time_ms: count(&m, "duration_api_ms"),
                            steps: count(&m, "num_turns"),
                        }),
                    }));
                }
                let message = if failed {
                    Some(
                        m["result"]
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| m["errors"].to_string()),
                    )
                } else {
                    None
                };
                // Native commands may only return their output in result, without
                // assistant text. Preserve that output without duplicating a turn.
                if !failed
                    && !self.emitted_text
                    && let Some(text) = m["result"].as_str().filter(|text| !text.is_empty())
                {
                    output.push(Action::Event(EventKind::Text { text: text.into() }));
                }
                output.push(Action::Finished { failed, message });
                self.last_result = m["result"].as_str().map(str::to_owned);
            }
            _ => {}
        }
        Ok(output)
    }
}
fn native_task_id(message: &Value) -> Result<&str> {
    message["task_id"]
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 256)
        .context("无效的 Claude 后台任务标识")
}
fn bounded_field(value: &Value, key: &str, limit: usize) -> Option<String> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(limit).collect())
}
/// Full native history is indexed page by page; page limits never trim fields.
pub(super) fn parse_child_history(
    raw: super::claude_sessions::ChildPageBytes,
    parent: &str,
    child: &str,
    session_id: &str,
    config: &LaunchConfig,
) -> Result<super::ChildHistoryPage> {
    let mut cursor = raw.cursor;
    let mut events = Vec::new();
    let at = crate::store::now();
    for line in raw.bytes.split_inclusive(|b| *b == b'\n') {
        let base = cursor.offset;
        cursor.offset += line.len() as u64;
        let value: Value =
            serde_json::from_slice(line).context("子会话原始记录格式无效，未跳过内容")?;
        if !matches!(value["type"].as_str(), Some("assistant" | "user")) {
            continue;
        }
        if value["sessionId"] != parent || value["agentId"] != child || value["isSidechain"] != true
        {
            bail!("原生子会话记录不属于此任务");
        }
        let Some(blocks) = value["message"]["content"].as_array() else {
            continue;
        };
        if blocks.len() > 2048 || base > 9_007_199_254_740_991 / 4096 {
            bail!("子会话记录标识超出范围，内容未截断");
        }
        for (index, block) in blocks.iter().enumerate() {
            let seq = (base * 4096 + index as u64 * 2) as f64;
            let event = match (value["type"].as_str(), block["type"].as_str()) {
                (Some("assistant"), Some("text")) => block["text"]
                    .as_str()
                    .map(|text| EventKind::Text { text: text.into() }),
                (Some("assistant"), Some("tool_use")) => block["id"]
                    .as_str()
                    .zip(block["name"].as_str())
                    .map(|(id, name)| EventKind::Tool {
                        id: id.into(),
                        name: name.into(),
                        input: block["input"].clone(),
                    }),
                (Some("user"), Some("tool_result")) => {
                    block["tool_use_id"]
                        .as_str()
                        .map(|id| EventKind::ToolResult {
                            id: id.into(),
                            content: block["content"].clone(),
                            is_error: block["is_error"].as_bool().unwrap_or(false),
                        })
                }
                _ => None,
            };
            let Some(event) = event else {
                continue;
            };
            let event = config.redact_event(event)?;
            let mut report = None;
            match &event {
                EventKind::Tool { id, name, input } if name == "SubagentHandback" => {
                    if let Some(text) = input["message"].as_str() {
                        cursor.handbacks.insert(id.clone(), text.into());
                    }
                    if cursor.handbacks.len() > 128
                        || cursor.handbacks.values().map(String::len).sum::<usize>()
                            > latte_work_protocol::MAX_FRAME
                    {
                        bail!("待确认报告超出安全范围，内容未截断");
                    }
                }
                EventKind::ToolResult { id, .. } => {
                    let pending = cursor.handbacks.remove(id);
                    if handback_delivered(block) {
                        report = pending;
                    }
                }
                _ => {}
            }
            events.push(latte_work_protocol::Event {
                seq,
                session_id: session_id.into(),
                at,
                event,
            });
            if let Some(text) = report {
                events.push(latte_work_protocol::Event {
                    seq: seq + 1.0,
                    session_id: session_id.into(),
                    at,
                    event: EventKind::Text { text },
                });
            }
        }
    }
    cursor.has_more = raw.has_more;
    Ok(super::ChildHistoryPage {
        events,
        cursor,
        has_more: raw.has_more,
    })
}

pub(super) fn parse_child_transcript(
    raw: super::claude_sessions::ChildBytes,
    parent: &str,
    child: &str,
    session_id: &str,
    config: &LaunchConfig,
) -> Result<super::ChildTranscript> {
    use std::collections::VecDeque;
    let (mut events, mut bytes, mut offset, mut truncated) =
        (VecDeque::new(), 0usize, raw.start, raw.truncated);
    let at = crate::store::now();
    let mut handbacks = HashMap::<String, String>::new();
    for line in raw.bytes.split_inclusive(|b| *b == b'\n') {
        let base = offset;
        offset += line.len() as u64;
        let value: Value = match serde_json::from_slice(line) {
            Ok(value) => value,
            Err(_) => {
                if line.ends_with(b"\n") {
                    truncated = true;
                }
                continue;
            }
        };
        if !matches!(value["type"].as_str(), Some("assistant" | "user")) {
            continue;
        }
        if value["sessionId"] != parent || value["agentId"] != child || value["isSidechain"] != true
        {
            bail!("原生子会话记录不属于此任务");
        }
        let Some(blocks) = value["message"]["content"].as_array() else {
            continue;
        };
        if base > 9_007_199_254_740_991 / 128 {
            bail!("原生子会话记录过大");
        }
        truncated |= blocks.len() > 128;
        for (index, block) in blocks.iter().take(128).enumerate() {
            // A handback's prose is the report; the later Agent result is only
            // a delivery receipt. Promote it only after the matching native
            // tool result explicitly confirms delivery, before field clipping.
            if value["type"] == "assistant"
                && block["type"] == "tool_use"
                && block["name"] == "SubagentHandback"
                && let (Some(id), Some(message)) =
                    (block["id"].as_str(), block["input"]["message"].as_str())
                && id.len() <= 256
            {
                handbacks.insert(id.into(), message.into());
            }
            let report = if value["type"] == "user" && block["type"] == "tool_result" {
                block["tool_use_id"]
                    .as_str()
                    .and_then(|id| handbacks.remove(id).filter(|_| handback_delivered(block)))
            } else {
                None
            };
            let event = match (value["type"].as_str(), block["type"].as_str()) {
                (Some("assistant"), Some("text")) => block["text"]
                    .as_str()
                    .map(|text| EventKind::Text { text: text.into() }),
                (Some("assistant"), Some("tool_use")) => block["id"]
                    .as_str()
                    .zip(block["name"].as_str())
                    .map(|(id, name)| EventKind::Tool {
                        id: id.into(),
                        name: name.into(),
                        input: block["input"].clone(),
                    }),
                (Some("user"), Some("tool_result")) => {
                    block["tool_use_id"]
                        .as_str()
                        .map(|id| EventKind::ToolResult {
                            id: id.into(),
                            content: block["content"].clone(),
                            is_error: block["is_error"].as_bool().unwrap_or(false),
                        })
                }
                _ => None,
            };
            let Some(event) = event else {
                continue;
            };
            let (event, partial) =
                crate::store::bounded_subagent_event(config.redact_event(event)?)?;
            truncated |= partial;
            let size = serde_json::to_vec(&event)?.len() + 128;
            events.push_back((
                latte_work_protocol::Event {
                    seq: (base * 128 + index as u64) as f64,
                    session_id: session_id.into(),
                    at,
                    event,
                },
                size,
            ));
            bytes += size;
            if let Some(report) = report {
                let EventKind::Text { text } =
                    config.redact_event(EventKind::Text { text: report })?
                else {
                    unreachable!("redaction preserves the event kind")
                };
                let (reports, partial) =
                    report_events(text, (base * 128 + index as u64) as f64, session_id, at);
                truncated |= partial;
                for event in reports {
                    let size = serde_json::to_vec(&event.event)?.len() + 128;
                    events.push_back((event, size));
                    bytes += size;
                }
            }
            while events.len() > 500 || bytes > 256 * 1024 {
                bytes -= events.pop_front().expect("bounded nonempty events").1;
                truncated = true;
            }
        }
    }
    Ok((
        events.into_iter().map(|(event, _)| event).collect(),
        truncated,
    ))
}

fn report_events(mut text: String, seq: f64, session_id: &str, at: f64) -> super::ChildTranscript {
    let truncated = text.len() > 128 * 1024;
    if truncated {
        let mut end = 128 * 1024;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n…（报告已截断）");
    }
    let mut events = Vec::new();
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let mut end = rest.len().min(4 * 1024);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        events.push(latte_work_protocol::Event {
            seq: seq + (events.len() + 1) as f64 / 64.0,
            session_id: session_id.into(),
            at,
            event: EventKind::Text {
                text: rest[..end].into(),
            },
        });
        rest = &rest[end..];
    }
    (events, truncated)
}

/// Older persisted snapshots already passed the owning launch context's
/// redaction. Normalize those in memory, without reopening native files or
/// requiring a process to still be running. New snapshots are idempotent.
pub(super) fn normalize_child_transcript(
    snapshot: super::ChildTranscript,
) -> Result<super::ChildTranscript> {
    use std::collections::VecDeque;
    let (source, mut truncated) = snapshot;
    let mut pending = HashMap::<&str, &str>::new();
    let (mut events, mut bytes) = (VecDeque::new(), 0usize);
    for (index, event) in source.iter().enumerate() {
        let mut report = None;
        match &event.event {
            EventKind::Tool { id, name, input } if name == "SubagentHandback" => {
                if let Some(text) = input["message"].as_str() {
                    pending.insert(id, text);
                }
            }
            EventKind::ToolResult {
                id,
                content,
                is_error,
            } => {
                if let Some(text) = pending.remove(id.as_str())
                    && handback_delivered(&json!({"content":content,"is_error":is_error}))
                {
                    report = Some(text);
                }
            }
            _ => {}
        }
        let mut additions = vec![event.clone()];
        if let Some(text) = report {
            let (reports, partial) =
                report_events(text.into(), event.seq, &event.session_id, event.at);
            // Any following prose may be a delivery receipt, not the report.
            // Only an exact existing report sequence makes promotion redundant.
            let already_present = source.get(index + 1..index + 1 + reports.len())
                .is_some_and(|following| following.iter().zip(&reports).all(|(actual, expected)|
                    matches!((&actual.event, &expected.event), (EventKind::Text { text: a }, EventKind::Text { text: b }) if a == b)
                ));
            if !already_present {
                additions.extend(reports);
            }
            truncated |= partial;
        }
        for event in additions {
            let size = serde_json::to_vec(&event)?.len();
            bytes += size;
            events.push_back((event, size));
        }
        while events.len() > 500 || bytes > 256 * 1024 {
            bytes -= events.pop_front().expect("bounded nonempty events").1;
            truncated = true;
        }
    }
    Ok((
        events.into_iter().map(|(event, _)| event).collect(),
        truncated,
    ))
}

pub(super) fn confirmed_child_report(tool: &EventKind, result: &EventKind) -> Option<String> {
    let EventKind::Tool { id, name, input } = tool else {
        return None;
    };
    let EventKind::ToolResult {
        id: result_id,
        content,
        is_error,
    } = result
    else {
        return None;
    };
    if name != "SubagentHandback"
        || id != result_id
        || !handback_delivered(&json!({"content":content,"is_error":is_error}))
    {
        return None;
    }
    input["message"].as_str().map(str::to_owned)
}

fn handback_delivered(block: &Value) -> bool {
    if block["is_error"].as_bool().unwrap_or(false) {
        return false;
    }
    let success = |text: &str| {
        serde_json::from_str::<Value>(text).is_ok_and(|value| value["success"] == true)
    };
    match &block["content"] {
        Value::String(text) => success(text),
        Value::Array(blocks) => blocks
            .iter()
            .any(|part| part["type"] == "text" && part["text"].as_str().is_some_and(success)),
        _ => false,
    }
}

fn subagent_action(
    id: &str,
    tool_use_id: Option<String>,
    fields: &Value,
    status: Option<SubagentStatus>,
) -> Action {
    Action::Event(EventKind::Subagent {
        update: SubagentUpdate {
            id: id.into(),
            tool_use_id,
            title: bounded_field(fields, "description", 512)
                .or_else(|| bounded_field(fields, "title", 512)),
            status,
            summary: bounded_field(fields, "summary", 2048),
            last_tool: bounded_field(fields, "last_tool_name", 256),
        },
    })
}

fn parse_commands(value: &Value) -> Result<Vec<latte_work_protocol::AgentSlashCommand>> {
    let entries = value
        .as_array()
        .context("此 Claude CLI 未返回命令列表，请更新 CLI 后重试")?;
    if entries.len() > 2000 {
        bail!("Agent 命令列表超出限制");
    }
    let mut names = HashSet::new();
    let mut result = Vec::new();
    for entry in entries {
        let name = entry["name"].as_str().context("无效的 Agent 命令名称")?;
        if name.is_empty()
            || name.len() > 256
            || name
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || c == '/')
        {
            bail!("无效的 Agent 命令名称");
        }
        if !names.insert(name.to_owned()) {
            continue;
        }
        let description = entry["description"].as_str().unwrap_or_default();
        let argument_hint = entry["argumentHint"].as_str().unwrap_or_default();
        result.push(latte_work_protocol::AgentSlashCommand {
            name: name.into(),
            display_name: ["displayName", "display_name", "title"]
                .iter()
                .filter_map(|key| entry[key].as_str())
                .map(str::trim)
                .find(|value| {
                    !value.is_empty()
                        && value.chars().count() <= 80
                        && !value.chars().any(char::is_control)
                })
                .map(str::to_owned),
            description: description.chars().take(2048).collect(),
            argument_hint: argument_hint.chars().take(512).collect(),
            ui_action: matches!(name, "agents" | "tasks" | "list-agents")
                .then_some(latte_work_protocol::AgentCommandUiAction::Subagents),
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_title_events_and_rename_acknowledgements_never_start_a_turn() {
        use super::*;
        let mut adapter = Claude {
            phase: Phase::BetweenTurns,
            ..Default::default()
        };
        let automatic = adapter
            .decode(json!({"type":"system","subtype":"session_title_changed","title":"Generated"}))
            .unwrap();
        assert!(matches!(&automatic[..],[Action::Title(title)] if title=="Generated"));
        let actions = adapter
            .advance(Input::RenameTitle {
                id: "rename-1",
                title: "Manual",
            })
            .unwrap();
        assert!(
            matches!(&actions[..],[Action::Write(bytes)] if String::from_utf8_lossy(bytes).contains("rename_session"))
        );
        assert!(
            adapter
                .decode(json!({"type":"system","subtype":"session_title_changed","title":"Manual"}))
                .unwrap()
                .is_empty()
        );
        let ack=adapter.decode(json!({"type":"control_response","response":{"request_id":"rename-1","subtype":"success"}})).unwrap();
        assert!(matches!(&ack[..],[Action::TitleRenamed(Ok(title))] if title=="Manual"));
        assert!(matches!(adapter.phase, Phase::BetweenTurns));
        adapter
            .advance(Input::RenameTitle {
                id: "rename-2",
                title: "Rejected",
            })
            .unwrap();
        let error=adapter.decode(json!({"type":"control_response","response":{"request_id":"rename-2","subtype":"error","error":"unsupported"}})).unwrap();
        assert!(matches!(&error[..], [Action::TitleRenamed(Err(_))]));
        assert!(matches!(adapter.phase, Phase::BetweenTurns));
    }
    #[test]
    fn background_work_requires_native_tasks_not_a_stale_busy_session_state() {
        use super::*;
        let mut adapter = Claude {
            phase: Phase::BetweenTurns,
            session_state: Some("busy".into()),
            ..Default::default()
        };
        assert!(!adapter.can_reconfigure());
        assert!(!adapter.has_background_work());
        adapter.tasks.insert("child".into());
        assert!(adapter.has_background_work());
        adapter.decode(json!({"type":"system","subtype":"task_notification","task_id":"child","status":"completed"})).unwrap();
        assert!(!adapter.has_background_work());
        adapter.foreground_agents.insert("foreground".into());
        assert!(adapter.has_background_work());
    }
    #[test]
    fn child_and_idle_results_never_fail_or_finish_the_parent() {
        use super::*;
        let mut adapter = Claude {
            phase: Phase::Running,
            ..Default::default()
        };
        adapter.foreground_agents.insert("child-tool".into());
        let child = adapter.decode(json!({"type":"result","parent_tool_use_id":"child-tool","is_error":false,"result":"child result"})).unwrap();
        assert!(child.iter().any(|action| matches!(action, Action::Event(EventKind::Subagent {update}) if update.status == Some(SubagentStatus::Completed))));
        assert!(
            !child
                .iter()
                .any(|action| matches!(action, Action::Finished { .. }))
        );
        assert!(matches!(adapter.phase, Phase::Running));
        adapter
            .decode(json!({"type":"result","is_error":false,"result":"parent result"}))
            .unwrap();
        assert!(
            adapter
                .decode(json!({"type":"result","is_error":false,"result":"parent result"}))
                .unwrap()
                .is_empty()
        );
        assert!(
            adapter
                .decode(json!({"type":"result","is_error":false}))
                .unwrap()
                .is_empty()
        );
        assert!(matches!(adapter.phase, Phase::BetweenTurns));
        let followup = adapter
            .decode(json!({"type":"result","is_error":false,"result":"new autonomous result"}))
            .unwrap();
        assert!(matches!(followup.first(), Some(Action::TurnStarted)));
        assert!(
            followup
                .iter()
                .any(|action| matches!(action, Action::Finished { failed: false, .. }))
        );
        let failure = adapter
            .decode(json!({"type":"result","is_error":true,"result":"API error"}))
            .unwrap();
        assert!(
            failure
                .iter()
                .any(|action| matches!(action, Action::Finished { failed: true, .. }))
        );
        let mut opening = Claude {
            phase: Phase::Opening,
            ..Default::default()
        };
        assert!(
            opening
                .decode(json!({"type":"result","is_error":false}))
                .is_err()
        );
    }
    #[test]
    fn child_output_streams_and_tool_results_never_modify_parent_stream_accounting() {
        use super::*;
        let mut adapter = Claude {
            phase: Phase::BetweenTurns,
            streamed: true,
            ..Default::default()
        };
        adapter.foreground_agents.insert("parent-tool".into());
        let streamed = adapter.decode(json!({"type":"stream_event","parent_tool_use_id":"parent-tool","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"child progress"}}})).unwrap();
        assert!(
            matches!(&streamed[..], [Action::SubagentEvent { event: EventKind::Text { text }, .. }] if text == "child progress")
        );
        let message = adapter.decode(json!({"type":"assistant","parent_tool_use_id":"parent-tool","message":{"content":[{"type":"text","text":"child progress"},{"type":"tool_use","id":"read","name":"Read","input":{"file_path":"child.txt"}}]}})).unwrap();
        assert_eq!(
            message
                .iter()
                .filter(|a| matches!(
                    a,
                    Action::SubagentEvent {
                        event: EventKind::Tool { .. },
                        ..
                    }
                ))
                .count(),
            1
        );
        assert!(!message.iter().any(|a| matches!(
            a,
            Action::SubagentEvent {
                event: EventKind::Text { .. },
                ..
            } | Action::TurnStarted
                | Action::Finished { .. }
        )));
        assert!(adapter.streamed);
        assert!(matches!(adapter.phase, Phase::BetweenTurns));
        let result = adapter.decode(json!({"type":"user","parent_tool_use_id":"parent-tool","message":{"content":[{"type":"tool_result","tool_use_id":"read","content":"native output","is_error":true}]}})).unwrap();
        assert!(
            matches!(&result[..], [Action::SubagentEvent { tool_use_id, event: EventKind::ToolResult { is_error: true, content, .. } }] if tool_use_id == "parent-tool" && content == "native output")
        );
    }
    #[test]
    fn excess_child_streams_wait_for_full_messages_without_failing_parent_or_losing_prefixes() {
        use super::*;
        let mut adapter = Claude {
            phase: Phase::Running,
            ..Default::default()
        };
        for i in 0..513 {
            let parent = format!("child-{i}");
            adapter.foreground_agents.insert(parent.clone());
            let actions = adapter.decode(json!({"type":"stream_event","parent_tool_use_id":parent,"event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"prefix"}}})).unwrap();
            assert_eq!(actions.len(), usize::from(i < 512));
        }
        assert_eq!(adapter.streamed_children.len(), 512);
        // Freeing a slot must not start capturing the skipped child's tail,
        // which would suppress its complete message and lose its prefix.
        adapter.decode(json!({"type":"assistant","parent_tool_use_id":"child-0","message":{"content":[{"type":"text","text":"prefix"}]}})).unwrap();
        assert!(adapter.decode(json!({"type":"stream_event","parent_tool_use_id":"child-512","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"tail"}}})).unwrap().is_empty());
        let full = adapter.decode(json!({"type":"assistant","parent_tool_use_id":"child-512","message":{"content":[{"type":"text","text":"prefixtail"}]}})).unwrap();
        assert!(
            matches!(&full[..], [Action::SubagentEvent { event: EventKind::Text { text }, .. }] if text == "prefixtail")
        );
        assert!(matches!(adapter.phase, Phase::Running));
        let finish = adapter
            .decode(json!({"type":"result","is_error":false,"result":"parent complete"}))
            .unwrap();
        assert!(
            finish
                .iter()
                .any(|a| matches!(a, Action::Finished { failed: false, .. }))
        );
    }
    #[test]
    fn bounded_child_window_keeps_a_complete_first_record_at_a_nonzero_offset() {
        use super::*;
        let config = LaunchConfig {
            provider: None,
            settings_dir: PathBuf::from("/tmp"),
            model: None,
            effort: None,
            permission_mode: None,
        };
        let line = json!({"type":"assistant","sessionId":"parent","agentId":"child","isSidechain":true,"message":{"content":[{"type":"text","text":"first complete record"}]}});
        for prefix in ["", "partial JSON fragment\n"] {
            let raw = super::super::claude_sessions::ChildBytes {
                bytes: format!("{prefix}{line}\n").into_bytes(),
                start: 1024,
                truncated: true,
            };
            let (events, truncated) =
                parse_child_transcript(raw, "parent", "child", "public", &config).unwrap();
            assert!(truncated);
            assert!(
                matches!(&events[..], [event] if matches!(&event.event, EventKind::Text { text } if text == "first complete record"))
            );
        }
        let raw = super::super::claude_sessions::ChildBytes {
            bytes: format!("{line}\n").into_bytes(),
            start: 1024,
            truncated: true,
        };
        assert!(
            parse_child_transcript(raw, "different-parent", "child", "public", &config).is_err()
        );
    }
    #[test]
    fn complete_child_history_preserves_long_reports_and_cross_page_confirmation() {
        use super::*;
        let config = LaunchConfig {
            provider: None,
            settings_dir: PathBuf::from("/tmp"),
            model: None,
            effort: None,
            permission_mode: None,
        };
        let report = "完整报告".repeat(20_000);
        let record = |kind: &str, content: Value| json!({"type":kind,"sessionId":"parent","agentId":"a123","isSidechain":true,"message":{"content":content}});
        let bytes = |r: Value| format!("{r}\n").into_bytes();
        let first=parse_child_history(super::super::claude_sessions::ChildPageBytes{bytes:bytes(record("assistant",json!([{"type":"thinking","thinking":"hidden"},{"type":"tool_use","id":"h","name":"SubagentHandback","input":{"message":report}}]))),cursor:Default::default(),has_more:true},"parent","a123","public",&config).unwrap();
        assert!(first.has_more);
        let EventKind::Tool { input, .. } = &first.events[0].event else {
            panic!("tool");
        };
        assert_eq!(input["message"], report);
        // The pending report survives a page boundary and restart of its cursor.
        let cursor = serde_json::from_str(&serde_json::to_string(&first.cursor).unwrap()).unwrap();
        let next=parse_child_history(super::super::claude_sessions::ChildPageBytes{bytes:bytes(record("user",json!([{"type":"tool_result","tool_use_id":"h","content":"{\"success\":true}","is_error":false}]))),cursor,has_more:false},"parent","a123","public",&config).unwrap();
        assert!(next.cursor.handbacks.is_empty());
        assert!(matches!(&next.events[1].event,EventKind::Text{text} if text==&report));
        assert!(next.events[1].seq > next.events[0].seq);
        assert!(
            !serde_json::to_string(&first.events)
                .unwrap()
                .contains("hidden")
        );
        let wrong = super::super::claude_sessions::ChildPageBytes {
            bytes: bytes(record(
                "assistant",
                json!([{"type":"text","text":"wrong owner"}]),
            )),
            cursor: Default::default(),
            has_more: false,
        };
        assert!(parse_child_history(wrong, "other", "a123", "public", &config).is_err());
    }

    #[test]
    fn native_child_records_validate_identity_and_redact_before_truncation() {
        use super::*;
        let secret = "fixture-provider-secret";
        let config = LaunchConfig {
            provider: Some(crate::providers::ResolvedProvider { metadata: serde_json::from_value(json!({"id":"p","name":"fixture","protocol":"anthropic_messages","base_url":"http://127.0.0.1","model":"m","models":[],"model_labels":{},"auth":"bearer","has_credential":true,"revision":"1"})).unwrap(), credential: secret.into() }),
            settings_dir: PathBuf::from("/tmp"), model: None, effort: None, permission_mode: None,
        };
        let text = format!("{}{secret}{}", "x".repeat(16_370), "z".repeat(32_768));
        let record = json!({"type":"assistant","sessionId":"parent","agentId":"a123","isSidechain":true,"message":{"content":[{"type":"thinking","thinking":"private reasoning"},{"type":"text","text":text},{"type":"tool_use","id":"t","name":"Read","input":{"file_path":"a.txt","credential":secret}}]}});
        let raw = || super::super::claude_sessions::ChildBytes {
            bytes: format!("{record}\n").into_bytes(),
            start: 0,
            truncated: false,
        };
        let (events, truncated) =
            parse_child_transcript(raw(), "parent", "a123", "public", &config).unwrap();
        let encoded = serde_json::to_string(&events).unwrap();
        assert!(truncated && events.iter().all(|e| e.session_id == "public"));
        assert!(
            !encoded.contains(secret)
                && !encoded.contains("fixture-provider")
                && !encoded.contains("private reasoning")
        );
        assert!(encoded.contains("[redacted]"));
        let full = parse_child_history(
            super::super::claude_sessions::ChildPageBytes {
                bytes: raw().bytes,
                cursor: Default::default(),
                has_more: false,
            },
            "parent",
            "a123",
            "public",
            &config,
        )
        .unwrap();
        let encoded_full = serde_json::to_string(&full.events).unwrap();
        assert!(!encoded_full.contains(secret) && !encoded_full.contains("private reasoning"));
        assert!(
            matches!(&full.events[0].event,EventKind::Text{text:actual} if actual==&config.redact(&text))
        );
        assert!(parse_child_transcript(raw(), "other-parent", "a123", "public", &config).is_err());
        assert!(parse_child_transcript(raw(), "parent", "other-child", "public", &config).is_err());
    }

    #[test]
    fn native_handback_promotes_only_confirmed_report_before_tool_input_clipping() {
        use super::*;
        let config = LaunchConfig {
            provider: None,
            settings_dir: PathBuf::from("/tmp"),
            model: None,
            effort: None,
            permission_mode: None,
        };
        let report = "报告正文🧪\n".repeat(3000);
        let record = |kind: &str, blocks: Value| {
            json!({
                "type":kind,"sessionId":"parent","agentId":"a123","isSidechain":true,
                "message":{"content":blocks}
            })
        };
        let tool = record(
            "assistant",
            json!([{"type":"tool_use","id":"handback","name":"SubagentHandback","input":{"message":report}}]),
        );
        let result = |id: &str, success: bool, is_error: bool| {
            record(
                "user",
                json!([{"type":"tool_result","tool_use_id":id,"is_error":is_error,"content":[{"type":"text","text":json!({"success":success,"message":"Report delivered to your caller."}).to_string()}]}]),
            )
        };
        let parse = |records: Vec<Value>| {
            parse_child_transcript(
                super::super::claude_sessions::ChildBytes {
                    bytes: records
                        .iter()
                        .map(|r| format!("{r}\n"))
                        .collect::<String>()
                        .into_bytes(),
                    start: 0,
                    truncated: false,
                },
                "parent",
                "a123",
                "public",
                &config,
            )
            .unwrap()
        };
        for records in [
            vec![tool.clone()],
            vec![tool.clone(), result("other", true, false)],
            vec![tool.clone(), result("handback", false, false)],
            vec![tool.clone(), result("handback", true, true)],
        ] {
            assert!(
                !parse(records)
                    .0
                    .iter()
                    .any(|e| matches!(e.event, EventKind::Text { .. }))
            );
        }
        let (events, truncated) = parse(vec![tool.clone(), result("handback", true, false)]);
        let delivered: String = events
            .iter()
            .filter_map(|e| match &e.event {
                EventKind::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(delivered, report);
        assert!(truncated); // tool input was clipped, but the report is complete
        assert!(events.windows(2).all(|pair| pair[0].seq < pair[1].seq));
        assert!(matches!(events[1].event, EventKind::ToolResult { .. }));
        let original = serde_json::to_value(&(events.clone(), truncated)).unwrap();
        let normalized = normalize_child_transcript((events, truncated)).unwrap();
        assert_eq!(serde_json::to_value(normalized).unwrap(), original);
        // A pre-fix persisted snapshot has the tool and receipt but no report
        // text. Its already-redacted input must work after daemon restart.
        let old_report = "完整历史报告".repeat(1000);
        let old_tool = record(
            "assistant",
            json!([{"type":"tool_use","id":"handback","name":"SubagentHandback","input":{"message":old_report}}]),
        );
        let mut cached = parse(vec![old_tool]);
        cached.0.push(latte_work_protocol::Event {
            seq: 1000000.0,
            session_id: "public".into(),
            at: 0.0,
            event: EventKind::ToolResult {
                id: "handback".into(),
                content: json!([{"type":"text","text":"{\"success\":true}"}]),
                is_error: false,
            },
        });
        // A receipt or empty streaming fragment must not hide the report.
        for text in ["", "Report delivered. Read it in the parent session."] {
            let mut source = cached.clone();
            source.0.push(latte_work_protocol::Event {
                seq: 1000001.0,
                session_id: "public".into(),
                at: 0.0,
                event: EventKind::Text { text: text.into() },
            });
            let restored = normalize_child_transcript(source).unwrap();
            let actual: String = restored
                .0
                .iter()
                .filter_map(|e| match &e.event {
                    EventKind::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(actual, format!("{old_report}{text}"));
            assert_eq!(
                serde_json::to_value(normalize_child_transcript(restored.clone()).unwrap())
                    .unwrap(),
                serde_json::to_value(restored).unwrap()
            );
        }
        let restored = normalize_child_transcript(cached).unwrap();
        let restored_text: String = restored
            .0
            .iter()
            .filter_map(|e| match &e.event {
                EventKind::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(restored_text, old_report);
        let oversized = record(
            "assistant",
            json!([{"type":"tool_use","id":"handback","name":"SubagentHandback","input":{"message":"界".repeat(60000)}}]),
        );
        let (events, truncated) = parse(vec![oversized, result("handback", true, false)]);
        let report: String = events
            .iter()
            .filter_map(|e| match &e.event {
                EventKind::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(truncated && report.ends_with("（报告已截断）"));
        assert!(report.len() < 129 * 1024);
    }

    #[test]
    fn subagents_native_lifecycle_and_foreground_results_are_distinct_from_shell_tasks() {
        use super::*;
        let mut agent = Claude::default();
        let tool = json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"agent-tool","name":"Agent","input":{"description":"检查结构"}}]}});
        let actions = agent.decode(tool).unwrap();
        assert!(actions.iter().any(|action| matches!(action, Action::Event(EventKind::Subagent { update }) if update.id == "tool:agent-tool" && update.status == Some(SubagentStatus::Running))));
        agent.decode(json!({"type":"system","subtype":"task_started","task_id":"native-child","task_type":"local_agent","tool_use_id":"agent-tool","description":"检查结构"})).unwrap();
        let progress = agent.decode(json!({"type":"system","subtype":"task_progress","task_id":"native-child","description":"Reading README.md","last_tool_name":"Read"})).unwrap();
        assert!(progress.iter().any(|action| matches!(action, Action::Event(EventKind::Subagent { update }) if update.title.is_none() && update.last_tool.as_deref()==Some("Read"))));
        let early = agent.decode(json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"agent-tool","content":"Launched in background"}]}})).unwrap();
        assert!(
            !early
                .iter()
                .any(|action| matches!(action, Action::Event(EventKind::Subagent { .. })))
        );
        let killed = agent.decode(json!({"type":"system","subtype":"task_updated","task_id":"native-child","patch":{"status":"killed"}})).unwrap();
        assert!(killed.iter().any(|action| matches!(action, Action::Event(EventKind::Subagent { update }) if update.status == Some(SubagentStatus::Stopped))));
        assert!(agent.tasks.is_empty());
        let summary = agent.decode(json!({"type":"system","subtype":"task_notification","task_id":"native-child","status":"stopped","summary":"已停止"})).unwrap();
        assert!(summary.iter().any(|action| matches!(action, Action::Event(EventKind::Subagent { update }) if update.summary.as_deref() == Some("已停止"))));
        let shell = agent.decode(json!({"type":"system","subtype":"task_started","task_id":"shell","task_type":"local_bash"})).unwrap();
        assert!(shell.is_empty());
        assert!(agent.decode(json!({"type":"system","subtype":"task_progress","task_id":"shell","description":"shell progress"})).unwrap().is_empty());
        agent.decode(json!({"type":"system","subtype":"task_updated","task_id":"shell","patch":{"status":"completed"}})).unwrap();
        assert!(agent.tasks.is_empty());
        agent.decode(json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"foreground","name":"Task","input":{"description":"前台"}}]}})).unwrap();
        let done = agent.decode(json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"foreground","content":"完成","is_error":false}]}})).unwrap();
        assert!(done.iter().any(|action| matches!(action, Action::Event(EventKind::Subagent { update }) if update.status == Some(SubagentStatus::Completed))));
        agent.decode(json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"legacy-tool","name":"Agent","input":{"description":"Legacy child"}}]}})).unwrap();
        let legacy = agent.decode(json!({"type":"system","subtype":"task_started","task_id":"legacy-native","tool_use_id":"legacy-tool"})).unwrap();
        assert!(legacy.iter().any(|action| matches!(action, Action::Event(EventKind::Subagent {update}) if update.id=="legacy-native")));
        assert!(!agent.foreground_agents.contains("legacy-tool"));
        let catalog =
            parse_commands(&json!([{"name":"agents"},{"name":"tasks"},{"name":"project:agents"},{"name":"list-agents"}]))
                .unwrap();
        assert!(catalog[0].ui_action.is_some() && catalog[1].ui_action.is_some());
        assert!(catalog[3].ui_action.is_some());
        assert!(catalog[2].ui_action.is_none());
    }

    #[test]
    fn thinking_status_is_deduplicated_and_never_carries_reasoning_text() {
        use super::*;
        let mut agent = Claude::default();
        let thinking = json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"private reasoning"}}});
        let actions = agent.decode(thinking.clone()).unwrap();
        assert!(matches!(
            &actions[..],
            [Action::Event(EventKind::Progress {
                phase: ExecutionPhase::Thinking
            })]
        ));
        assert!(agent.decode(thinking.clone()).unwrap().is_empty());
        let mut child = thinking;
        child["parent_tool_use_id"] = json!("child");
        assert!(agent.decode(child).unwrap().is_empty());
        let stopped = agent
            .decode(json!({"type":"stream_event","event":{"type":"content_block_stop"}}))
            .unwrap();
        assert!(matches!(
            &stopped[..],
            [Action::Event(EventKind::Progress {
                phase: ExecutionPhase::Waiting
            })]
        ));
        let reply = agent.decode(json!({"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"text"}}})).unwrap();
        assert!(matches!(
            &reply[..],
            [Action::Event(EventKind::Progress {
                phase: ExecutionPhase::Replying
            })]
        ));
    }

    #[test]
    fn usage_is_request_snapshot_not_cumulative_and_excludes_subagents() {
        use super::*;
        let mut agent = Claude {
            phase: Phase::Running,
            ..Default::default()
        };
        let message = json!({"type":"assistant", "message":{"model":"m", "usage":{
            "input_tokens":100,"cache_read_input_tokens":600,"cache_creation_input_tokens":300,"output_tokens":1
        },"content":[]}});
        let first = agent.decode(message.clone()).unwrap();
        assert!(
            matches!(&first[0], Action::Event(EventKind::Usage { context:Some(c), .. }) if c.used_tokens == 1000.0)
        );
        assert!(agent.decode(message.clone()).unwrap().is_empty());
        let mut child = message.clone();
        child["parent_tool_use_id"] = json!("child");
        child["message"]["usage"]["input_tokens"] = json!(9000);
        assert!(agent.decode(child).unwrap().is_empty());
        let result = agent.decode(json!({"type":"result","modelUsage":{"m":{"contextWindow":200000,"inputTokens":9999999}},"usage":{"input_tokens":200,"cache_read_input_tokens":1200,"cache_creation_input_tokens":600,"output_tokens":80},"duration_api_ms":52500,"num_turns":3})).unwrap();
        assert!(
            matches!(&result[0], Action::Event(EventKind::Usage {context:Some(c),totals:Some(t)}) if c.used_tokens == 1000.0 && c.window_tokens == Some(200000.0) && t.output_tokens == Some(80.0) && t.steps == Some(3.0))
        );
        agent
            .decode(json!({"type":"system","subtype":"compact_boundary"}))
            .unwrap();
        assert!(agent.context.is_none());
    }
    #[test]
    fn usage_reads_gateway_delta_and_matches_accounting_model() {
        use super::*;
        let mut agent = Claude {
            phase: Phase::Running,
            ..Default::default()
        };
        let start = json!({"type":"stream_event","event":{"type":"message_start","message":{"model":"ark/seed-evolving","usage":{"input_tokens":0,"output_tokens":0}}}});
        agent.decode(start.clone()).unwrap();
        agent.decode(json!({"type":"assistant","message":{"model":"ark/seed-evolving","usage":{"input_tokens":0,"output_tokens":0},"content":[]}})).unwrap();
        assert!(agent.context.is_none());
        let delta = json!({"type":"stream_event","event":{"type":"message_delta","usage":{"input_tokens":1784,"cache_read_input_tokens":72504,"output_tokens":1100}}});
        for _ in 0..2 {
            let events = agent.decode(delta.clone()).unwrap();
            assert!(events.iter().any(|e| matches!(e, Action::Event(EventKind::Usage {context:Some(c),totals:Some(t)}) if c.used_tokens == 74288.0 && t.input_tokens == Some(1784.0) && t.steps == Some(1.0))));
        }
        agent
            .decode(json!({"type":"stream_event","event":{"type":"message_stop"}}))
            .unwrap();
        agent.decode(start).unwrap();
        let events = agent.decode(delta).unwrap();
        assert!(events.iter().any(|e| matches!(e, Action::Event(EventKind::Usage {context:Some(c),totals:Some(t)}) if c.used_tokens == 74288.0 && t.input_tokens == Some(3568.0) && t.steps == Some(2.0))));
        let result = agent.decode(json!({"type":"result","modelUsage":{"seed-evolving[1m]":{"contextWindow":1000000}},"usage":{"input_tokens":3568,"cache_read_input_tokens":145008,"cache_creation_input_tokens":0,"output_tokens":2200},"duration_api_ms":84356,"num_turns":2})).unwrap();
        assert!(
            matches!(&result[0],Action::Event(EventKind::Usage {context:Some(c),totals:Some(t)}) if c.window_tokens == Some(1000000.0) && c.used_tokens == 74288.0 && t.model_time_ms == Some(84356.0))
        );
    }

    #[test]
    fn usage_missing_capacity_and_invalid_counters_are_not_guessed() {
        use super::*;
        assert_eq!(count(&json!({"n":-1}), "n"), None);
        assert_eq!(count(&json!({"n":9007199254740992_u64}), "n"), None);
        let mut agent = Claude {
            phase: Phase::Running,
            ..Default::default()
        };
        agent
            .decode(json!({"type":"assistant","message":{"model":"m","usage":{"input_tokens":12}}}))
            .unwrap();
        let result = agent
            .decode(json!({"type":"result","modelUsage":{"other":{"contextWindow":1000000}}}))
            .unwrap();
        assert!(
            matches!(&result[0],Action::Event(EventKind::Usage {context:Some(c),..}) if c.window_tokens.is_none())
        );
    }

    #[test]
    fn usage_matches_explicit_launch_alias_without_using_child_capacity() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::providers::LaunchConfig {
            provider: None,
            model: Some("relay/seed-0812".into()),
            effort: None,
            permission_mode: None,
            settings_dir: dir.path().into(),
        };
        let mut agent = Claude::default();
        let _launch = agent.command("claude", "/tmp", None, &config).unwrap();
        agent.phase = Phase::Running;
        agent.decode(json!({"type":"system","subtype":"init","parent_tool_use_id":"child","model":"child-model"})).unwrap();
        agent.decode(json!({"type":"assistant","message":{"model":"model_api/experimental_0812","usage":{"input_tokens":12345,"cache_read_input_tokens":57347}}})).unwrap();
        agent.decode(json!({"type":"result","parent_tool_use_id":"child","modelUsage":{"relay/seed-0812":{"contextWindow":1000000}}})).unwrap();
        assert!(agent.context.as_ref().unwrap().window_tokens.is_none());
        let result = agent.decode(json!({"type":"result","modelUsage":{"relay/seed-0812":{"contextWindow":200000},"experimental_0812[1m]":{"contextWindow":1000000}}})).unwrap();
        assert!(
            matches!(&result[0], Action::Event(EventKind::Usage {context:Some(c),..}) if c.model == "model_api/experimental_0812" && c.used_tokens == 69692.0 && c.window_tokens == Some(200000.0))
        );
    }

    #[test]
    fn usage_tracks_cli_resolved_model_and_keeps_capacity_only_until_model_changes() {
        let mut agent = Claude {
            phase: Phase::Running,
            ..Default::default()
        };
        agent
            .decode(json!({"type":"system","subtype":"init","model":"relay/seed-0812"}))
            .unwrap();
        agent.decode(json!({"type":"assistant","message":{"model":"model_api/experimental_0812","usage":{"input_tokens":100}}})).unwrap();
        agent
            .decode(
                json!({"type":"result","modelUsage":{"relay/seed-0812":{"contextWindow":200000}}}),
            )
            .unwrap();
        agent.decode(json!({"type":"assistant","message":{"model":"model_api/experimental_0812","usage":{"input_tokens":200}}})).unwrap();
        let result = agent.decode(json!({"type":"result"})).unwrap();
        assert!(
            matches!(&result[0], Action::Event(EventKind::Usage {context:Some(c),..}) if c.used_tokens == 200.0 && c.window_tokens == Some(200000.0))
        );
        agent
            .decode(json!({"type":"system","subtype":"init","model":"relay/new-model"}))
            .unwrap();
        assert!(agent.context.as_ref().unwrap().window_tokens.is_none());
        agent.decode(json!({"type":"assistant","message":{"model":"model_api/experimental_0812","usage":{"input_tokens":300}}})).unwrap();
        let result = agent
            .decode(
                json!({"type":"result","modelUsage":{"relay/seed-0812":{"contextWindow":200000}}}),
            )
            .unwrap();
        assert!(
            matches!(&result[0], Action::Event(EventKind::Usage {context:Some(c),..}) if c.used_tokens == 300.0 && c.window_tokens.is_none())
        );
    }

    #[test]
    fn usage_does_not_use_subagent_deltas_or_ambiguous_capacity() {
        use super::*;
        let mut agent = Claude {
            phase: Phase::Running,
            ..Default::default()
        };
        agent
            .decode(
                json!({"type":"assistant","message":{"model":"ark/m","usage":{"input_tokens":12}}}),
            )
            .unwrap();
        for event in [
            json!({"type":"message_start","message":{"model":"child","usage":{"input_tokens":999}}}),
            json!({"type":"message_delta","usage":{"input_tokens":999}}),
            json!({"type":"message_stop"}),
        ] {
            agent
                .decode(json!({"type":"stream_event","parent_tool_use_id":"child","event":event}))
                .unwrap();
        }
        assert_eq!(agent.context.as_ref().unwrap().used_tokens, 12.0);
        assert_eq!(agent.live_steps, 0.0);
        let result = agent.decode(json!({"type":"result","modelUsage":{"m[1m]":{"contextWindow":1000000},"m":{"contextWindow":200000}}})).unwrap();
        assert!(
            matches!(&result[0], Action::Event(EventKind::Usage {context:Some(c),..}) if c.window_tokens.is_none())
        );
    }

    #[test]
    fn permission_choices_follow_installed_cli_and_do_not_invent_modes() {
        let modes = super::modes_from_help(
            "--permission-mode <mode> (choices: \"default\", \"plan\")\n  --other auto",
        )
        .unwrap();
        assert_eq!(
            modes.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["default", "plan"]
        );
        let modern = super::modes_from_help("--permission-mode <mode>\n (choices: \"manual\", \"auto\",\n \"bypassPermissions\", \"future-mode\")").unwrap();
        assert_eq!(
            modern.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["manual", "auto", "bypassPermissions"]
        );
        assert!(modern.last().unwrap().elevated);
        assert!(super::modes_from_help("--other (choices: \"auto\")").is_err());
        assert!(
            super::modes_from_help("--permission-mode <mode>\n  --other (choices: \"auto\")")
                .is_err()
        );
        assert!(
            super::modes_from_help("--permission-mode <mode> (choices: \"future-mode\")").is_err()
        );
    }

    #[test]
    fn permission_choices_require_the_native_invalid_argument_diagnostic() {
        let diagnostic = format!(
            "error: option '--permission-mode <mode>' argument '{}' is invalid. Allowed choices are acceptEdits, auto, bypassPermissions, manual, dontAsk, plan, future-mode.\n",
            super::PERMISSION_PROBE
        );
        let modes = super::modes_from_diagnostic(&diagnostic).unwrap();
        assert_eq!(
            modes.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            [
                "manual",
                "acceptEdits",
                "plan",
                "auto",
                "dontAsk",
                "bypassPermissions"
            ]
        );
        assert!(modes.last().unwrap().elevated);
        for invalid in [
            "error: unknown option '--permission-mode'",
            "Allowed choices are bypassPermissions.",
            "error: option '--permission-mode <mode>' argument 'different-request' is invalid. Allowed choices are bypassPermissions.",
        ] {
            assert!(super::modes_from_diagnostic(invalid).is_err());
        }
        assert!(
            super::modes_from_diagnostic(&diagnostic.replace(
                "acceptEdits, auto, bypassPermissions, manual, dontAsk, plan, future-mode",
                "future-mode"
            ))
            .is_err()
        );
    }

    #[test]
    fn permission_launch_omits_inheritance_and_passes_explicit_modes_on_resume() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::providers::LaunchConfig {
            provider: None,
            model: None,
            effort: None,
            permission_mode: None,
            settings_dir: dir.path().into(),
        };
        for mode in [None, Some("plan"), Some("bypassPermissions"), None] {
            config.permission_mode = mode.map(str::to_owned);
            let mut adapter = super::Claude::default();
            let launch = super::AgentAdapter::command(
                &mut adapter,
                "claude",
                "/tmp",
                Some("native"),
                &config,
            )
            .unwrap();
            let args: Vec<_> = launch
                .command
                .as_std()
                .get_args()
                .map(|v| v.to_string_lossy().into_owned())
                .collect();
            assert_eq!(
                args.iter()
                    .find(|a| a.starts_with("--permission-mode="))
                    .cloned(),
                mode.map(|m| format!("--permission-mode={m}"))
            );
            assert!(!args.iter().any(|a| a == "--dangerously-skip-permissions"
                || a == "--allow-dangerously-skip-permissions"));
        }
    }

    #[test]
    fn discovers_native_commands_without_sending_a_prompt() {
        let mut adapter = Claude::default();
        assert_eq!(
            encoded(&adapter.advance(Input::DiscoverCommands).unwrap())[0]["request"]["subtype"],
            "initialize"
        );
        let response = json!({"type":"control_response","response":{"request_id":"latte-init","subtype":"success","response":{"commands":[{"name":"compact","displayName":"Compact conversation","description":"Compact history","argumentHint":"[instructions]"},{"name":"project:check","description":"Project command"}]}}});
        let actions = message(&mut adapter, response).unwrap();
        assert!(encoded(&actions).is_empty());
        assert!(
            matches!(&actions[0], Action::Commands(commands) if commands.len() == 2 && commands[0].argument_hint == "[instructions]" && commands[0].display_name.as_deref() == Some("Compact conversation"))
        );
        assert!(parse_commands(&json!(null)).is_err());
        assert!(parse_commands(&json!([{"name":"bad name"}])).is_err());
    }
    #[test]
    fn dispatches_native_commands_verbatim_and_rejects_unknown_commands() {
        let response = json!({"type":"control_response","response":{"request_id":"latte-init","subtype":"success","response":{"commands":[{"name":"compact"}]}}});
        let mut adapter = Claude::default();
        adapter
            .advance(Input::Start {
                prompt: "/compact 保留关键结论",
            })
            .unwrap();
        let actions = message(&mut adapter, response.clone()).unwrap();
        assert_eq!(
            encoded(&actions)[0]["message"]["content"],
            "/compact 保留关键结论"
        );
        let result = message(
            &mut adapter,
            json!({"type":"result","is_error":false,"result":"Not enough messages to compact."}),
        )
        .unwrap();
        assert!(
            matches!(&result[0], Action::Event(EventKind::Text { text }) if text == "Not enough messages to compact.")
        );
        let mut adapter = Claude::default();
        adapter
            .advance(Input::Start {
                prompt: "/not-supported",
            })
            .unwrap();
        assert!(message(&mut adapter, response).is_err());
    }

    #[test]
    fn effort_choices_follow_claude_model_capabilities() {
        use latte_work_protocol::Effort::{High, Low, Max, Medium, Xhigh};
        for model in ["haiku", "claude-haiku-4-5", "claude-sonnet-4-5"] {
            assert!(super::effort_levels(Some(model)).is_empty());
        }
        assert_eq!(
            super::effort_levels(Some("claude-sonnet-4-6")),
            &[Low, Medium, High, Max]
        );
        assert_eq!(
            super::effort_levels(Some("claude-opus-4-7")),
            &[Low, Medium, High, Xhigh, Max]
        );
    }

    use super::*;
    #[test]
    fn provider_settings_are_private_ephemeral_and_default_is_unchanged() {
        use crate::providers::ResolvedProvider;
        use latte_work_protocol::{Provider, ProviderAuth, ProviderProtocol};
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let mut config = LaunchConfig {
            provider: None,
            model: None,
            effort: None,
            permission_mode: None,
            settings_dir: dir.path().into(),
        };
        let default = Claude::default()
            .command("claude", "/tmp", None, &config)
            .unwrap();
        let settings: Value = serde_json::from_slice(
            &std::fs::read(default._settings.as_ref().unwrap().path()).unwrap(),
        )
        .unwrap();
        assert_eq!(settings, json!({"env":{}}));
        assert!(
            !default
                .command
                .as_std()
                .get_envs()
                .any(|(key, _)| key == "CLAUDE_CODE_EFFORT_LEVEL")
        );
        config.provider = Some(ResolvedProvider {
            metadata: Provider {
                id: "one".into(),
                name: "Team".into(),
                protocol: ProviderProtocol::AnthropicMessages,
                base_url: "https://example.test".into(),
                model: "my-model".into(),
                models: vec![],
                model_labels: Default::default(),
                auth: ProviderAuth::Bearer,
                has_credential: true,
                revision: "test".into(),
            },
            credential: "only-in-private-settings".into(),
        });
        let prepared = Claude::default()
            .command("claude", "/tmp", Some("native-session"), &config)
            .unwrap();
        config.provider.as_mut().unwrap().metadata.auth = ProviderAuth::None;
        let unauthenticated = Claude::default()
            .command("claude", "/tmp", None, &config)
            .unwrap();
        let unauth_settings: Value = serde_json::from_slice(
            &std::fs::read(unauthenticated._settings.as_ref().unwrap().path()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            unauth_settings["env"]["ANTHROPIC_AUTH_TOKEN"],
            "latte-work-no-auth"
        );
        assert_eq!(unauth_settings["env"]["ANTHROPIC_API_KEY"], "");
        assert!(
            !unauth_settings
                .to_string()
                .contains("only-in-private-settings")
        );
        config.provider.as_mut().unwrap().metadata.auth = ProviderAuth::Bearer;
        let path = prepared._settings.as_ref().unwrap().path().to_owned();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let settings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            settings["env"]["ANTHROPIC_AUTH_TOKEN"],
            "only-in-private-settings"
        );
        assert_eq!(settings["env"]["ANTHROPIC_API_KEY"], "");
        assert_eq!(
            settings["env"]["ANTHROPIC_DEFAULT_SONNET_MODEL"],
            "my-model"
        );
        let args = prepared
            .command
            .as_std()
            .get_args()
            .map(|a| a.to_string_lossy())
            .collect::<Vec<_>>();
        assert!(args.contains(&"--model=my-model".into()));
        assert!(args.contains(&"--resume=native-session".into()));
        assert!(!args.iter().any(|a| a.contains("only-in-private-settings")));
        drop(prepared);
        assert!(!path.exists());
    }
    fn message(adapter: &mut dyn AgentAdapter, value: Value) -> Result<Vec<Action>> {
        adapter.advance(Input::Message(&value.to_string()))
    }
    fn encoded(actions: &[Action]) -> Vec<Value> {
        actions
            .iter()
            .filter_map(|action| match action {
                Action::Write(bytes) => {
                    assert_eq!(bytes.last(), Some(&b'\n'));
                    Some(serde_json::from_slice(bytes).unwrap())
                }
                _ => None,
            })
            .collect()
    }
    fn initialized() -> Value {
        json!({"type":"control_response","response":{"request_id":"latte-init","subtype":"success"}})
    }
    #[test]
    fn adapter_owns_handshake_and_sends_prompt_exactly_once() {
        let mut adapter = super::super::create("claude").unwrap();
        let start = adapter
            .advance(Input::Start {
                prompt: "hello\n你好",
            })
            .unwrap();
        assert!(!start.iter().any(|a| matches!(a, Action::Ready)));
        let outgoing = encoded(&start);
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0]["request"]["subtype"], "initialize");
        let unrelated = message(adapter.as_mut(), json!({"type":"control_response","response":{"request_id":"another","subtype":"success"}})).unwrap();
        assert!(unrelated.is_empty());
        let ready = message(adapter.as_mut(), initialized()).unwrap();
        assert!(matches!(ready[0], Action::Ready));
        let outgoing = encoded(&ready);
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0]["message"]["content"], "hello\n你好");
        assert!(message(adapter.as_mut(), initialized()).unwrap().is_empty());
        assert!(
            adapter
                .advance(Input::Start {
                    prompt: "must not replay"
                })
                .is_err()
        );
    }
    #[test]
    fn adapter_rejects_failed_handshake_malformed_input_and_premature_completion() {
        let mut adapter = Claude::default();
        assert!(message(&mut adapter, initialized()).is_err());
        adapter
            .advance(Input::Start {
                prompt: "must not send",
            })
            .unwrap();
        assert!(
            adapter
                .advance(Input::Approval {
                    id: "r",
                    input: json!({}),
                    allow: true
                })
                .is_err()
        );
        assert!(message(&mut adapter, json!({"type":"control_response","response":{"request_id":"latte-init","subtype":"error","error":"denied"}})).is_err());
        assert!(adapter.advance(Input::Message("invalid JSON")).is_err());
        assert!(message(&mut adapter, json!({"type":"result","is_error":false})).is_err());
    }
    #[test]
    fn native_user_question_is_not_approval_and_answers_preserve_original_input() {
        let mut adapter = Claude::default();
        adapter
            .advance(Input::Start { prompt: "question" })
            .unwrap();
        message(&mut adapter, initialized()).unwrap();
        let input = json!({"questions":[{"header":"Format","question":"Which format?","multiSelect":false,"options":[{"label":"Short","description":"Brief"},{"label":"Long","description":"Full"}]}],"metadata":{"original":true}});
        let actions = message(&mut adapter, json!({"type":"control_request","request_id":"native-question","request":{"subtype":"can_use_tool","tool_use_id":"tool","tool_name":"AskUserQuestion","input":input}})).unwrap();
        let Action::Question { questions, .. } = &actions[0] else {
            panic!()
        };
        assert!(encoded(&actions).is_empty());
        let wire = encoded(
            &adapter
                .advance(Input::QuestionAnswer {
                    id: "native-question",
                    input: input.clone(),
                    questions: questions.clone(),
                    answers: Some(vec![latte_work_protocol::QuestionAnswer {
                        question_id: "q0".into(),
                        option_indices: vec![1],
                        text: None,
                    }]),
                })
                .unwrap(),
        );
        assert_eq!(
            wire[0]["response"]["response"]["updatedInput"]["questions"],
            input["questions"]
        );
        assert_eq!(
            wire[0]["response"]["response"]["updatedInput"]["metadata"],
            input["metadata"]
        );
        assert_eq!(
            wire[0]["response"]["response"]["updatedInput"]["answers"]["Which format?"],
            "Long"
        );
        let malformed = message(&mut adapter, json!({"type":"control_request","request_id":"bad","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[]}}})).unwrap();
        assert_eq!(
            encoded(&malformed)[0]["response"]["response"]["behavior"],
            "deny"
        );
        assert!(
            !malformed
                .iter()
                .any(|a| matches!(a, Action::Question { .. } | Action::Approval { .. }))
        );
    }
    #[test]
    fn adapter_encodes_approval_and_finishes_only_on_native_result() {
        let mut adapter = Claude::default();
        adapter.advance(Input::Start { prompt: "approve" }).unwrap();
        message(&mut adapter, initialized()).unwrap();
        let approval = message(&mut adapter, json!({"type":"control_request","request_id":"native-1","request":{"subtype":"can_use_tool","tool_name":"Write","input":{"file_path":"a.txt"}}})).unwrap();
        assert!(matches!(&approval[0], Action::Approval {id, ..} if id == "native-1"));
        assert!(encoded(&approval).is_empty());
        let replies = adapter
            .advance(Input::Approval {
                id: "native-1",
                input: json!({"file_path":"a.txt"}),
                allow: true,
            })
            .unwrap();
        let wire = encoded(&replies);
        assert_eq!(wire[0]["response"]["request_id"], "native-1");
        assert_eq!(
            wire[0]["response"]["response"]["updatedInput"]["file_path"],
            "a.txt"
        );
        let text = message(
            &mut adapter,
            json!({"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}),
        )
        .unwrap();
        assert!(
            !text
                .iter()
                .any(|action| matches!(action, Action::Finished { .. }))
        );
        let result = message(&mut adapter, json!({"type":"result","is_error":false})).unwrap();
        assert!(matches!(result[0], Action::Finished { failed: false, .. }));
        assert!(message(&mut adapter, initialized()).unwrap().is_empty());
        assert!(
            adapter
                .advance(Input::Approval {
                    id: "native-1",
                    input: json!({}),
                    allow: true
                })
                .is_ok()
        );
    }
    #[test]
    fn streams_without_duplicating_final_message() {
        let mut a = Claude::default();
        assert_eq!(a.decode(json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hi"}}})).unwrap().len(),1);
        assert!(
            a.decode(
                json!({"type":"assistant","message":{"content":[{"type":"text","text":"Hi"}]}})
            )
            .unwrap()
            .is_empty()
        );
    }
    #[test]
    fn approvals_are_explicit_and_native_errors_fail() {
        let mut a = Claude::default();
        let response = a.approval("r", json!({"command":"pwd"}), true);
        assert_eq!(
            response["response"]["response"]["updatedInput"]["command"],
            "pwd"
        );
        assert_eq!(
            a.approval("r", json!({}), false)["response"]["response"]["behavior"],
            "deny"
        );
        assert!(a.decode(json!({"type":"control_response","response":{"request_id":"latte-init","subtype":"error","error":"bad"}})).is_err());
        a.phase = Phase::Running;
        assert!(matches!(
            &a.decode(json!({"type":"result","is_error":true,"result":"auth failed"}))
                .unwrap()[0],
            Action::Finished { failed: true, .. }
        ));
    }
}
