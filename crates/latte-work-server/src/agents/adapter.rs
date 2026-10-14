//! Internal contract for a persistent agent session over bounded line-framed stdio.
//! Adapters own wire encoding and protocol order, never process supervision or storage.
use crate::providers::LaunchConfig;
use anyhow::Result;
use latte_work_protocol::EventKind;
use serde_json::Value;
use tokio::process::Command;

pub struct AgentCommand {
    pub command: Command,
    /// Keep private launch files alive until the agent exits, including spawn errors.
    pub _settings: Option<tempfile::NamedTempFile>,
}

pub enum Input<'a> {
    /// Initialize for metadata only. Must never send a user prompt.
    DiscoverCommands,
    /// Open the bidirectional session without sending a user prompt.
    Open,
    RenameTitle {
        id: &'a str,
        title: &'a str,
    },
    /// Interrupt the active turn while retaining the session's process.
    Interrupt,
    /// Start a turn. The first turn initializes; later turns reuse the same transport.
    Start {
        prompt: &'a str,
    },
    /// Hand an input to the native agent while retaining the active turn.
    Queue {
        request_id: &'a str,
        prompt: &'a str,
    },
    /// One complete line without its delimiter. Only the adapter interprets its payload.
    Message(&'a str),
    /// Delivered only after Runtime validates and consumes a pending public approval ID.
    Approval {
        id: &'a str,
        input: Value,
        allow: bool,
    },
    QuestionAnswer {
        id: &'a str,
        input: Value,
        questions: Vec<latte_work_protocol::UserQuestion>,
        answers: Option<Vec<latte_work_protocol::QuestionAnswer>>,
    },
}

pub enum Action {
    /// Authoritative native session name, including external manual renames.
    NativeName(String),
    TitleRenamed(Result<String, String>),
    SourceUsed {
        id: String,
        name: String,
    },
    Commands(Vec<latte_work_protocol::AgentSlashCommand>),
    /// Already encoded bytes, including framing. Runtime only bounds and writes them.
    Write(Vec<u8>),
    /// Stop the startup deadline; this does not implicitly send any message.
    Ready,
    /// Native evidence that the main agent started an autonomous follow-up turn.
    TurnStarted,
    /// Native completion evidence for inputs consumed in this turn (possibly a batch).
    /// A delivered input does not necessarily start an independent native turn.
    InputsConsumed(Vec<String>),
    Interrupted,
    Event(EventKind),
    /// Native child output; never changes the parent turn's text or execution state.
    SubagentEvent {
        tool_use_id: String,
        event: EventKind,
    },
    NativeSession(String),
    Approval {
        id: String,
        tool_use_id: Option<String>,
        tool: String,
        input: Value,
    },
    Question {
        id: String,
        tool_use_id: Option<String>,
        input: Value,
        questions: Vec<latte_work_protocol::UserQuestion>,
    },
    /// Must follow explicit native completion evidence, never EOF or a successful exit.
    Finished {
        failed: bool,
        message: Option<String>,
    },
}

pub trait AgentAdapter: Send {
    /// Only true between turns when replacing the process cannot discard live work.
    fn can_reconfigure(&self) -> bool;
    /// Actual native tasks that outlive the main turn; distinct from reconfiguration readiness.
    fn has_background_work(&self) -> bool;
    /// Only children observed by this process share its credential redaction context.
    fn knows_subagent(&self, native_id: &str) -> bool;
    fn set_initial_title(&mut self, title: Option<String>);
    /// Prepare launch resources; may retain resume/config context for later protocol steps.
    fn command(
        &mut self,
        binary: &std::path::Path,
        cwd: &str,
        resume: Option<&str>,
        config: &LaunchConfig,
    ) -> Result<AgentCommand>;
    /// Return ordered, bounded actions in response to a host or protocol input.
    fn advance(&mut self, input: Input<'_>) -> Result<Vec<Action>>;
}
