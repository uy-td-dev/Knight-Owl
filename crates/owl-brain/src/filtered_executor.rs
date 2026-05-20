//! Tool-allowlist + policy wrapper around any [`ToolExecutor`].
//!
//! Used by sub-agent spawning + workflow runs: a child agent's `agent.toml`
//! lists `allowed_tools = [...]`, the host wraps the parent's executor with
//! [`FilteredExecutor`] using that allowlist, and the wrapped executor
//! refuses any call whose tool name is outside the set.
//!
//! Behaviour:
//! - Empty allowlist → **all tools allowed** (back-compat with the
//!   pre-orchestra single-agent flow where tool filtering didn't exist).
//! - Tool not in allowlist → returns [`BrainError::ToolDispatch`] with a
//!   helpful message naming the tool + allowed set.  Engine surfaces this
//!   as a `tool_response` with `error: true` so the LLM can recover.
//! - `has_tool` reports the filtered view — the LLM only sees what it can
//!   actually call (no surprise rejections mid-conversation).
//!
//! Approval gates (R-21 spirit):
//! - When constructed via [`FilteredExecutor::with_policy`], each tool can
//!   carry a [`ToolPolicy`].  Calls to [`ToolPolicy::RequireApproval`] tools
//!   pause and consult the injected [`ApprovalGate`]; rejection surfaces
//!   as a recoverable `ToolDispatch` error so the LLM can pivot.
//! - Tools with [`ToolPolicy::Deny`] are refused outright (same as missing
//!   from the allowlist).
//! - Tools missing from the policy map use `default_policy` (defaults to
//!   [`ToolPolicy::AutoApprove`] for back-compat).
//!
//! This wrapper is generic and stateless — it does not allocate per call.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use owl_protocol::tools::{ToolCall, ToolPolicy, ToolResult};

use crate::approval::{ApprovalGate, PermissiveGate};
use crate::error::BrainError;
use crate::reasoning_loop::ToolExecutor;

/// Wraps an inner executor and gates dispatch by tool-name allowlist + policy.
pub struct FilteredExecutor {
    inner:    Arc<dyn ToolExecutor>,
    allowed:  HashSet<String>,
    policies: HashMap<String, ToolPolicy>,
    default_policy: ToolPolicy,
    gate:     Arc<dyn ApprovalGate>,
}

impl FilteredExecutor {
    /// Build a filter from any iterable of names.
    ///
    /// An empty `allowed` set means "no filtering" — every call passes
    /// through to the inner executor.  No per-tool policies are applied;
    /// the default approval gate is [`PermissiveGate`].
    pub fn new<I, S>(inner: Arc<dyn ToolExecutor>, allowed: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            inner,
            allowed: allowed.into_iter().map(Into::into).collect(),
            policies: HashMap::new(),
            default_policy: ToolPolicy::default(),
            gate: Arc::new(PermissiveGate),
        }
    }

    /// Attach per-tool [`ToolPolicy`] map + an [`ApprovalGate`].
    ///
    /// Tools not present in `policies` use `default_policy`.  Calls to
    /// tools marked [`ToolPolicy::RequireApproval`] consult `gate` before
    /// being dispatched; rejection surfaces as a recoverable `ToolDispatch`
    /// error so the agent can pivot or report it to the user.
    pub fn with_policy(
        mut self,
        policies: HashMap<String, ToolPolicy>,
        default_policy: ToolPolicy,
        gate: Arc<dyn ApprovalGate>,
    ) -> Self {
        self.policies = policies;
        self.default_policy = default_policy;
        self.gate = gate;
        self
    }

    /// True if `name` would pass the allowlist filter (or if the filter is empty).
    pub fn allows(&self, name: &str) -> bool {
        self.allowed.is_empty() || self.allowed.contains(name)
    }

    /// Iterate the active allowlist (sorted for stable error messages).
    fn allowed_sorted(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.allowed.iter().map(String::as_str).collect();
        v.sort_unstable();
        v
    }

    /// Resolve the policy for `name`.  Falls back to `default_policy`.
    fn policy_for(&self, name: &str) -> ToolPolicy {
        self.policies.get(name).copied().unwrap_or(self.default_policy)
    }
}

#[async_trait]
impl ToolExecutor for FilteredExecutor {
    async fn execute(&self, call: ToolCall) -> Result<ToolResult, BrainError> {
        if !self.allows(&call.name) {
            let allowed = self.allowed_sorted().join(", ");
            return Err(BrainError::ToolDispatch(format!(
                "tool `{}` is not in this agent's allowlist (allowed: [{}])",
                call.name, allowed
            )));
        }
        match self.policy_for(&call.name) {
            ToolPolicy::Deny => {
                return Err(BrainError::ToolDispatch(format!(
                    "tool `{}` is denied by policy",
                    call.name
                )));
            }
            ToolPolicy::RequireApproval => {
                let decision = self.gate.request(&call, None).await;
                if !decision.is_approved() {
                    return Err(BrainError::ToolDispatch(format!(
                        "tool `{}` was rejected by the user",
                        call.name
                    )));
                }
            }
            ToolPolicy::AutoApprove => {}
        }
        self.inner.execute(call).await
    }

