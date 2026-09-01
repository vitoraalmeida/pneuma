use std::io::{self, IsTerminal, Write};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use pneuma::use_cases::deployment::{DeploymentEvent, DeploymentStep, RetirementWarning};

use super::shared::log_verbose;

// Renders deployment events without making control execution depend on terminal behavior.
pub(super) struct DeploymentProgressRenderer {
    output: ProgressOutput,
}

enum ProgressOutput {
    Stable {
        verbose: bool,
        requested_input: Option<(String, String)>,
    },
    Animated {
        events: Option<Sender<DeploymentEvent>>,
        thread: Option<JoinHandle<()>>,
    },
}

impl DeploymentProgressRenderer {
    pub(super) fn new(verbose: bool, requested_input: Option<(&str, &str)>) -> Self {
        if !verbose && io::stderr().is_terminal() {
            let (sender, receiver) = mpsc::channel();
            let show_request = requested_input.is_some();
            let thread = thread::spawn(move || render_animated_progress(receiver, show_request));
            return Self {
                output: ProgressOutput::Animated {
                    events: Some(sender),
                    thread: Some(thread),
                },
            };
        }

        Self {
            output: ProgressOutput::Stable {
                verbose,
                requested_input: requested_input
                    .map(|(input_kind, input)| (input_kind.to_owned(), input.to_owned())),
            },
        }
    }

    // Reports events observationally: a disconnected animation thread changes no command result.
    pub(super) fn report(&mut self, event: DeploymentEvent) {
        match &mut self.output {
            ProgressOutput::Stable {
                verbose,
                requested_input,
            } => render_stable_event(
                event,
                *verbose,
                requested_input
                    .as_ref()
                    .map(|(input_kind, input)| (input_kind.as_str(), input.as_str())),
            ),
            ProgressOutput::Animated { events, .. } => {
                if let Some(events) = events {
                    let _ = events.send(event);
                }
            }
        }
    }

