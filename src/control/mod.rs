//! Interface-neutral execution boundary.
//!
//! The [`ControlExecutor`] owns the immutable host configuration and executes
//! [`Command`]s synchronously, returning typed [`CommandResult`]s and typed
//! [`ControlError`]s. It never writes to the terminal, never detects a TTY,
//! and never selects process exit codes: presentation remains in the CLI and
//! future adapters. Every execution that needs SQLite acquires the shared
//! database-wide lock, opens one connection, performs one command, closes the
//! connection, and releases the lock.

mod command;
mod error;
mod host;
mod result;

pub use self::command::Command;
pub use self::error::ControlError;
pub use self::host::HostConfiguration;
pub use self::result::CommandResult;

use crate::adapters::database::{self, DatabaseError, DatabaseLock, LockMode};
use crate::domain::application::ApplicationName;
use crate::domain::release::OciArtifact;
use crate::domain::system::SystemName;
use crate::use_cases::reconciliation::ReconciliationReadError;
use crate::use_cases::{application, deployment, exposure, reconciliation, system};

/// Executes commands against the configured host without presentation concerns.
pub struct ControlExecutor {
    host: HostConfiguration,
}

impl ControlExecutor {
    // Builds an executor over explicitly supplied host configuration.
    pub fn new(host: HostConfiguration) -> Self {
        Self { host }
    }

    // Resolves host configuration from the documented Pneuma environment variables.
    pub fn from_environment() -> Self {
        Self::new(HostConfiguration::from_environment())
    }

    pub fn host(&self) -> &HostConfiguration {
        &self.host
    }

    // Executes one command, acquiring and releasing the database-wide lock around it.
    pub fn execute(&self, command: Command) -> Result<CommandResult, ControlError> {
        let mut ignore_events = |_| {};
        self.execute_with_events(command, &mut ignore_events)
    }

