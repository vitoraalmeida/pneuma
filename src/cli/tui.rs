use std::collections::VecDeque;
use std::io::{self, IsTerminal};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute, terminal,
};
use pneuma::control::{Command, CommandResult, ControlExecutor};
use pneuma::domain::application::{ApplicationName, DesiredRuntimeState};
use pneuma::domain::deployment::{DeploymentHistory, DeploymentStatus};
use pneuma::domain::exposure::Visibility;
use pneuma::domain::git::{RelativeManifestPath, is_remote_git_location};
use pneuma::domain::runtime::ObservedRuntimeState;
use pneuma::domain::system::{System, SystemName};
use pneuma::use_cases::application::{ApplicationCatalogEntry, RuntimeObservation};
use pneuma::use_cases::deployment::{DeploymentEvent, DeploymentResult};
use pneuma::use_cases::system::SystemDetails;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Position},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap},
};

use super::{
    error::{CliError, CliErrorClass},
    output,
};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(50);

// Shared presentation palette: panel titles, key badges, and state values use
// one vocabulary so every screen reads the same way.
fn title_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

fn field_style() -> Style {
    Style::default().bg(Color::Black).fg(Color::White)
}

fn label_span(label: &str) -> Span<'static> {
    Span::styled(format!("{label}: "), Style::default().fg(Color::DarkGray))
}

fn value_span(value: String) -> Span<'static> {
    Span::raw(value)
}

fn absent_span() -> Span<'static> {
    Span::styled("None", Style::default().fg(Color::DarkGray))
}

fn key_hint(key: &str, description: &str) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            format!(" {key} "),
            Style::default().fg(Color::Black).bg(Color::Cyan),
        ),
        Span::raw(format!(" {description}  ")),
    ]
}

fn badge(text: &str, background: Color) -> Span<'static> {
    Span::styled(
        format!(" {text} "),
        Style::default().fg(Color::Black).bg(background),
    )
}

fn desired_state_color(state: DesiredRuntimeState) -> Color {
    match state {
        DesiredRuntimeState::Running => Color::Green,
        DesiredRuntimeState::Stopped => Color::Gray,
    }
}

fn observed_state_color(state: &ObservedRuntimeState) -> Color {
    match state {
        ObservedRuntimeState::Running => Color::Green,
        ObservedRuntimeState::Stopped => Color::Gray,
        ObservedRuntimeState::Missing | ObservedRuntimeState::Failed => Color::Red,
        ObservedRuntimeState::Created
        | ObservedRuntimeState::Starting
        | ObservedRuntimeState::Stopping
        | ObservedRuntimeState::Unknown { .. } => Color::Yellow,
    }
}

fn deployment_status_color(status: DeploymentStatus) -> Color {
    match status {
        DeploymentStatus::Succeeded => Color::Green,
        DeploymentStatus::Failed => Color::Red,
        DeploymentStatus::Pending
        | DeploymentStatus::Starting
        | DeploymentStatus::Verifying
        | DeploymentStatus::Activating => Color::Yellow,
    }
}

// Runs the TUI adapter without constructing host configuration or opening the database.
pub(super) fn run() -> Result<(), CliError> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(CliError::TuiRequiresTerminal);
    }

    let mut terminal = TuiTerminal::open().map_err(|source| CliError::TuiTerminal { source })?;
    let mut session = Session::new();
    let result = terminal.run(&mut session);
    let worker = session.shutdown();
    let restored = terminal.restore();

    match (result, worker, restored) {
        (Err(source), _, _) | (Ok(()), Err(source), _) | (Ok(()), Ok(()), Err(source)) => {
            Err(CliError::TuiTerminal { source })
        }
        (Ok(()), Ok(()), Ok(())) => Ok(()),
    }
}

// Owns the terminal mode while the TUI is active so every exit path restores it.
struct TuiTerminal {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    active: bool,
}

impl TuiTerminal {
    fn open() -> io::Result<Self> {
        let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        terminal::enable_raw_mode()?;
        if let Err(error) = execute!(
            terminal.backend_mut(),
            terminal::EnterAlternateScreen,
            cursor::Hide
        ) {
            let _ = execute!(
                terminal.backend_mut(),
                cursor::Show,
                terminal::LeaveAlternateScreen
            );
            let _ = terminal::disable_raw_mode();
            return Err(error);
        }

        Ok(Self {
            terminal,
            active: true,
        })
    }

