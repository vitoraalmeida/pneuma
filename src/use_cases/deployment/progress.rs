use std::fmt;

use crate::domain::deployment::DeploymentStatus;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeploymentStep {
    LoadSpecification,
    CreateDeployment,
    CreateContainer,
    StartContainer,
    ObserveContainer,
    RegisterCandidate,
    HealthCheckAndPromotion,
    InternalHealthCheck,
    ApplyPublicRoute,
    ExternalHealthCheck,
    PromoteCandidate,
    CleanupCandidate,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DeploymentProgress {
    StepStarted {
        step: DeploymentStep,
        detail: String,
    },
    StepCompleted {
        step: DeploymentStep,
        detail: String,
    },
    StateChanged {
        deployment_id: String,
        status: DeploymentStatus,
    },
    FailurePersisted {
        deployment_id: String,
        code: &'static str,
    },
}

impl fmt::Display for DeploymentStep {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::LoadSpecification => "load application specification",
            Self::CreateDeployment => "create deployment",
            Self::CreateContainer => "create candidate container",
            Self::StartContainer => "start candidate container",
            Self::ObserveContainer => "observe candidate container",
            Self::RegisterCandidate => "register candidate runtime",
            Self::HealthCheckAndPromotion => "health check and promotion",
            Self::InternalHealthCheck => "internal health check",
            Self::ApplyPublicRoute => "apply public route",
            Self::ExternalHealthCheck => "external health check",
            Self::PromoteCandidate => "promote public candidate",
            Self::CleanupCandidate => "clean up candidate",
        };
        formatter.write_str(name)
    }
}

impl fmt::Display for DeploymentProgress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StepStarted { step, detail } => {
                write!(formatter, "{step}: started ({detail})")
            }
            Self::StepCompleted { step, detail } => {
                write!(formatter, "{step}: completed ({detail})")
            }
            Self::StateChanged {
                deployment_id,
                status,
            } => write!(
                formatter,
                "deployment {deployment_id}: state changed to {status:?}"
            ),
            Self::FailurePersisted {
                deployment_id,
                code,
            } => write!(
                formatter,
                "deployment {deployment_id}: state changed to Failed; failure persisted ({code})"
            ),
        }
    }
}

// Delivers optional deployment progress without coupling orchestration to a UI.
pub(crate) struct ProgressReporter<'a> {
    callback: Option<&'a mut dyn FnMut(DeploymentProgress)>,
}

impl<'a> ProgressReporter<'a> {
    // Creates a no-op reporter for callers that do not request progress events.
    pub(crate) fn disabled() -> Self {
        Self { callback: None }
    }

    // Wraps the caller callback used to report synchronous orchestration events.
    pub(crate) fn enabled(callback: &'a mut dyn FnMut(DeploymentProgress)) -> Self {
        Self {
            callback: Some(callback),
        }
    }

    // Reports the start of a deployment step before its side effects begin.
    pub(crate) fn started(&mut self, step: DeploymentStep, detail: String) {
        self.emit(DeploymentProgress::StepStarted { step, detail });
    }

    // Invokes the optional callback while keeping disabled reporting side-effect free.
    fn emit(&mut self, event: DeploymentProgress) {
        if let Some(callback) = &mut self.callback {
            callback(event);
        }
    }

    // Reports successful completion only after the step has finished.
    pub(crate) fn completed(&mut self, step: DeploymentStep, detail: String) {
        self.emit(DeploymentProgress::StepCompleted { step, detail });
    }

    // Reports a persisted deployment-state transition.
    pub(crate) fn state_changed(&mut self, deployment_id: &str, status: DeploymentStatus) {
        self.emit(DeploymentProgress::StateChanged {
            deployment_id: deployment_id.to_owned(),
            status,
        });
    }

    // Reports that failure evidence has been durably recorded.
    pub(crate) fn failure_persisted(&mut self, deployment_id: &str, code: &'static str) {
        self.emit(DeploymentProgress::FailurePersisted {
            deployment_id: deployment_id.to_owned(),
            code,
        });
    }
}