    // Executes one command while forwarding semantic deployment events to an observer.
    pub fn execute_with_events(
        &self,
        command: Command,
        events: &mut dyn FnMut(deployment::DeploymentEvent),
    ) -> Result<CommandResult, ControlError> {
        let _lock = self.acquire_shared_database_lock()?;
        let mut connection = database::open(&self.host.database_path)
            .map_err(|source| ControlError::Database { source })?;

        match command {
            Command::SystemCreate { name, description } => {
                let name = SystemName::new(&name)
                    .map_err(|source| ControlError::InvalidSystemName { source })?;
                let system = system::create_system(&mut connection, &name, description.as_deref())
                    .map_err(|source| ControlError::SystemCreate { source })?;
                Ok(CommandResult::SystemCreated(system))
            }
            Command::SystemList => {
                let systems = system::list_systems(&connection)
                    .map_err(|source| ControlError::SystemList { source })?;
                Ok(CommandResult::Systems(systems))
            }
            Command::SystemShow { name } => {
                let name = SystemName::new(&name)
                    .map_err(|source| ControlError::InvalidSystemName { source })?;
                let details = system::show_system(&connection, &name)
                    .map_err(|source| ControlError::SystemShow { source })?;
                Ok(CommandResult::SystemDetails(details))
            }
            Command::ImportApplication {
                repository,
                system_name,
                manifest_path,
            } => {
                let application = application::import_remote_application(
                    &mut connection,
                    &repository,
                    &self.host.workspace_path,
                    system_name.as_deref(),
                    manifest_path.as_deref(),
                )
                .map_err(|source| ControlError::Import { source })?;
                Ok(CommandResult::ApplicationImported(application))
            }
            Command::ListApplications => {
                let entries = application::list_application_catalog(&connection)
                    .map_err(|source| ControlError::ListApplications { source })?;
                Ok(CommandResult::Applications(entries))
            }
            Command::ListDeployments { application_name } => {
                let resolved = application::resolve_application(&connection, &application_name)
                    .map_err(|source| ControlError::ApplicationLookup { source })?;
                let deployments = deployment::list_deployments(&connection, &resolved.id)
                    .map_err(|source| ControlError::ListDeployments { source })?;
                Ok(CommandResult::ApplicationDeployments {
                    application_name: resolved.name,
                    deployments,
                })
            }
            Command::ApplicationStatus { application_name } => {
                let resolved = application::resolve_application(&connection, &application_name)
                    .map_err(|source| ControlError::ApplicationLookup { source })?;
                let observation = application::report_application_status(
                    &mut connection,
                    &resolved.id,
                    &resolved.name,
                )
                .map_err(|source| ControlError::RuntimeStatus { source })?;
                Ok(CommandResult::ApplicationStatus {
                    application_name: resolved.name,
                    observation,
                })
            }
            Command::ApplicationStop { application_name } => {
                let resolved = application::resolve_application(&connection, &application_name)
                    .map_err(|source| ControlError::ApplicationLookup { source })?;
                let observation =
                    application::stop_application(&mut connection, &resolved.id, &resolved.name)
                        .map_err(|source| ControlError::RuntimeStop { source })?;
                Ok(CommandResult::ApplicationStopped {
                    application_name: resolved.name,
                    observation,
                })
            }
            Command::ApplicationStart { application_name } => {
                let resolved = application::resolve_application(&connection, &application_name)
                    .map_err(|source| ControlError::ApplicationLookup { source })?;
                let observation =
                    application::start_application(&mut connection, &resolved.id, &resolved.name)
                        .map_err(|source| ControlError::RuntimeStart { source })?;
                Ok(CommandResult::ApplicationStarted {
                    application_name: resolved.name,
                    observation,
                })
            }
            Command::DeployImage {
                application_name,
                image_reference,
            } => {
                let resolved = application::resolve_application(&connection, &application_name)
                    .map_err(|source| ControlError::ApplicationLookup { source })?;
                let artifact = OciArtifact::parse(&image_reference)
                    .map_err(|source| ControlError::InvalidOciArtifact { source })?;
                events(deployment::DeploymentEvent::DeploymentRequested {
                    application_name: resolved.name.clone(),
                });
                let public_configuration = deployment::PublicDeploymentConfiguration {
                    managed_caddy_directory: self.host.caddy_managed_path.clone(),
                    caddyfile_path: self.host.caddyfile_path.clone(),
                };
                let deployment = deployment::deploy_oci_with_events(
                    &mut connection,
                    &resolved.id,
                    &artifact,
                    None,
                    Some(&public_configuration),
                    events,
                )
                .map_err(|source| ControlError::DeployOci { source })?;
                Ok(CommandResult::ApplicationDeployed {
                    application_name: resolved.name,
                    deployment,
                })
            }
            Command::DeployBranch {
                application_name,
                branch,
            } => {
                let resolved = application::resolve_application(&connection, &application_name)
                    .map_err(|source| ControlError::ApplicationLookup { source })?;
                events(deployment::DeploymentEvent::DeploymentRequested {
                    application_name: resolved.name.clone(),
                });
                let public_configuration = deployment::PublicDeploymentConfiguration {
                    managed_caddy_directory: self.host.caddy_managed_path.clone(),
                    caddyfile_path: self.host.caddyfile_path.clone(),
                };
                let deployment = deployment::deploy_branch_with_events(
                    &mut connection,
                    &resolved.id,
                    Some(&branch),
                    Some(&public_configuration),
                    events,
                )
                .map_err(|source| ControlError::DeployBranch { source })?;
                Ok(CommandResult::ApplicationDeployed {
                    application_name: resolved.name,
                    deployment,
                })
            }
            Command::Rollback { application_name } => {
                let resolved = application::resolve_application(&connection, &application_name)
                    .map_err(|source| ControlError::ApplicationLookup { source })?;
                events(deployment::DeploymentEvent::DeploymentRequested {
                    application_name: resolved.name.clone(),
                });
                let public_configuration = deployment::PublicDeploymentConfiguration {
                    managed_caddy_directory: self.host.caddy_managed_path.clone(),
                    caddyfile_path: self.host.caddyfile_path.clone(),
                };
                let deployment = deployment::rollback_deployment_with_events(
                    &mut connection,
                    &resolved.id,
                    Some(&public_configuration),
                    events,
                )
                .map_err(|source| ControlError::Rollback { source })?;
                Ok(CommandResult::ApplicationRolledBack {
                    application_name: resolved.name,
                    deployment,
                })
            }
            Command::VisibilitySet {
                application_name,
                visibility,
            } => {
                let resolved = application::resolve_application(&connection, &application_name)
                    .map_err(|source| ControlError::ApplicationLookup { source })?;
                let change = exposure::change_exposure(
                    &mut connection,
                    &resolved.id,
                    visibility,
                    &self.host.caddy_managed_path,
                    &self.host.caddyfile_path,
                )
                .map_err(|source| ControlError::VisibilitySet { source })?;
                Ok(CommandResult::ExposureChanged {
                    application_name: resolved.name,
                    change,
                })
            }
            Command::Reconcile { application_name } => {
                let application_name = ApplicationName::new(&application_name).map_err(|_| {
                    ControlError::Reconcile {
                        source: ReconciliationReadError::ApplicationNotFound { application_name },
                    }
                })?;
                let result = reconciliation::reconcile_application(
                    &mut connection,
                    &application_name,
                    &self.host.caddy_managed_path,
                    &self.host.caddyfile_path,
                )
                .map_err(|source| ControlError::Reconcile { source })?;
                Ok(CommandResult::Reconciled {
                    application_name,
                    result,
                })
            }
        }
    }

    // Normal commands share the database-wide lock; contention is a typed busy error.
    fn acquire_shared_database_lock(&self) -> Result<DatabaseLock, ControlError> {
        match DatabaseLock::try_acquire(&self.host.database_path, LockMode::Shared) {
            Ok(Some(lock)) => Ok(lock),
            Ok(None) => Err(ControlError::Database {
                source: DatabaseError::DatabaseBusy {
                    path: self.host.database_path.clone(),
                },
            }),
            Err(source) => Err(ControlError::Database { source }),
        }
    }
}
