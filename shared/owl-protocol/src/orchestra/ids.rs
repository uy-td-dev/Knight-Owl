//! Newtype identifiers — enforced at the type system to prevent passing an
//! `AgentId` where a `SkillId` is expected.
//!
//! All ids share a single validation rule (see [`is_valid_id`]) so file system
//! paths derived from them are always safe.

use serde::{Deserialize, Serialize};

use super::error::OrchestraProtoError;

/// Validate an entity id against the canonical pattern:
/// `^[a-z][a-z0-9_-]{1,63}$` — must start with a letter, 2..=64 chars,
/// lowercase letters / digits / underscore / hyphen only.
///
/// The pattern guarantees:
/// - Safe to use as a directory or filename on every supported OS.
/// - No collisions on case-insensitive filesystems.
/// - No leading hyphen (avoids being parsed as a CLI flag).
pub fn is_valid_id(s: &str) -> bool {
    if s.is_empty() || s.len() > 64 { return false; }
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Reserved ids that user-defined entities may not use.
pub const RESERVED_IDS: &[&str] = &["default", "system", "none", "owl", "knight"];

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// Construct after full validation; rejects reserved ids.
            pub fn new(s: impl Into<String>) -> Result<Self, OrchestraProtoError> {
                let s = s.into();
                if !is_valid_id(&s) {
                    return Err(OrchestraProtoError::InvalidId(s));
                }
                if RESERVED_IDS.contains(&s.as_str()) {
                    return Err(OrchestraProtoError::ReservedId(s));
                }
                Ok(Self(s))
            }

            /// Construct without rejecting reserved ids — used by the runtime
            /// to materialise built-in entities such as the `default` agent.
            pub fn new_reserved(s: impl Into<String>) -> Result<Self, OrchestraProtoError> {
                let s = s.into();
                if !is_valid_id(&s) {
                    return Err(OrchestraProtoError::InvalidId(s));
                }
                Ok(Self(s))
            }

            /// Borrow as `&str`.
            pub fn as_str(&self) -> &str { &self.0 }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str { &self.0 }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_type!(
    /// Stable identifier of an agent (folder name under `.knight-owl/agents/`).
    AgentId
);
id_type!(
    /// Stable identifier of a skill (filename stem under `.knight-owl/skills/`).
    SkillId
);
id_type!(
    /// Stable identifier of a workflow (filename stem under `.knight-owl/workflows/`).
    WorkflowId
);
id_type!(
    /// Stable identifier of a slash command (filename stem under `.knight-owl/commands/`).
    CommandId
);
id_type!(
    /// Identifier of a step inside a workflow — unique within that workflow only.
    StepId
);

/// Tool name as registered in `owl-armory` (e.g. `"read_file"`).
///
/// Not validated by [`is_valid_id`] — tool names follow snake_case but are
/// owned by the armory crate's registry, not by the orchestra spec.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct ToolName(pub String);

impl ToolName {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}
impl AsRef<str> for ToolName { fn as_ref(&self) -> &str { &self.0 } }
impl std::fmt::Display for ToolName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}

/// Provider-qualified model identifier, e.g. `"gemini-2.5-flash"`.
///
/// Provider routing happens via [`super::agent::ProviderRef`]; this carries
/// only the bare model id string the provider SDK expects.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct ModelRef(pub String);

impl ModelRef {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

/// Run-scoped trace id — propagated from a workflow root through every nested
/// agent / tool / sub-agent so harness can correlate events.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct TraceId(pub String);

impl TraceId {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_ids_accepted() {
        for ok in &["a", "researcher", "code-reviewer", "v2_agent", "x9"] {
            assert!(is_valid_id(ok), "{ok} should be valid");
        }
    }

    #[test]
    fn invalid_ids_rejected() {
        for bad in &["", "A", "1agent", "-foo", "foo bar", "foo.bar", "ñ", "x".repeat(65).as_str()] {
            assert!(!is_valid_id(bad), "{bad:?} should be invalid");
        }
    }

    #[test]
    fn reserved_ids_blocked_by_new_but_allowed_by_new_reserved() {
        for r in RESERVED_IDS {
            assert!(matches!(
                AgentId::new(*r),
                Err(OrchestraProtoError::ReservedId(_))
            ));
            assert!(AgentId::new_reserved(*r).is_ok());
        }
    }

    #[test]
    fn round_trips_through_serde() {
        let id = AgentId::new("researcher").unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, r#""researcher""#);
        let back: AgentId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }
}
