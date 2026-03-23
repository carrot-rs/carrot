use std::sync::Arc;

use inazuma::{App, Global};

use crate::agent::SharedCliAgent;

/// App-wide registry of every `CliAgent` implementation. Lives as an
/// Inazuma `Global`; installed in `cli_agents::init`.
///
/// The registry is append-only during app lifetime — agents register
/// themselves in their own `init()` functions (see
/// `agents/claude_code.rs`) and never deregister.
#[derive(Default)]
pub struct CliAgentRegistry {
    agents: Vec<SharedCliAgent>,
}

impl CliAgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, agent: SharedCliAgent) {
        self.agents.push(agent);
    }

    pub fn agents(&self) -> &[SharedCliAgent] {
        &self.agents
    }

    pub fn agent_by_id(&self, id: &str) -> Option<SharedCliAgent> {
        self.agents
            .iter()
            .find(|agent| agent.id() == id)
            .map(Arc::clone)
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn global_mut(cx: &mut App) -> &mut Self {
        cx.global_mut::<Self>()
    }
}

impl Global for CliAgentRegistry {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{
        BlockOutputSnapshot, CliAgent, CliAgentCapabilities, CliAgentMatch, ProcessInfo,
    };
    use crate::hook_events::CliAgentHookEvent;
    use crate::session::CliAgentSessionState;
    use std::path::{Path, PathBuf};

    struct FakeAgent;

    impl CliAgent for FakeAgent {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn display_name(&self) -> &'static str {
            "Fake"
        }
        fn binary_names(&self) -> &'static [&'static str] {
            &["fake"]
        }
        fn icon_path(&self) -> &'static str {
            "icons/agents/fake.svg"
        }
        fn capabilities(&self) -> CliAgentCapabilities {
            CliAgentCapabilities::empty()
        }
        fn classify(&self, _process: &ProcessInfo, _cx: &App) -> Option<CliAgentMatch> {
            None
        }
        fn state_from_hook(&self, _event: &CliAgentHookEvent) -> Option<CliAgentSessionState> {
            None
        }
        fn state_from_output(
            &self,
            _snapshot: &BlockOutputSnapshot,
        ) -> Option<CliAgentSessionState> {
            None
        }
        fn session_transcript_path(&self, _cwd: &Path) -> Option<PathBuf> {
            None
        }
    }

    #[test]
    fn register_and_lookup() {
        let mut registry = CliAgentRegistry::new();
        let agent: SharedCliAgent = Arc::new(FakeAgent);
        registry.register(agent);
        assert_eq!(registry.agents().len(), 1);
        assert!(registry.agent_by_id("fake").is_some());
        assert!(registry.agent_by_id("missing").is_none());
    }
}
