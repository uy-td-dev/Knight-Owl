//! The reasoning loop state machine.
//!
//! State transitions: `Idle → Planning → Acting → Observing → Idle`.
//! The loop never imports concrete model or tool types — only trait objects.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use futures_util::StreamExt;
use rig::agent::AgentBuilder;
use rig::agent::MultiTurnStreamItem;
use rig::streaming::{StreamedAssistantContent, StreamingPrompt};
use tracing::{debug, info, warn};

use owl_protocol::events::AgentEvent;
use owl_protocol::experience::{ExperienceStore, InsightKind, TaskMemory, TaskOutcome};
use owl_protocol::state::AgentState;
use owl_protocol::tools::{ToolCall, ToolResult};

use crate::context::ContextProvider;
use crate::distillation::DistillationWorker;
use crate::event_sink::{EventSink, NullSink};
use crate::memory::{MemoryEntry, MemoryStore};
use crate::runner::AgentRunner;
use crate::BrainError;

/// Background distillation fires every N completed tasks.
const DISTILL_EVERY_N_TASKS: u32 = 10;

static TASK_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Configuration for the reasoning loop.
#[derive(Debug, Clone)]
pub struct ReasoningConfig {
    /// Maximum number of tool-call iterations before giving up.
    pub max_steps: usize,
    /// How many memory entries to include in each prompt context.
    /// **Auto-tuned by `model_class` when constructed via `for_model`.**
    pub memory_context_limit: usize,
    /// System prompt injected at agent build time.
    pub system_prompt: String,
    /// Model class — drives prompt selection + budget defaults.
    pub model_class: crate::ModelClass,
    /// Max bytes of tool stdout/stderr forwarded back to the model.
    pub tool_result_max_bytes: usize,
}

impl Default for ReasoningConfig {
    fn default() -> Self {
        Self::for_model(crate::ModelClass::default())
    }
}

impl ReasoningConfig {
    /// Construct sensible defaults for a given model size class.
    ///
    /// Picks the right system prompt + context budgets so a 2 B local model
    /// isn't asked to digest the same prompt + history budget as Claude.
    pub fn for_model(class: crate::ModelClass) -> Self {
        Self {
            max_steps:             10,
            memory_context_limit:  class.memory_limit(),
            system_prompt:         crate::prompt::system_for(class).to_string(),
            model_class:           class,
            tool_result_max_bytes: class.tool_result_max_bytes(),
        }
    }

    /// Construct from a model id string (e.g. `"gemma4:e2b"`,
    /// `"claude-sonnet-4-6"`).  Auto-detects class via
    /// [`ModelClass::from_model_id`].
    pub fn for_model_id(model_id: &str) -> Self {
        Self::for_model(crate::ModelClass::from_model_id(model_id))
    }
}

/// Abstraction over tool dispatch — injected into `ReasoningLoop`.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute a tool call and return its result.
    async fn execute(&self, call: ToolCall) -> Result<ToolResult, BrainError>;

    /// Returns `true` if the executor knows this tool name.
    fn has_tool(&self, name: &str) -> bool;
}

/// The orchestrating reasoning loop.
///
/// Generic over any `rig` completion model and any injected executor/memory.
pub struct ReasoningLoop<M>
where
    M: rig::completion::CompletionModel,
{
    model: M,
    tools: Arc<dyn ToolExecutor>,
    memory: Arc<dyn MemoryStore>,
    context: Option<Arc<dyn ContextProvider>>,
    experience: Option<Arc<dyn ExperienceStore>>,
    event_sink: Arc<dyn EventSink>,
    config: ReasoningConfig,
}

