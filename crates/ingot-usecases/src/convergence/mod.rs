mod flow;
mod service;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

pub use flow::{
    ApprovalFinalizeReadiness, CheckoutFinalizationReadiness, ConvergenceApprovalContext,
    ConvergenceSystemActionPort, FinalizePreparedTrigger, FinalizeTargetRefResult,
    FinalizedIntegrationWorkspaceCleanup, PreparedConvergenceFinalizePort, RejectApprovalContext,
    SystemActionItemState, SystemActionProjectState, apply_finalization_mutation_and_load_cleanup,
    build_convergence_approval_context, build_reject_approval_context,
    finalize_prepared_convergence, find_or_create_finalize_operation,
    should_auto_finalize_prepared_convergence, should_invalidate_prepared_convergence,
    should_prepare_convergence,
};
pub use service::{
    ConvergenceService, build_convergence_queue_entry, invalidate_prepared_convergence,
    promote_queue_heads,
};
