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
use pneuma::use_cases::application::{ApplicationCatalogEntry, RuntimeObservation};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use super::{error::CliError, output};

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

enum ReadRequest {
    Catalog,
    Deployments { application_name: String },
    Status { application_name: String },
}

impl ReadRequest {
    fn command(&self) -> Command {
        match self {
            Self::Catalog => Command::ListApplications,
            Self::Deployments { application_name } => Command::ListDeployments {
                application_name: application_name.clone(),
            },
            Self::Status { application_name } => Command::ApplicationStatus {
                application_name: application_name.clone(),
            },
        }
    }
}

enum WorkerRequest {
    Execute { id: u64, command: Command },
    Shutdown,
}

enum WorkerReply {
    Finished {
        id: u64,
        result: Result<CommandResult, String>,
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
                let result = executor.execute(command).map_err(|error| error.to_string());
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
    active: Option<(u64, ReadRequest)>,
    queued: VecDeque<ReadRequest>,
    catalog: QueryState<Vec<ApplicationCatalogEntry>>,
    selected_application: Option<String>,
    detail_application: Option<String>,
    deployments: QueryState<Vec<DeploymentHistory>>,
    runtime: QueryState<RuntimeObservation>,
    route: Route,
    error: Option<String>,
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
            error: None,
            quit_after_completion: false,
        };
        session.enqueue(ReadRequest::Catalog);
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

    fn enqueue(&mut self, request: ReadRequest) {
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
        if event.code == KeyCode::Char('q')
            || (event.code == KeyCode::Char('c') && event.modifiers.contains(KeyModifiers::CONTROL))
        {
            self.queued.clear();
            self.quit_after_completion = true;
            return Ok(());
        }

        match (self.route, event.code) {
            (Route::Catalog, KeyCode::Down | KeyCode::Char('j')) => self.select_next(),
            (Route::Catalog, KeyCode::Up | KeyCode::Char('k')) => self.select_previous(),
            (Route::Catalog, KeyCode::Enter) => self.open_details(),
            (Route::Catalog, KeyCode::Char('r')) => self.refresh_catalog(),
            (Route::Catalog, KeyCode::Esc) => self.quit_after_completion = true,
            (Route::Details, KeyCode::Esc) => self.route = Route::Catalog,
            (Route::Details, KeyCode::Char('r')) => self.refresh_details(),
            _ => {}
        }
        Ok(())
    }

    fn refresh_catalog(&mut self) {
        if self.is_busy() {
            return;
        }
        self.catalog = QueryState::Loading;
        self.error = None;
        self.enqueue(ReadRequest::Catalog);
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
        self.enqueue(ReadRequest::Deployments {
            application_name: application_name.clone(),
        });
        self.enqueue(ReadRequest::Status { application_name });
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

    fn apply_result(&mut self, request: ReadRequest, result: Result<CommandResult, String>) {
        match result {
            Ok(result) => self.apply_success(request, result),
            Err(error) => self.apply_error(request, error),
        }
    }

    fn apply_success(&mut self, request: ReadRequest, result: CommandResult) {
        match (request, result) {
            (ReadRequest::Catalog, CommandResult::Applications(entries)) => {
                self.apply_catalog(entries);
            }
            (
                ReadRequest::Deployments { application_name },
                CommandResult::ApplicationDeployments {
                    application_name: result_name,
                    deployments,
                },
            ) if application_name == result_name.as_str() => {
                self.deployments = QueryState::Ready(deployments);
            }
            (
                ReadRequest::Status { application_name },
                CommandResult::ApplicationStatus {
                    application_name: result_name,
                    observation,
                },
            ) if application_name == result_name.as_str() => {
                self.runtime = QueryState::Ready(observation);
            }
            (request, _) => self.apply_error(
                request,
                "TUI received an unexpected control result".to_owned(),
            ),
        }
    }

    fn apply_error(&mut self, request: ReadRequest, error: String) {
        match request {
            ReadRequest::Catalog => {
                self.catalog = QueryState::Failed(error.clone());
                self.selected_application = None;
            }
            ReadRequest::Deployments { .. } => self.deployments = QueryState::Failed(error.clone()),
            ReadRequest::Status { .. } => self.runtime = QueryState::Failed(error.clone()),
        }
        self.error = Some(error);
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
    } else if let Some(error) = &session.error {
        format!("Error: {error}")
    } else if session.route == Route::Details {
        "Esc: catalog  r: refresh  q: quit".to_owned()
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
}