impl<M> ReasoningLoop<M>
where
    M: rig::completion::CompletionModel + Clone + Send + Sync + 'static,
{
    /// Construct a new reasoning loop with injected dependencies.
    ///
    /// The event sink defaults to [`NullSink`] (no-op).  Attach a real one
    /// via [`Self::with_event_sink`] to observe step-level progress.
    pub fn new(
        model: M,
        tools: Arc<dyn ToolExecutor>,
        memory: Arc<dyn MemoryStore>,
        config: ReasoningConfig,
    ) -> Self {
        Self {
            model, tools, memory,
            context: None, experience: None,
            event_sink: Arc::new(NullSink),
            config,
        }
    }

    /// Attach a code-context provider so the loop can query the codebase graph
    /// before each planning step.
    pub fn with_context(mut self, provider: Arc<dyn ContextProvider>) -> Self {
        self.context = Some(provider);
        self
    }

    /// Attach an experience store to enable R-22 reflection + insight queries.
    pub fn with_experience(mut self, store: Arc<dyn ExperienceStore>) -> Self {
        self.experience = Some(store);
        self
    }

    /// Attach an event sink so the host can observe step-level progress
    /// (state changes, tool calls, completion).  See [`crate::event_sink`].
    pub fn with_event_sink(mut self, sink: Arc<dyn EventSink>) -> Self {
        self.event_sink = sink;
        self
    }

    /// Run the reasoning loop for the given prompt, returning the final answer.
    pub async fn run(&self, prompt: &str) -> Result<String, BrainError> {
        self.memory
            .push(MemoryEntry { role: "user".into(), content: prompt.into() })
            .await?;

        let agent = AgentBuilder::new(self.model.clone())
            .preamble(&self.config.system_prompt)
            .build();

        let mut state = AgentState::Idle;
        let mut steps = 0;
        let mut action_log: Vec<String> = Vec::new();
        // Track tool calls dispatched in THIS run so we can detect and
        // break loops where the model keeps re-emitting the same call
        // instead of synthesising the result (common in small models).
        let mut dispatched: std::collections::HashSet<String> = std::collections::HashSet::new();

        let result = loop {
            if steps >= self.config.max_steps {
                break Err(BrainError::MaxStepsExceeded(self.config.max_steps));
            }

            state = transition(state);
            debug!(?state, step = steps, "reasoning loop step");
            self.event_sink.emit(AgentEvent::StateChanged { state: state.clone() }).await;

            match &state {
                AgentState::Planning => {
                    let context = self.build_context(prompt).await?;

                    // Native token streaming (rig 0.36+): consume the model's
                    // delta stream, emit one [`AgentEvent::TextChunk`] per
                    // text delta to the sink, and accumulate the full text
                    // for downstream tool-call parsing + memory persistence.
                    //
                    // `stream_prompt(...).await` returns a `StreamingResult`
                    // (a `Stream<Item = Result<RawStreamingChoice, ...>>`)
                    // directly — no Result wrap, so no `.map_err()` here.
                    let mut stream = agent.stream_prompt(context.as_str()).await;

                    let mut response = String::new();
                    while let Some(chunk_res) = stream.next().await {
                        let chunk = chunk_res
                            .map_err(|e| BrainError::Completion(e.to_string()))?;
                        match chunk {
                            MultiTurnStreamItem::StreamAssistantItem(
                                StreamedAssistantContent::Text(t)
                            ) => {
                                self.event_sink
                                    .emit(AgentEvent::TextChunk { content: t.text.clone() })
                                    .await;
                                response.push_str(&t.text);
                            }
                            MultiTurnStreamItem::FinalResponse(_) => {
                                // Final aggregated response — text already
                                // accumulated chunk-by-chunk above.
                            }
                            // Other variants (tool-call deltas, user items) are
                            // intentionally ignored: the loop's existing parser
                            // pulls full `{"tool":..., "args":...}` blocks from
                            // the assembled response text below.
                            _ => {}
                        }
                    }

                    let mut calls = try_parse_tool_calls(&response);
                    // Anti-loop: drop any duplicate call we already
                    // dispatched in this run.  Models like qwen2.5-coder
                    // sometimes re-emit the same `list_dir({path:"."})`
                    // turn after turn instead of using the prior result.
                    let original_len = calls.len();
                    calls.retain(|c| {
                        let key = format!(
                            "{}::{}",
                            c.name,
                            serde_json::to_string(&c.args).unwrap_or_default()
                        );
                        dispatched.insert(key)
                    });
                    if calls.len() < original_len {
                        debug!(
                            dropped = original_len - calls.len(),
                            "skipped duplicate tool calls (anti-loop)"
                        );
                    }
                    if !calls.is_empty() {
                        for c in &calls {
                            action_log.push(format!("tool:{}", c.name));
                            self.event_sink.emit(AgentEvent::ToolCalling { call: c.clone() }).await;
                        }
                        self.memory
                            .push(MemoryEntry {
                                role: "assistant".into(),
                                content: response.clone(),
                            })
                            .await?;
                        dispatch_tools_parallel_with_sink_budgeted(
                            calls, &*self.tools, &*self.memory, &*self.event_sink,
                            self.config.tool_result_max_bytes,
                        ).await?;
                        state = AgentState::Observing;
                        steps += 1;
                        continue;
                    }

                    // Anti-loop kicked in: model only emitted duplicate tool
                    // calls.  Don't surface the raw JSON to the user — force
                    // a synthesis turn that asks the model to summarise the
                    // tool results in plain prose, then break with THAT.
                    let model_text_is_only_dup_json = original_len > 0;
                    if model_text_is_only_dup_json {
                        debug!("anti-loop: forcing synthesis turn");
                        self.memory
                            .push(MemoryEntry {
                                role: "user".into(),
                                content: "Bạn đã có đủ kết quả từ các tool ở trên. \
                                          KHÔNG gọi tool nữa. Viết một câu trả lời \
                                          bằng văn bản tóm tắt những gì đã tìm được. \
                                          (You already have the tool results above. \
                                          Do NOT call any more tools. Write a plain-text \
                                          reply summarising what you found.)".into(),
                            })
                            .await?;
                        let synth_ctx = self.build_context(prompt).await?;
                        let mut stream = agent.stream_prompt(synth_ctx.as_str()).await;
                        let mut synth_text = String::new();
                        while let Some(chunk_res) = stream.next().await {
                            if let Ok(MultiTurnStreamItem::StreamAssistantItem(
                                StreamedAssistantContent::Text(t)
                            )) = chunk_res {
                                self.event_sink
                                    .emit(AgentEvent::TextChunk { content: t.text.clone() })
                                    .await;
                                synth_text.push_str(&t.text);
                            }
                        }
                        // Strip any leftover JSON the model might still emit.
                        let cleaned = strip_tool_json(&synth_text);
                        info!("reasoning loop completed (synthesis) in {} steps", steps + 1);
                        self.memory
                            .push(MemoryEntry { role: "assistant".into(), content: cleaned.clone() })
                            .await?;
                        break Ok(cleaned);
                    }

                    info!("reasoning loop completed in {} steps", steps + 1);
                    // Always strip stray tool-call JSON from the final
                    // response text — small/mid models sometimes emit
                    // raw JSON or ```json fences EVEN AFTER the loop
                    // decides it's a plain-text answer.  The cleaned
                    // version is what the user sees + what we persist.
                    let cleaned = strip_tool_json(&response);
                    let final_text = if cleaned.is_empty() {
                        // Model emitted ONLY tool JSON with no prose —
                        // force one more synthesis turn to extract a
                        // real reply from the tool observations.
                        debug!("loop completed with empty post-strip text; forcing synthesis");
                        self.memory.push(MemoryEntry {
                            role: "user".into(),
                            content: "Bạn đã có đủ kết quả tool ở trên. \
                                      KHÔNG gọi tool nữa. Viết một câu \
                                      văn bản tóm tắt những gì đã tìm \
                                      được, bằng tiếng Việt.".into(),
                        }).await?;
                        let synth_ctx = self.build_context(prompt).await?;
                        let mut s = agent.stream_prompt(synth_ctx.as_str()).await;
                        let mut t = String::new();
                        while let Some(c) = s.next().await {
                            if let Ok(MultiTurnStreamItem::StreamAssistantItem(
                                StreamedAssistantContent::Text(text_chunk)
                            )) = c {
                                self.event_sink
                                    .emit(AgentEvent::TextChunk { content: text_chunk.text.clone() })
                                    .await;
                                t.push_str(&text_chunk.text);
                            }
                        }
                        strip_tool_json(&t)
                    } else {
                        cleaned
                    };
                    self.memory
                        .push(MemoryEntry { role: "assistant".into(), content: final_text.clone() })
                        .await?;
                    break Ok(final_text);
                }

                AgentState::Acting | AgentState::Observing | AgentState::Idle => {
                    steps += 1;
                }
            }
        };

        // ── R-22 Reflection ─────────────────────────────────────────────────
        // Persist a TaskMemory row regardless of success/failure.
        // This is a best-effort write — never let it mask the real result.
        if let Some(exp) = &self.experience {
            let outcome = match &result {
                Ok(_)  => TaskOutcome::Success,
                Err(e) => TaskOutcome::Failure {
                    reason:      e.to_string(),
                    stderr_tail: String::new(),
                },
            };
            let memory_row = TaskMemory {
                id:         uuid::Uuid::new_v4().to_string(),
                request:    prompt.to_string(),
                actions:    action_log,
                outcome,
                code_refs:  vec![],
                session_id: String::new(),
            };
            if let Err(e) = exp.store_task_memory(memory_row).await {
                warn!(err = %e, "reflection write failed (non-fatal)");
            }

            // ── Background distillation every N tasks ──────────────────
            let count = TASK_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
            if count % DISTILL_EVERY_N_TASKS == 0 {
                let model = self.model.clone();
                let exp = Arc::clone(exp);
                tokio::spawn(async move {
                    let worker = DistillationWorker::new(model, exp);
                    match worker.run_once(50).await {
                        Ok(n) => info!(insights = n, "background distillation complete"),
                        Err(e) => warn!(err = %e, "background distillation failed"),
                    }
                });
            }
        }

        // Final terminal event: Done on success, Error on failure.
        match &result {
            Ok(text) => self.event_sink.emit(AgentEvent::Done { response: text.clone() }).await,
            Err(e)   => self.event_sink.emit(AgentEvent::Error { message: e.to_string() }).await,
        }

        result
    }

    /// Single-shot multi-modal Q&A — sends `prompt` + image/text
    /// [`Attachment`]s as a native multi-modal user message, streams the
    /// response, and returns.  Does NOT enter the tool-loop: vision-aware
    /// agentic flows (image → tool → answer) are a future enhancement that
    /// requires threading the multi-modal initial message through every
    /// iteration of the loop.
    ///
    /// Use `run()` for tool-using turns; this for "describe / extract / OCR
    /// from this image" use cases.
    async fn run_with_attachments_impl(
        &self,
        prompt: &str,
        attachments: &[owl_protocol::attachment::Attachment],
    ) -> Result<String, BrainError> {
        use owl_protocol::attachment::Attachment;
        use rig::completion::message::{
            DocumentSourceKind, Image, ImageMediaType, Message, Text, UserContent,
        };
        use rig::OneOrMany;

        // 1. Persist the user turn to memory using the text-fallback view so
        //    the transcript stays human-readable across resumes.
        let memory_text = if attachments.is_empty() {
            prompt.to_string()
        } else {
            let inlined: String = attachments
                .iter()
                .map(Attachment::to_text_fallback)
                .collect::<Vec<_>>()
                .join("\n\n");
            format!("{inlined}\n\n{prompt}")
        };
        self.memory
            .push(MemoryEntry { role: "user".into(), content: memory_text })
            .await?;

        // 2. Build a native multi-modal Message: text first, then each
        //    attachment as its own UserContent block.
        let mut blocks: Vec<UserContent> = Vec::with_capacity(attachments.len() + 1);
        blocks.push(UserContent::Text(Text { text: prompt.to_string() }));
        for att in attachments {
            match att {
                Attachment::Image { mime_type, data, .. } => {
                    let media = match mime_type.as_str() {
                        "image/jpeg" | "image/jpg" => Some(ImageMediaType::JPEG),
                        "image/png"  => Some(ImageMediaType::PNG),
                        "image/gif"  => Some(ImageMediaType::GIF),
                        "image/webp" => Some(ImageMediaType::WEBP),
                        _            => None,
                    };
                    blocks.push(UserContent::Image(Image {
                        data:       DocumentSourceKind::Base64(data.clone()),
                        media_type: media,
                        detail:     None,
                        additional_params: None,
                    }));
                }
                Attachment::Text { content, .. } => {
                    // Text attachments are inlined as additional Text blocks
                    // (some providers handle multiple Text blocks; falling
                    // back to a single concatenated block is also OK).
                    blocks.push(UserContent::Text(Text { text: content.clone() }));
                }
            }
        }
        let multi_modal = Message::User {
            content: OneOrMany::many(blocks)
                .map_err(|e| BrainError::Completion(format!("multi-modal build: {e}")))?,
        };

        // 3. Stream the response — emit per-delta TextChunk events.
        let agent = AgentBuilder::new(self.model.clone())
            .preamble(&self.config.system_prompt)
            .build();
        let mut stream = agent.stream_prompt(multi_modal).await;

        let mut response = String::new();
        while let Some(chunk_res) = stream.next().await {
            let chunk = chunk_res
                .map_err(|e| BrainError::Completion(e.to_string()))?;
            if let MultiTurnStreamItem::StreamAssistantItem(
                StreamedAssistantContent::Text(t)
            ) = chunk {
                self.event_sink
                    .emit(AgentEvent::TextChunk { content: t.text.clone() })
                    .await;
                response.push_str(&t.text);
            }
        }

        self.memory
            .push(MemoryEntry { role: "assistant".into(), content: response.clone() })
            .await?;
        self.event_sink
            .emit(AgentEvent::Done { response: response.clone() })
            .await;
        Ok(response)
    }

    /// Build the full prompt context: active insights + code snippets + conversation.
    ///
    /// Insight prefix is `"global"` + the loop's scope (currently global only).
    async fn build_context(&self, prompt: &str) -> Result<String, BrainError> {
        let entries = self.memory.recall(prompt, self.config.memory_context_limit).await?;
        // De-dupe + filter low-value entries so small models don't get
        // tricked into echoing past canned replies.  Drop:
        // - tool observations (role == "tool") — noisy, not useful
        //   for next-turn continuity
        // - exact-duplicate consecutive (role, content) pairs
        // - assistant lines that look like a pure greeting and would
        //   bias the model toward repeating them.
        let is_canned_greeting = |s: &str| {
            let t = s.trim().to_lowercase();
            t.len() < 80 && (
                t.starts_with("xin chào") || t.starts_with("chào bạn")
                || t.starts_with("hello") || t.starts_with("hi ")
                || t.contains("tôi có thể giúp")
                || t.contains("how can i help")
                || t.contains("how may i help")
            )
        };
        // Also drop refusal patterns — these "I can't access your files /
        // please paste the code" replies poison the small model's future
        // turns: it sees its own past refusal and just repeats it.
        let is_refusal = |s: &str| {
            let t = s.to_lowercase();
            t.contains("xin vui lòng cung cấp")
                || t.contains("không có quyền truy cập")
                || t.contains("vui lòng cung cấp nội dung")
                || t.contains("don't have access")
                || t.contains("do not have access")
                || t.contains("please provide the")
                || t.contains("please share the")
                || t.contains("could you provide")
                || t.contains("please paste")
                || t.contains("không thể trực tiếp")
                || t.contains("không thể thực hiện yêu cầu")
                || t.contains("sao chép và dán")
        };
        // Some models parrot the prompt's few-shot example wording back into
        // their replies.  Drop those so future turns don't see fake data.
        let is_parroted_example = |s: &str| {
            let t = s.to_lowercase();
            t.contains("turn 1 —") || t.contains("turn 2 —")
                || t.contains("turn 1 -") || t.contains("turn 2 -")
                || (t.contains("after tool returns") && t.contains("you:"))
        };
        // DO NOT drop role=="tool" entries — those are the observations
        // the model needs in order to synthesise an answer from a prior
        // turn's tool call.  Dropping them caused the model to loop:
        // it saw its own tool request but never the result, so it kept
        // re-emitting the request thinking it had no data yet.
        let mut dedup: Vec<&owl_protocol::memory::MemoryEntry> = Vec::new();
        for e in &entries {
            if e.role == "assistant" && is_canned_greeting(&e.content)    { continue; }
            if e.role == "assistant" && is_refusal(&e.content)            { continue; }
            if e.role == "assistant" && is_parroted_example(&e.content)   { continue; }
            if let Some(prev) = dedup.last() {
                if prev.role == e.role && prev.content == e.content { continue; }
            }
            dedup.push(e);
        }
        let transcript = dedup
            .iter()
            .map(|e| format!("{}: {}", e.role, e.content))
            .collect::<Vec<_>>()
            .join("\n");

        let mut parts: Vec<String> = Vec::new();

        // Prepend active anti-pattern insights so the agent avoids known pitfalls.
        //
        // Insights are RELEVANCE-GATED: a distilled observation about
        // "user learning Rust" must NOT prepend itself to every conversation
        // — that biases the agent toward Rust answers for unrelated chat.
        // Only inject insights whose summary shares a keyword with the
        // current user prompt (case-insensitive, ≥ 4-char tokens) AND that
        // are AntiPattern kind (Pattern / Rule insights describe positive
        // habits the agent should follow implicitly, not warn about).
        if let Some(exp) = &self.experience {
            match exp.recall_insights("global").await {
                Ok(insights) => {
                    let prompt_lower = prompt.to_lowercase();
                    let prompt_tokens: std::collections::HashSet<&str> =
                        prompt_lower.split(|c: char| !c.is_alphanumeric())
                            .filter(|t| t.len() >= 4)
                            .collect();
                    let warnings: Vec<String> = insights
                        .iter()
                        .filter(|i| matches!(i.kind, InsightKind::AntiPattern))
                        .filter(|i| {
                            // Skip injection unless the insight's summary shares
                            // at least one ≥ 4-char token with the user's prompt.
                            let lower = i.summary.to_lowercase();
                            prompt_tokens.iter().any(|t| lower.contains(t))
                        })
                        .map(|i| format!("⚠ {}", i.summary))
                        .collect();
                    if !warnings.is_empty() {
                        parts.push(format!(
                            "<relevant_pitfalls>\n{}\n</relevant_pitfalls>",
                            warnings.join("\n")
                        ));
                    }
                }
                Err(e) => warn!(err = %e, "insight recall failed (non-fatal)"),
            }
        }

        // Code graph context.
        let mut code_chunks = 0usize;
        if let Some(provider) = &self.context {
            match provider.retrieve(prompt).await {
                Ok(chunks) if !chunks.is_empty() => {
                    code_chunks = chunks.len();
                    parts.push(format!(
                        "<code_context>\n{}\n</code_context>",
                        chunks.join("\n---\n")
                    ));
                }
                Err(e) => warn!(err = %e, "context retrieval failed (non-fatal)"),
                _ => {}
            }
        }

        // Diagnostics: surface what the agent actually saw so the user can
        // tell whether SurrealDB recall + graph context are wired.
        info!(
            mem_entries = dedup.len(),
            insights_injected = parts.iter().any(|p| p.starts_with("<relevant_pitfalls>")),
            code_chunks,
            has_ctx_provider = self.context.is_some(),
            has_exp_store = self.experience.is_some(),
            prompt_chars = prompt.len(),
            "build_context"
        );

        parts.push(transcript);
        Ok(parts.join("\n\n"))
    }
}

