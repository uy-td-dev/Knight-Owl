//! Spec loaders.
//!
//! - [`FsLoader`] — production filesystem loader.
//! - [`InMemoryLoader`] — string-keyed, used by tests / harness so spec
//!   parsing logic exercises identical code paths in both contexts.
//!
//! The trait deliberately operates on **a single path** at a time; folder
//! traversal + cross-reference resolution happen in [`crate::registry`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use serde::Deserialize;

use owl_protocol::orchestra::{
    AgentSpec, CommandSpec, OrchestraProtoError, ProviderRef, SkillSpec, WorkflowSpec,
    SCHEMA_VERSION,
};

use crate::error::{FrontmatterErrorKind, OrchestraError};
use crate::frontmatter;
use crate::path::agent_files;

/// Stateless spec parser — implementations differ only in how they fetch
/// raw bytes (filesystem vs in-memory map).
#[async_trait]
pub trait Loader: Send + Sync {
    /// Load an agent from a directory containing `agent.toml` + `system.md`.
    async fn load_agent(&self, dir: &Path) -> Result<AgentSpec, OrchestraError>;

    /// Load a single skill `.md` file.
    async fn load_skill(&self, file: &Path) -> Result<SkillSpec, OrchestraError>;

    /// Load a single workflow `.toml` file.
    async fn load_workflow(&self, file: &Path) -> Result<WorkflowSpec, OrchestraError>;

    /// Load a single command `.md` file.
    async fn load_command(&self, file: &Path) -> Result<CommandSpec, OrchestraError>;
}

// ─── FsLoader (production) ───────────────────────────────────────────────────

/// Filesystem-backed loader.
#[derive(Debug, Default, Clone)]
pub struct FsLoader;

#[async_trait]
impl Loader for FsLoader {
    async fn load_agent(&self, dir: &Path) -> Result<AgentSpec, OrchestraError> {
        let toml_path   = dir.join(agent_files::AGENT_TOML);
        let system_path = dir.join(agent_files::SYSTEM_MD);

        let toml_text = read_file(&toml_path).await?;
        let system_md = read_file(&system_path).await
            .map_err(|_| OrchestraError::MissingFile {
                agent_dir: dir.to_path_buf(),
                file:      agent_files::SYSTEM_MD,
            })?;

        let raw: RawAgent = toml::from_str(&toml_text)
            .map_err(|e| OrchestraError::Toml { path: toml_path.clone(), source: e })?;
        let spec = raw.into_spec(system_md, &toml_path)?;

        // Folder name must match declared id.
        if let Some(folder_name) = dir.file_name().and_then(|n| n.to_str()) {
            if folder_name != spec.id.as_str() {
                return Err(OrchestraError::IdMismatch {
                    folder:   folder_name.to_string(),
                    declared: spec.id.0.clone(),
                    path:     toml_path,
                });
            }
        }
        Ok(spec)
    }

    async fn load_skill(&self, file: &Path) -> Result<SkillSpec, OrchestraError> {
        let text = read_file(file).await?;
        parse_skill(&text, file)
    }

    async fn load_workflow(&self, file: &Path) -> Result<WorkflowSpec, OrchestraError> {
        let text = read_file(file).await?;
        let raw: RawWorkflow = toml::from_str(&text)
            .map_err(|e| OrchestraError::Toml { path: file.to_path_buf(), source: e })?;
        raw.into_spec(file)
    }

    async fn load_command(&self, file: &Path) -> Result<CommandSpec, OrchestraError> {
        let text = read_file(file).await?;
        parse_command(&text, file)
    }
}

async fn read_file(path: &Path) -> Result<String, OrchestraError> {
    tokio::fs::read_to_string(path).await
        .map_err(|e| OrchestraError::Io { path: path.to_path_buf(), source: e })
}

// ─── InMemoryLoader (tests / harness) ────────────────────────────────────────

/// Loader that resolves paths against an in-memory `HashMap<PathBuf, String>`.
/// Useful in unit tests where touching the real filesystem would slow things
/// down or pollute the workspace.
pub struct InMemoryLoader {
    files: Mutex<HashMap<PathBuf, String>>,
}

impl InMemoryLoader {
    pub fn new() -> Self { Self { files: Mutex::new(HashMap::new()) } }

    /// Add (or replace) a virtual file.
    pub fn put(&self, path: impl Into<PathBuf>, contents: impl Into<String>) {
        self.files.lock().unwrap().insert(path.into(), contents.into());
    }
}

