//! Versioned, agent-independent desktop/host wire contract.
pub mod lifecycle;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

pub const VERSION: u32 = 1;
pub const MAX_FRAME: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
}
/// Ephemeral PTY owned by the host daemon; never restored after daemon restart.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TerminalInfo {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub exited: bool,
    pub exit_code: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ready,
    Running,
    Waiting,
    Completed,
    Failed,
    Stopped,
    Unknown,
}
impl Status {
    pub fn active(&self) -> bool {
        matches!(self, Self::Running | Self::Waiting)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Session {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub agent: String,
    pub native_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<Effort>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    pub status: Status,
    pub created_at: f64,
    #[serde(default)]
    pub custom_title: bool,
    #[serde(default)]
    pub pinned_at: Option<f64>,
    #[serde(default)]
    pub unread: bool,
    #[serde(default)]
    pub archived: bool,
    /// Live server projection only; absent on older servers and in durable history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_session_open: Option<bool>,
    /// Active turn, pending approval or background work; live evidence only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_session_busy: Option<bool>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}
/// Stable keyset cursor for history across all visible projects on one host.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RecentCursor {
    pub updated_at: f64,
    pub id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RecentSession {
    pub session: Session,
    pub updated_at: f64,
}
impl Effort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}
/// Presentation-only execution phase; never contains reasoning text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionPhase {
    Waiting,
    Thinking,
    Replying,
}

/// Latest main-agent request input, never cumulative billing usage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ContextUsage {
    pub model: String,
    pub used_tokens: f64,
    pub window_tokens: Option<f64>,
}
/// Main-agent totals for a single submitted turn. Missing telemetry stays unknown.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TurnUsage {
    pub input_tokens: Option<f64>,
    pub cache_read_tokens: Option<f64>,
    pub cache_write_tokens: Option<f64>,
    pub output_tokens: Option<f64>,
    pub model_time_ms: Option<f64>,
    pub steps: Option<f64>,
}
/// Native child execution evidence, separate from the main turn's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Running,
    Paused,
    Completed,
    Failed,
    Stopped,
    Unknown,
}
impl SubagentStatus {
    pub fn active(self) -> bool {
        matches!(self, Self::Running | Self::Paused)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SubagentUpdate {
    pub id: String,
    pub tool_use_id: Option<String>,
    pub title: Option<String>,
    /// Native agent role, when explicitly reported by the adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_type: Option<String>,
    pub status: Option<SubagentStatus>,
    pub summary: Option<String>,
    pub last_tool: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Subagent {
    pub id: String,
    pub native_id: String,
    pub tool_use_id: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_type: Option<String>,
    pub status: SubagentStatus,
    pub summary: Option<String>,
    pub last_tool: Option<String>,
    pub started_at: Option<f64>,
    pub updated_at: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    TurnChanges {
        changes: TurnChanges,
    },
    Subagent {
        update: SubagentUpdate,
    },
    Progress {
        phase: ExecutionPhase,
    },
    Usage {
        context: Option<ContextUsage>,
        totals: Option<TurnUsage>,
    },
    User {
        text: String,
        request_id: String,
    },
    /// Persisted native-input handoff; does not claim execution has started.
    QueuedMessageSent {
        text: String,
        request_id: String,
    },
    Text {
        text: String,
    },
    Tool {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        id: String,
        content: Value,
        is_error: bool,
    },
    Question {
        request_id: String,
        tool_use_id: Option<String>,
        questions: Vec<UserQuestion>,
    },
    QuestionResolved {
        request_id: String,
        answers: Option<Vec<QuestionAnswer>>,
    },
    Approval {
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        tool_use_id: Option<String>,
        tool: String,
        input: Value,
    },
    ApprovalResolved {
        request_id: String,
        allow: bool,
    },
    State {
        status: Status,
        message: Option<String>,
    },
    Notice {
        text: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Event {
    pub seq: f64,
    pub session_id: String,
    pub at: f64,
    pub event: EventKind,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub directory: bool,
}
/// Adapter-owned native permission choice; IDs are never translated by the UI.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AgentPermissionMode {
    pub id: String,
    pub label: String,
    pub description: String,
    pub elevated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub provider_protocols: Vec<ProviderProtocol>,
    pub detail: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ProviderProtocol {
    AnthropicMessages,
    OpenaiChat,
    OpenaiResponses,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAuth {
    None,
    Bearer,
    ApiKey,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub protocol: ProviderProtocol,
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub model_labels: std::collections::BTreeMap<String, String>,
    pub auth: ProviderAuth,
    pub has_credential: bool,
    #[serde(default)]
    pub revision: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ProviderTarget {
    pub ssh: String,
    pub server_path: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AgentProviderBinding {
    pub agent: String,
    pub provider_id: String,
    pub provider_revision: String,
}
// Only used by native clients and host services over private transport; never a UI response.
#[derive(Clone, Serialize, Deserialize, TS)]
pub struct ProviderSnapshot {
    pub provider: Provider,
    pub credential: String,
}
impl std::fmt::Debug for ProviderSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderSnapshot")
            .field("provider", &self.provider)
            .field("credential", &"[redacted]")
            .finish()
    }
}
/// An explicit per-turn override, independent of the receiving host's saved bindings.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "snapshot", rename_all = "snake_case")]
pub enum TurnProvider {
    Cli,
    Snapshot(ProviderSnapshot),
}
#[derive(Clone, Serialize, Deserialize, TS)]
pub struct ProviderDraft {
    pub id: Option<String>,
    pub name: String,
    pub protocol: ProviderProtocol,
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub model_labels: std::collections::BTreeMap<String, String>,
    pub auth: ProviderAuth,
    /// None preserves a saved credential; plaintext is write-only over the private host transport.
    pub credential: Option<String>,
}
impl std::fmt::Debug for ProviderDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderDraft")
            .field("id", &self.id)
            .field("credential", &"[redacted]")
            .finish_non_exhaustive()
    }
}
/// Current installed skill metadata, read on the execution host without invoking it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub directory: String,
}
/// A command reported by the selected native Agent in the project context.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct QueuedMessage {
    pub request_id: String,
    pub text: String,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub permission_mode: Option<String>,
    pub revision: u32,
    pub created_at: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AgentSlashCommand {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub description: String,
    pub argument_hint: String,
    /// Presentation hint supplied only for an advertised native command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ui_action: Option<AgentCommandUiAction>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandUiAction {
    Subagents,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Request {
    Hello {
        version: u32,
    },
    Projects,
    Terminals {
        project_id: String,
    },
    CreateTerminal {
        project_id: String,
        terminal_id: String,
        cols: u16,
        rows: u16,
    },
    ReadTerminal {
        terminal_id: String,
        after: f64,
    },
    WriteTerminal {
        terminal_id: String,
        data: Vec<u8>,
    },
    ResizeTerminal {
        terminal_id: String,
        cols: u16,
        rows: u16,
    },
    CloseTerminal {
        terminal_id: String,
    },
    Providers,
    AgentPermissions {
        agent: String,
    },
    AgentCommands {
        agent: String,
        project_id: String,
    },
    Models {
        agent: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        project_id: Option<String>,
    },
    SaveProvider {
        provider: ProviderDraft,
    },
    DeleteProvider {
        id: String,
    },
    BindAgentProvider {
        agent: String,
        provider_id: Option<String>,
        target: Option<ProviderTarget>,
    },
    ProvidersForHost {
        host_id: String,
    },
    ForgetHostProviders {
        host_id: String,
    },
    BindHostAgentProvider {
        host_id: String,
        agent: String,
        provider_id: Option<String>,
    },
    /// Native-only export of the current local association for a remote host.
    ExportHostAgentProvider {
        host_id: String,
        agent: String,
    },
    ModelsForProvider {
        agent: String,
        model: Option<String>,
        project_id: Option<String>,
        provider: Option<Provider>,
    },
    Session {
        session_id: String,
    },
    SyncAgentProvider {
        agent: String,
        snapshot: Option<ProviderSnapshot>,
    },
    AddProject {
        path: String,
        #[serde(default)]
        name: Option<String>,
    },
    RenameProject {
        project_id: String,
        name: String,
    },
    RemoveProject {
        project_id: String,
    },
    BrowseDirectories {
        path: Option<String>,
    },
    Sessions {
        project_id: String,
    },
    PinnedSessions,
    RecentSessions {
        before: Option<RecentCursor>,
    },
    RenameSession {
        session_id: String,
        title: String,
    },
    PinSession {
        session_id: String,
        pinned: bool,
    },
    MarkSessionUnread {
        session_id: String,
        unread: bool,
    },
    ArchiveSession {
        session_id: String,
        archived: bool,
    },
    CreateSession {
        project_id: String,
        agent: String,
    },
    /// Open/resume the native Agent without sending a prompt. Idempotent.
    OpenAgentSession {
        session_id: String,
        #[serde(default)]
        #[ts(optional)]
        provider: Option<TurnProvider>,
    },
    /// Explicitly release this session's native process and all its background work.
    CloseAgentSession {
        session_id: String,
        /// Idle bulk close must recheck atomically in the session actor.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        only_if_idle: Option<bool>,
    },
    SendQueued {
        #[serde(default)]
        #[ts(optional)]
        provider: Option<TurnProvider>,
        session_id: String,
        request_id: String,
        text: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        effort: Option<Effort>,
        #[serde(default)]
        permission_mode: Option<String>,
    },
    MessageQueue {
        session_id: String,
    },
    /// Compare the complete pending order before replacing it atomically.
    ReorderMessageQueue {
        session_id: String,
        expected: Vec<String>,
        order: Vec<String>,
    },
    TakeQueuedMessage {
        session_id: String,
        request_id: String,
        revision: u32,
    },
    DispatchQueuedMessage {
        #[serde(default)]
        interrupt: bool,
        session_id: String,
        request_id: String,
        revision: u32,
        provider: Option<TurnProvider>,
    },
    EditQueuedMessage {
        session_id: String,
        request_id: String,
        revision: u32,
        text: String,
    },
    CancelQueuedMessage {
        session_id: String,
        request_id: String,
        revision: u32,
    },
    ClearMessageQueue {
        session_id: String,
    },
    ResumeMessageQueue {
        session_id: String,
        provider: Option<TurnProvider>,
    },
    Send {
        #[serde(default)]
        #[ts(optional)]
        provider: Option<TurnProvider>,
        session_id: String,
        request_id: String,
        text: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        effort: Option<Effort>,
        #[serde(default)]
        permission_mode: Option<String>,
    },
    /// Bounded history window, newest first when before is absent.
    History {
        session_id: String,
        before: Option<f64>,
    },
    Poll {
        session_id: String,
        after: f64,
    },
    Subagents {
        session_id: String,
    },
    SubagentTranscript {
        session_id: String,
        task_id: String,
    },
    SubagentHistory {
        #[serde(default)]
        #[ts(optional)]
        provider: Option<TurnProvider>,
        session_id: String,
        task_id: String,
        after: Option<f64>,
        source: Option<String>,
    },
    AnswerQuestion {
        session_id: String,
        request_id: String,
        answers: Option<Vec<QuestionAnswer>>,
    },
    Approve {
        session_id: String,
        request_id: String,
        allow: bool,
    },
    Cancel {
        session_id: String,
    },
    Files {
        project_id: String,
        path: String,
    },
    BeginAttachment {
        name: String,
        #[ts(type = "number")]
        size: u64,
    },
    AttachmentChunk {
        id: String,
        #[ts(type = "number")]
        offset: u64,
        data: Vec<u8>,
    },
    FinishAttachment {
        id: String,
    },
    AbortAttachment {
        id: String,
    },
    ResolveReference {
        project_id: String,
        path: String,
    },
    ReferenceAccess {
        project_id: String,
    },
    RevokeReferenceAccess {
        project_id: String,
        path: String,
    },
    ReadFile {
        project_id: String,
        path: String,
    },
    Diff {
        project_id: String,
    },
    Changes {
        project_id: String,
    },
    GitInfo {
        project_id: String,
    },
    GitCommits {
        project_id: String,
        head: Option<String>,
        offset: u32,
    },
    GitReview {
        project_id: String,
        scope: GitReviewScope,
        base: Option<String>,
    },
    GitReviewDiff {
        project_id: String,
        scope: GitReviewScope,
        base: Option<String>,
        head: Option<String>,
        path: String,
        full_context: bool,
    },
    ChangeSummary {
        project_id: String,
        session_id: Option<String>,
    },
    UndoTurnChanges {
        session_id: String,
        request_id: String,
    },
    TurnChangeSummary {
        session_id: String,
        request_id: String,
    },
    LastTurnChanges {
        session_id: String,
    },
    TurnChangeDiff {
        session_id: String,
        request_id: String,
        path: String,
    },
    TaskChangeDiff {
        session_id: String,
        path: String,
    },
    SkillInfo {
        session_id: String,
        name: String,
    },
    Sources {
        session_id: String,
    },
    PreviewSource {
        project_id: String,
        session_id: Option<String>,
        path: String,
        #[ts(type = "number")]
        offset: u64,
    },
    ChangeDiff {
        project_id: String,
        path: String,
        section: ChangeSection,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ChangeSection {
    Unstaged,
    Staged,
    Untracked,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct UserQuestion {
    pub id: String,
    pub header: String,
    pub text: String,
    pub options: Vec<QuestionOption>,
    pub multi_select: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct QuestionAnswer {
    pub question_id: String,
    pub option_indices: Vec<u32>,
    pub text: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct GitChange {
    pub path: String,
    pub previous_path: Option<String>,
    pub section: ChangeSection,
    pub status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum GitReviewScope {
    Branch,
    Commit,
    Worktree,
    Unstaged,
    Staged,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct GitRef {
    pub name: String,
    pub full_name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct GitCommit {
    pub oid: String,
    pub subject: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct GitInfo {
    pub branch: Option<String>,
    pub head: Option<String>,
    pub default_base: Option<String>,
    pub refs: Vec<GitRef>,
    pub truncated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct GitReviewFile {
    pub path: String,
    pub previous_path: Option<String>,
    pub status: String,
    pub added: Option<u32>,
    pub removed: Option<u32>,
    pub binary: bool,
    pub untracked: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct GitReview {
    /// Resolved commits pin a branch comparison even after refs move.
    pub base: Option<String>,
    pub head: Option<String>,
    pub entries: Vec<GitReviewFile>,
    pub added: u32,
    pub removed: u32,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TaskChange {
    pub path: String,
    pub status: String,
    pub added: Option<u32>,
    pub removed: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ChangeSummary {
    pub entries: Vec<TaskChange>,
    pub added: u32,
    pub removed: u32,
    pub binary_files: u32,
    pub truncated: bool,
    pub baseline_at: Option<f64>,
    pub unavailable: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TurnUndoStatus {
    Ready,
    Reverted,
    Unknown,
}
/// Frozen worktree delta between admission and the main result/confirmed stop.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TurnChanges {
    pub request_id: String,
    pub summary: ChangeSummary,
    pub interrupted: bool,
    pub background_pending: bool,
    pub undo: TurnUndoStatus,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    File,
    Directory,
    Tool,
    Connector,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct TaskSource {
    pub id: String,
    pub name: String,
    pub kind: SourceKind,
    pub path: Option<String>,
    pub mime_type: Option<String>,
    pub tools: Vec<String>,
    pub uses: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    RecentSessions {
        sessions: Vec<RecentSession>,
        next: Option<RecentCursor>,
    },
    LastTurnChanges {
        changes: Option<TurnChanges>,
    },
    TurnChanges {
        changes: TurnChanges,
    },
    ChangeSummary {
        summary: ChangeSummary,
    },
    SkillInfo {
        skill: Option<SkillInfo>,
    },
    Sources {
        entries: Vec<TaskSource>,
        truncated: bool,
    },
    SourcePreview {
        data: Vec<u8>,
        mime_type: String,
        #[ts(type = "number")]
        size: u64,
        #[ts(type = "number")]
        next: u64,
        has_more: bool,
    },
    SourceDirectory {
        entries: Vec<FileEntry>,
        truncated: bool,
    },
    Subagents {
        tasks: Vec<Subagent>,
        truncated: bool,
    },
    SubagentTranscript {
        events: Vec<Event>,
        truncated: bool,
    },
    SubagentHistory {
        #[serde(default)]
        #[ts(optional)]
        prompt: Option<String>,
        events: Vec<Event>,
        has_more: bool,
        after: Option<f64>,
        source_has_more: bool,
        incomplete: bool,
        source: String,
    },
    Changes {
        entries: Vec<GitChange>,
        truncated: bool,
    },
    GitInfo {
        info: GitInfo,
    },
    GitCommits {
        head: Option<String>,
        commits: Vec<GitCommit>,
        has_more: bool,
    },
    GitReview {
        review: GitReview,
    },
    AgentPermissions {
        modes: Vec<AgentPermissionMode>,
    },
    AgentCommands {
        commands: Vec<AgentSlashCommand>,
    },
    Terminals {
        terminals: Vec<TerminalInfo>,
    },
    Terminal {
        terminal: TerminalInfo,
    },
    TerminalOutput {
        terminal: TerminalInfo,
        data: Vec<u8>,
        next: f64,
        has_more: bool,
        truncated: bool,
    },
    Models {
        models: Vec<String>,
        provider: Option<String>,
        default_model: Option<String>,
        effort_levels: Vec<Effort>,
        /// Display metadata only; keys remain the original selectable model IDs.
        #[serde(default)]
        model_labels: std::collections::BTreeMap<String, String>,
    },
    /// Native-only response. Must never be forwarded to the WebView.
    ProviderSnapshot {
        snapshot: Option<ProviderSnapshot>,
        #[serde(default)]
        configured: bool,
    },
    Providers {
        providers: Vec<Provider>,
        bindings: Vec<AgentProviderBinding>,
    },
    Hello {
        version: u32,
        server_id: String,
        agents: Vec<AgentInfo>,
        #[serde(default)]
        permission_settings: bool,
        #[serde(default)]
        history_window: bool,
    },
    Projects {
        projects: Vec<Project>,
    },
    Project {
        project: Project,
    },
    Sessions {
        sessions: Vec<Session>,
    },
    Session {
        session: Session,
    },
    QueuedMessageTaken {
        entry: QueuedMessage,
    },
    MessageQueued {
        duplicate: bool,
    },
    MessageQueue {
        entries: Vec<QueuedMessage>,
        paused: bool,
        reason: Option<String>,
    },
    Accepted {
        duplicate: bool,
    },
    History {
        events: Vec<Event>,
        session: Session,
        /// More earlier events exist. Events are returned in ascending order.
        has_more: bool,
        /// Fetch an earlier window before revealing a split text or pending approval.
        needs_earlier: bool,
        /// Exclusive raw-event cursor, independent of compacted text event IDs.
        before: Option<f64>,
    },
    Events {
        events: Vec<Event>,
        session: Session,
        has_more: bool,
    },
    Files {
        entries: Vec<FileEntry>,
    },
    AttachmentUpload {
        id: String,
    },
    FileReference {
        entry: FileEntry,
    },
    ReferenceAccess {
        entries: Vec<FileEntry>,
        truncated: bool,
    },
    Directories {
        path: String,
        parent: Option<String>,
        entries: Vec<FileEntry>,
        truncated: bool,
    },
    Content {
        text: String,
        truncated: bool,
    },
    Ok,
    Error {
        code: String,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subagent_roles_are_optional_for_old_history_and_preserved_on_the_wire() {
        let old = serde_json::json!({
            "id":"child", "native_id":"native", "tool_use_id":null,
            "title":"Review changes", "status":"completed", "summary":null,
            "last_tool":"Read", "started_at":1, "updated_at":2
        });
        let mut task: Subagent = serde_json::from_value(old).unwrap();
        assert_eq!(task.agent_type, None);
        task.agent_type = Some("review:code-reviewer".into());
        let encoded = serde_json::to_value(&task).unwrap();
        assert_eq!(encoded["agent_type"], "review:code-reviewer");
        let restored: Subagent = serde_json::from_value(encoded).unwrap();
        assert_eq!(restored.agent_type, task.agent_type);
        let update: SubagentUpdate = serde_json::from_value(serde_json::json!({
            "id":"child", "tool_use_id":null, "title":null,
            "status":"completed", "summary":null, "last_tool":null
        }))
        .unwrap();
        assert_eq!(update.agent_type, None);
    }
    #[test]
    fn contract_uses_explicit_tag_and_rejects_unknown_method() {
        let request: Request = serde_json::from_str(r#"{"method":"hello","version":1}"#).unwrap();
        assert!(matches!(request, Request::Hello { version: 1 }));
        assert!(serde_json::from_str::<Request>(r#"{"method":"shell","command":"rm"}"#).is_err());
        assert!(!Status::Unknown.active());
        assert!(Status::Waiting.active());
    }
}

#[cfg(test)]
mod model_metadata_compatibility {
    use super::*;
    #[test]
    fn requests_and_responses_without_labels_remain_valid() {
        let request: Request =
            serde_json::from_str(r#"{"method":"models","agent":"claude","model":null}"#).unwrap();
        assert!(matches!(
            request,
            Request::Models {
                project_id: None,
                ..
            }
        ));
        let response: Response = serde_json::from_str(r#"{"kind":"models","models":["sonnet"],"provider":null,"default_model":null,"effort_levels":[]}"#).unwrap();
        assert!(
            matches!(response, Response::Models { model_labels, .. } if model_labels.is_empty())
        );
    }
}

#[cfg(test)]
mod session_projection_compatibility {
    use super::*;
    #[test]
    fn old_sessions_have_no_live_state_and_do_not_persist_it() {
        let session: Session = serde_json::from_str(r#"{"id":"s","project_id":"p","title":"old","agent":"claude","native_id":"old-native","status":"completed","created_at":1}"#).unwrap();
        assert_eq!(session.agent_session_open, None);
        assert!(
            serde_json::to_value(&session)
                .unwrap()
                .get("agent_session_open")
                .is_none()
        );
    }
}