/// Advance the state machine by one step.
fn transition(state: AgentState) -> AgentState {
    match state {
        AgentState::Idle      => AgentState::Planning,
        AgentState::Planning  => AgentState::Acting,
        AgentState::Acting    => AgentState::Observing,
        AgentState::Observing => AgentState::Idle,
    }
}

/// Dispatch a tool call and record the observation in memory.
#[allow(dead_code)] // kept for tests / single-call code paths; parallel path is the default
async fn dispatch_tool(
    call: ToolCall,
    executor: &dyn ToolExecutor,
    memory: &dyn MemoryStore,
) -> Result<(), BrainError> {
    let result = executor.execute(call).await?;
    let observation = serde_json::to_string(&result)?;
    memory
        .push(MemoryEntry { role: "tool".into(), content: observation })
        .await
        .map_err(Into::into)
}

/// Dispatch multiple tool calls concurrently and record observations in
/// **input order** (not completion order) for deterministic replay.
///
/// Per-call errors from the executor (denied / rejected by approval gate /
/// tool failure) are caught and serialised as `{"error": "…"}` observations
/// — they don't abort the whole batch.  This matches the Anthropic
/// "parallel function calling" semantics where the LLM gets one observation
/// per call regardless of individual outcome.
///
/// A `MemoryStore` write failure DOES abort, since dropping observations
/// silently would corrupt the conversation history.
#[allow(dead_code)] // streaming path supersedes this; kept for tests/legacy
async fn dispatch_tools_parallel(
    calls: Vec<ToolCall>,
    executor: &dyn ToolExecutor,
    memory: &dyn MemoryStore,
) -> Result<(), BrainError> {
    dispatch_tools_parallel_with_sink(calls, executor, memory, &NullSink).await
}

