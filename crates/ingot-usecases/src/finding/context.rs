use ingot_domain::revision_context::{RevisionContext, RevisionContextSummary};

pub fn parse_revision_context_summary(
    context: Option<&RevisionContext>,
) -> Option<RevisionContextSummary> {
    context.map(RevisionContextSummary::from)
}
