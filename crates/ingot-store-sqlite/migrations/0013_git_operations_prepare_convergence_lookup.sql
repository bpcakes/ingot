-- Accelerate item-detail conflict metadata lookup for latest prepare convergence operations.
CREATE INDEX IF NOT EXISTS idx_git_operations_prepare_convergence_lookup
ON git_operations(operation_kind, entity_type, entity_id, created_at DESC, id DESC);