/// Same as [`dispatch_tools_parallel`] but emits an [`AgentEvent::ToolCalled`]
/// per result (in input order) so the host UI can render live progress.
async fn dispatch_tools_parallel_with_sink(
    calls: Vec<ToolCall>,
    executor: &dyn ToolExecutor,
    memory: &dyn MemoryStore,
    sink: &dyn EventSink,
) -> Result<(), BrainError> {
    dispatch_tools_parallel_with_sink_budgeted(calls, executor, memory, sink, usize::MAX).await
}

/// Variant accepting a `tool_result_max_bytes` cap — observations fed back
/// to the LLM are truncated to this many bytes so small models don't
/// drown in raw command output.
async fn dispatch_tools_parallel_with_sink_budgeted(
    calls: Vec<ToolCall>,
    executor: &dyn ToolExecutor,
    memory: &dyn MemoryStore,
    sink: &dyn EventSink,
    max_bytes: usize,
) -> Result<(), BrainError> {
    use futures_util::future::join_all;

    let calls_for_err: Vec<ToolCall> = calls.clone();
    let futures = calls.into_iter().map(|c| async move {
        executor.execute(c).await
    });
    let results = join_all(futures).await;

    for (i, r) in results.into_iter().enumerate() {
        let (tool_name, raw) = match &r {
            Ok(result) => {
                sink.emit(AgentEvent::ToolCalled { result: result.clone() }).await;
                (result.name.clone(), serde_json::to_string(&result.output)?)
            }
            Err(e) => {
                let synthetic = ToolResult::err(&calls_for_err[i].name, e.to_string());
                sink.emit(AgentEvent::ToolCalled { result: synthetic }).await;
                (
                    calls_for_err[i].name.clone(),
                    serde_json::to_string(&serde_json::json!({ "error": e.to_string() }))?,
                )
            }
        };
        let pretty = pretty_print_observation(&raw);
        let observation = truncate_for_model(&pretty, max_bytes);
        // Prefix with the tool name + an instruction to make the result
        // unmistakable in the transcript — small models often skim past
        // raw JSON blobs and parrot prompt examples instead.
        let labelled = format!(
            "RESULT FROM TOOL [{tool_name}] — use these exact values in your next reply:\n{observation}"
        );
        memory
            .push(MemoryEntry { role: "tool".into(), content: labelled })
            .await?;
    }
    Ok(())
}