impl Default for InMemoryLoader {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl Loader for InMemoryLoader {
    async fn load_agent(&self, dir: &Path) -> Result<AgentSpec, OrchestraError> {
        let toml_path   = dir.join(agent_files::AGENT_TOML);
        let system_path = dir.join(agent_files::SYSTEM_MD);
        let files = self.files.lock().unwrap();
        let toml_text = files.get(&toml_path)
            .ok_or_else(|| OrchestraError::Io {
                path: toml_path.clone(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "not in InMemoryLoader"),
            })?
            .clone();
        let system_md = files.get(&system_path)
            .ok_or_else(|| OrchestraError::MissingFile {
                agent_dir: dir.to_path_buf(),
                file:      agent_files::SYSTEM_MD,
            })?
            .clone();
        drop(files);

        let raw: RawAgent = toml::from_str(&toml_text)
            .map_err(|e| OrchestraError::Toml { path: toml_path.clone(), source: e })?;
        let spec = raw.into_spec(system_md, &toml_path)?;

        if let Some(folder_name) = dir.file_name().and_then(|n| n.to_str()) {
            if folder_name != spec.id.as_str() {
                return Err(OrchestraError::IdMismatch {
                    folder:   folder_name.to_string(),
                    declared: spec.id.0.clone(),
                    path:     toml_path,
                });
            }
        }
        Ok(spec)
    }

    async fn load_skill(&self, file: &Path) -> Result<SkillSpec, OrchestraError> {
        let text = self.read(file)?;
        parse_skill(&text, file)
    }

    async fn load_workflow(&self, file: &Path) -> Result<WorkflowSpec, OrchestraError> {
        let text = self.read(file)?;
        let raw: RawWorkflow = toml::from_str(&text)
            .map_err(|e| OrchestraError::Toml { path: file.to_path_buf(), source: e })?;
        raw.into_spec(file)
    }

