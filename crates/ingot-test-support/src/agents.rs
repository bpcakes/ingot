use ingot_domain::agent::{Agent, AgentCapability};
use ingot_domain::test_support::AgentBuilder;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestAgentProfile {
    Mutating,
    ReviewOnly,
    Full,
}

impl TestAgentProfile {
    fn capabilities(self) -> Vec<AgentCapability> {
        match self {
            Self::Mutating => vec![
                AgentCapability::MutatingJobs,
                AgentCapability::StructuredOutput,
            ],
            Self::ReviewOnly => vec![
                AgentCapability::ReadOnlyJobs,
                AgentCapability::StructuredOutput,
            ],
            Self::Full => vec![
                AgentCapability::MutatingJobs,
                AgentCapability::ReadOnlyJobs,
                AgentCapability::StructuredOutput,
            ],
        }
    }
}

pub fn agent_fixture(name: &str, profile: TestAgentProfile) -> Agent {
    AgentBuilder::new(name, profile.capabilities()).build()
}