    fn run(&mut self, session: &mut Session) -> io::Result<()> {
        loop {
            session.drain_replies()?;
            if session.should_exit() {
                return Ok(());
            }

            self.terminal.draw(|frame| draw_shell(frame, session))?;
            if event::poll(EVENT_POLL_INTERVAL)? {
                let Event::Key(event) = event::read()? else {
                    continue;
                };
                if event.kind == KeyEventKind::Press {
                    session.handle_key(event)?;
                }
            }
        }
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;

        let screen = execute!(
            self.terminal.backend_mut(),
            cursor::Show,
            terminal::LeaveAlternateScreen
        );
        let raw_mode = terminal::disable_raw_mode();
        match (screen, raw_mode) {
            (Err(error), _) | (Ok(()), Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
}

impl Drop for TuiTerminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

// Command groups own one top-level tab each; every tab renders a listing
// column and a details column, and navigation stays inside the adapter.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Systems,
    Applications,
    Deployments,
}

impl Tab {
    const ALL: [Tab; 3] = [Tab::Systems, Tab::Applications, Tab::Deployments];

    fn index(self) -> usize {
        match self {
            Tab::Systems => 0,
            Tab::Applications => 1,
            Tab::Deployments => 2,
        }
    }

    fn from_index(index: usize) -> Tab {
        Tab::ALL[index % Tab::ALL.len()]
    }

    fn label(self) -> &'static str {
        match self {
            Tab::Systems => "Systems",
            Tab::Applications => "Applications",
            Tab::Deployments => "Deployments",
        }
    }

    fn next(self) -> Tab {
        Self::from_index(self.index() + 1)
    }

    fn previous(self) -> Tab {
        Self::from_index(self.index() + Tab::ALL.len() - 1)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Listing,
    Details,
    Log,
}

enum QueryState<T> {
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

struct ActionOutcome {
    // The detail view the outcome belongs to: an application name for
    // application actions, a system name for system actions.
    scope: String,
    message: String,
}

// A deployment log is session-only state: it is created when a deployment
// action dispatches, accumulates its semantic events, and is retained after
// the command finishes so the operator can still read what happened.
enum DeploymentLogState {
    Running,
    Completed,
    Failed(String),
}

struct DeploymentLog {
    application_name: String,
    lines: Vec<String>,
    state: DeploymentLogState,
    scroll: u16,
    tail_follow: bool,
    // Viewport metrics recorded by the last render: the wrapped row count of
    // the whole log and the visible row count of the log panel interior.
    total_rows: u16,
    viewport_rows: u16,
}

impl DeploymentLog {
    fn new(application_name: String) -> Self {
        Self {
            application_name,
            lines: Vec::new(),
            state: DeploymentLogState::Running,
            scroll: 0,
            tail_follow: true,
            total_rows: 0,
            viewport_rows: 0,
        }
    }

    fn record_event(&mut self, event: &DeploymentEvent) {
        self.lines.push(deployment_event_line(event));
    }

    fn finish(&mut self, result: &Result<CommandResult, WorkerError>) {
        self.state = match result {
            Ok(_) => DeploymentLogState::Completed,
            Err(error) => DeploymentLogState::Failed(error.display()),
        };
    }

    // The tail follows the newest rows until scrolling moves away from the
    // bottom; a detached view anchors on `scroll` and incoming events append
    // below it without moving the visible rows.
    fn max_scroll(&self) -> u16 {
        self.total_rows.saturating_sub(self.viewport_rows)
    }

    fn render_offset(&self) -> u16 {
        if self.tail_follow {
            self.max_scroll()
        } else {
            self.scroll.min(self.max_scroll())
        }
    }

    fn scroll_up(&mut self, rows: u16) {
        let anchored = if self.tail_follow {
            self.max_scroll()
        } else {
            self.scroll
        };
        self.tail_follow = false;
        self.scroll = anchored.saturating_sub(rows);
    }

    fn scroll_down(&mut self, rows: u16) {
        if self.tail_follow {
            return;
        }
        self.scroll = self.scroll.saturating_add(rows);
        if self.scroll >= self.max_scroll() {
            self.scroll = self.max_scroll();
            self.tail_follow = true;
        }
    }

    fn scroll_to_start(&mut self) {
        self.tail_follow = false;
        self.scroll = 0;
    }

    fn scroll_to_end(&mut self) {
        self.tail_follow = true;
        self.scroll = self.max_scroll();
    }
}

#[derive(Clone, PartialEq, Eq)]
enum PendingAction {
    SystemCreate {
        name: String,
        description: Option<String>,
    },
    ImportApplication {
        repository: String,
        system_name: Option<String>,
        manifest_path: Option<String>,
    },
    Start {
        application_name: String,
    },
    Stop {
        application_name: String,
    },
    Reconcile {
        application_name: String,
    },
    SetVisibility {
        application_name: String,
        visibility: Visibility,
    },
    DeployBranch {
        application_name: String,
        branch: String,
    },
    DeployImage {
        application_name: String,
        image_reference: String,
    },
    Rollback {
        application_name: String,
    },
}

impl PendingAction {
    // Only application-scoped actions carry an application name; system and
    // import actions scope their outcomes themselves.
    fn application_name(&self) -> Option<&str> {
        match self {
            Self::SystemCreate { .. } | Self::ImportApplication { .. } => None,
            Self::Start { application_name }
            | Self::Stop { application_name }
            | Self::Reconcile { application_name }
            | Self::SetVisibility {
                application_name, ..
            }
            | Self::DeployBranch {
                application_name, ..
            }
            | Self::DeployImage {
                application_name, ..
            }
            | Self::Rollback { application_name } => Some(application_name),
        }
    }

    fn command(&self) -> Command {
        match self {
            Self::SystemCreate { name, description } => Command::SystemCreate {
                name: name.clone(),
                description: description.clone(),
            },
            Self::ImportApplication {
                repository,
                system_name,
                manifest_path,
            } => Command::ImportApplication {
                repository: repository.clone(),
                system_name: system_name.clone(),
                manifest_path: manifest_path.clone(),
            },
            Self::Start { application_name } => Command::ApplicationStart {
                application_name: application_name.clone(),
            },
            Self::Stop { application_name } => Command::ApplicationStop {
                application_name: application_name.clone(),
            },
            Self::Reconcile { application_name } => Command::Reconcile {
                application_name: application_name.clone(),
            },
            Self::SetVisibility {
                application_name,
                visibility,
            } => Command::VisibilitySet {
                application_name: application_name.clone(),
                visibility: *visibility,
            },
            Self::DeployBranch {
                application_name,
                branch,
            } => Command::DeployBranch {
                application_name: application_name.clone(),
                branch: branch.clone(),
            },
            Self::DeployImage {
                application_name,
                image_reference,
            } => Command::DeployImage {
                application_name: application_name.clone(),
                image_reference: image_reference.clone(),
            },
            Self::Rollback { application_name } => Command::Rollback {
                application_name: application_name.clone(),
            },
        }
    }

    fn targets_visibility(&self, visibility: Visibility) -> bool {
        matches!(self, Self::SetVisibility { visibility: target, .. } if *target == visibility)
    }

    // Forms submit explicitly, so the actions they produce never enter the
    // confirmation modal; the text only exists for key-triggered actions.
    fn confirmation_text(&self) -> String {
        match self {
            Self::SystemCreate { name, .. } => format!("Create system {name}?"),
            Self::ImportApplication { repository, .. } => {
                format!("Import application from {repository}?")
            }
            Self::Start { application_name } => format!(
                "Start {application_name}? This changes the desired runtime intent and may control the runtime."
            ),
            Self::Stop { application_name } => format!(
                "Stop {application_name}? This changes the desired runtime intent and may control the runtime."
            ),
            Self::Reconcile { application_name } => format!(
                "Reconcile {application_name}? This may repair persisted runtime or route drift."
            ),
            Self::SetVisibility {
                application_name,
                visibility,
            } => format!(
                "Set {application_name} visibility to {}? This may change Caddy exposure.",
                output::visibility_label(*visibility)
            ),
            Self::DeployBranch {
                application_name,
                branch,
            } => format!(
                "Deploy {application_name} from branch {branch}? This resolves and deploys its current artifact."
            ),
            Self::DeployImage {
                application_name,
                image_reference,
            } => format!(
                "Deploy {application_name} from image {image_reference}? This deploys the digest-pinned artifact."
            ),
            Self::Rollback { application_name } => format!(
                "Roll back {application_name} to its previous successful release? This deploys a new rollback deployment."
            ),
        }
    }

    fn is_deployment(&self) -> bool {
        matches!(
            self,
            Self::DeployBranch { .. } | Self::DeployImage { .. } | Self::Rollback { .. }
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeploySource {
    Branch,
    Image,
}

struct DeployForm {
    application_name: String,
    branch: String,
    image: String,
    source: DeploySource,
    error: Option<String>,
}

impl DeployForm {
    fn new(application_name: String) -> Self {
        Self {
            application_name,
            branch: String::new(),
            image: String::new(),
            source: DeploySource::Branch,
            error: None,
        }
    }

    fn toggle_source(&mut self) {
        self.error = None;
        self.source = match self.source {
            DeploySource::Branch => DeploySource::Image,
            DeploySource::Image => DeploySource::Branch,
        };
    }

    fn push(&mut self, character: char) {
        self.error = None;
        match self.source {
            DeploySource::Branch => self.branch.push(character),
            DeploySource::Image => self.image.push(character),
        }
    }

    fn backspace(&mut self) {
        self.error = None;
        match self.source {
            DeploySource::Branch => {
                self.branch.pop();
            }
            DeploySource::Image => {
                self.image.pop();
            }
        }
    }

    fn value(&self) -> &str {
        match self.source {
            DeploySource::Branch => &self.branch,
            DeploySource::Image => &self.image,
        }
    }

    fn submit(&mut self) -> Option<PendingAction> {
        match self.source {
            DeploySource::Branch => {
                if self.branch.is_empty() {
                    self.error = Some("Enter a branch or tag to deploy.".to_owned());
                    None
                } else {
                    Some(PendingAction::DeployBranch {
                        application_name: self.application_name.clone(),
                        branch: self.branch.clone(),
                    })
                }
            }
            DeploySource::Image => {
                if self.image.is_empty() {
                    self.error = Some("Enter a digest-pinned image reference.".to_owned());
                    None
                } else {
                    Some(PendingAction::DeployImage {
                        application_name: self.application_name.clone(),
                        image_reference: self.image.clone(),
                    })
                }
            }
        }
    }
}

enum Mode {
    Normal,
    Confirm(PendingAction),
    Form(Form),
    Deploy(DeployForm),
}

// One editable text input of a multi-field form.
struct FormField {
    label: &'static str,
    value: String,
}

#[derive(Clone)]
enum FormKind {
    SystemCreate,
    ImportIntoSystem { system_name: String },
    ImportApplication,
}

// A multi-field text form. Field order in `fields` is the submit contract, so
// `submit` reads values by position.
struct Form {
    title: &'static str,
    context: Option<String>,
    kind: FormKind,
    fields: Vec<FormField>,
    focused: usize,
    error: Option<String>,
}

impl Form {
    fn system_create() -> Self {
        Self {
            title: "Create system",
            context: None,
            kind: FormKind::SystemCreate,
            fields: vec![
                FormField {
                    label: "Name",
                    value: String::new(),
                },
                FormField {
                    label: "Description (optional)",
                    value: String::new(),
                },
            ],
            focused: 0,
            error: None,
        }
    }

    fn import_into_system(system_name: String) -> Self {
        Self {
            title: "Add application to system",
            context: Some(format!("System: {system_name}")),
            kind: FormKind::ImportIntoSystem { system_name },
            fields: vec![
                FormField {
                    label: "Repository",
                    value: String::new(),
                },
                FormField {
                    label: "Manifest path (optional)",
                    value: String::new(),
                },
            ],
            focused: 0,
            error: None,
        }
    }

    fn import_application() -> Self {
        Self {
            title: "Import application",
            context: None,
            kind: FormKind::ImportApplication,
            fields: vec![
                FormField {
                    label: "Repository",
                    value: String::new(),
                },
                FormField {
                    label: "System (optional)",
                    value: String::new(),
                },
                FormField {
                    label: "Manifest path (optional)",
                    value: String::new(),
                },
            ],
            focused: 0,
            error: None,
        }
    }

    fn push(&mut self, character: char) {
        self.error = None;
        self.fields[self.focused].value.push(character);
    }

    fn backspace(&mut self) {
        self.error = None;
        self.fields[self.focused].value.pop();
    }

    fn focus_next(&mut self) {
        self.focused = (self.focused + 1) % self.fields.len();
    }

    fn focus_previous(&mut self) {
        self.focused = (self.focused + self.fields.len() - 1) % self.fields.len();
    }

    fn field(&self, index: usize) -> &str {
        &self.fields[index].value
    }

    fn optional_field(&self, index: usize) -> Option<String> {
        let value = self.field(index);
        (!value.is_empty()).then(|| value.to_owned())
    }

    fn require_remote_repository(&mut self, index: usize) -> Option<String> {
        let repository = self.field(index).to_owned();
        if is_remote_git_location(&repository) {
            Some(repository)
        } else {
            self.error = Some(
                "Enter a remote Git repository (a transport URL or git@host:path).".to_owned(),
            );
            None
        }
    }

    fn optional_manifest_path(&mut self, index: usize) -> Option<Option<String>> {
        let value = self.field(index).to_owned();
        if value.is_empty() {
            return Some(None);
        }
        match RelativeManifestPath::new(&value) {
            Ok(_) => Some(Some(value)),
            Err(source) => {
                self.error = Some(source.to_string());
                None
            }
        }
    }

    fn optional_system_name(&mut self, index: usize) -> Option<Option<String>> {
        let value = self.field(index).to_owned();
        if value.is_empty() {
            return Some(None);
        }
        match SystemName::new(&value) {
            Ok(_) => Some(Some(value)),
            Err(source) => {
                self.error = Some(source.to_string());
                None
            }
        }
    }

    // Validates locally with the same public boundaries the control layer
    // enforces, so invalid input never reaches the worker.
    fn submit(&mut self) -> Option<PendingAction> {
        self.error = None;
        match self.kind.clone() {
            FormKind::SystemCreate => {
                let name = self.field(0).to_owned();
                if let Err(source) = SystemName::new(&name) {
                    self.error = Some(source.to_string());
                    return None;
                }
                Some(PendingAction::SystemCreate {
                    name,
                    description: self.optional_field(1),
                })
            }
            FormKind::ImportIntoSystem { system_name } => {
                let repository = self.require_remote_repository(0)?;
                let manifest_path = self.optional_manifest_path(1)?;
                Some(PendingAction::ImportApplication {
                    repository,
                    system_name: Some(system_name.clone()),
                    manifest_path,
                })
            }
            FormKind::ImportApplication => {
                let repository = self.require_remote_repository(0)?;
                let system_name = self.optional_system_name(1)?;
                let manifest_path = self.optional_manifest_path(2)?;
                Some(PendingAction::ImportApplication {
                    repository,
                    system_name,
                    manifest_path,
                })
            }
        }
    }
}

enum Request {
    Systems,
    SystemShow { system_name: String },
    Catalog,
    Deployments { application_name: String },
    Status { application_name: String },
    Action(PendingAction),
}

impl Request {
    fn command(&self) -> Command {
        match self {
            Self::Systems => Command::SystemList,
            Self::SystemShow { system_name } => Command::SystemShow {
                name: system_name.clone(),
            },
            Self::Catalog => Command::ListApplications,
            Self::Deployments { application_name } => Command::ListDeployments {
                application_name: application_name.clone(),
            },
            Self::Status { application_name } => Command::ApplicationStatus {
                application_name: application_name.clone(),
            },
            Self::Action(action) => action.command(),
        }
    }

    fn action(&self) -> Option<&PendingAction> {
        match self {
            Self::Action(action) => Some(action),
            _ => None,
        }
    }
}

struct WorkerError {
    class: CliErrorClass,
    message: String,
}

impl WorkerError {
    fn from_control(error: pneuma::control::ControlError) -> Self {
        let error = CliError::from_control(error);
        Self {
            class: error.class(),
            message: error.to_string(),
        }
    }

    fn display(&self) -> String {
        format!("{}: {}", error_class_label(self.class), self.message)
    }
}

fn error_class_label(class: CliErrorClass) -> &'static str {
    match class {
        CliErrorClass::Failure => "Failure",
        CliErrorClass::Usage => "Usage",
        CliErrorClass::NotFound => "Not found",
        CliErrorClass::Conflict => "Conflict",
        CliErrorClass::External => "External",
    }
}

enum WorkerRequest {
    Execute { id: u64, command: Command },
    Shutdown,
}

enum WorkerReply {
    Event {
        id: u64,
        event: DeploymentEvent,
    },
    Finished {
        id: u64,
        result: Result<CommandResult, WorkerError>,
    },
}

struct Worker {
    requests: Option<Sender<WorkerRequest>>,
    replies: Receiver<WorkerReply>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    fn new() -> Self {
        let (request_tx, request_rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let handle = thread::spawn(move || run_worker(request_rx, reply_tx));
        Self {
            requests: Some(request_tx),
            replies: reply_rx,
            handle: Some(handle),
        }
    }

    fn execute(&self, id: u64, command: Command) -> io::Result<()> {
        self.requests
            .as_ref()
            .ok_or_else(|| io::Error::other("TUI worker is unavailable"))?
            .send(WorkerRequest::Execute { id, command })
            .map_err(|_| io::Error::other("TUI worker disconnected"))
    }

    fn shutdown(&mut self) -> io::Result<()> {
        if let Some(requests) = self.requests.take() {
            let _ = requests.send(WorkerRequest::Shutdown);
        }
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        handle
            .join()
            .map_err(|_| io::Error::other("TUI worker panicked"))
    }
}

fn run_worker(requests: Receiver<WorkerRequest>, replies: Sender<WorkerReply>) {
    let executor = ControlExecutor::from_environment();
    while let Ok(request) = requests.recv() {
        match request {
            WorkerRequest::Execute { id, command } => {
                let result = match &command {
                    Command::DeployImage { .. }
                    | Command::DeployBranch { .. }
                    | Command::Rollback { .. } => {
                        let events = replies.clone();
                        executor
                            .execute_with_events(command, &mut |event| {
                                // Event delivery is observational: a disconnected
                                // interface never changes command execution.
                                let _ = events.send(WorkerReply::Event { id, event });
                            })
                            .map_err(WorkerError::from_control)
                    }
                    _ => executor.execute(command).map_err(WorkerError::from_control),
                };
                if replies.send(WorkerReply::Finished { id, result }).is_err() {
                    return;
                }
            }
            WorkerRequest::Shutdown => return,
        }
    }
}

struct Session {
    worker: Worker,
    next_request_id: u64,
    active: Option<(u64, Request)>,
    queued: VecDeque<Request>,
    tab: Tab,
    focus: Focus,
    systems: QueryState<Vec<System>>,
    selected_system: Option<String>,
    system_details_target: Option<String>,
    system_details: QueryState<SystemDetails>,
    catalog: QueryState<Vec<ApplicationCatalogEntry>>,
    selected_application: Option<String>,
    observations_application: Option<String>,
    deployments: QueryState<Vec<DeploymentHistory>>,
    runtime: QueryState<RuntimeObservation>,
    mode: Mode,
    error: Option<String>,
    outcome: Option<ActionOutcome>,
    deployment_log: Option<DeploymentLog>,
    quit_after_completion: bool,
}

impl Session {
    fn new() -> Self {
        let mut session = Self {
            worker: Worker::new(),
            next_request_id: 1,
            active: None,
            queued: VecDeque::new(),
            tab: Tab::Applications,
            focus: Focus::Listing,
            systems: QueryState::Loading,
            selected_system: None,
            system_details_target: None,
            system_details: QueryState::Idle,
            catalog: QueryState::Loading,
            selected_application: None,
            observations_application: None,
            deployments: QueryState::Idle,
            runtime: QueryState::Idle,
            mode: Mode::Normal,
            error: None,
            outcome: None,
            deployment_log: None,
            quit_after_completion: false,
        };
        session.enqueue(Request::Catalog);
        session.enqueue(Request::Systems);
        session
    }

    fn shutdown(&mut self) -> io::Result<()> {
        self.worker.shutdown()
    }

    fn is_busy(&self) -> bool {
        self.active.is_some() || !self.queued.is_empty()
    }

    fn should_exit(&self) -> bool {
        self.quit_after_completion && !self.is_busy()
    }

    fn enqueue(&mut self, request: Request) {
        self.queued.push_back(request);
    }

    fn dispatch_next(&mut self) -> io::Result<()> {
        if self.active.is_some() {
            return Ok(());
        }
        let Some(request) = self.queued.pop_front() else {
            return Ok(());
        };
        let id = self.next_request_id;
        self.next_request_id += 1;
        let command = request.command();
        self.worker.execute(id, command)?;
        self.active = Some((id, request));
        self.dispatch_deployment_log();
        Ok(())
    }

    fn dispatch_deployment_log(&mut self) {
        let Some((_, Request::Action(action))) = self.active.as_ref() else {
            return;
        };
        let Some(application_name) = action.application_name().filter(|_| action.is_deployment())
        else {
            return;
        };
        self.deployment_log = Some(DeploymentLog::new(application_name.to_owned()));
    }

    // Retention: the finished log keeps its lines and records the classified
    // outcome; only a new deployment replaces it.
    fn finish_deployment_log(
        &mut self,
        request: &Request,
        result: &Result<CommandResult, WorkerError>,
    ) {
        let Request::Action(action) = request else {
            return;
        };
        if !action.is_deployment() {
            return;
        }
        let Some(log) = self.deployment_log.as_mut() else {
            return;
        };
        if Some(log.application_name.as_str()) != action.application_name() {
            return;
        }
        log.finish(result);
    }

    fn drain_replies(&mut self) -> io::Result<()> {
        self.dispatch_next()?;
        loop {
            match self.worker.replies.try_recv() {
                Ok(WorkerReply::Event { id, event }) => {
                    let Some((active_id, _)) = self.active.as_ref() else {
                        return Err(io::Error::other("TUI received an unexpected worker reply"));
                    };
                    if id != *active_id {
                        return Err(io::Error::other("TUI worker reply order changed"));
                    }
                    let deployment_active = self.active.as_ref().is_some_and(|(_, request)| {
                        request
                            .action()
                            .is_some_and(|action| action.is_deployment())
                    });
                    if deployment_active {
                        if let Some(log) = self.deployment_log.as_mut() {
                            log.record_event(&event);
                        }
                    }
                }
                Ok(WorkerReply::Finished { id, result }) => {
                    let Some((active_id, request)) = self.active.take() else {
                        return Err(io::Error::other("TUI received an unexpected worker reply"));
                    };
                    if id != active_id {
                        return Err(io::Error::other("TUI worker reply order changed"));
                    }
                    // Retention: the finished log keeps its lines and records
                    // the classified outcome; only a new deployment replaces it.
                    self.finish_deployment_log(&request, &result);
                    self.apply_result(request, result);
                    self.dispatch_next()?;
                }
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => {
                    return Err(io::Error::other("TUI worker disconnected"));
                }
            }
        }
    }

    fn handle_key(&mut self, event: KeyEvent) -> io::Result<()> {
        match &self.mode {
            Mode::Form(_) => return self.handle_form_key(event),
            Mode::Deploy(_) => return self.handle_deploy_form_key(event),
            Mode::Confirm(action) => return self.handle_confirmation(event, action.clone()),
            Mode::Normal => {}
        }

        if event.code == KeyCode::Char('q')
            || (event.code == KeyCode::Char('c') && event.modifiers.contains(KeyModifiers::CONTROL))
        {
            self.request_quit();
            return Ok(());
        }

        // Tab switching owns the digit keys and the Tab cycle while no modal
        // owns input; it is navigation, so it stays disabled while busy.
        match event.code {
            KeyCode::Char('1') if !self.is_busy() => self.switch_tab(Tab::Systems),
            KeyCode::Char('2') if !self.is_busy() => self.switch_tab(Tab::Applications),
            KeyCode::Char('3') if !self.is_busy() => self.switch_tab(Tab::Deployments),
            KeyCode::Tab if !self.is_busy() => self.switch_tab(self.tab.next()),
            KeyCode::BackTab if !self.is_busy() => self.switch_tab(self.tab.previous()),
            _ => {}
        }

        // Focus moves are always safe: they never start work, so an operator
        // can reach the details column or return to the listing even while a
        // load or action is running.
        match (self.focus, event.code) {
            (Focus::Log, KeyCode::Left | KeyCode::Esc) => {
                self.focus = Focus::Details;
                return Ok(());
            }
            (Focus::Details, KeyCode::Left | KeyCode::Esc) => {
                self.focus = Focus::Listing;
                return Ok(());
            }
            // Enter descends into the deployment log when one exists for the
            // selected application; focus moves never start work.
            (Focus::Details, KeyCode::Enter) if self.deployment_log_for_detail().is_some() => {
                self.focus = Focus::Log;
                return Ok(());
            }
            (Focus::Listing, KeyCode::Enter) => {
                self.focus = Focus::Details;
                return Ok(());
            }
            (Focus::Listing, KeyCode::Left) if !self.is_busy() => {
                self.switch_tab(self.tab.previous());
                return Ok(());
            }
            (Focus::Listing, KeyCode::Right) if !self.is_busy() => {
                self.switch_tab(self.tab.next());
                return Ok(());
            }
            _ => {}
        }

        match (self.tab, self.focus) {
            (Tab::Systems, Focus::Listing) => match event.code {
                KeyCode::Down | KeyCode::Char('j') if !self.is_busy() => self.select_next_system(),
                KeyCode::Up | KeyCode::Char('k') if !self.is_busy() => {
                    self.select_previous_system()
                }
                KeyCode::Char('n') if !self.is_busy() => self.open_system_create_form(),
                KeyCode::Char('a') if !self.is_busy() => self.add_application_to_selected_system(),
                KeyCode::Char('r') if !self.is_busy() => self.refresh_systems(),
                _ => {}
            },
            (Tab::Systems, Focus::Details) => match event.code {
                KeyCode::Down | KeyCode::Char('j') if !self.is_busy() => self.select_next_system(),
                KeyCode::Up | KeyCode::Char('k') if !self.is_busy() => {
                    self.select_previous_system()
                }
                KeyCode::Char('a') if !self.is_busy() => self.add_application_to_selected_system(),
                KeyCode::Char('r') if !self.is_busy() => self.refresh_system_details(),
                _ => {}
            },
            (Tab::Systems, Focus::Log) | (Tab::Deployments, Focus::Log) => {}
            (Tab::Applications, Focus::Listing) => match event.code {
                KeyCode::Down | KeyCode::Char('j') if !self.is_busy() => self.select_next(),
                KeyCode::Up | KeyCode::Char('k') if !self.is_busy() => self.select_previous(),
                KeyCode::Char('n') if !self.is_busy() => self.open_application_import_form(),
                KeyCode::Char('r') if !self.is_busy() => self.refresh_catalog(),
                _ => {}
            },
            (Tab::Applications, Focus::Details) => match event.code {
                KeyCode::Down | KeyCode::Char('j') if !self.is_busy() => self.select_next(),
                KeyCode::Up | KeyCode::Char('k') if !self.is_busy() => self.select_previous(),
                KeyCode::Char('r') if !self.is_busy() => self.refresh_details(),
                KeyCode::Char('s') => {
                    if let Some(application_name) = self.selected_application.clone() {
                        self.confirm(PendingAction::Start { application_name });
                    }
                }
                KeyCode::Char('x') => {
                    if let Some(application_name) = self.selected_application.clone() {
                        self.confirm(PendingAction::Stop { application_name });
                    }
                }
                KeyCode::Char('c') => {
                    if let Some(application_name) = self.selected_application.clone() {
                        self.confirm(PendingAction::Reconcile { application_name });
                    }
                }
                KeyCode::Char('d') => {
                    if let Some(application_name) = self.selected_application.clone() {
                        self.open_deploy_form(application_name);
                    }
                }
                KeyCode::Char('b') => {
                    if let Some(application_name) = self.selected_application.clone() {
                        self.confirm(PendingAction::Rollback { application_name });
                    }
                }
                KeyCode::Char('p') => {
                    if let Some(application_name) = self.selected_application.clone() {
                        self.confirm(PendingAction::SetVisibility {
                            application_name,
                            visibility: Visibility::Public,
                        });
                    }
                }
                KeyCode::Char('i') => {
                    if let Some(application_name) = self.selected_application.clone() {
                        self.confirm(PendingAction::SetVisibility {
                            application_name,
                            visibility: Visibility::Internal,
                        });
                    }
                }
                _ => {}
            },
            // The log pane scrolls without changing the selection and without
            // issuing commands, so it stays enabled while a command runs.
            (Tab::Applications, Focus::Log) => match event.code {
                KeyCode::Up | KeyCode::Char('k') => self.scroll_deployment_log_up(1),
                KeyCode::Down | KeyCode::Char('j') => self.scroll_deployment_log_down(1),
                KeyCode::PageUp => self.scroll_deployment_log_up(self.log_page_rows()),
                KeyCode::PageDown => self.scroll_deployment_log_down(self.log_page_rows()),
                KeyCode::Home => {
                    if let Some(log) = self.deployment_log_for_detail_mut() {
                        log.scroll_to_start();
                    }
                }
                KeyCode::End => {
                    if let Some(log) = self.deployment_log_for_detail_mut() {
                        log.scroll_to_end();
                    }
                }
                _ => {}
            },
            (Tab::Deployments, Focus::Listing) => match event.code {
                KeyCode::Down | KeyCode::Char('j') if !self.is_busy() => self.select_next(),
                KeyCode::Up | KeyCode::Char('k') if !self.is_busy() => self.select_previous(),
                KeyCode::Char('r') if !self.is_busy() => self.refresh_catalog(),
                _ => {}
            },
            (Tab::Deployments, Focus::Details) => match event.code {
                KeyCode::Down | KeyCode::Char('j') if !self.is_busy() => self.select_next(),
                KeyCode::Up | KeyCode::Char('k') if !self.is_busy() => self.select_previous(),
                KeyCode::Char('r') if !self.is_busy() => self.refresh_details(),
                _ => {}
            },
        }
        Ok(())
    }

    fn handle_form_key(&mut self, event: KeyEvent) -> io::Result<()> {
        let Mode::Form(mut form) = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return Ok(());
        };
        if event.code == KeyCode::Char('c') && event.modifiers.contains(KeyModifiers::CONTROL) {
            self.request_quit();
            return Ok(());
        }
        match event.code {
            KeyCode::Esc => {}
            KeyCode::Enter => match form.submit() {
                Some(action) => self.execute_action(action),
                None => self.mode = Mode::Form(form),
            },
            KeyCode::Backspace => {
                form.backspace();
                self.mode = Mode::Form(form);
            }
            KeyCode::Tab | KeyCode::Down => {
                form.focus_next();
                self.mode = Mode::Form(form);
            }
            KeyCode::BackTab | KeyCode::Up => {
                form.focus_previous();
                self.mode = Mode::Form(form);
            }
            KeyCode::Char(character) => {
                form.push(character);
                self.mode = Mode::Form(form);
            }
            _ => self.mode = Mode::Form(form),
        }
        Ok(())
    }

    fn handle_deploy_form_key(&mut self, event: KeyEvent) -> io::Result<()> {
        let Mode::Deploy(mut form) = std::mem::replace(&mut self.mode, Mode::Normal) else {
            return Ok(());
        };
        if event.code == KeyCode::Char('c') && event.modifiers.contains(KeyModifiers::CONTROL) {
            self.request_quit();
            return Ok(());
        }
        match event.code {
            KeyCode::Esc => {}
            KeyCode::Enter => match form.submit() {
                Some(action) => self.execute_action(action),
                None => self.mode = Mode::Deploy(form),
            },
            KeyCode::Backspace => {
                form.backspace();
                self.mode = Mode::Deploy(form);
            }
            KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                form.toggle_source();
                self.mode = Mode::Deploy(form);
            }
            KeyCode::Char(character) => {
                form.push(character);
                self.mode = Mode::Deploy(form);
            }
            _ => self.mode = Mode::Deploy(form),
        }
        Ok(())
    }

    fn handle_confirmation(&mut self, event: KeyEvent, action: PendingAction) -> io::Result<()> {
        match event.code {
            KeyCode::Enter | KeyCode::Char('y') => {
                self.mode = Mode::Normal;
                self.execute_action(action);
            }
            KeyCode::Esc | KeyCode::Char('n') => self.mode = Mode::Normal,
            _ => {}
        }
        Ok(())
    }

    fn request_quit(&mut self) {
        self.queued.clear();
        self.quit_after_completion = true;
    }

    fn confirm(&mut self, action: PendingAction) {
        self.error = None;
        self.outcome = None;
        self.mode = Mode::Confirm(action);
    }

    fn open_deploy_form(&mut self, application_name: String) {
        self.error = None;
        self.outcome = None;
        self.mode = Mode::Deploy(DeployForm::new(application_name));
    }

    fn switch_tab(&mut self, tab: Tab) {
        if self.is_busy() || self.tab == tab {
            return;
        }
        self.tab = tab;
        self.focus = Focus::Listing;
        // The details columns follow the selection, so entering a tab makes
        // sure the data for the current selection is present or loading.
        match tab {
            Tab::Systems => self.ensure_system_details_loaded(),
            Tab::Deployments => self.ensure_observations_loaded(),
            Tab::Applications => {}
        }
    }

    fn refresh_systems(&mut self) {
        if self.is_busy() {
            return;
        }
        self.systems = QueryState::Loading;
        self.error = None;
        self.enqueue(Request::Systems);
    }

    // The details columns follow the selection automatically: loading is
    // deduplicated against the target recorded with each request group.
    fn ensure_system_details_loaded(&mut self) {
        if self.is_busy() {
            return;
        }
        let Some(selected) = self.selected_system.clone() else {
            return;
        };
        let matches_selection = match &self.system_details {
            QueryState::Ready(details) => details.system.name.as_str() == selected,
            QueryState::Loading => self.system_details_target.as_deref() == Some(selected.as_str()),
            QueryState::Idle | QueryState::Failed(_) => false,
        };
        if matches_selection {
            return;
        }
        self.system_details = QueryState::Loading;
        self.system_details_target = Some(selected.clone());
        self.error = None;
        self.enqueue(Request::SystemShow {
            system_name: selected,
        });
    }

    fn ensure_observations_loaded(&mut self) {
        if self.is_busy() {
            return;
        }
        let Some(selected) = self.selected_application.clone() else {
            return;
        };
        if self.observations_application.as_deref() == Some(selected.as_str()) {
            return;
        }
        self.load_observations(selected);
    }

    fn refresh_system_details(&mut self) {
        if self.is_busy() {
            return;
        }
        let Some(selected) = self.selected_system.clone() else {
            return;
        };
        self.system_details = QueryState::Loading;
        self.system_details_target = Some(selected);
        self.error = None;
        self.enqueue(Request::SystemShow {
            system_name: self.system_details_target.clone().expect("target was set"),
        });
    }

    fn load_observations(&mut self, application_name: String) {
        self.observations_application = Some(application_name.clone());
        self.deployments = QueryState::Loading;
        self.runtime = QueryState::Loading;
        self.error = None;
        self.enqueue(Request::Deployments {
            application_name: application_name.clone(),
        });
        self.enqueue(Request::Status { application_name });
    }

    fn open_system_create_form(&mut self) {
        self.error = None;
        self.outcome = None;
        self.mode = Mode::Form(Form::system_create());
    }

    fn add_application_to_selected_system(&mut self) {
        let Some(system_name) = self.selected_system.clone() else {
            self.error = Some("Select a system before adding an application.".to_owned());
            return;
        };
        self.error = None;
        self.outcome = None;
        self.mode = Mode::Form(Form::import_into_system(system_name));
    }

    fn open_application_import_form(&mut self) {
        self.error = None;
        self.outcome = None;
        self.mode = Mode::Form(Form::import_application());
    }

    fn select_next_system(&mut self) {
        let QueryState::Ready(systems) = &self.systems else {
            return;
        };
        self.selected_system =
            next_selection(systems, self.selected_system.as_deref(), 1, system_name);
        self.ensure_system_details_loaded();
    }

    fn select_previous_system(&mut self) {
        let QueryState::Ready(systems) = &self.systems else {
            return;
        };
        self.selected_system =
            next_selection(systems, self.selected_system.as_deref(), -1, system_name);
        self.ensure_system_details_loaded();
    }

    fn execute_action(&mut self, action: PendingAction) {
        self.error = None;
        self.outcome = None;
        self.queued.clear();
        self.enqueue(Request::Action(action));
    }

    fn action_is_pending(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|(_, request)| matches!(request, Request::Action(_)))
            || self
                .queued
                .iter()
                .any(|request| matches!(request, Request::Action(_)))
    }

    fn refresh_catalog(&mut self) {
        if self.is_busy() {
            return;
        }
        self.catalog = QueryState::Loading;
        self.error = None;
        self.enqueue(Request::Catalog);
    }

    fn refresh_details(&mut self) {
        if self.is_busy() {
            return;
        }
        let Some(application_name) = self.selected_application.clone() else {
            return;
        };
        self.load_observations(application_name);
    }

    fn select_next(&mut self) {
        let QueryState::Ready(entries) = &self.catalog else {
            return;
        };
        self.selected_application = next_selection(
            entries,
            self.selected_application.as_deref(),
            1,
            application_entry_name,
        );
        self.ensure_observations_loaded();
    }

    fn select_previous(&mut self) {
        let QueryState::Ready(entries) = &self.catalog else {
            return;
        };
        self.selected_application = next_selection(
            entries,
            self.selected_application.as_deref(),
            -1,
            application_entry_name,
        );
        self.ensure_observations_loaded();
    }

    fn apply_result(&mut self, request: Request, result: Result<CommandResult, WorkerError>) {
        match result {
            Ok(result) => self.apply_success(request, result),
            Err(error) => self.apply_error(request, error.display()),
        }
    }

    fn apply_success(&mut self, request: Request, result: CommandResult) {
        match (request, result) {
            (Request::Systems, CommandResult::Systems(systems)) => self.apply_systems(systems),
            (Request::SystemShow { system_name }, CommandResult::SystemDetails(details))
                if system_name == details.system.name.to_string() =>
            {
                self.system_details = QueryState::Ready(details);
            }
            (Request::Catalog, CommandResult::Applications(entries)) => {
                self.apply_catalog(entries);
            }
            (
                Request::Action(PendingAction::SystemCreate {
                    name: requested,
                    description,
                }),
                CommandResult::SystemCreated(system),
            ) if requested == system.name.to_string() => {
                self.outcome = Some(ActionOutcome {
                    scope: system.name.to_string(),
                    message: format!("Created system {}", system.name),
                });
                self.refresh_after_action(&PendingAction::SystemCreate {
                    name: system.name.to_string(),
                    description,
                });
            }
            (
                Request::Action(action @ PendingAction::ImportApplication { .. }),
                CommandResult::ApplicationImported(application),
            ) => {
                self.outcome = Some(ActionOutcome {
                    scope: application.name.as_str().to_owned(),
                    message: format!("Imported application {}", application.name),
                });
                self.selected_application = Some(application.name.as_str().to_owned());
                self.refresh_after_action(&action);
            }
            (
                Request::Deployments { application_name },
                CommandResult::ApplicationDeployments {
                    application_name: result_name,
                    deployments,
                },
            ) if application_name == result_name.as_str() => {
                self.deployments = QueryState::Ready(deployments);
            }
            (
                Request::Status { application_name },
                CommandResult::ApplicationStatus {
                    application_name: result_name,
                    observation,
                },
            ) if application_name == result_name.as_str() => {
                self.runtime = QueryState::Ready(observation);
            }
            (
                Request::Action(PendingAction::Start { application_name }),
                CommandResult::ApplicationStarted {
                    application_name: result_name,
                    observation,
                },
            ) if application_name == result_name.as_str() => {
                self.runtime = QueryState::Ready(observation);
                self.outcome = Some(ActionOutcome {
                    scope: application_name,
                    message: format!("Started {result_name}"),
                });
                self.refresh_after_action(&PendingAction::Start {
                    application_name: result_name.as_str().to_owned(),
                });
            }
            (
                Request::Action(PendingAction::Stop { application_name }),
                CommandResult::ApplicationStopped {
                    application_name: result_name,
                    observation,
                },
            ) if application_name == result_name.as_str() => {
                self.runtime = QueryState::Ready(observation);
                self.outcome = Some(ActionOutcome {
                    scope: application_name,
                    message: format!("Stopped {result_name}"),
                });
                self.refresh_after_action(&PendingAction::Stop {
                    application_name: result_name.as_str().to_owned(),
                });
            }
            (
                Request::Action(action @ PendingAction::Reconcile { .. }),
                CommandResult::Reconciled {
                    application_name: result_name,
                    result,
                },
            ) if action.application_name() == Some(result_name.as_str()) => {
                self.outcome = Some(ActionOutcome {
                    scope: result_name.as_str().to_owned(),
                    message: output::reconciliation_result(&result_name, &result),
                });
                self.refresh_after_action(&action);
            }
            (
                Request::Action(action @ PendingAction::SetVisibility { .. }),
                CommandResult::ExposureChanged {
                    application_name: result_name,
                    change,
                },
            ) if action.application_name() == Some(result_name.as_str())
                && action.targets_visibility(change.visibility) =>
            {
                self.outcome = Some(ActionOutcome {
                    scope: result_name.as_str().to_owned(),
                    message: output::visibility_change(&result_name, &change),
                });
                self.refresh_after_action(&action);
            }
            (
                Request::Action(
                    action @ (PendingAction::DeployBranch { .. }
                    | PendingAction::DeployImage { .. }),
                ),
                CommandResult::ApplicationDeployed {
                    application_name: result_name,
                    deployment,
                },
            ) if action.application_name() == Some(result_name.as_str()) => {
                self.outcome = Some(ActionOutcome {
                    scope: result_name.as_str().to_owned(),
                    message: deployment_result_message("Deployed", &result_name, &deployment),
                });
                self.refresh_after_action(&action);
            }
            (
                Request::Action(action @ PendingAction::Rollback { .. }),
                CommandResult::ApplicationRolledBack {
                    application_name: result_name,
                    deployment,
                },
            ) if action.application_name() == Some(result_name.as_str()) => {
                self.outcome = Some(ActionOutcome {
                    scope: result_name.as_str().to_owned(),
                    message: deployment_result_message("Rolled back", &result_name, &deployment),
                });
                self.refresh_after_action(&action);
            }
            (request, _) => self.apply_error(
                request,
                "TUI received an unexpected control result".to_owned(),
            ),
        }
    }

    fn apply_error(&mut self, request: Request, error: String) {
        match request {
            Request::Systems => self.systems = QueryState::Failed(error.clone()),
            Request::SystemShow { .. } => {
                self.system_details = QueryState::Failed(error.clone());
            }
            Request::Catalog => {
                self.catalog = QueryState::Failed(error.clone());
                self.selected_application = None;
            }
            Request::Deployments { .. } => self.deployments = QueryState::Failed(error.clone()),
            Request::Status { .. } => self.runtime = QueryState::Failed(error.clone()),
            Request::Action(action) => self.refresh_after_action(&action),
        }
        self.error = Some(error);
    }

    fn refresh_after_action(&mut self, action: &PendingAction) {
        if self.quit_after_completion || self.action_is_pending() {
            return;
        }
        match action {
            PendingAction::SystemCreate { name, .. } => {
                self.systems = QueryState::Loading;
                self.selected_system = Some(name.clone());
                self.enqueue(Request::Systems);
            }
            PendingAction::ImportApplication { system_name, .. } => {
                self.catalog = QueryState::Loading;
                self.enqueue(Request::Catalog);
                if let Some(system_name) = system_name {
                    self.systems = QueryState::Loading;
                    self.enqueue(Request::Systems);
                    if matches!(self.system_details, QueryState::Ready(_)) {
                        self.system_details = QueryState::Loading;
                        self.enqueue(Request::SystemShow {
                            system_name: system_name.clone(),
                        });
                    }
                }
            }
            PendingAction::Start { application_name }
            | PendingAction::Stop { application_name } => {
                self.refresh_application_catalog();
                self.runtime = QueryState::Loading;
                self.enqueue(Request::Status {
                    application_name: application_name.clone(),
                });
            }
            PendingAction::Reconcile { application_name }
            | PendingAction::DeployBranch {
                application_name, ..
            }
            | PendingAction::DeployImage {
                application_name, ..
            }
            | PendingAction::Rollback { application_name } => {
                self.refresh_application_catalog();
                self.deployments = QueryState::Loading;
                self.runtime = QueryState::Loading;
                self.enqueue(Request::Deployments {
                    application_name: application_name.clone(),
                });
                self.enqueue(Request::Status {
                    application_name: application_name.clone(),
                });
            }
            PendingAction::SetVisibility { .. } => self.refresh_application_catalog(),
        }
    }

    // Catalog refreshes cover both the Applications tab listing and the
    // Deployments tab listing, which share the application catalog.
    fn refresh_application_catalog(&mut self) {
        self.catalog = QueryState::Loading;
        self.enqueue(Request::Catalog);
    }

    fn apply_systems(&mut self, systems: Vec<System>) {
        self.selected_system =
            preserved_selection(&systems, self.selected_system.take(), system_name);
        self.systems = QueryState::Ready(systems);
        // The first (or preserved) system's details load without an explicit
        // request so the details column is never empty behind a selection.
        self.ensure_system_details_loaded();
    }

    fn apply_catalog(&mut self, entries: Vec<ApplicationCatalogEntry>) {
        self.selected_application = preserved_selection(
            &entries,
            self.selected_application.take(),
            application_entry_name,
        );
        self.catalog = QueryState::Ready(entries);
        self.ensure_observations_loaded();
    }

    fn selected_entry(&self) -> Option<&ApplicationCatalogEntry> {
        let QueryState::Ready(entries) = &self.catalog else {
            return None;
        };
        let selected_name = self.selected_application.as_deref()?;
        entries
            .iter()
            .find(|entry| entry.summary.name.as_str() == selected_name)
    }

    fn runtime_for_detail(&self) -> Option<&QueryState<RuntimeObservation>> {
        let selected = self.selected_application.as_deref()?;
        (self.observations_application.as_deref() == Some(selected)).then_some(&self.runtime)
    }

    fn outcome_for_detail(&self) -> Option<&str> {
        let outcome = self.outcome.as_ref()?;
        (self.selected_application.as_deref() == Some(outcome.scope.as_str()))
            .then_some(outcome.message.as_str())
    }

    fn outcome_for_system_detail(&self) -> Option<&str> {
        let QueryState::Ready(details) = &self.system_details else {
            return None;
        };
        let outcome = self.outcome.as_ref()?;
        (details.system.name.as_str() == outcome.scope).then_some(outcome.message.as_str())
    }

    fn deployment_log_for_detail(&self) -> Option<&DeploymentLog> {
        let log = self.deployment_log.as_ref()?;
        (self.selected_application.as_deref() == Some(log.application_name.as_str())).then_some(log)
    }

    fn deployment_log_for_detail_mut(&mut self) -> Option<&mut DeploymentLog> {
        let selected = self.selected_application.clone()?;
        let log = self.deployment_log.as_mut()?;
        (log.application_name == selected).then_some(log)
    }

    fn scroll_deployment_log_up(&mut self, rows: u16) {
        if let Some(log) = self.deployment_log_for_detail_mut() {
            log.scroll_up(rows);
        }
    }

    fn scroll_deployment_log_down(&mut self, rows: u16) {
        if let Some(log) = self.deployment_log_for_detail_mut() {
            log.scroll_down(rows);
        }
    }

    fn log_page_rows(&self) -> u16 {
        self.deployment_log_for_detail()
            .map(|log| log.viewport_rows.max(1))
            .unwrap_or(1)
    }

    // Records the wrapped row count and visible height of the log panel so
    // scrolling keys can clamp against real rendered bounds.
    fn update_deployment_log_metrics(&mut self, log_area: ratatui::layout::Rect) {
        let Some(log) = self.deployment_log_for_detail_mut() else {
            return;
        };
        let width = log_area.width.saturating_sub(2);
        log.viewport_rows = log_area.height.saturating_sub(2);
        let text = deployment_log_text(log);
        log.total_rows = if width < 1 {
            u16::try_from(text.len()).unwrap_or(u16::MAX)
        } else {
            u16::try_from(
                Paragraph::new(text)
                    .wrap(Wrap { trim: true })
                    .line_count(width),
            )
            .unwrap_or(u16::MAX)
        };
    }
}

fn deployment_result_message(
    verb: &str,
    application_name: &ApplicationName,
    deployment: &DeploymentResult,
) -> String {
    format!(
        "{verb} {application_name}: deployment {} promoted ({})",
        deployment.deployment_id,
        deployment.artifact.reference()
    )
}

fn deployment_event_line(event: &DeploymentEvent) -> String {
    match event {
        DeploymentEvent::DeploymentRequested { application_name } => {
            format!("Deploying {application_name}...")
        }
        event => super::progress::render_deployment_event(event),
    }
}

fn deployment_log_text(log: &DeploymentLog) -> Vec<Line<'static>> {
    let mut lines = log
        .lines
        .iter()
        .map(|line| Line::from(line.clone()))
        .collect::<Vec<_>>();
    if let DeploymentLogState::Failed(error) = &log.state {
        lines.push(Line::styled(
            format!("Deployment failed: {error}"),
            Style::default().fg(Color::Red),
        ));
    }
    lines
}

fn deployment_log_state_label(state: &DeploymentLogState) -> &'static str {
    match state {
        DeploymentLogState::Running => "running",
        DeploymentLogState::Completed => "completed",
        DeploymentLogState::Failed(_) => "failed",
    }
}

fn render_deployment_log_panel(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    log: &DeploymentLog,
    focused: bool,
) {
    let (title, title_style) = if focused {
        let state = deployment_log_state_label(&log.state);
        let offset = log.render_offset();
        let start = log.total_rows.min(offset.saturating_add(1));
        let end = log.total_rows.min(offset.saturating_add(log.viewport_rows));
        (
            Line::styled(
                format!(
                    " Deployment log · {} · {state} · rows {start}–{end} of {} ",
                    log.application_name, log.total_rows
                ),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        match &log.state {
            DeploymentLogState::Running => (
                Line::styled(
                    " Deployment progress ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Style::default(),
            ),
            DeploymentLogState::Completed => (
                Line::styled(
                    " Deployment log (completed) ",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Style::default(),
            ),
            DeploymentLogState::Failed(_) => (
                Line::styled(
                    " Deployment log (failed) ",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Style::default(),
            ),
        }
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .title_style(title_style);
    if focused {
        block = block.border_style(Style::default().fg(Color::Cyan));
    }
    frame.render_widget(
        Paragraph::new(deployment_log_text(log))
            .wrap(Wrap { trim: true })
            .scroll((log.render_offset(), 0))
            .block(block),
        area,
    );
}

fn application_entry_name(entry: &ApplicationCatalogEntry) -> &str {
    entry.summary.name.as_str()
}

fn system_name(system: &System) -> &str {
    system.name.as_str()
}

fn preserved_selection<T>(
    entries: &[T],
    previous: Option<String>,
    name: fn(&T) -> &str,
) -> Option<String> {
    previous
        .filter(|previous| entries.iter().any(|entry| name(entry) == previous))
        .or_else(|| entries.first().map(|entry| name(entry).to_owned()))
}

fn next_selection<T>(
    entries: &[T],
    current: Option<&str>,
    movement: isize,
    name: fn(&T) -> &str,
) -> Option<String> {
    let current_index =
        current.and_then(|current| entries.iter().position(|entry| name(entry) == current));
    let next_index = match (entries.len(), current_index, movement) {
        (0, _, _) => return None,
        (_, Some(index), 1) => (index + 1).min(entries.len() - 1),
        (_, Some(index), -1) => index.saturating_sub(1),
        (_, _, 1) => 0,
        (_, _, -1) => entries.len() - 1,
        _ => return current.map(str::to_owned),
    };
    Some(name(&entries[next_index]).to_owned())
}

fn draw_shell(frame: &mut ratatui::Frame<'_>, session: &mut Session) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(frame.area());
    let content = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(areas[1]);

    draw_tab_bar(frame, areas[0], session);
    match session.tab {
        Tab::Systems => {
            draw_systems_listing(frame, content[0], session);
            draw_system_details(frame, content[1], session);
        }
        Tab::Applications => {
            draw_catalog(frame, content[0], session);
            draw_detail(frame, content[1], session);
        }
        Tab::Deployments => {
            draw_catalog(frame, content[0], session);
            draw_runtime_details(frame, content[1], session);
        }
    }
    draw_footer(frame, areas[2], session);
    match &session.mode {
        Mode::Confirm(action) => draw_confirmation(frame, action),
        Mode::Form(form) => draw_form(frame, form),
        Mode::Deploy(form) => draw_deploy_form(frame, form),
        Mode::Normal => {}
    }
}

fn draw_tab_bar(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, session: &Session) {
    let tabs = Tabs::new(Tab::ALL.map(Tab::label))
        .select(session.tab.index())
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(tabs, area);
}

fn draw_systems_listing(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    session: &Session,
) {
    let block = || {
        Block::default()
            .borders(Borders::ALL)
            .title(" Systems ")
            .title_style(title_style())
    };
    match &session.systems {
        QueryState::Idle => frame.render_widget(
            Paragraph::new("Press r to load systems.").block(block()),
            area,
        ),
        QueryState::Loading => {
            frame.render_widget(Paragraph::new("Loading systems...").block(block()), area)
        }
        QueryState::Failed(error) => frame.render_widget(
            Paragraph::new(format!("Could not load systems:\n{error}"))
                .style(Style::default().fg(Color::Red))
                .wrap(Wrap { trim: true })
                .block(block()),
            area,
        ),
        QueryState::Ready(systems) if systems.is_empty() => frame.render_widget(
            Paragraph::new("No systems are registered. Press n to create one.").block(block()),
            area,
        ),
        QueryState::Ready(systems) => {
            let items = systems
                .iter()
                .map(|system| {
                    let description = system
                        .description
                        .as_deref()
                        .map_or_else(absent_span, |description| Span::raw(description.to_owned()));
                    ListItem::new(vec![
                        Line::from(Span::styled(
                            system.name.to_string(),
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        )),
                        Line::from(vec![Span::raw("  "), description]),
                    ])
                })
                .collect::<Vec<_>>();
            let list = List::new(items).block(block()).highlight_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::REVERSED),
            );
            let mut state = ListState::default();
            state.select(session.selected_system.as_deref().and_then(|name| {
                systems
                    .iter()
                    .position(|system| system.name.as_str() == name)
            }));
            frame.render_stateful_widget(list, area, &mut state);
        }
    }
}

fn draw_system_details(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    session: &Session,
) {
    if session.selected_system.is_none() {
        frame.render_widget(
            Paragraph::new("No system is selected.")
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" System details ")
                        .title_style(title_style()),
                ),
            area,
        );
        return;
    }

    let detail_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(4)])
        .split(area);
    let details = match &session.system_details {
        QueryState::Idle => vec![Line::from(
            "The system details were not requested yet. Press r to load them.",
        )],
        QueryState::Loading => vec![Line::from("Loading system details...")],
        QueryState::Failed(error) => vec![
            Line::from("Could not load system details:"),
            Line::styled(error.clone(), Style::default().fg(Color::Red)),
        ],
        QueryState::Ready(details) => system_details_lines(details),
    };
    frame.render_widget(
        Paragraph::new(details).wrap(Wrap { trim: true }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" System details ")
                .title_style(title_style()),
        ),
        detail_areas[0],
    );
    let action_text = match session.outcome_for_system_detail() {
        Some(message) => vec![Line::from(message.to_owned())],
        None => vec![Line::from("No action has completed for this system.")],
    };
    frame.render_widget(
        Paragraph::new(action_text).wrap(Wrap { trim: true }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Last action ")
                .title_style(title_style()),
        ),
        detail_areas[1],
    );
}