    // Stops the terminal renderer before command output or errors are printed.
    pub(super) fn finish(&mut self) {
        let ProgressOutput::Animated { events, thread } = &mut self.output else {
            return;
        };
        events.take();
        if let Some(thread) = thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for DeploymentProgressRenderer {
    fn drop(&mut self) {
        self.finish();
    }
}

fn render_stable_event(
    event: DeploymentEvent,
    verbose: bool,
    requested_input: Option<(&str, &str)>,
) {
    if let DeploymentEvent::DeploymentRequested { application_name } = event {
        match requested_input {
            Some((input_kind, input)) => {
                if verbose {
                    log_verbose(
                        true,
                        format!(
                            "deployment input: application {application_name}, {input_kind} {input}"
                        ),
                    );
                } else {
                    eprintln!("Deploying {application_name}...");
                }
            }
            None => log_verbose(
                verbose,
                format!("rolling back application {application_name}"),
            ),
        }
        return;
    }
    if verbose || matches!(event, DeploymentEvent::RetirementWarning { .. }) {
        eprintln!("{}", render_deployment_event(&event));
    }
}

fn render_animated_progress(receiver: mpsc::Receiver<DeploymentEvent>, show_request: bool) {
    let frames = ['-', '\\', '|', '/'];
    let mut frame = 0;
    let mut current_step = None;
    let mut visible = false;

    loop {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(DeploymentEvent::DeploymentRequested { application_name }) if show_request => {
                eprintln!("Deploying {application_name}...");
            }
            Ok(DeploymentEvent::StepStarted { step }) => {
                clear_progress(&mut visible);
                current_step = Some(step);
            }
            Ok(DeploymentEvent::StepCompleted { step }) => {
                if current_step == Some(step) {
                    clear_progress(&mut visible);
                    current_step = None;
                }
            }
            Ok(event @ DeploymentEvent::RetirementWarning { .. }) => {
                clear_progress(&mut visible);
                eprintln!("{}", render_deployment_event(&event));
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(step) = current_step {
                    render_spinner(frames[frame], step);
                    frame = (frame + 1) % frames.len();
                    visible = true;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                clear_progress(&mut visible);
                return;
            }
        }
    }
}

fn render_spinner(frame: char, step: DeploymentStep) {
    let mut stderr = io::stderr().lock();
    let _ = write!(stderr, "\r{frame} {}...", deployment_step_label(step));
    let _ = stderr.flush();
}

fn clear_progress(visible: &mut bool) {
    if !*visible {
        return;
    }
    let mut stderr = io::stderr().lock();
    let _ = write!(stderr, "\r\x1b[2K");
    let _ = stderr.flush();
    *visible = false;
}

// Renders use-case events with the CLI's stable text vocabulary.
fn render_deployment_event(event: &DeploymentEvent) -> String {
    match event {
        DeploymentEvent::DeploymentRequested { .. } => {
            unreachable!("deployment requests are rendered separately")
        }
        DeploymentEvent::StepStarted { step } => {
            format!("{}: started", deployment_step_label(*step))
        }
        DeploymentEvent::StepCompleted { step } => {
            format!("{}: completed", deployment_step_label(*step))
        }
        DeploymentEvent::StateChanged {
            deployment_id,
            status,
        } => format!("deployment {deployment_id}: state changed to {status:?}"),
        DeploymentEvent::FailurePersisted {
            deployment_id,
            code,
        } => format!(
            "deployment {deployment_id}: state changed to Failed; failure persisted ({code})"
        ),
        DeploymentEvent::RetirementWarning {
            runtime_id,
            warning,
        } => match warning {
            RetirementWarning::UnitRetirementFailed { diagnostic } => {
                format!("warning: previous runtime {runtime_id} could not be retired: {diagnostic}")
            }
            RetirementWarning::ContainerRemovalUnproven { diagnostic } => format!(
                "warning: previous runtime {runtime_id} unit was retired but its container removal could not be proven: {diagnostic}"
            ),
            RetirementWarning::PersistenceFailed => format!(
                "warning: previous runtime {runtime_id} was retired but could not be marked removed"
            ),
        },
    }
}

fn deployment_step_label(step: DeploymentStep) -> &'static str {
    match step {
        DeploymentStep::ResolveBranch => "resolve branch",
        DeploymentStep::ResolveImageDigest => "resolve image digest",
        DeploymentStep::PullImage => "pull image",
        DeploymentStep::LoadSpecification => "load application specification",
        DeploymentStep::CreateDeployment => "create deployment",
        DeploymentStep::ReservePort => "reserve runtime port",
        DeploymentStep::CreateUnit => "create candidate unit",
        DeploymentStep::ReloadSystemd => "reload systemd user manager",
        DeploymentStep::StartContainer => "start candidate container",
        DeploymentStep::ResolveContainer => "resolve candidate container",
        DeploymentStep::ObserveContainer => "observe candidate container",
        DeploymentStep::RegisterCandidate => "register candidate runtime",
        DeploymentStep::InternalHealthCheck => "internal health check",
        DeploymentStep::ApplyPublicRoute => "apply public route",
        DeploymentStep::ExternalHealthCheck => "external health check",
        DeploymentStep::PromoteCandidate => "health check and promotion",
        DeploymentStep::CleanupCandidate => "clean up candidate",
        DeploymentStep::RetirePreviousRuntime => "retire previous runtime",
    }
}

#[cfg(test)]
mod tests {
    use super::{deployment_step_label, render_deployment_event};
    use pneuma::use_cases::deployment::{DeploymentEvent, DeploymentStep};

    #[test]
    fn keeps_verbose_step_text_stable() {
        assert_eq!(
            render_deployment_event(&DeploymentEvent::StepStarted {
                step: DeploymentStep::PullImage,
            }),
            "pull image: started"
        );
    }

    #[test]
    fn labels_every_animated_step() {
        assert_eq!(
            deployment_step_label(DeploymentStep::ResolveBranch),
            "resolve branch"
        );
        assert_eq!(
            deployment_step_label(DeploymentStep::RetirePreviousRuntime),
            "retire previous runtime"
        );
    }
}
