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
use pneuma::domain::deployment::DeploymentHistory;
use pneuma::domain::exposure::Visibility;
use pneuma::use_cases::application::{ApplicationCatalogEntry, RuntimeObservation};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Modifier, Style},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use super::{
    error::{CliError, CliErrorClass},
    output,
};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(50);

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Route {
    Catalog,
    Details,
}

enum QueryState<T> {
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

#[derive(Clone, PartialEq, Eq)]
enum PendingAction {
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
}

impl PendingAction {
    fn application_name(&self) -> &str {
        match self {
            Self::Start { application_name }
            | Self::Stop { application_name }
            | Self::Reconcile { application_name }
            | Self::SetVisibility {
                application_name, ..
            } => application_name,
        }
    }

    fn command(&self) -> Command {
        match self {
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
        }
    }

    fn targets_visibility(&self, visibility: Visibility) -> bool {
        matches!(self, Self::SetVisibility { visibility: target, .. } if *target == visibility)
    }

    fn confirmation_text(&self) -> String {
        match self {
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
        }
    }
}

enum Mode {
    Normal,
    Confirm(PendingAction),
}

enum Request {
    Catalog,
    Deployments { application_name: String },
    Status { application_name: String },
    Action(PendingAction),
}

impl Request {
    fn command(&self) -> Command {
        match self {
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
                let result = executor.execute(command).map_err(WorkerError::from_control);
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
    catalog: QueryState<Vec<ApplicationCatalogEntry>>,
    selected_application: Option<String>,
    detail_application: Option<String>,
    deployments: QueryState<Vec<DeploymentHistory>>,
    runtime: QueryState<RuntimeObservation>,
    route: Route,
    mode: Mode,
    error: Option<String>,
    outcome: Option<String>,
    quit_after_completion: bool,
}

impl Session {
    fn new() -> Self {
        let mut session = Self {
            worker: Worker::new(),
            next_request_id: 1,
            active: None,
            queued: VecDeque::new(),
            catalog: QueryState::Loading,
            selected_application: None,
            detail_application: None,
            deployments: QueryState::Idle,
            runtime: QueryState::Idle,
            route: Route::Catalog,
            mode: Mode::Normal,
            error: None,
            outcome: None,
            quit_after_completion: false,
        };
        session.enqueue(Request::Catalog);
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
        Ok(())
    }

    fn drain_replies(&mut self) -> io::Result<()> {
        self.dispatch_next()?;
        loop {
            match self.worker.replies.try_recv() {
                Ok(WorkerReply::Finished { id, result }) => {
                    let Some((active_id, request)) = self.active.take() else {
                        return Err(io::Error::other("TUI received an unexpected worker reply"));
                    };
                    if id != active_id {
                        return Err(io::Error::other("TUI worker reply order changed"));
                    }
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
        if let Mode::Confirm(action) = &self.mode {
            return self.handle_confirmation(event, action.clone());
        }

        if event.code == KeyCode::Char('q')
            || (event.code == KeyCode::Char('c') && event.modifiers.contains(KeyModifiers::CONTROL))
        {
            self.queued.clear();
            self.quit_after_completion = true;
            return Ok(());
        }
        match (self.route, event.code) {
            (Route::Catalog, KeyCode::Down | KeyCode::Char('j')) if !self.is_busy() => {
                self.select_next()
            }
            (Route::Catalog, KeyCode::Up | KeyCode::Char('k')) if !self.is_busy() => {
                self.select_previous()
            }
            (Route::Catalog, KeyCode::Enter) if !self.is_busy() => self.open_details(),
            (Route::Catalog, KeyCode::Char('r')) if !self.is_busy() => self.refresh_catalog(),
            (Route::Catalog, KeyCode::Esc) if !self.is_busy() => self.quit_after_completion = true,
            (Route::Details, KeyCode::Esc) if !self.is_busy() => self.route = Route::Catalog,
            (Route::Details, KeyCode::Char('r')) if !self.is_busy() => self.refresh_details(),
            (Route::Details, KeyCode::Char('s')) => {
                if let Some(application_name) = self.detail_application.clone() {
                    self.confirm(PendingAction::Start { application_name });
                }
            }
            (Route::Details, KeyCode::Char('x')) => {
                if let Some(application_name) = self.detail_application.clone() {
                    self.confirm(PendingAction::Stop { application_name });
                }
            }
            (Route::Details, KeyCode::Char('c')) => {
                if let Some(application_name) = self.detail_application.clone() {
                    self.confirm(PendingAction::Reconcile { application_name });
                }
            }
            (Route::Details, KeyCode::Char('p')) => {
                if let Some(application_name) = self.detail_application.clone() {
                    self.confirm(PendingAction::SetVisibility {
                        application_name,
                        visibility: Visibility::Public,
                    });
                }
            }
            (Route::Details, KeyCode::Char('i')) => {
                if let Some(application_name) = self.detail_application.clone() {
                    self.confirm(PendingAction::SetVisibility {
                        application_name,
                        visibility: Visibility::Internal,
                    });
                }
            }
            _ => {}
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

    fn confirm(&mut self, action: PendingAction) {
        if self.action_is_pending() {
            return;
        }
        self.error = None;
        self.outcome = None;
        self.mode = Mode::Confirm(action);
    }

    fn execute_action(&mut self, action: PendingAction) {
        self.error = None;
        self.outcome = None;
        self.queued.retain(|request| {
            !matches!(
                request,
                Request::Catalog | Request::Deployments { .. } | Request::Status { .. }
            )
        });
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

    fn open_details(&mut self) {
        if self.is_busy() {
            return;
        }
        let Some(application_name) = self.selected_application.clone() else {
            return;
        };
        self.route = Route::Details;
        self.detail_application = Some(application_name);
        self.refresh_details();
    }

    fn refresh_details(&mut self) {
        if self.is_busy() {
            return;
        }
        let Some(application_name) = self.detail_application.clone() else {
            return;
        };
        self.deployments = QueryState::Loading;
        self.runtime = QueryState::Loading;
        self.error = None;
        self.enqueue(Request::Deployments {
            application_name: application_name.clone(),
        });
        self.enqueue(Request::Status { application_name });
    }

    fn select_next(&mut self) {
        let QueryState::Ready(entries) = &self.catalog else {
            return;
        };
        self.selected_application =
            next_selection(entries, self.selected_application.as_deref(), 1);
    }

    fn select_previous(&mut self) {
        let QueryState::Ready(entries) = &self.catalog else {
            return;
        };
        self.selected_application =
            next_selection(entries, self.selected_application.as_deref(), -1);
    }

    fn apply_result(&mut self, request: Request, result: Result<CommandResult, WorkerError>) {
        match result {
            Ok(result) => self.apply_success(request, result),
            Err(error) => self.apply_error(request, error.display()),
        }
    }

    fn apply_success(&mut self, request: Request, result: CommandResult) {
        match (request, result) {
            (Request::Catalog, CommandResult::Applications(entries)) => {
                self.apply_catalog(entries);
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
                self.outcome = Some(format!("Started {result_name}"));
                self.refresh_after_action(&PendingAction::Start { application_name });
            }
            (
                Request::Action(PendingAction::Stop { application_name }),
                CommandResult::ApplicationStopped {
                    application_name: result_name,
                    observation,
                },
            ) if application_name == result_name.as_str() => {
                self.runtime = QueryState::Ready(observation);
                self.outcome = Some(format!("Stopped {result_name}"));
                self.refresh_after_action(&PendingAction::Stop { application_name });
            }
            (
                Request::Action(action @ PendingAction::Reconcile { .. }),
                CommandResult::Reconciled {
                    application_name: result_name,
                    result,
                },
            ) if action.application_name() == result_name.as_str() => {
                self.outcome = Some(output::reconciliation_result(&result_name, &result));
                self.refresh_after_action(&action);
            }
            (
                Request::Action(action @ PendingAction::SetVisibility { .. }),
                CommandResult::ExposureChanged {
                    application_name: result_name,
                    change,
                },
            ) if action.application_name() == result_name.as_str()
                && action.targets_visibility(change.visibility) =>
            {
                self.outcome = Some(output::visibility_change(&result_name, &change));
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
        if self.quit_after_completion {
            return;
        }
        self.catalog = QueryState::Loading;
        self.enqueue(Request::Catalog);
        match action {
            PendingAction::Start { application_name }
            | PendingAction::Stop { application_name } => {
                self.runtime = QueryState::Loading;
                self.enqueue(Request::Status {
                    application_name: application_name.clone(),
                });
            }
            PendingAction::Reconcile { application_name } => {
                self.deployments = QueryState::Loading;
                self.runtime = QueryState::Loading;
                self.enqueue(Request::Deployments {
                    application_name: application_name.clone(),
                });
                self.enqueue(Request::Status {
                    application_name: application_name.clone(),
                });
            }
            PendingAction::SetVisibility { .. } => {}
        }
    }

    fn apply_catalog(&mut self, entries: Vec<ApplicationCatalogEntry>) {
        self.selected_application = preserved_selection(&entries, self.selected_application.take());
        self.catalog = QueryState::Ready(entries);
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
}

fn preserved_selection(
    entries: &[ApplicationCatalogEntry],
    previous: Option<String>,
) -> Option<String> {
    previous
        .filter(|name| {
            entries
                .iter()
                .any(|entry| entry.summary.name.as_str() == name)
        })
        .or_else(|| {
            entries
                .first()
                .map(|entry| entry.summary.name.as_str().to_owned())
        })
}

fn next_selection(
    entries: &[ApplicationCatalogEntry],
    current: Option<&str>,
    movement: isize,
) -> Option<String> {
    let current_index = current.and_then(|name| {
        entries
            .iter()
            .position(|entry| entry.summary.name.as_str() == name)
    });
    let next_index = match (entries.len(), current_index, movement) {
        (0, _, _) => return None,
        (_, Some(index), 1) => (index + 1).min(entries.len() - 1),
        (_, Some(index), -1) => index.saturating_sub(1),
        (_, _, 1) => 0,
        (_, _, -1) => entries.len() - 1,
        _ => return current.map(str::to_owned),
    };
    Some(entries[next_index].summary.name.as_str().to_owned())
}

fn draw_shell(frame: &mut ratatui::Frame<'_>, session: &Session) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(frame.area());
    let content = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(areas[0]);

    draw_catalog(frame, content[0], session);
    draw_detail(frame, content[1], session);
    draw_footer(frame, areas[1], session);
    if let Mode::Confirm(action) = &session.mode {
        draw_confirmation(frame, action);
    }
}

fn draw_catalog(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, session: &Session) {
    match &session.catalog {
        QueryState::Idle => frame.render_widget(
            Paragraph::new("Refresh to load applications.").block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Applications "),
            ),
            area,
        ),
        QueryState::Loading => frame.render_widget(
            Paragraph::new("Loading applications...").block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Applications "),
            ),
            area,
        ),
        QueryState::Failed(error) => frame.render_widget(
            Paragraph::new(format!("Could not load applications:\n{error}"))
                .wrap(Wrap { trim: true })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Applications "),
                ),
            area,
        ),
        QueryState::Ready(entries) if entries.is_empty() => frame.render_widget(
            Paragraph::new("No applications are registered.").block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Applications "),
            ),
            area,
        ),
        QueryState::Ready(entries) => {
            let items = entries
                .iter()
                .map(|entry| {
                    let deployment = if entry.deployed {
                        "Has successful deployment"
                    } else {
                        "No successful deployment"
                    };
                    ListItem::new(format!("{}\n  {deployment}", entry.summary.name))
                })
                .collect::<Vec<_>>();
            let list = List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Applications "),
                )
                .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
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

fn draw_detail(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, session: &Session) {
    if session.route == Route::Catalog {
        frame.render_widget(
            Paragraph::new("Select an application and press Enter to inspect it.")
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true })
                .block(Block::default().borders(Borders::ALL).title(" Details ")),
            area,
        );
        return;
    }

    let detail_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(35),
            Constraint::Percentage(30),
            Constraint::Percentage(35),
        ])
        .split(area);
    let details = session.selected_entry().map_or_else(
        || "The selected application is no longer in the catalog.".to_owned(),
        application_details_text,
    );
    frame.render_widget(
        Paragraph::new(details).wrap(Wrap { trim: true }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Application details "),
        ),
        detail_areas[0],
    );
    frame.render_widget(
        Paragraph::new(runtime_text(&session.runtime))
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Runtime status "),
            ),
        detail_areas[1],
    );
    frame.render_widget(
        Paragraph::new(deployment_history_text(&session.deployments))
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Deployment history "),
            ),
        detail_areas[2],
    );
}

fn application_details_text(entry: &ApplicationCatalogEntry) -> String {
    format!(
        "Application: {}\nRepository: {}\nDefault branch: {}\nDesired runtime state: {}\nHas successful deployment: {}\nActive deployment ID: {}",
        entry.summary.name,
        entry.summary.repository,
        entry.summary.default_branch.as_deref().unwrap_or("None"),
        output::desired_runtime_state_label(entry.summary.desired_runtime_state),
        if entry.deployed { "yes" } else { "no" },
        entry
            .summary
            .active_deployment_id
            .as_ref()
            .map_or_else(|| "None".to_owned(), ToString::to_string)
    )
}

fn runtime_text(state: &QueryState<RuntimeObservation>) -> String {
    match state {
        QueryState::Idle => "Open application details to load runtime status.".to_owned(),
        QueryState::Loading => "Loading runtime status...".to_owned(),
        QueryState::Failed(error) => format!("Could not load runtime status:\n{error}"),
        QueryState::Ready(observation) => format!(
            "Desired runtime state: {}\nObserved runtime state: {}\nRuntime ID: {}\nContainer ID: {}\nObserved endpoint: {}",
            output::desired_runtime_state_label(observation.desired_runtime_state),
            output::observed_runtime_state_label(&observation.observed_runtime_state),
            observation.runtime_id,
            observation.container_id,
            observation
                .observed_endpoint
                .map_or_else(|| "None".to_owned(), |endpoint| endpoint.to_string())
        ),
    }
}

fn deployment_history_text(state: &QueryState<Vec<DeploymentHistory>>) -> String {
    match state {
        QueryState::Idle => "Open application details to load deployment history.".to_owned(),
        QueryState::Loading => "Loading deployment history...".to_owned(),
        QueryState::Failed(error) => format!("Could not load deployment history:\n{error}"),
        QueryState::Ready(deployments) if deployments.is_empty() => "No deployments.".to_owned(),
        QueryState::Ready(deployments) => deployments
            .iter()
            .map(|history| {
                format!(
                    "{} | {} | {} | {}{}",
                    history.deployment.id,
                    output::deployment_type_label(history.deployment.deployment_type),
                    output::deployment_status_label(history.deployment.status()),
                    history.release.artifact.reference(),
                    if history.is_active { " | active" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn draw_footer(frame: &mut ratatui::Frame<'_>, area: ratatui::layout::Rect, session: &Session) {
    let message = if session.quit_after_completion && session.is_busy() {
        "Finishing the current refresh before quitting...".to_owned()
    } else if session.is_busy() {
        "Refreshing... a confirmed action will run next; navigation is disabled.".to_owned()
    } else if let Some(error) = &session.error {
        format!("Error: {error}")
    } else if let Some(outcome) = &session.outcome {
        outcome.clone()
    } else if session.route == Route::Details {
        "s: start  x: stop  c: reconcile  p: public  i: internal  Esc: catalog  r: refresh  q: quit"
            .to_owned()
    } else {
        "Up/Down or j/k: select  Enter: details  r: refresh  q: quit".to_owned()
    };
    frame.render_widget(
        Paragraph::new(message)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::TOP)),
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
        Paragraph::new(format!(
            "{}\n\nEnter/y: confirm  Esc/n: cancel",
            action.confirmation_text()
        ))
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Confirm action "),
        ),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use pneuma::domain::application::{ApplicationName, ApplicationSummary, DesiredRuntimeState};
    use pneuma::domain::identity::{ApplicationId, SystemId};

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
        session.detail_application = Some("atlas".to_owned());
        session.route = Route::Details;
        session
    }

    #[test]
    fn catalog_refresh_preserves_selection_or_falls_back_to_first_entry() {
        let entries = vec![entry("atlas"), entry("beacon")];

        assert_eq!(
            preserved_selection(&entries, Some("beacon".to_owned())),
            Some("beacon".to_owned())
        );
        assert_eq!(
            preserved_selection(&entries, Some("removed".to_owned())),
            Some("atlas".to_owned())
        );
        assert_eq!(preserved_selection(&[], Some("atlas".to_owned())), None);
    }

    #[test]
    fn catalog_navigation_stops_at_the_list_boundaries() {
        let entries = vec![entry("atlas"), entry("beacon")];

        assert_eq!(
            next_selection(&entries, Some("atlas"), -1),
            Some("atlas".to_owned())
        );
        assert_eq!(
            next_selection(&entries, Some("beacon"), 1),
            Some("beacon".to_owned())
        );
        assert_eq!(next_selection(&[], None, 1), None);
    }

    #[test]
    fn application_details_use_persisted_and_not_runtime_labels() {
        let rendered = application_details_text(&entry("atlas"));

        assert!(rendered.contains("Desired runtime state: Running"));
        assert!(rendered.contains("Has successful deployment: no"));
        assert!(rendered.contains("Active deployment ID: None"));
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

        assert!(matches!(session.route, Route::Details));
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
}