fn system_details_lines(details: &SystemDetails) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(vec![
            label_span("System"),
            Span::styled(
                details.system.name.to_string(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            label_span("Description"),
            details
                .system
                .description
                .clone()
                .map_or_else(absent_span, value_span),
        ]),
        Line::from(vec![
            label_span("Applications"),
            Span::raw(details.applications.len().to_string()),
        ]),
    ];
    if details.applications.is_empty() {
        lines.push(Line::from(
            "No applications belong to this system. Press a to add one.",
        ));
    } else {
        for application in &details.applications {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    application.name.as_str().to_owned(),
                    Style::default().fg(Color::White),
                ),
            ]));
        }
    }
    lines
}

fn draw_runtime_details(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    session: &Session,
) {
    if session.selected_application.is_none() {
        frame.render_widget(
            Paragraph::new("No application is selected.")
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Runtime and deployments ")
                        .title_style(title_style()),
                ),
            area,
        );
        return;
    }

    let detail_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(40), Constraint::Min(4)])
        .split(area);
    frame.render_widget(
        Paragraph::new(runtime_lines(&session.runtime))
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Runtime status ")
                    .title_style(title_style()),
            ),
        detail_areas[0],
    );
    frame.render_widget(
        Paragraph::new(deployment_history_lines(&session.deployments))
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Deployment history ")
                    .title_style(title_style()),
            ),
        detail_areas[1],
    );
}