    fn has_tool(&self, name: &str) -> bool {
        self.allows(name)
            && self.policy_for(name) != ToolPolicy::Deny
            && self.inner.has_tool(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval::{ApprovalDecision, ApprovalGate};
    use owl_protocol::tools::ToolCall;

    /// Test double — succeeds with a fixed payload, records the last call.
    struct Echo;

    #[async_trait]
    impl ToolExecutor for Echo {
        async fn execute(&self, call: ToolCall) -> Result<ToolResult, BrainError> {
            Ok(ToolResult::ok(&call.name, serde_json::json!({ "echoed": call.name })))
        }
        fn has_tool(&self, _name: &str) -> bool { true }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall { name: name.to_string(), args: serde_json::json!({}) }
    }

    #[tokio::test]
    async fn empty_allowlist_passes_everything() {
        let f = FilteredExecutor::new(Arc::new(Echo), Vec::<String>::new());
        let out = f.execute(call("read_file")).await.expect("should pass");
        assert_eq!(out.name, "read_file");
    }

    #[tokio::test]
    async fn allowed_tool_passes() {
        let f = FilteredExecutor::new(Arc::new(Echo), ["read_file", "grep"]);
        let out = f.execute(call("read_file")).await.expect("should pass");
        assert_eq!(out.name, "read_file");
    }

    #[tokio::test]
    async fn disallowed_tool_rejected() {
        let f = FilteredExecutor::new(Arc::new(Echo), ["read_file"]);
        let err = f.execute(call("bash")).await
            .expect_err("disallowed tool should error");
        match err {
            BrainError::ToolDispatch(msg) => {
                assert!(msg.contains("`bash`"));
                assert!(msg.contains("read_file"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn has_tool_respects_filter() {
        let f = FilteredExecutor::new(Arc::new(Echo), ["read_file"]);
        assert!( f.has_tool("read_file"));
        assert!(!f.has_tool("bash"));
    }

    /// Gate that records calls and returns a fixed decision.
    struct ScriptedGate { approved: bool, calls: tokio::sync::Mutex<Vec<String>> }

    #[async_trait]
    impl ApprovalGate for ScriptedGate {
        async fn request(&self, c: &ToolCall, _r: Option<&str>) -> ApprovalDecision {
            self.calls.lock().await.push(c.name.clone());
            ApprovalDecision::from_bool(self.approved)
        }
    }

    #[tokio::test]
    async fn require_approval_consults_gate_and_passes() {
        let gate = Arc::new(ScriptedGate { approved: true, calls: tokio::sync::Mutex::new(vec![]) });
        let mut policies = HashMap::new();
        policies.insert("bash".into(), ToolPolicy::RequireApproval);
        let f = FilteredExecutor::new(Arc::new(Echo), Vec::<String>::new())
            .with_policy(policies, ToolPolicy::AutoApprove, gate.clone());
        let out = f.execute(call("bash")).await.expect("approved");
        assert_eq!(out.name, "bash");
        assert_eq!(gate.calls.lock().await.as_slice(), &["bash".to_string()]);
    }

    #[tokio::test]
    async fn require_approval_rejection_surfaces_error() {
        let gate = Arc::new(ScriptedGate { approved: false, calls: tokio::sync::Mutex::new(vec![]) });
        let mut policies = HashMap::new();
        policies.insert("bash".into(), ToolPolicy::RequireApproval);
        let f = FilteredExecutor::new(Arc::new(Echo), Vec::<String>::new())
            .with_policy(policies, ToolPolicy::AutoApprove, gate);
        let err = f.execute(call("bash")).await.expect_err("rejected");
        match err {
            BrainError::ToolDispatch(m) => assert!(m.contains("rejected by the user")),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn deny_policy_blocks_without_consulting_gate() {
        let gate = Arc::new(ScriptedGate { approved: true, calls: tokio::sync::Mutex::new(vec![]) });
        let mut policies = HashMap::new();
        policies.insert("rm".into(), ToolPolicy::Deny);
        let f = FilteredExecutor::new(Arc::new(Echo), Vec::<String>::new())
            .with_policy(policies, ToolPolicy::AutoApprove, gate.clone());
        let err = f.execute(call("rm")).await.expect_err("denied");
        assert!(matches!(err, BrainError::ToolDispatch(_)));
        assert!(gate.calls.lock().await.is_empty(), "gate should not be called for denied tools");
    }

    #[test]
    fn has_tool_with_empty_filter_falls_through() {
        let f = FilteredExecutor::new(Arc::new(Echo), Vec::<String>::new());
        assert!(f.has_tool("anything"));
    }
}
