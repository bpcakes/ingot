use ingot_domain::revision::ApprovalPolicy;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct IngotConfig {
    #[serde(default)]
    pub defaults: DefaultsConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DefaultsConfig {
    pub candidate_rework_budget: u32,
    pub integration_rework_budget: u32,
    pub approval_policy: ApprovalPolicy,
    pub overflow_strategy: OverflowStrategy,
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            candidate_rework_budget: 2,
            integration_rework_budget: 2,
            approval_policy: ApprovalPolicy::Required,
            overflow_strategy: OverflowStrategy::Truncate,
        }
    }
}

impl DefaultsConfig {
    pub(crate) fn merge_partial(&mut self, partial: PartialDefaultsConfig) {
        if let Some(candidate_rework_budget) = partial.candidate_rework_budget {
            self.candidate_rework_budget = candidate_rework_budget;
        }
        if let Some(integration_rework_budget) = partial.integration_rework_budget {
            self.integration_rework_budget = integration_rework_budget;
        }
        if let Some(approval_policy) = partial.approval_policy {
            self.approval_policy = approval_policy;
        }
        if let Some(overflow_strategy) = partial.overflow_strategy {
            self.overflow_strategy = overflow_strategy;
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverflowStrategy {
    Truncate,
    ManifestOnly,
    Fail,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct RawConfig {
    pub defaults: Option<PartialDefaultsConfig>,
}

impl RawConfig {
    pub(crate) fn merge_into(self, config: &mut IngotConfig) {
        if let Some(defaults) = self.defaults {
            config.defaults.merge_partial(defaults);
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct PartialDefaultsConfig {
    pub candidate_rework_budget: Option<u32>,
    pub integration_rework_budget: Option<u32>,
    pub approval_policy: Option<ApprovalPolicy>,
    pub overflow_strategy: Option<OverflowStrategy>,
}