fn draw_catalog(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, session: &Session) {
    let block = || {
        Block::default()
            .borders(Borders::ALL)
            .title(" Applications ")
            .title_style(title_style())
    };
    match &session.catalog {
        QueryState::Idle => frame.render_widget(
            Paragraph::new("Refresh to load applications.").block(block()),
            area,
        ),
        QueryState::Loading => frame.render_widget(
            Paragraph::new("Loading applications...").block(block()),
            area,
        ),
        QueryState::Failed(error) => frame.render_widget(
            Paragraph::new(format!("Could not load applications:\n{error}"))
                .style(Style::default().fg(Color::Red))
                .wrap(Wrap { trim: true })
                .block(block()),
            area,
        ),
        QueryState::Ready(entries) if entries.is_empty() => frame.render_widget(
            Paragraph::new("No applications are registered.").block(block()),
            area,
        ),
        QueryState::Ready(entries) => {
            let items = entries
                .iter()
                .map(|entry| {
                    let deployment = if entry.deployed {
                        Span::styled(
                            "Has successful deployment",
                            Style::default().fg(Color::Green),
                        )
                    } else {
                        Span::styled(
                            "No successful deployment",
                            Style::default().fg(Color::DarkGray),
                        )
                    };
                    ListItem::new(vec![
                        Line::from(Span::styled(
                            entry.summary.name.as_str().to_owned(),
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        )),
                        Line::from(vec![Span::raw("  "), deployment]),
                    ])
                })
                .collect::<Vec<_>>();
            let list = List::new(items).block(block()).highlight_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::REVERSED),
            );
            let mut state = ListState::default();
            state.select(session.selected_application.as_deref().and_then(|name| {
                entries
                    .iter()
                    .position(|entry| entry.summary.name.as_str() == name)
            }));
            frame.render_stateful_widget(list, area, &mut state);
        }
    }
}

