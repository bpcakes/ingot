use std::fs;

use ingot_config::OverflowStrategy;
use ingot_config::loader::{ConfigError, load_config};
use ingot_domain::revision::ApprovalPolicy;
use ingot_test_support::env::temp_dir;

#[test]
fn load_config_parses_typed_approval_policy() {
    let dir = temp_dir("valid");
    let config_path = dir.join("config.yml");
    fs::write(
        &config_path,
        "defaults:\n  candidate_rework_budget: 7\n  integration_rework_budget: 9\n  approval_policy: not_required\n  overflow_strategy: truncate\n",
    )
    .expect("write config");

    let config = load_config(&config_path, None).expect("load config");

    assert_eq!(config.defaults.approval_policy, ApprovalPolicy::NotRequired);
    assert_eq!(config.defaults.candidate_rework_budget, 7);
    assert_eq!(config.defaults.integration_rework_budget, 9);
    assert_eq!(
        config.defaults.overflow_strategy,
        OverflowStrategy::Truncate
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn load_config_rejects_invalid_approval_policy() {
    let dir = temp_dir("invalid");
    let config_path = dir.join("config.yml");
    fs::write(
        &config_path,
        "defaults:\n  approval_policy: later\n  overflow_strategy: truncate\n",
    )
    .expect("write config");

    let error = load_config(&config_path, None).expect_err("invalid approval_policy");

    match error {
        ConfigError::Parse(message) => assert!(message.contains("approval_policy")),
        other => panic!("expected parse error, got {other:?}"),
    }

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn load_config_merges_partial_project_defaults_over_global_defaults() {
    let dir = temp_dir("merge");
    let global_path = dir.join("global.yml");
    let project_path = dir.join("project.yml");
    fs::write(
        &global_path,
        "defaults:\n  candidate_rework_budget: 7\n  integration_rework_budget: 9\n  approval_policy: required\n  overflow_strategy: manifest_only\n",
    )
    .expect("write global config");
    fs::write(
        &project_path,
        "defaults:\n  approval_policy: not_required\n  overflow_strategy: fail\n",
    )
    .expect("write project config");

    let config = load_config(&global_path, Some(&project_path)).expect("load config");

    assert_eq!(config.defaults.candidate_rework_budget, 7);
    assert_eq!(config.defaults.integration_rework_budget, 9);
    assert_eq!(config.defaults.approval_policy, ApprovalPolicy::NotRequired);
    assert_eq!(config.defaults.overflow_strategy, OverflowStrategy::Fail);

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn load_config_allows_partial_global_defaults() {
    let dir = temp_dir("partial-global");
    let global_path = dir.join("global.yml");
    fs::write(&global_path, "defaults:\n  candidate_rework_budget: 5\n")
        .expect("write global config");

    let config = load_config(&global_path, None).expect("load config");

    assert_eq!(config.defaults.candidate_rework_budget, 5);
    assert_eq!(config.defaults.integration_rework_budget, 2);
    assert_eq!(config.defaults.approval_policy, ApprovalPolicy::Required);
    assert_eq!(
        config.defaults.overflow_strategy,
        OverflowStrategy::Truncate
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn load_config_rejects_invalid_overflow_strategy() {
    let dir = temp_dir("invalid-overflow");
    let config_path = dir.join("config.yml");
    fs::write(&config_path, "defaults:\n  overflow_strategy: summarize\n").expect("write config");

    let error = load_config(&config_path, None).expect_err("invalid overflow_strategy");

    match error {
        ConfigError::Parse(message) => assert!(message.contains("overflow_strategy")),
        other => panic!("expected parse error, got {other:?}"),
    }

    let _ = fs::remove_dir_all(dir);
}
