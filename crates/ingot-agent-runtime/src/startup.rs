use ingot_usecases::ReconciliationService;

use crate::{
    JobDispatcher, RuntimeError, RuntimeReconciliationPort, bootstrap, drain_until_idle,
    usecase_to_runtime_error,
};

pub(crate) struct StartupReconciler {
    dispatcher: JobDispatcher,
}

impl StartupReconciler {
    pub(crate) fn new(dispatcher: &JobDispatcher) -> Self {
        Self {
            dispatcher: dispatcher.clone(),
        }
    }

    pub(crate) async fn reconcile(&self) -> Result<(), RuntimeError> {
        bootstrap::ensure_default_agents(&self.dispatcher.db).await?;
        let _ = self.dispatcher.reconcile_startup_assigned_jobs().await?;
        let _ = self
            .dispatcher
            .reconcile_startup_daemon_validation_jobs()
            .await?;
        ReconciliationService::new(RuntimeReconciliationPort {
            dispatcher: self.dispatcher.clone(),
        })
        .reconcile_startup()
        .await
        .map_err(usecase_to_runtime_error)?;
        drain_until_idle(|| self.dispatcher.tick_system_action()).await?;
        let _ = self.dispatcher.recover_projected_jobs().await?;
        Ok(())
    }
}