fn draw_detail(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, session: &mut Session) {
    if session.selected_application.is_none() {
        frame.render_widget(
            Paragraph::new("No application is selected.")
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true })
                .block(Block::default().borders(Borders::ALL).title(" Details ")),
            area,
        );
        return;
    }

    let log_available = session.deployment_log_for_detail().is_some();
    // While the log pane owns the focus it takes the whole details column with
    // a highlighted border; the summary panels return when focus leaves it.
    if session.focus == Focus::Log && log_available {
        session.update_deployment_log_metrics(area);
        if let Some(log) = session.deployment_log_for_detail() {
            render_deployment_log_panel(frame, area, log, true);
        }
        return;
    }

    let detail_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if log_available {
            vec![
                Constraint::Percentage(35),
                Constraint::Min(9),
                Constraint::Length(4),
            ]
        } else {
            vec![Constraint::Percentage(70), Constraint::Length(4)]
        })
        .split(area);
    let summary_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(detail_areas[0]);
    let details = session.selected_entry().map_or_else(
        || {
            vec![Line::from(
                "The selected application is no longer in the catalog.",
            )]
        },
        application_details_lines,
    );
    frame.render_widget(
        Paragraph::new(details).wrap(Wrap { trim: true }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Application details ")
                .title_style(title_style()),
        ),
        summary_areas[0],
    );
    let idle_runtime = QueryState::Idle;
    let runtime = session.runtime_for_detail().unwrap_or(&idle_runtime);
    frame.render_widget(
        Paragraph::new(runtime_lines(runtime))
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Runtime status ")
                    .title_style(title_style()),
            ),
        summary_areas[1],
    );
    let log_area;
    let action_area = if log_available {
        session.update_deployment_log_metrics(detail_areas[1]);
        let (log_area_split, remaining) = detail_areas[1..].split_at(1);
        log_area = log_area_split[0];
        if let Some(log) = session.deployment_log_for_detail() {
            render_deployment_log_panel(frame, log_area, log, false);
        }
        let (action_area_split, _) = remaining.split_at(1);
        action_area_split[0]
    } else {
        detail_areas[1]
    };
    let action_text = match session.outcome_for_detail() {
        Some(message) => vec![Line::from(message.to_owned())],
        None => vec![Line::from("No action has completed for this application.")],
    };
    frame.render_widget(
        Paragraph::new(action_text).wrap(Wrap { trim: true }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(Line::styled(" Last action ", title_style())),
        ),
        action_area,
    );
}

fn application_details_lines(entry: &ApplicationCatalogEntry) -> Vec<Line<'static>> {
    vec![
        Line::from(vec![
            label_span("Application"),
            Span::styled(
                entry.summary.name.as_str().to_owned(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            label_span("Repository"),
            value_span(entry.summary.repository.clone()),
        ]),
        Line::from(vec![
            label_span("Default branch"),
            entry
                .summary
                .default_branch
                .clone()
                .map_or_else(absent_span, value_span),
        ]),
        Line::from(vec![
            label_span("Desired runtime state"),
            Span::styled(
                output::desired_runtime_state_label(entry.summary.desired_runtime_state).to_owned(),
                Style::default().fg(desired_state_color(entry.summary.desired_runtime_state)),
            ),
        ]),
        Line::from(vec![
            label_span("Has successful deployment"),
            if entry.deployed {
                Span::styled("yes", Style::default().fg(Color::Green))
            } else {
                Span::styled("no", Style::default().fg(Color::DarkGray))
            },
        ]),
        Line::from(vec![
            label_span("Active deployment ID"),
            entry
                .summary
                .active_deployment_id
                .as_ref()
                .map_or_else(absent_span, |id| value_span(id.to_string())),
        ]),
    ]
}

fn runtime_lines(state: &QueryState<RuntimeObservation>) -> Vec<Line<'static>> {
    match state {
        QueryState::Idle => vec![Line::from(
            "The runtime status was not requested yet. Press r to load it.",
        )],
        QueryState::Loading => vec![Line::from("Loading runtime status...")],
        QueryState::Failed(error) => vec![
            Line::from("Could not load runtime status:"),
            Line::styled(error.clone(), Style::default().fg(Color::Red)),
        ],
        QueryState::Ready(observation) => vec![
            Line::from(vec![
                label_span("Desired runtime state"),
                Span::styled(
                    output::desired_runtime_state_label(observation.desired_runtime_state)
                        .to_owned(),
                    Style::default().fg(desired_state_color(observation.desired_runtime_state)),
                ),
            ]),
            Line::from(vec![
                label_span("Observed runtime state"),
                Span::styled(
                    output::observed_runtime_state_label(&observation.observed_runtime_state),
                    Style::default().fg(observed_state_color(&observation.observed_runtime_state)),
                ),
            ]),
            Line::from(vec![
                label_span("Runtime ID"),
                value_span(observation.runtime_id.to_string()),
            ]),
            Line::from(vec![
                label_span("Container ID"),
                value_span(observation.container_id.to_string()),
            ]),
            Line::from(vec![
                label_span("Observed endpoint"),
                observation
                    .observed_endpoint
                    .as_ref()
                    .map_or_else(absent_span, |endpoint| value_span(endpoint.to_string())),
            ]),
        ],
    }
}

fn deployment_history_lines(state: &QueryState<Vec<DeploymentHistory>>) -> Vec<Line<'static>> {
    match state {
        QueryState::Idle => vec![Line::from(
            "The deployment history was not requested yet. Press r to load it.",
        )],
        QueryState::Loading => vec![Line::from("Loading deployment history...")],
        QueryState::Failed(error) => vec![
            Line::from("Could not load deployment history:"),
            Line::styled(error.clone(), Style::default().fg(Color::Red)),
        ],
        QueryState::Ready(deployments) if deployments.is_empty() => {
            vec![Line::from("No deployments.")]
        }
        QueryState::Ready(deployments) => deployments
            .iter()
            .map(|history| {
                let mut line = vec![
                    Span::styled(
                        history.deployment.id.to_string(),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::raw(" | "),
                    Span::raw(
                        output::deployment_type_label(history.deployment.deployment_type)
                            .to_owned(),
                    ),
                    Span::raw(" | "),
                    Span::styled(
                        output::deployment_status_label(history.deployment.status()).to_owned(),
                        Style::default().fg(deployment_status_color(history.deployment.status())),
                    ),
                    Span::raw(" | "),
                    Span::raw(history.release.artifact.reference().to_owned()),
                ];
                if history.is_active {
                    line.push(Span::styled(" | active", Style::default().fg(Color::Green)));
                }
                Line::from(line)
            })
            .collect(),
    }
}

fn draw_footer(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, session: &Session) {
    let line = if session.quit_after_completion && session.is_busy() {
        Line::from(vec![
            badge("quitting", Color::Yellow),
            Span::raw(" Finishing the current refresh before quitting..."),
        ])
    } else if session.is_busy() {
        Line::from(vec![
            badge("busy", Color::Yellow),
            Span::raw(" Refreshing... a confirmed action will run next; navigation is disabled."),
        ])
    } else if let Some(error) = &session.error {
        Line::from(vec![
            badge("error", Color::Red),
            Span::styled(format!(" {error}"), Style::default().fg(Color::Red)),
        ])
    } else if session.tab == Tab::Applications && session.focus == Focus::Log {
        let mut spans = Vec::new();
        for (key, description) in [
            ("Up/Down or j/k", "line"),
            ("PgUp/PgDn", "page"),
            ("Home/End", "oldest/newest"),
            ("Esc", "details"),
            ("q", "quit"),
        ] {
            spans.extend(key_hint(key, description));
        }
        Line::from(spans)
    } else if session.tab == Tab::Applications && session.focus == Focus::Details {
        let mut spans = Vec::new();
        for (key, description) in [
            ("s", "start"),
            ("x", "stop"),
            ("c", "reconcile"),
            ("d", "deploy"),
            ("b", "rollback"),
            ("p", "public"),
            ("i", "internal"),
        ] {
            spans.extend(key_hint(key, description));
        }
        if session.deployment_log_for_detail().is_some() {
            spans.extend(key_hint("Enter", "log"));
        }
        for (key, description) in [("Esc", "catalog"), ("r", "refresh"), ("q", "quit")] {
            spans.extend(key_hint(key, description));
        }
        Line::from(spans)
    } else {
        let mut spans = Vec::new();
        for (key, description) in [
            ("1/2/3 or Left/Right", "tab"),
            ("Up/Down or j/k", "select"),
            ("Enter", "details"),
            ("r", "refresh"),
            ("q", "quit"),
        ] {
            spans.extend(key_hint(key, description));
        }
        if session.tab == Tab::Systems && session.focus == Focus::Listing {
            spans.extend(key_hint("n", "new system"));
            spans.extend(key_hint("a", "add application"));
        } else if session.tab == Tab::Applications && session.focus == Focus::Listing {
            spans.extend(key_hint("n", "import"));
        }
        Line::from(spans)
    };
    frame.render_widget(
        Paragraph::new(line)
            .style(Style::default().bg(Color::DarkGray).fg(Color::Gray))
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(Color::DarkGray)),
            ),
        area,
    );
}

fn draw_confirmation(frame: &mut ratatui::Frame<'_>, action: &PendingAction) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Length(8),
            Constraint::Percentage(30),
        ])
        .split(frame.area());
    let popup = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(20),
            Constraint::Percentage(60),
            Constraint::Percentage(20),
        ])
        .split(vertical[1])[1];
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(action.confirmation_text()),
            Line::default(),
            Line::from(vec![
                badge("Enter/y", Color::Green),
                Span::raw(" confirm   "),
                badge("Esc/n", Color::Red),
                Span::raw(" cancel"),
            ]),
        ])
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Confirm action ")
                .title_style(title_style()),
        ),
        popup,
    );
}

fn draw_form(frame: &mut ratatui::Frame<'_>, form: &Form) {
    let content_lines = 2 + form.fields.len() as u16 + 2 + u16::from(form.error.is_some());
    let height = content_lines + 2;
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(height),
            Constraint::Fill(2),
        ])
        .split(frame.area());
    let popup = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(20),
            Constraint::Percentage(60),
            Constraint::Percentage(20),
        ])
        .split(vertical[1])[1];
    frame.render_widget(Clear, popup);

    let interior_x = popup.x + 1;
    let interior_y = popup.y + 1;
    let interior_width = usize::from(popup.width.saturating_sub(2));

    let mut lines = Vec::new();
    if let Some(context) = &form.context {
        lines.push(Line::from(Span::styled(
            context.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::default());
    }
    let field_first_row = lines.len() as u16;
    for (index, field) in form.fields.iter().enumerate() {
        // The focused field scrolls horizontally so the terminal cursor marks
        // the exact edit position.
        let prefix = format!("{}: ", field.label);
        let prefix_width = prefix.chars().count();
        let available = interior_width.saturating_sub(prefix_width + 1).max(1);
        let value = field.value.chars().collect::<Vec<_>>();
        let visible: String = if index == form.focused && value.len() > available {
            value[value.len() - available..].iter().collect()
        } else if value.len() > available {
            value[..available].iter().collect()
        } else {
            value.iter().collect()
        };
        let pad = available.saturating_sub(visible.chars().count());
        let mut line = vec![Span::raw(prefix)];
        if index == form.focused {
            line.push(Span::styled(visible, field_style()));
            line.push(Span::styled(" ".repeat(pad), field_style()));
        } else {
            line.push(Span::styled(visible, Style::default().fg(Color::DarkGray)));
        }
        lines.push(Line::from(line));
    }
    lines.push(Line::default());
    if let Some(error) = &form.error {
        lines.push(Line::styled(error.clone(), Style::default().fg(Color::Red)));
    }
    lines.push(Line::from(vec![
        badge("Enter", Color::Green),
        Span::raw(" submit  "),
        badge("Tab/Up/Down", Color::Cyan),
        Span::raw(" field  "),
        badge("Esc", Color::DarkGray),
        Span::raw(" cancel"),
    ]));

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", form.title))
                .title_style(title_style()),
        ),
        popup,
    );

    // Place the terminal cursor at the end of the focused field value.
    let focused_prefix_width = form.fields[form.focused].label.chars().count() + 2;
    let focused_value = form.fields[form.focused].value.chars().count();
    let available = interior_width
        .saturating_sub(focused_prefix_width + 1)
        .max(1);
    let visible = focused_value.min(available);
    frame.set_cursor_position(Position {
        x: interior_x + focused_prefix_width as u16 + visible as u16,
        y: interior_y + field_first_row + form.focused as u16,
    });
}