/// Pretty-print a tool result so it's readable in the transcript.
/// Small models reason better over indented JSON than dense one-liners.
fn pretty_print_observation(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .and_then(|v| serde_json::to_string_pretty(&v))
        .unwrap_or_else(|_| raw.to_string())
}

/// Strip any leftover `{"tool":...}` JSON blocks + ```json fences from a
/// model text response.  Used by the synthesis fallback so the user
/// sees a clean text reply even if the model still emits stray tool JSON.
fn strip_tool_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Skip ```json fences entirely (open + content + close).
        if bytes[i..].starts_with(b"```") {
            if let Some(end) = text[i + 3..].find("```") {
                i = i + 3 + end + 3;
                continue;
            } else {
                break; // unterminated fence — bail
            }
        }
        // Skip any standalone `{"tool":...}` JSON block.
        if bytes[i] == b'{' {
            let start = i;
            let mut depth = 0i32;
            let mut in_string = false;
            let mut escape = false;
            let mut end = start;
            for (j, &ch) in bytes[start..].iter().enumerate() {
                if escape { escape = false; continue; }
                if ch == b'\\' && in_string { escape = true; continue; }
                if ch == b'"' { in_string = !in_string; continue; }
                if in_string { continue; }
                if ch == b'{' { depth += 1; }
                if ch == b'}' {
                    depth -= 1;
                    if depth == 0 { end = start + j + 1; break; }
                }
            }
            if end > start {
                let candidate = &text[start..end];
                if candidate.contains("\"tool\"") {
                    i = end;
                    continue;
                }
            }
        }
        out.push(text[i..].chars().next().unwrap());
        i += text[i..].chars().next().unwrap().len_utf8();
    }
    out.trim().to_string()
}