    async fn load_command(&self, file: &Path) -> Result<CommandSpec, OrchestraError> {
        let text = self.read(file)?;
        parse_command(&text, file)
    }
}

impl InMemoryLoader {
    fn read(&self, path: &Path) -> Result<String, OrchestraError> {
        self.files.lock().unwrap().get(path).cloned()
            .ok_or_else(|| OrchestraError::Io {
                path: path.to_path_buf(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "not in InMemoryLoader"),
            })
    }
}

// ─── Raw TOML structs (parse target) ─────────────────────────────────────────
//
// These mirror the on-disk file layout and convert into the strongly-typed
// `*Spec` from `owl-protocol`.  Splitting them out lets the file format be
// "wide" (lots of optional fields, defaults) while the runtime spec stays
// tight.

#[derive(Debug, Deserialize)]
struct RawAgent {
    schema_version: u32,
    identity:       RawIdentity,
    #[serde(default)]
    model:          Option<RawModel>,
    #[serde(default)]
    capabilities:   RawCapabilities,
    #[serde(default)]
    budget:         RawBudget,
}

#[derive(Debug, Deserialize)]
struct RawIdentity {
    id:          String,
    name:        String,
    description: String,
}

#[derive(Debug, Deserialize)]
struct RawModel {
    #[serde(default)]
    provider:  Option<String>,
    #[serde(default)]
    id:        Option<String>,
    #[serde(default)]
    max_steps: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
struct RawCapabilities {
    #[serde(default)]
    can_spawn:     bool,
    #[serde(default)]
    allowed_tools: Vec<String>,
    #[serde(default)]
    skills:        Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct RawBudget {
    #[serde(default)]
    max_input_tokens:  Option<u64>,
    #[serde(default)]
    max_output_tokens: Option<u64>,
    #[serde(default)]
    max_depth:         Option<u8>,
}

impl RawAgent {
    fn into_spec(self, system_md: String, _source: &Path) -> Result<AgentSpec, OrchestraError> {
        use owl_protocol::orchestra::{AgentId, ModelRef, ModelSpec, SkillId, ToolName};

        check_schema(self.schema_version)?;

        let id = AgentId::new(&self.identity.id).map_err(OrchestraError::Spec)?;

        // Take model fields by value before constructing the spec — this
        // avoids re-borrowing `self.model` for max_steps further down.
        let (model, max_steps) = match self.model {
            Some(m) => {
                let model = ModelSpec {
                    provider: parse_provider(m.provider.as_deref())?,
                    id:       ModelRef::new(m.id.unwrap_or_default()),
                };
                (Some(model), m.max_steps.unwrap_or(DEFAULT_MAX_STEPS))
            }
            None => (None, DEFAULT_MAX_STEPS),
        };

        let allowed_tools = self.capabilities.allowed_tools.into_iter()
            .map(ToolName::new).collect();
        let skills = self.capabilities.skills.into_iter()
            .map(|s| SkillId::new(&s))
            .collect::<Result<Vec<_>, _>>()
            .map_err(OrchestraError::Spec)?;

        let max_depth = self.budget.max_depth.unwrap_or(0).min(MAX_DEPTH_HARD_CAP);

        Ok(AgentSpec {
            schema_version:    self.schema_version,
            id,
            name:              self.identity.name,
            description:       self.identity.description,
            model,
            system_prompt:     system_md.trim_end().to_string(),
            max_steps,
            allowed_tools,
            skills,
            can_spawn:         self.capabilities.can_spawn,
            max_depth,
            max_input_tokens:  self.budget.max_input_tokens,
            max_output_tokens: self.budget.max_output_tokens,
        })
    }
}

/// Default reasoning-loop budget when an agent omits `[model].max_steps`.
const DEFAULT_MAX_STEPS:    usize = 12;
/// Hardcoded upper bound on `[budget].max_depth` (sub-agent recursion safety).
const MAX_DEPTH_HARD_CAP:   u8    = 3;

fn parse_provider(s: Option<&str>) -> Result<ProviderRef, OrchestraError> {
    Ok(match s.unwrap_or("inherit") {
        "inherit"   => ProviderRef::Inherit,
        "gemini"    => ProviderRef::Gemini,
        "anthropic" => ProviderRef::Anthropic,
        "ollama"    => ProviderRef::Ollama,
        other       => return Err(OrchestraError::Spec(
            OrchestraProtoError::Validation(format!("unknown provider `{other}`"))
        )),
    })
}

fn check_schema(found: u32) -> Result<(), OrchestraError> {
    if found > SCHEMA_VERSION {
        return Err(OrchestraError::Spec(OrchestraProtoError::SchemaVersionTooNew {
            found, supported: SCHEMA_VERSION,
        }));
    }
    Ok(())
}

// Skill ───────────────────────────────────────────────────────────────────────

fn parse_skill(text: &str, source: &Path) -> Result<SkillSpec, OrchestraError> {
    let parsed = frontmatter::parse::<RawSkill>(text)
        .map_err(|e| OrchestraError::Frontmatter {
            path: source.to_path_buf(),
            kind: match e {
                FrontmatterErrorKind::MissingOpening => FrontmatterErrorKind::MissingOpening,
                FrontmatterErrorKind::MissingClosing => FrontmatterErrorKind::MissingClosing,
                FrontmatterErrorKind::Toml(t)        => FrontmatterErrorKind::Toml(t),
            },
        })?;
    parsed.front.into_spec(parsed.body)
}

#[derive(Debug, Deserialize)]
struct RawSkill {
    schema_version: u32,
    id:             String,
    name:           String,
    description:    String,
    #[serde(default)]
    trigger:        Option<String>,
    #[serde(default)]
    recommended_tools: Vec<String>,
}

impl RawSkill {
    fn into_spec(self, body: String) -> Result<SkillSpec, OrchestraError> {
        use owl_protocol::orchestra::{SkillId, ToolName};
        check_schema(self.schema_version)?;
        Ok(SkillSpec {
            schema_version:    self.schema_version,
            id:                SkillId::new(&self.id).map_err(OrchestraError::Spec)?,
            name:              self.name,
            description:       self.description,
            trigger:           self.trigger,
            recommended_tools: self.recommended_tools.into_iter().map(ToolName::new).collect(),
            body,
        })
    }
}

// Workflow ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct RawWorkflow {
    schema_version: u32,
    identity:       RawIdentity,
    #[serde(default)]
    budget:         RawWorkflowBudget,
    #[serde(default)]
    steps:          Vec<RawStep>,
}

#[derive(Debug, Deserialize, Default)]
struct RawWorkflowBudget {
    #[serde(default = "default_timeout")]
    timeout_ms:       u64,
    #[serde(default)]
    max_total_tokens: Option<u64>,
    #[serde(default)]
    on_failure:       Option<String>,
}

fn default_timeout() -> u64 { 300_000 }

#[derive(Debug, Deserialize)]
struct RawStep {
    id:     String,
    agent:  String,
    #[serde(default)]
    depends: Vec<String>,
    prompt: String,
    #[serde(default)]
    on_failure: Option<String>,
}

impl RawWorkflow {
    fn into_spec(self, _source: &Path) -> Result<WorkflowSpec, OrchestraError> {
        use owl_protocol::orchestra::{
            AgentId, StepId, StepSpec, WorkflowId,
        };
        check_schema(self.schema_version)?;

        let trigger = None; // TODO: surface `trigger` field once UI consumes it
        let on_failure = parse_failure(self.budget.on_failure.as_deref())?;

        let steps = self.steps.into_iter()
            .map(|s| -> Result<StepSpec, OrchestraError> {
                Ok(StepSpec {
                    id:     StepId::new_reserved(&s.id).map_err(OrchestraError::Spec)?,
                    agent:  AgentId::new_reserved(&s.agent).map_err(OrchestraError::Spec)?,
                    depends: s.depends.into_iter()
                        .map(|d| StepId::new_reserved(&d).map_err(OrchestraError::Spec))
                        .collect::<Result<Vec<_>, _>>()?,
                    prompt: s.prompt,
                    on_failure: parse_failure_opt(s.on_failure.as_deref())?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Validate: every `depends` references a known step in the same workflow.
        let known: std::collections::HashSet<&str> = steps.iter()
            .map(|s| s.id.as_str()).collect();
        for s in &steps {
            for dep in &s.depends {
                if !known.contains(dep.as_str()) {
                    return Err(OrchestraError::Spec(
                        OrchestraProtoError::UnknownStepDependency {
                            step:    s.id.0.clone(),
                            missing: dep.0.clone(),
                        }
                    ));
                }
            }
        }

        Ok(WorkflowSpec {
            schema_version:   self.schema_version,
            id:               WorkflowId::new(&self.identity.id).map_err(OrchestraError::Spec)?,
            name:             self.identity.name,
            description:      self.identity.description,
            trigger,
            steps,
            on_failure,
            timeout_ms:       self.budget.timeout_ms,
            max_total_tokens: self.budget.max_total_tokens,
        })
    }
}

fn parse_failure(s: Option<&str>) -> Result<owl_protocol::orchestra::FailurePolicy, OrchestraError> {
    use owl_protocol::orchestra::FailurePolicy;
    Ok(match s.unwrap_or("abort") {
        "abort"      => FailurePolicy::Abort,
        "retry_once" => FailurePolicy::RetryOnce,
        "continue"   => FailurePolicy::Continue,
        other        => return Err(OrchestraError::Spec(
            OrchestraProtoError::Validation(format!("unknown failure policy `{other}`"))
        )),
    })
}

fn parse_failure_opt(s: Option<&str>) -> Result<Option<owl_protocol::orchestra::FailurePolicy>, OrchestraError> {
    match s {
        None    => Ok(None),
        Some(v) => parse_failure(Some(v)).map(Some),
    }
}

// Command ─────────────────────────────────────────────────────────────────────

fn parse_command(text: &str, source: &Path) -> Result<CommandSpec, OrchestraError> {
    let parsed = frontmatter::parse::<RawCommand>(text)
        .map_err(|kind| OrchestraError::Frontmatter { path: source.to_path_buf(), kind })?;
    parsed.front.into_spec(parsed.body)
}

#[derive(Debug, Deserialize)]
struct RawCommand {
    schema_version: u32,
    id:             String,
    name:           String,
    description:    String,
    expansion:      RawExpansion,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RawExpansion {
    Text     { template: String },
    Workflow { workflow: String },
    Tool     { name: String, #[serde(default)] args: serde_json::Value },
}

impl RawCommand {
    fn into_spec(self, _body: String) -> Result<CommandSpec, OrchestraError> {
        use owl_protocol::orchestra::{CommandExpansion, CommandId, ToolName, WorkflowId};
        check_schema(self.schema_version)?;
        let expansion = match self.expansion {
            RawExpansion::Text { template } =>
                CommandExpansion::Text { template },
            RawExpansion::Workflow { workflow } =>
                CommandExpansion::Workflow {
                    workflow: WorkflowId::new(&workflow).map_err(OrchestraError::Spec)?,
                },
            RawExpansion::Tool { name, args } =>
                CommandExpansion::Tool { name: ToolName::new(name), args },
        };
        Ok(CommandSpec {
            schema_version: self.schema_version,
            id:             CommandId::new(&self.id).map_err(OrchestraError::Spec)?,
            name:           self.name,
            description:    self.description,
            expansion,
        })
    }
}
