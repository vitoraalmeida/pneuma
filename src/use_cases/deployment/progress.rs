use crate::domain::application::ApplicationName;
use crate::domain::deployment::{DeploymentFailureCode, DeploymentStatus};
use crate::domain::identity::{DeploymentId, RuntimeInstanceId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentStep {
    ResolveBranch,
    ResolveImageDigest,
    PullImage,
    LoadSpecification,
    CreateDeployment,
    ReservePort,
    CreateUnit,
    ReloadSystemd,
    StartContainer,
    ResolveContainer,
    ObserveContainer,
    RegisterCandidate,
    InternalHealthCheck,
    ApplyPublicRoute,
    ExternalHealthCheck,
    PromoteCandidate,
    CleanupCandidate,
    RetirePreviousRuntime,
}

// Describes a best-effort prior-runtime retirement result without coupling the workflow to a UI.
#[derive(Debug, PartialEq, Eq)]
pub enum RetirementWarning {
    UnitRetirementFailed { diagnostic: String },
    ContainerRemovalUnproven { diagnostic: String },
    PersistenceFailed,
}

// Closed semantic events emitted by the control boundary and deployment workflow.
#[derive(Debug, PartialEq, Eq)]
pub enum DeploymentEvent {
    DeploymentRequested {
        application_name: ApplicationName,
    },
    StepStarted {
        step: DeploymentStep,
    },
    StepCompleted {
        step: DeploymentStep,
    },
    StateChanged {
        deployment_id: DeploymentId,
        status: DeploymentStatus,
    },
    FailurePersisted {
        deployment_id: DeploymentId,
        code: DeploymentFailureCode,
    },
    RetirementWarning {
        runtime_id: RuntimeInstanceId,
        warning: RetirementWarning,
    },
}

// Delivers optional deployment events without coupling orchestration to a UI.
pub(crate) struct EventReporter<'a> {
    callback: Option<&'a mut dyn FnMut(DeploymentEvent)>,
}

impl<'a> EventReporter<'a> {
    // Creates a no-op reporter for callers that do not request workflow events.
    pub(crate) fn disabled() -> Self {
        Self { callback: None }
    }

    // Wraps the caller callback used to report synchronous orchestration events.
    pub(crate) fn enabled(callback: &'a mut dyn FnMut(DeploymentEvent)) -> Self {
        Self {
            callback: Some(callback),
        }
    }

    // Reports the start of a deployment step before its side effects begin.
    pub(crate) fn started(&mut self, step: DeploymentStep) {
        self.emit(DeploymentEvent::StepStarted { step });
    }

    // Invokes the optional callback while keeping disabled reporting side-effect free.
    fn emit(&mut self, event: DeploymentEvent) {
        if let Some(callback) = &mut self.callback {
            callback(event);
        }
    }

    // Reports successful completion only after the step has finished.
    pub(crate) fn completed(&mut self, step: DeploymentStep) {
        self.emit(DeploymentEvent::StepCompleted { step });
    }

    // Reports a persisted deployment-state transition.
    pub(crate) fn state_changed(&mut self, deployment_id: &DeploymentId, status: DeploymentStatus) {
        self.emit(DeploymentEvent::StateChanged {
            deployment_id: deployment_id.clone(),
            status,
        });
    }

    // Reports that failure evidence has been durably recorded.
    pub(crate) fn failure_persisted(
        &mut self,
        deployment_id: &DeploymentId,
        code: DeploymentFailureCode,
    ) {
        self.emit(DeploymentEvent::FailurePersisted {
            deployment_id: deployment_id.clone(),
            code,
        });
    }

    // Reports a best-effort predecessor retirement warning while preserving its semantic cause.
    pub(crate) fn retirement_warning(
        &mut self,
        runtime_id: &RuntimeInstanceId,
        warning: RetirementWarning,
    ) {
        self.emit(DeploymentEvent::RetirementWarning {
            runtime_id: runtime_id.clone(),
            warning,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{DeploymentEvent, EventReporter, RetirementWarning};
    use crate::domain::deployment::DeploymentFailureCode;
    use crate::domain::identity::{DeploymentId, RuntimeInstanceId};

    #[test]
    fn preserves_typed_failure_codes_and_retirement_warnings() {
        let deployment_id = DeploymentId::new("11111111111111111111111111111111").unwrap();
        let runtime_id = RuntimeInstanceId::new("22222222222222222222222222222222").unwrap();
        let mut observed = Vec::new();
        let mut collect = |event| observed.push(event);
        let mut events = EventReporter::enabled(&mut collect);

        events.failure_persisted(&deployment_id, DeploymentFailureCode::RuntimeStart);
        events.retirement_warning(&runtime_id, RetirementWarning::PersistenceFailed);

        assert!(matches!(
            observed.as_slice(),
            [
                DeploymentEvent::FailurePersisted {
                    deployment_id: observed_deployment_id,
                    code: DeploymentFailureCode::RuntimeStart,
                },
                DeploymentEvent::RetirementWarning {
                    runtime_id: observed_runtime_id,
                    warning: RetirementWarning::PersistenceFailed,
                }
            ] if observed_deployment_id == &deployment_id && observed_runtime_id == &runtime_id
        ));
    }
}