/// Truncate a tool observation to `max_bytes`, keeping the head + a
/// `…(truncated N bytes)…` tail marker so the model knows what's missing.
/// `usize::MAX` disables truncation entirely.
fn truncate_for_model(s: &str, max_bytes: usize) -> String {
    if max_bytes == usize::MAX || s.len() <= max_bytes {
        return s.to_string();
    }
    // Cut on a char boundary to avoid producing invalid UTF-8.
    let cut = s.char_indices()
        .map(|(i, _)| i)
        .take_while(|i| *i <= max_bytes)
        .last()
        .unwrap_or(0);
    let dropped = s.len() - cut;
    format!("{}\n…(truncated {dropped} bytes)…", &s[..cut])
}

#[async_trait]
impl<M> AgentRunner for ReasoningLoop<M>
where
    M: rig::completion::CompletionModel + Clone + Send + Sync + 'static,
    <M as rig::completion::CompletionModel>::StreamingResponse: rig::completion::GetTokenUsage,
{
    async fn run(&self, prompt: &str) -> Result<String, BrainError> {
        ReasoningLoop::run(self, prompt).await
    }

    async fn run_with_attachments(
        &self,
        prompt: &str,
        attachments: &[owl_protocol::attachment::Attachment],
    ) -> Result<String, BrainError> {
        if attachments.is_empty() {
            return self.run(prompt).await;
        }
        ReasoningLoop::run_with_attachments_impl(self, prompt, attachments).await
    }
}