fn draw_deploy_form(frame: &mut ratatui::Frame<'_>, form: &DeployForm) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Length(9),
            Constraint::Percentage(30),
        ])
        .split(frame.area());
    let popup = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(20),
            Constraint::Percentage(60),
            Constraint::Percentage(20),
        ])
        .split(vertical[1])[1];
    frame.render_widget(Clear, popup);
    let (source_label, other_source_label, value_label) = match form.source {
        DeploySource::Branch => ("branch", "image", "Branch or tag"),
        DeploySource::Image => ("image", "branch", "Image reference"),
    };

    // The editable field sits on a fixed row: the value scrolls horizontally so
    // the terminal cursor always marks the exact edit position.
    let prefix = format!("{value_label}: ");
    let interior_x = popup.x + 1;
    let interior_y = popup.y + 1;
    let interior_width = usize::from(popup.width.saturating_sub(2));
    let prefix_width = prefix.chars().count();
    let available = interior_width.saturating_sub(prefix_width + 1).max(1);
    let value = form.value().chars().collect::<Vec<_>>();
    let visible: String = if value.len() > available {
        value[value.len() - available..].iter().collect()
    } else {
        value.iter().collect()
    };
    let field_pad = available.saturating_sub(visible.chars().count());

    let mut lines = vec![
        Line::from(vec![
            Span::raw("Deploy "),
            Span::styled(
                form.application_name.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::default(),
        Line::from(vec![
            Span::raw("Source: "),
            badge(source_label, Color::Cyan),
            Span::raw(format!(" Tab switches to {other_source_label}")),
        ]),
        Line::from(vec![
            label_span(value_label),
            Span::styled(visible.clone(), field_style()),
            Span::styled(" ".repeat(field_pad), field_style()),
        ]),
    ];
    if let Some(error) = &form.error {
        lines.push(Line::styled(error.clone(), Style::default().fg(Color::Red)));
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![
        badge("Enter", Color::Green),
        Span::raw(" deploy  "),
        badge("Tab", Color::Cyan),
        Span::raw(" switch source  "),
        badge("Backspace", Color::Cyan),
        Span::raw(" edit  "),
        badge("Esc", Color::DarkGray),
        Span::raw(" cancel"),
    ]));

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Deploy application ")
                .title_style(title_style()),
        ),
        popup,
    );
    let cursor_column = interior_x + prefix_width as u16 + visible.chars().count() as u16;
    frame.set_cursor_position(Position {
        x: cursor_column,
        y: interior_y + 3,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pneuma::domain::application::{ApplicationName, ApplicationSummary, DesiredRuntimeState};
    use pneuma::domain::identity::{ApplicationId, RuntimeInstanceId, SystemId};
    use pneuma::domain::runtime::ContainerId;

    fn entry(name: &str) -> ApplicationCatalogEntry {
        ApplicationCatalogEntry {
            summary: ApplicationSummary {
                id: ApplicationId::new("11111111111111111111111111111111").unwrap(),
                system_id: SystemId::new("22222222222222222222222222222222").unwrap(),
                name: ApplicationName::new(name).unwrap(),
                repository: format!("https://example.test/{name}.git"),
                default_branch: Some("main".to_owned()),
                desired_runtime_state: DesiredRuntimeState::Running,
                active_deployment_id: None,
            },
            deployed: false,
        }
    }

    fn detail_session() -> Session {
        let mut session = Session::new();
        session.queued.clear();
        session.catalog = QueryState::Ready(vec![entry("atlas")]);
        session.selected_application = Some("atlas".to_owned());
        session.observations_application = Some("atlas".to_owned());
        session.tab = Tab::Applications;
        session.focus = Focus::Details;
        session
    }

    fn runtime_observation() -> RuntimeObservation {
        RuntimeObservation {
            desired_runtime_state: DesiredRuntimeState::Running,
            observed_runtime_state: ObservedRuntimeState::Running,
            runtime_id: RuntimeInstanceId::new("33333333333333333333333333333333").unwrap(),
            container_id: ContainerId::from("abcdef123456"),
            observed_endpoint: Some("127.0.0.1:30000".parse().unwrap()),
        }
    }

    fn rendered_shell(session: &mut Session) -> String {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30))
            .expect("test backend must initialize");
        terminal
            .draw(|frame| draw_shell(frame, session))
            .expect("TUI render must succeed");
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol().to_owned())
            .collect()
    }

    #[test]
    fn catalog_refresh_preserves_selection_or_falls_back_to_first_entry() {
        let entries = vec![entry("atlas"), entry("beacon")];
        let name = application_entry_name;

        assert_eq!(
            preserved_selection(&entries, Some("beacon".to_owned()), name),
            Some("beacon".to_owned())
        );
        assert_eq!(
            preserved_selection(&entries, Some("removed".to_owned()), name),
            Some("atlas".to_owned())
        );
        assert_eq!(
            preserved_selection(&[], Some("atlas".to_owned()), name),
            None
        );
    }

    #[test]
    fn catalog_navigation_stops_at_the_list_boundaries() {
        let entries = vec![entry("atlas"), entry("beacon")];
        let name = application_entry_name;

        assert_eq!(
            next_selection(&entries, Some("atlas"), -1, name),
            Some("atlas".to_owned())
        );
        assert_eq!(
            next_selection(&entries, Some("beacon"), 1, name),
            Some("beacon".to_owned())
        );
        assert_eq!(next_selection(&[], None, 1, name), None);
    }

    #[test]
    fn application_details_use_persisted_and_not_runtime_labels() {
        let rendered = application_details_lines(&entry("atlas"))
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Desired runtime state: Running"));
        assert!(rendered.contains("Has successful deployment: no"));
        assert!(rendered.contains("Active deployment ID: None"));
    }

    #[test]
    fn application_details_render_each_matching_runtime_query_state() {
        let cases: [(QueryState<RuntimeObservation>, &str); 4] = [
            (QueryState::Idle, "The runtime status was not requested"),
            (QueryState::Loading, "Loading runtime status..."),
            (
                QueryState::Failed("External: podman unavailable".to_owned()),
                "External: podman unavailable",
            ),
            (
                QueryState::Ready(runtime_observation()),
                "Observed runtime state: Running",
            ),
        ];

        for (runtime, expected) in cases {
            let mut session = detail_session();
            session.runtime = runtime;

            let rendered = rendered_shell(&mut session);

            assert!(rendered.contains("Runtime status"), "{rendered:?}");
            assert!(rendered.contains(expected), "{rendered:?}");
            session.shutdown().unwrap();
        }
    }

    #[test]
    fn application_details_never_render_another_applications_runtime() {
        let mut session = detail_session();
        session.catalog = QueryState::Ready(vec![entry("atlas"), entry("beacon")]);
        session.selected_application = Some("beacon".to_owned());
        session.observations_application = Some("atlas".to_owned());
        session.runtime = QueryState::Ready(runtime_observation());

        let rendered = rendered_shell(&mut session);

        assert!(rendered.contains("Application: beacon"), "{rendered:?}");
        assert!(
            rendered.contains("The runtime status was not requested"),
            "{rendered:?}"
        );
        assert!(!rendered.contains("Observed runtime state: Running"));
        session.shutdown().unwrap();
    }

    #[test]
    fn action_outcome_is_shown_only_for_its_application_detail() {
        let mut session = detail_session();
        session.catalog = QueryState::Ready(vec![entry("atlas"), entry("beacon")]);
        session.outcome = Some(ActionOutcome {
            scope: "atlas".to_owned(),
            message: "Started atlas".to_owned(),
        });

        assert_eq!(session.outcome_for_detail(), Some("Started atlas"));

        session
            .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        session
            .handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
            .unwrap();
        session
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        assert_eq!(session.outcome_for_detail(), None);
        session.shutdown().unwrap();
    }

    #[test]
    fn confirmation_owns_keys_until_the_operator_cancels_or_confirms() {
        let mut session = detail_session();

        session
            .handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(
            &session.mode,
            Mode::Confirm(PendingAction::Start { application_name }) if application_name == "atlas"
        ));

        session
            .handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(&session.mode, Mode::Confirm(_)));
        assert!(!session.quit_after_completion);

        session
            .handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.mode, Mode::Normal));
        assert!(session.queued.is_empty());
        session.shutdown().unwrap();
    }

    #[test]
    fn confirmation_enqueues_the_exact_existing_control_command() {
        let mut session = detail_session();

        session
            .handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE))
            .unwrap();
        session
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        assert!(matches!(session.mode, Mode::Normal));
        assert_eq!(
            session.queued.front().map(Request::command),
            Some(Command::VisibilitySet {
                application_name: "atlas".to_owned(),
                visibility: Visibility::Public,
            })
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn lifecycle_and_reconcile_actions_map_to_the_existing_commands() {
        let application_name = "atlas".to_owned();

        assert_eq!(
            PendingAction::Start {
                application_name: application_name.clone(),
            }
            .command(),
            Command::ApplicationStart {
                application_name: application_name.clone(),
            }
        );
        assert_eq!(
            PendingAction::Stop {
                application_name: application_name.clone(),
            }
            .command(),
            Command::ApplicationStop {
                application_name: application_name.clone(),
            }
        );
        assert_eq!(
            PendingAction::Reconcile {
                application_name: application_name.clone(),
            }
            .command(),
            Command::Reconcile {
                application_name: application_name.clone(),
            }
        );
        assert_eq!(
            PendingAction::SetVisibility {
                application_name: application_name.clone(),
                visibility: Visibility::Internal,
            }
            .command(),
            Command::VisibilitySet {
                application_name,
                visibility: Visibility::Internal,
            }
        );
    }

    #[test]
    fn confirmed_action_replaces_pending_refreshes_without_leaving_the_detail_view() {
        let mut session = detail_session();
        session.active = Some((
            41,
            Request::Status {
                application_name: "atlas".to_owned(),
            },
        ));
        session.queued.push_back(Request::Catalog);
        session.queued.push_back(Request::Deployments {
            application_name: "atlas".to_owned(),
        });

        session
            .handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
            .unwrap();
        session
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        assert!(matches!(session.focus, Focus::Details));
        assert_eq!(
            session
                .queued
                .iter()
                .map(Request::command)
                .collect::<Vec<_>>(),
            vec![Command::ApplicationStart {
                application_name: "atlas".to_owned(),
            }]
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn confirmed_successor_action_suppresses_the_previous_action_refresh() {
        let mut session = detail_session();
        session.active = Some((
            41,
            Request::Action(PendingAction::Stop {
                application_name: "atlas".to_owned(),
            }),
        ));

        session
            .handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE))
            .unwrap();
        session
            .handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
            .unwrap();
        session.refresh_after_action(&PendingAction::Stop {
            application_name: "atlas".to_owned(),
        });

        assert_eq!(
            session
                .queued
                .iter()
                .map(Request::command)
                .collect::<Vec<_>>(),
            vec![Command::ApplicationStart {
                application_name: "atlas".to_owned(),
            }]
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn action_failures_keep_their_class_and_schedule_a_refresh() {
        let mut session = detail_session();
        let action = PendingAction::Stop {
            application_name: "atlas".to_owned(),
        };

        session.apply_error(
            Request::Action(action),
            WorkerError {
                class: CliErrorClass::Conflict,
                message: "application is busy".to_owned(),
            }
            .display(),
        );

        assert_eq!(
            session.error.as_deref(),
            Some("Conflict: application is busy")
        );
        assert!(matches!(session.catalog, QueryState::Loading));
        assert!(matches!(session.runtime, QueryState::Loading));
        assert_eq!(
            session
                .queued
                .iter()
                .map(Request::command)
                .collect::<Vec<_>>(),
            vec![
                Command::ListApplications,
                Command::ApplicationStatus {
                    application_name: "atlas".to_owned(),
                },
            ]
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn deploy_form_edits_text_and_submits_the_exact_branch_command() {
        let mut session = detail_session();

        session
            .handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.mode, Mode::Deploy(_)));

        for character in "main".chars() {
            session
                .handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .unwrap();
        }
        // The deploy form owns printable text: `q` edits instead of quitting.
        session
            .handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.mode, Mode::Deploy(_)));
        assert!(!session.quit_after_completion);
        session
            .handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
            .unwrap();

        session
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.mode, Mode::Normal));
        assert_eq!(
            session.queued.front().map(Request::command),
            Some(Command::DeployBranch {
                application_name: "atlas".to_owned(),
                branch: "main".to_owned(),
            })
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn deploy_form_switches_to_the_image_source_and_submits_the_exact_image_command() {
        let mut session = detail_session();

        session
            .handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE))
            .unwrap();
        session
            .handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE))
            .unwrap();
        session
            .handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();
        session
            .handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
            .unwrap();
        let reference = "registry.example/team/service@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        for character in reference.chars() {
            session
                .handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .unwrap();
        }
        session
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        assert!(matches!(session.mode, Mode::Normal));
        assert_eq!(
            session.queued.front().map(Request::command),
            Some(Command::DeployImage {
                application_name: "atlas".to_owned(),
                image_reference: reference.to_owned(),
            })
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn deploy_form_rejects_an_empty_source_without_dispatching() {
        let mut session = detail_session();

        session
            .handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE))
            .unwrap();
        session
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        assert!(matches!(session.mode, Mode::Deploy(_)));
        assert!(session.queued.is_empty());
        let Mode::Deploy(form) = &session.mode else {
            panic!("deploy form must stay open");
        };
        assert_eq!(
            form.error.as_deref(),
            Some("Enter a branch or tag to deploy.")
        );

        session
            .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.mode, Mode::Normal));
        assert!(session.queued.is_empty());
        session.shutdown().unwrap();
    }

    #[test]
    fn rollback_requires_confirmation_and_maps_to_the_existing_command() {
        let mut session = detail_session();

        session
            .handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(
            &session.mode,
            Mode::Confirm(PendingAction::Rollback { application_name }) if application_name == "atlas"
        ));
        session
            .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.mode, Mode::Normal));
        assert!(session.queued.is_empty());

        session
            .handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE))
            .unwrap();
        session
            .handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.mode, Mode::Normal));
        assert_eq!(
            session.queued.front().map(Request::command),
            Some(Command::Rollback {
                application_name: "atlas".to_owned(),
            })
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn deployment_events_render_with_the_shared_progress_vocabulary() {
        assert_eq!(
            deployment_event_line(&DeploymentEvent::DeploymentRequested {
                application_name: ApplicationName::new("atlas").unwrap(),
            }),
            "Deploying atlas..."
        );
        assert_eq!(
            deployment_event_line(&DeploymentEvent::StepStarted {
                step: pneuma::use_cases::deployment::DeploymentStep::PullImage,
            }),
            "pull image: started"
        );
    }

    #[test]
    fn deployment_log_is_scoped_to_its_application_detail() {
        let mut session = detail_session();
        session.catalog = QueryState::Ready(vec![entry("atlas"), entry("beacon")]);
        session.active = Some((
            41,
            Request::Action(PendingAction::DeployBranch {
                application_name: "atlas".to_owned(),
                branch: "main".to_owned(),
            }),
        ));
        session.deployment_log = Some(deployment_log(vec![
            "Deploying atlas...".to_owned(),
            "pull image: started".to_owned(),
        ]));

        assert_eq!(
            session
                .deployment_log_for_detail()
                .map(|log| log.lines.len()),
            Some(2)
        );

        // Selecting another application scopes the log panel away, but the
        // log itself is retained for its own application.
        session.selected_application = Some("beacon".to_owned());
        assert!(session.deployment_log_for_detail().is_none());
        assert_eq!(
            session.deployment_log.as_ref().map(|log| log.lines.len()),
            Some(2)
        );
        session.shutdown().unwrap();
    }

    fn deployment_log(lines: Vec<String>) -> DeploymentLog {
        let mut log = DeploymentLog::new("atlas".to_owned());
        log.lines = lines;
        log
    }

    #[test]
    fn deployment_log_survives_a_successful_finish_and_refreshes() {
        let mut session = detail_session();
        session.deployment_log = Some(deployment_log(vec![
            "Deploying atlas...".to_owned(),
            "state changed to Succeeded".to_owned(),
        ]));

        session.finish_deployment_log(
            &Request::Action(PendingAction::DeployBranch {
                application_name: "atlas".to_owned(),
                branch: "main".to_owned(),
            }),
            &Ok(CommandResult::ApplicationDeployed {
                application_name: ApplicationName::new("atlas").unwrap(),
                deployment: deployment_result(),
            }),
        );
        session.refresh_details();

        let log = session
            .deployment_log
            .as_ref()
            .expect("a finished deployment log must be retained");
        assert!(matches!(log.state, DeploymentLogState::Completed));
        assert_eq!(log.lines.len(), 2);
        assert!(log.tail_follow);
        session.shutdown().unwrap();
    }

    #[test]
    fn deployment_failure_before_any_event_is_recorded_in_the_log() {
        let mut session = detail_session();
        session.deployment_log = Some(DeploymentLog::new("atlas".to_owned()));

        session.finish_deployment_log(
            &Request::Action(PendingAction::Rollback {
                application_name: "atlas".to_owned(),
            }),
            &Err(WorkerError {
                class: CliErrorClass::Conflict,
                message: "already has an operation in progress".to_owned(),
            }),
        );

        let log = session
            .deployment_log
            .as_ref()
            .expect("a failed deployment log must be retained");
        assert!(matches!(
            &log.state,
            DeploymentLogState::Failed(error)
                if error == "Conflict: already has an operation in progress"
        ));
        assert!(log.lines.is_empty());
        session.shutdown().unwrap();
    }

    #[test]
    fn non_deployment_actions_and_new_dispatches_treat_the_log_correctly() {
        let mut session = detail_session();
        session.deployment_log = Some(deployment_log(vec!["Deploying atlas...".to_owned()]));

        // Confirming a deployment only queues it: the previous log stays.
        session.execute_action(PendingAction::DeployBranch {
            application_name: "atlas".to_owned(),
            branch: "feature".to_owned(),
        });
        assert!(session.deployment_log.as_ref().is_some_and(|log| {
            log.lines
                .first()
                .is_some_and(|line| line == "Deploying atlas...")
        }));

        // The queued deployment replaces the log only when it dispatches.
        session.active = Some((
            42,
            Request::Action(PendingAction::DeployBranch {
                application_name: "atlas".to_owned(),
                branch: "feature".to_owned(),
            }),
        ));
        session.dispatch_deployment_log();
        let log = session
            .deployment_log
            .as_ref()
            .expect("a dispatched deployment must open a new log");
        assert_eq!(log.application_name, "atlas");
        assert!(matches!(log.state, DeploymentLogState::Running));
        assert!(log.lines.is_empty());

        // A completed non-deployment action keeps the log untouched.
        session.apply_success(
            Request::Action(PendingAction::Start {
                application_name: "atlas".to_owned(),
            }),
            CommandResult::ApplicationStarted {
                application_name: ApplicationName::new("atlas").unwrap(),
                observation: runtime_observation(),
            },
        );
        let log = session
            .deployment_log
            .as_ref()
            .expect("a non-deployment action must not clear the log");
        assert!(matches!(log.state, DeploymentLogState::Running));
        session.shutdown().unwrap();
    }

    #[test]
    fn the_retained_log_renders_with_its_state_and_keeps_the_last_action() {
        let mut session = detail_session();
        session.deployment_log = Some(deployment_log(vec![
            "Deploying atlas...".to_owned(),
            "pull image: started".to_owned(),
        ]));
        session.outcome = Some(ActionOutcome {
            scope: "atlas".to_owned(),
            message: "Deployed atlas: deployment 1 promoted".to_owned(),
        });

        let rendered = rendered_shell(&mut session);
        assert!(rendered.contains("Deployment progress"), "{rendered:?}");
        assert!(rendered.contains("Deploying atlas..."), "{rendered:?}");
        assert!(rendered.contains("pull image: started"), "{rendered:?}");
        assert!(
            rendered.contains("Deployed atlas: deployment 1 promoted"),
            "{rendered:?}"
        );

        if let Some(log) = session.deployment_log.as_mut() {
            log.finish(&Ok(CommandResult::ApplicationDeployed {
                application_name: ApplicationName::new("atlas").unwrap(),
                deployment: deployment_result(),
            }));
        }
        let rendered = rendered_shell(&mut session);
        assert!(
            rendered.contains("Deployment log (completed)"),
            "{rendered:?}"
        );
        assert!(rendered.contains("pull image: started"), "{rendered:?}");
        session.shutdown().unwrap();
    }

    #[test]
    fn focus_descends_to_the_log_only_when_a_log_exists() {
        let mut session = detail_session();

        // Without a log, Enter keeps the focus in the details column.
        session
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.focus, Focus::Details));

        session.deployment_log = Some(DeploymentLog::new("atlas".to_owned()));
        session
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.focus, Focus::Log));

        session
            .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.focus, Focus::Details));
        session
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.focus, Focus::Log));
        session
            .handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.focus, Focus::Details));
        session
            .handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.focus, Focus::Listing));
        session.shutdown().unwrap();
    }

    fn scrollable_log() -> DeploymentLog {
        let mut log = deployment_log((0..10).map(|row| format!("event row {row}")).collect());
        log.total_rows = 10;
        log.viewport_rows = 4;
        log
    }

    fn log_focus_session() -> Session {
        let mut session = detail_session();
        session.deployment_log = Some(scrollable_log());
        session
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        session
    }

    #[test]
    fn log_scrolling_clamps_at_both_bounds_and_resumes_tail_following() {
        let mut session = log_focus_session();

        // Scrolling up from the tail anchors one row above the bottom and
        // never goes past the oldest row.
        session
            .handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
            .unwrap();
        let log = session.deployment_log_for_detail().expect("log is present");
        assert!(!log.tail_follow);
        assert_eq!(log.scroll, 5);
        for _ in 0..10 {
            session
                .handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
                .unwrap();
        }
        let log = session.deployment_log_for_detail().expect("log is present");
        assert_eq!(log.scroll, 0);

        // Scrolling back down clamps at the bottom and resumes tail-following.
        for _ in 0..10 {
            session
                .handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
                .unwrap();
        }
        let log = session.deployment_log_for_detail().expect("log is present");
        assert!(log.tail_follow);

        // Selection never changes while scrolling, and scrolling works while
        // a command is active because it never issues one.
        assert_eq!(session.selected_application.as_deref(), Some("atlas"));
        session.active = Some((
            7,
            Request::Action(PendingAction::DeployBranch {
                application_name: "atlas".to_owned(),
                branch: "main".to_owned(),
            }),
        ));
        session
            .handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE))
            .unwrap();
        let log = session.deployment_log_for_detail().expect("log is present");
        assert_eq!(log.scroll, 0);
        assert!(!log.tail_follow);
        session
            .handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE))
            .unwrap();
        let log = session.deployment_log_for_detail().expect("log is present");
        assert!(log.tail_follow);
        assert_eq!(log.scroll, log.max_scroll());
        session.shutdown().unwrap();
    }

    #[test]
    fn log_paging_uses_the_recorded_viewport() {
        let mut session = log_focus_session();

        session
            .handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE))
            .unwrap();
        let log = session.deployment_log_for_detail().expect("log is present");
        assert!(!log.tail_follow);
        assert_eq!(log.scroll, 2);

        session
            .handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE))
            .unwrap();
        let log = session.deployment_log_for_detail().expect("log is present");
        assert!(log.tail_follow);
        session.shutdown().unwrap();
    }

    #[test]
    fn incoming_events_never_move_a_detached_log_view() {
        let mut log = scrollable_log();
        log.tail_follow = false;
        log.scroll = 2;

        log.record_event(&DeploymentEvent::StepStarted {
            step: pneuma::use_cases::deployment::DeploymentStep::PullImage,
        });
        log.total_rows += 1;

        assert_eq!(log.scroll, 2);
        assert!(!log.tail_follow);
        assert_eq!(log.render_offset(), 2);
    }

    #[test]
    fn the_focused_log_takes_the_details_column_with_state_and_scroll_title() {
        let mut session = detail_session();
        session.deployment_log = Some(deployment_log(vec![
            "Deploying atlas...".to_owned(),
            "pull image: started".to_owned(),
        ]));
        session
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        let rendered = rendered_shell(&mut session);
        assert!(
            rendered.contains("Deployment log · atlas · running · rows 1–2 of 2"),
            "{rendered:?}"
        );
        // The focused log replaces the summary panels of the details column.
        assert!(!rendered.contains("Application details"), "{rendered:?}");
        assert!(!rendered.contains("Last action"), "{rendered:?}");
        // Both log rows render inside the focused pane.
        assert!(rendered.contains("Deploying atlas..."), "{rendered:?}");
        assert!(rendered.contains("pull image: started"), "{rendered:?}");
        session.shutdown().unwrap();
    }

    #[test]
    fn the_footer_advertises_log_scrolling_when_the_log_is_focused() {
        let mut session = detail_session();
        session.deployment_log = Some(DeploymentLog::new("atlas".to_owned()));

        session
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        let rendered = rendered_shell(&mut session);
        assert!(rendered.contains("oldest/newest"), "{rendered:?}");
        assert!(
            rendered.contains("oldest") || rendered.contains("PgUp"),
            "{rendered:?}"
        );

        session
            .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();
        let rendered = rendered_shell(&mut session);
        assert!(rendered.contains("Enter"), "{rendered:?}");
        assert!(rendered.contains(" log"), "{rendered:?}");
        session.shutdown().unwrap();
    }

    #[test]
    fn deploy_success_renders_the_typed_result_and_refreshes_history() {
        let mut session = detail_session();
        let action = PendingAction::DeployBranch {
            application_name: "atlas".to_owned(),
            branch: "main".to_owned(),
        };

        session.apply_success(
            Request::Action(action.clone()),
            CommandResult::ApplicationDeployed {
                application_name: ApplicationName::new("atlas").unwrap(),
                deployment: deployment_result(),
            },
        );

        let outcome = session
            .outcome
            .as_ref()
            .expect("deploy success must set an outcome");
        assert_eq!(outcome.scope, "atlas");
        assert_eq!(
            outcome.message,
            "Deployed atlas: deployment 0f8d3a2c41b64d7e9a0c5b6e1f2d3a4b promoted (registry.example/team/service@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa)"
        );
        assert_eq!(
            session
                .queued
                .iter()
                .map(Request::command)
                .collect::<Vec<_>>(),
            vec![
                Command::ListApplications,
                Command::ListDeployments {
                    application_name: "atlas".to_owned(),
                },
                Command::ApplicationStatus {
                    application_name: "atlas".to_owned(),
                },
            ]
        );
        assert_eq!(action.application_name(), Some("atlas"));
        session.shutdown().unwrap();
    }

    #[test]
    fn rollback_success_renders_the_typed_result_and_refreshes_history() {
        let mut session = detail_session();

        session.apply_success(
            Request::Action(PendingAction::Rollback {
                application_name: "atlas".to_owned(),
            }),
            CommandResult::ApplicationRolledBack {
                application_name: ApplicationName::new("atlas").unwrap(),
                deployment: deployment_result(),
            },
        );

        let outcome = session
            .outcome
            .as_ref()
            .expect("rollback success must set an outcome");
        assert_eq!(
            outcome.message,
            "Rolled back atlas: deployment 0f8d3a2c41b64d7e9a0c5b6e1f2d3a4b promoted (registry.example/team/service@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa)"
        );
        assert_eq!(
            session
                .queued
                .iter()
                .map(Request::command)
                .collect::<Vec<_>>(),
            vec![
                Command::ListApplications,
                Command::ListDeployments {
                    application_name: "atlas".to_owned(),
                },
                Command::ApplicationStatus {
                    application_name: "atlas".to_owned(),
                },
            ]
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn deploy_failure_keeps_the_class_and_refreshes_history_after_persisted_evidence() {
        let mut session = detail_session();

        session.apply_error(
            Request::Action(PendingAction::DeployBranch {
                application_name: "atlas".to_owned(),
                branch: "main".to_owned(),
            }),
            WorkerError {
                class: CliErrorClass::External,
                message: "deployment failed".to_owned(),
            }
            .display(),
        );

        assert_eq!(
            session.error.as_deref(),
            Some("External: deployment failed")
        );
        assert!(matches!(session.deployments, QueryState::Loading));
        assert_eq!(
            session
                .queued
                .iter()
                .map(Request::command)
                .collect::<Vec<_>>(),
            vec![
                Command::ListApplications,
                Command::ListDeployments {
                    application_name: "atlas".to_owned(),
                },
                Command::ApplicationStatus {
                    application_name: "atlas".to_owned(),
                },
            ]
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn the_deploy_form_cursor_tracks_the_edited_field() {
        use ratatui::backend::Backend as _;

        let mut session = detail_session();
        session
            .handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE))
            .unwrap();
        for character in "main".chars() {
            session
                .handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .unwrap();
        }

        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 24))
            .expect("test backend must initialize");
        terminal
            .draw(|frame| draw_shell(frame, &mut session))
            .expect("form render must succeed");
        let before = terminal
            .backend_mut()
            .get_cursor_position()
            .expect("form must position the cursor");

        session
            .handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE))
            .unwrap();
        terminal
            .draw(|frame| draw_shell(frame, &mut session))
            .expect("form render must succeed");
        let after_push = terminal
            .backend_mut()
            .get_cursor_position()
            .expect("form must position the cursor");
        assert_eq!(after_push.y, before.y);
        assert_eq!(after_push.x, before.x + 1);

        session
            .handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
            .unwrap();
        terminal
            .draw(|frame| draw_shell(frame, &mut session))
            .expect("form render must succeed");
        let after_pop = terminal
            .backend_mut()
            .get_cursor_position()
            .expect("form must position the cursor");
        assert_eq!(after_pop, before);
        session.shutdown().unwrap();
    }

    #[test]
    fn footer_key_hints_render_with_background_badges() {
        let mut session = detail_session();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30))
            .expect("test backend must initialize");
        terminal
            .draw(|frame| draw_shell(frame, &mut session))
            .expect("footer render must succeed");

        let footer_row = 29;
        let footer = (0..120u16)
            .map(|column| {
                terminal.backend().buffer()[(column, footer_row)]
                    .symbol()
                    .to_owned()
            })
            .collect::<String>();
        assert!(footer.contains(" start "), "{footer:?}");
        assert!(footer.contains(" rollback "), "{footer:?}");
        let badge_column = footer
            .find(" s ")
            .map(|index| index as u16)
            .expect("start badge must render");
        assert_eq!(
            terminal.backend().buffer()[(badge_column, footer_row)]
                .style()
                .bg,
            Some(Color::Cyan),
            "key badges must carry a background color"
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn tab_keys_switch_groups_and_reset_the_focus() {
        let mut session = detail_session();

        session
            .handle_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.tab, Tab::Systems));
        assert!(matches!(session.focus, Focus::Listing));

        session
            .handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.tab, Tab::Deployments));

        session
            .handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.tab, Tab::Systems));

        session
            .handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE))
            .unwrap();
        session
            .handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.tab, Tab::Applications));

        // Tab switching is navigation: it stays disabled while busy.
        session.active = Some((41, Request::Systems));
        session
            .handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.tab, Tab::Applications));
        session.active = None;
        session.shutdown().unwrap();
    }

    #[test]
    fn left_arrow_returns_from_details_to_the_listing_and_then_switches_tabs() {
        let mut session = detail_session();

        session
            .handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.focus, Focus::Listing));
        assert!(matches!(session.tab, Tab::Applications));
        // The detail target stays selected so returning is cheap.
        assert_eq!(session.observations_application.as_deref(), Some("atlas"));

        session
            .handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.tab, Tab::Systems));

        session
            .handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.tab, Tab::Applications));
        session.shutdown().unwrap();
    }

    #[test]
    fn system_create_form_submits_the_exact_existing_command() {
        let mut session = Session::new();
        session.queued.clear();
        session
            .handle_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE))
            .unwrap();
        session
            .handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
            .unwrap();
        assert!(matches!(session.mode, Mode::Form(_)));

        for character in "forge".chars() {
            session
                .handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .unwrap();
        }
        session
            .handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();
        for character in "Team system".chars() {
            session
                .handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .unwrap();
        }
        session
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        assert!(matches!(session.mode, Mode::Normal));
        assert_eq!(
            session.queued.front().map(Request::command),
            Some(Command::SystemCreate {
                name: "forge".to_owned(),
                description: Some("Team system".to_owned()),
            })
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn system_create_form_rejects_an_invalid_name_without_dispatching() {
        let mut session = Session::new();
        session.queued.clear();
        session
            .handle_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE))
            .unwrap();
        session
            .handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
            .unwrap();
        for character in "Invalid Name!".chars() {
            session
                .handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .unwrap();
        }
        session
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        assert!(matches!(session.mode, Mode::Form(_)));
        assert!(session.queued.is_empty());
        let Mode::Form(form) = &session.mode else {
            panic!("system form must stay open");
        };
        assert_eq!(
            form.error.as_deref(),
            Some("invalid system name `Invalid Name!`")
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn import_into_system_form_binds_the_selected_system() {
        let mut session = Session::new();
        session.queued.clear();
        session.tab = Tab::Systems;
        session.systems = QueryState::Ready(vec![system_fixture("forge")]);
        session.selected_system = Some("forge".to_owned());

        session
            .handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
            .unwrap();
        for character in "https://example.test/app.git".chars() {
            session
                .handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .unwrap();
        }
        session
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        assert!(matches!(session.mode, Mode::Normal));
        assert_eq!(
            session.queued.front().map(Request::command),
            Some(Command::ImportApplication {
                repository: "https://example.test/app.git".to_owned(),
                system_name: Some("forge".to_owned()),
                manifest_path: None,
            })
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn import_into_system_form_rejects_a_local_repository() {
        let mut session = Session::new();
        session.queued.clear();
        session.tab = Tab::Systems;
        session.systems = QueryState::Ready(vec![system_fixture("forge")]);
        session.selected_system = Some("forge".to_owned());

        session
            .handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE))
            .unwrap();
        for character in "/srv/checkouts/app".chars() {
            session
                .handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .unwrap();
        }
        session
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        assert!(matches!(session.mode, Mode::Form(_)));
        assert!(session.queued.is_empty());
        session.shutdown().unwrap();
    }

    #[test]
    fn application_import_form_submits_the_exact_existing_command() {
        let mut session = Session::new();
        session.queued.clear();
        session
            .handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
            .unwrap();

        for character in "https://example.test/app.git".chars() {
            session
                .handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .unwrap();
        }
        session
            .handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();
        for character in "forge".chars() {
            session
                .handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .unwrap();
        }
        session
            .handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();
        for character in "deploy/pneuma.toml".chars() {
            session
                .handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .unwrap();
        }
        session
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        assert!(matches!(session.mode, Mode::Normal));
        assert_eq!(
            session.queued.front().map(Request::command),
            Some(Command::ImportApplication {
                repository: "https://example.test/app.git".to_owned(),
                system_name: Some("forge".to_owned()),
                manifest_path: Some("deploy/pneuma.toml".to_owned()),
            })
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn system_create_success_selects_the_system_and_refreshes_the_systems() {
        let mut session = Session::new();
        session.queued.clear();

        session.apply_success(
            Request::Action(PendingAction::SystemCreate {
                name: "forge".to_owned(),
                description: Some("Team system".to_owned()),
            }),
            CommandResult::SystemCreated(system_fixture("forge")),
        );

        let outcome = session
            .outcome
            .as_ref()
            .expect("system create must set an outcome");
        assert_eq!(outcome.scope, "forge");
        assert_eq!(outcome.message, "Created system forge");
        assert_eq!(session.selected_system.as_deref(), Some("forge"));
        assert_eq!(
            session
                .queued
                .iter()
                .map(Request::command)
                .collect::<Vec<_>>(),
            vec![Command::SystemList]
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn import_success_selects_the_application_and_refreshes_the_affected_catalogs() {
        let mut session = Session::new();
        session.queued.clear();

        session.apply_success(
            Request::Action(PendingAction::ImportApplication {
                repository: "https://example.test/app.git".to_owned(),
                system_name: Some("forge".to_owned()),
                manifest_path: None,
            }),
            CommandResult::ApplicationImported(entry("atlas").summary),
        );

        assert_eq!(session.selected_application.as_deref(), Some("atlas"));
        assert_eq!(
            session
                .queued
                .iter()
                .map(Request::command)
                .collect::<Vec<_>>(),
            vec![Command::ListApplications, Command::SystemList,]
        );

        // With the system details open, the member list must be reloaded too.
        session.queued.clear();
        session.system_details = QueryState::Ready(pneuma::use_cases::system::SystemDetails {
            system: system_fixture("forge"),
            applications: Vec::new(),
        });
        session.apply_success(
            Request::Action(PendingAction::ImportApplication {
                repository: "https://example.test/other.git".to_owned(),
                system_name: Some("forge".to_owned()),
                manifest_path: None,
            }),
            CommandResult::ApplicationImported(entry("beacon").summary),
        );
        assert_eq!(
            session
                .queued
                .iter()
                .map(Request::command)
                .collect::<Vec<_>>(),
            vec![
                Command::ListApplications,
                Command::SystemList,
                Command::SystemShow {
                    name: "forge".to_owned(),
                },
            ]
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn deployments_tab_enter_loads_history_and_status_for_the_selection() {
        let mut session = Session::new();
        session.queued.clear();
        session.catalog = QueryState::Ready(vec![entry("atlas")]);
        session.selected_application = Some("atlas".to_owned());

        session
            .handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE))
            .unwrap();
        session
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();

        assert!(matches!(session.focus, Focus::Details));
        assert_eq!(
            session
                .queued
                .iter()
                .map(Request::command)
                .collect::<Vec<_>>(),
            vec![
                Command::ListDeployments {
                    application_name: "atlas".to_owned(),
                },
                Command::ApplicationStatus {
                    application_name: "atlas".to_owned(),
                },
            ]
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn system_details_render_the_persisted_grouping_and_members() {
        let details = pneuma::use_cases::system::SystemDetails {
            system: System {
                id: SystemId::new("33333333333333333333333333333333").unwrap(),
                name: SystemName::new("forge").unwrap(),
                description: Some("Team system".to_owned()),
            },
            applications: vec![entry("atlas").summary],
        };

        let rendered = system_details_lines(&details)
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("System: forge"));
        assert!(rendered.contains("Description: Team system"));
        assert!(rendered.contains("Applications: 1"));
        assert!(rendered.contains("atlas"));
    }

    #[test]
    fn the_multi_field_form_cursor_tracks_the_focused_field() {
        use ratatui::backend::Backend as _;

        let mut session = Session::new();
        session.queued.clear();
        session
            .handle_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE))
            .unwrap();
        session
            .handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE))
            .unwrap();
        for character in "forge".chars() {
            session
                .handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE))
                .unwrap();
        }

        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 24))
            .expect("test backend must initialize");
        terminal
            .draw(|frame| draw_shell(frame, &mut session))
            .expect("form render must succeed");
        let before = terminal
            .backend_mut()
            .get_cursor_position()
            .expect("form must position the cursor");

        session
            .handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
            .unwrap();
        terminal
            .draw(|frame| draw_shell(frame, &mut session))
            .expect("form render must succeed");
        let after = terminal
            .backend_mut()
            .get_cursor_position()
            .expect("form must position the cursor");
        assert_eq!(after.y, before.y + 1);

        session
            .handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
            .unwrap();
        terminal
            .draw(|frame| draw_shell(frame, &mut session))
            .expect("form render must succeed");
        let after_push = terminal
            .backend_mut()
            .get_cursor_position()
            .expect("form must position the cursor");
        assert_eq!(after_push.y, after.y);
        assert_eq!(after_push.x, after.x + 1);
        session.shutdown().unwrap();
    }

    #[test]
    fn first_system_details_load_without_an_explicit_request() {
        let mut session = Session::new();
        session.queued.clear();

        session.apply_systems(vec![system_fixture("forge"), system_fixture("atelier")]);

        assert_eq!(session.selected_system.as_deref(), Some("forge"));
        assert!(matches!(session.system_details, QueryState::Loading));
        assert_eq!(
            session
                .queued
                .iter()
                .map(Request::command)
                .collect::<Vec<_>>(),
            vec![Command::SystemShow {
                name: "forge".to_owned(),
            }]
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn system_selection_follows_the_arrows_and_reloads_the_details() {
        let mut session = Session::new();
        session.queued.clear();
        session.tab = Tab::Systems;
        session.apply_systems(vec![system_fixture("forge"), system_fixture("atelier")]);
        session.queued.clear();
        session.system_details = QueryState::Ready(pneuma::use_cases::system::SystemDetails {
            system: system_fixture("forge"),
            applications: Vec::new(),
        });

        session
            .handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
            .unwrap();

        assert_eq!(session.selected_system.as_deref(), Some("atelier"));
        assert!(matches!(session.system_details, QueryState::Loading));
        assert_eq!(
            session
                .queued
                .iter()
                .map(Request::command)
                .collect::<Vec<_>>(),
            vec![Command::SystemShow {
                name: "atelier".to_owned(),
            }]
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn entering_the_deployments_tab_loads_the_selection_observations() {
        let mut session = Session::new();
        session.queued.clear();
        session.catalog = QueryState::Ready(vec![entry("atlas")]);
        session.selected_application = Some("atlas".to_owned());
        session.observations_application = None;

        session
            .handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE))
            .unwrap();

        assert!(matches!(session.tab, Tab::Deployments));
        assert_eq!(
            session
                .queued
                .iter()
                .map(Request::command)
                .collect::<Vec<_>>(),
            vec![
                Command::ListDeployments {
                    application_name: "atlas".to_owned(),
                },
                Command::ApplicationStatus {
                    application_name: "atlas".to_owned(),
                },
            ]
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn observation_requests_follow_each_selection_change() {
        let mut session = Session::new();
        session.queued.clear();
        session.tab = Tab::Deployments;
        session.catalog = QueryState::Ready(vec![entry("atlas"), entry("beacon")]);
        session.selected_application = Some("atlas".to_owned());
        session.observations_application = Some("atlas".to_owned());

        session
            .handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
            .unwrap();

        assert_eq!(session.selected_application.as_deref(), Some("beacon"));
        assert_eq!(
            session
                .queued
                .iter()
                .map(Request::command)
                .collect::<Vec<_>>(),
            vec![
                Command::ListDeployments {
                    application_name: "beacon".to_owned(),
                },
                Command::ApplicationStatus {
                    application_name: "beacon".to_owned(),
                },
            ]
        );
        session.shutdown().unwrap();
    }

    #[test]
    fn details_render_for_the_first_selection_without_pressing_enter() {
        let mut session = Session::new();
        session.queued.clear();
        session.catalog = QueryState::Ready(vec![entry("atlas")]);
        session.selected_application = Some("atlas".to_owned());

        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 24))
            .expect("test backend must initialize");
        terminal
            .draw(|frame| draw_shell(frame, &mut session))
            .expect("application tab render must succeed");
        let screen = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol().to_owned())
            .collect::<String>();
        assert!(
            screen.contains("Application: atlas")
                && screen.contains("https://example.test/atlas.git"),
            "the selected application details must render without Enter: {screen:?}"
        );
        session.shutdown().unwrap();
    }

    fn system_fixture(name: &str) -> System {
        System {
            id: SystemId::new("33333333333333333333333333333333").unwrap(),
            name: SystemName::new(name).unwrap(),
            description: None,
        }
    }

    fn deployment_result() -> pneuma::use_cases::deployment::DeploymentResult {
        use pneuma::domain::identity::{DeploymentId, RuntimeInstanceId};
        use pneuma::domain::release::OciArtifact;

        pneuma::use_cases::deployment::DeploymentResult {
            deployment_id: DeploymentId::new("0f8d3a2c41b64d7e9a0c5b6e1f2d3a4b").unwrap(),
            runtime_id: RuntimeInstanceId::new("11111111111111111111111111111111").unwrap(),
            container_name: "system-atlas".to_owned(),
            artifact: OciArtifact::parse(
                "registry.example/team/service@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
            source_revision: None,
            finished_at: "2026-09-04 12:00:00".to_owned(),
        }
    }
}
