use std::io::{self, IsTerminal, Write};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use pneuma::use_cases::deployment::{DeploymentEvent, DeploymentStep, RetirementWarning};

use super::output::deployment_status_label;
use super::shared::{log_verbose, write_stderr_line};

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
                        verbose,
                        format!(
                            "deployment input: application {application_name}, {input_kind} {input}"
                        ),
                    );
                } else {
                    write_stderr_line(format!("Deploying {application_name}..."));
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
        write_stderr_line(render_deployment_event(&event));
    }
}

fn render_animated_progress(receiver: mpsc::Receiver<DeploymentEvent>, show_request: bool) {
    render_animated_progress_into(receiver, show_request, &mut io::stderr());
}

fn render_animated_progress_into<W: Write>(
    receiver: mpsc::Receiver<DeploymentEvent>,
    show_request: bool,
    sink: &mut W,
) {
    let frames = ['-', '\\', '|', '/'];
    let mut frame = 0;
    let mut current_step = None;
    let mut visible = false;

    loop {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(DeploymentEvent::DeploymentRequested { application_name }) if show_request => {
                let _ = writeln!(sink, "Deploying {application_name}...");
            }
            Ok(DeploymentEvent::StepStarted { step }) => {
                clear_progress(sink, &mut visible);
                current_step = Some(step);
            }
            Ok(DeploymentEvent::StepCompleted { step }) => {
                if current_step == Some(step) {
                    clear_progress(sink, &mut visible);
                    current_step = None;
                }
            }
            Ok(event @ DeploymentEvent::RetirementWarning { .. }) => {
                clear_progress(sink, &mut visible);
                let _ = writeln!(sink, "{}", render_deployment_event(&event));
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(step) = current_step {
                    render_spinner(sink, frames[frame], step);
                    frame = (frame + 1) % frames.len();
                    visible = true;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                clear_progress(sink, &mut visible);
                return;
            }
        }
    }
}

fn render_spinner<W: Write>(sink: &mut W, frame: char, step: DeploymentStep) {
    let _ = write!(sink, "\r{frame} {}...", deployment_step_label(step));
    let _ = sink.flush();
}

fn clear_progress<W: Write>(sink: &mut W, visible: &mut bool) {
    if !*visible {
        return;
    }
    let _ = write!(sink, "\r\x1b[2K");
    let _ = sink.flush();
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
        } => format!(
            "deployment {deployment_id}: state changed to {}",
            deployment_status_label(*status)
        ),
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
    use pneuma::domain::application::ApplicationName;
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
    fn state_changes_render_the_shared_deployment_status_label() {
        assert_eq!(
            render_deployment_event(&DeploymentEvent::StateChanged {
                deployment_id: pneuma::domain::identity::DeploymentId::new(
                    "0f8d3a2c41b64d7e9a0c5b6e1f2d3a4b"
                )
                .unwrap(),
                status: pneuma::domain::deployment::DeploymentStatus::Activating,
            }),
            "deployment 0f8d3a2c41b64d7e9a0c5b6e1f2d3a4b: state changed to Activating"
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

    #[cfg(target_os = "linux")]
    #[test]
    fn animated_tty_progress_emits_lifecycle_text_frames_and_clear_bytes() {
        use std::fs::File;
        use std::io::Read;
        use std::os::fd::FromRawFd;
        use std::sync::mpsc;
        use std::thread;
        use std::time::Duration;

        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        let opened = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(opened, 0);

        let mut sink = unsafe { File::from_raw_fd(slave) };
        let (sender, receiver) = mpsc::channel();
        let renderer = thread::spawn(move || {
            super::render_animated_progress_into(receiver, true, &mut sink);
        });
        sender
            .send(DeploymentEvent::DeploymentRequested {
                application_name: ApplicationName::new("another-site").unwrap(),
            })
            .unwrap();
        sender
            .send(DeploymentEvent::StepStarted {
                step: DeploymentStep::PullImage,
            })
            .unwrap();
        thread::sleep(Duration::from_millis(350));
        sender
            .send(DeploymentEvent::StepCompleted {
                step: DeploymentStep::PullImage,
            })
            .unwrap();
        drop(sender);
        renderer.join().unwrap();

        let mut bytes = Vec::new();
        let mut reader = unsafe { File::from_raw_fd(master) };
        let mut chunk = [0u8; 1024];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => bytes.extend_from_slice(&chunk[..read]),
            }
        }
        let output = String::from_utf8_lossy(&bytes);

        // A PTY translates line feeds with ONLCR, so assert the text without
        // assuming the exact newline bytes.
        assert!(
            output.contains("Deploying another-site..."),
            "unexpected TTY output: {output:?}"
        );
        assert!(
            output.matches("pull image...").count() >= 2,
            "expected at least two spinner frames: {output:?}"
        );
        assert!(
            output.contains("\r\x1b[2K"),
            "expected final clear-line control bytes: {output:?}"
        );
    }
}