/// Attempt to parse a tool call JSON block from a model response.
///
/// Backwards-compatible wrapper over [`try_parse_tool_calls`] — returns the
/// first parsed call (if any).  Kept for tests and any external caller that
/// only expects a single dispatch.
#[allow(dead_code)]
fn try_parse_tool_call(text: &str) -> Option<ToolCall> {
    try_parse_tool_calls(text).into_iter().next()
}

/// Find ALL balanced `{…}` blocks in `text` that deserialise as
/// `{"tool": "…", "args": {…}}` and return them in document order.
///
/// Enables Anthropic-style parallel tool calls: when the LLM emits multiple
/// tool invocations in one response, the loop dispatches them concurrently
/// and records observations in input order.
///
/// Robust against prose-wrapped JSON: unrelated JSON blocks that don't
/// match the `{tool, args}` shape are skipped, not aborted on.  Inner
/// nested objects are walked through string-escape state so braces inside
/// string literals don't break balancing.
fn try_parse_tool_calls(text: &str) -> Vec<ToolCall> {
    #[derive(serde::Deserialize)]
    struct Raw {
        tool: String,
        args: serde_json::Value,
    }

    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        // Extract balanced braces.
        let start = i;
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escape = false;
        let mut end = start;
        for (j, &ch) in bytes[start..].iter().enumerate() {
            if escape { escape = false; continue; }
            if ch == b'\\' && in_string { escape = true; continue; }
            if ch == b'"' { in_string = !in_string; continue; }
            if in_string { continue; }
            if ch == b'{' { depth += 1; }
            if ch == b'}' {
                depth -= 1;
                if depth == 0 {
                    end = start + j + 1;
                    break;
                }
            }
        }
        if depth != 0 {
            i += 1;
            continue;
        }
        let candidate = &text[start..end];
        if let Ok(raw) = serde_json::from_str::<Raw>(candidate) {
            out.push(ToolCall { name: raw.tool, args: raw.args });
        }
        i = end;
    }
    out
}
