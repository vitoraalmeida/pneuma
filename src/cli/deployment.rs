use rusqlite::Connection;

use pneuma::domain::release::OciArtifact;
use pneuma::use_cases::deployment::{
    DeployBranchError, DeployOciError, PublicDeploymentConfiguration, deploy_branch,
    deploy_branch_with_progress, deploy_oci, deploy_oci_with_progress, list_deployments,
    rollback_deployment,
};

use super::error::CliError;
use super::output;
use super::shared::{
    CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE, CADDYFILE_PATH_ENVIRONMENT_VARIABLE,
    DEFAULT_CADDY_MANAGED_PATH, DEFAULT_CADDYFILE_PATH, configured_path, log_verbose,
    resolve_application,
};

// Resolves the named application before listing only its deployment history.
pub(crate) fn run_deployments(
    connection: &Connection,
    verbose: bool,
    application_name: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    let application = resolve_application(connection, application_name)?;
    log_verbose(
        verbose,
        format!("list deployments of application {}", application.name),
    );
    let deployments = list_deployments(connection, &application.id)
        .map_err(|source| CliError::ListDeployments { source })?;
    println!(
        "{}",
        output::deployment_history(&application.name, &deployments)
    );
    Ok(())
}

// Selects the delivery mode requested by the deploy command options.
pub(crate) fn run_deploy(
    connection: &mut Connection,
    verbose: bool,
    application_name: &str,
    image_reference: Option<String>,
    branch: Option<String>,
) -> Result<(), CliError> {
    if let Some(branch) = branch {
        run_deploy_branch(connection, verbose, application_name, &branch)
    } else {
        let image_reference = image_reference.ok_or(CliError::MissingDeployOption)?;
        run_deploy_oci(connection, verbose, application_name, &image_reference)
    }
}

// Deploys a supplied OCI reference with host-configured public exposure paths.
pub(crate) fn run_deploy_oci(
    connection: &mut Connection,
    verbose: bool,
    application_name: &str,
    image_reference: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    let artifact = OciArtifact::parse(image_reference)
        .map_err(|source| CliError::InvalidOciArtifact { source })?;
    let application = resolve_application(connection, application_name)?;
    let public_configuration = public_deployment_configuration();
    if verbose {
        log_verbose(
            verbose,
            format!(
                "deployment input: application {}, image {image_reference}",
                application.name
            ),
        );
    } else {
        eprintln!("Deploying {}...", application.name);
    }
    let deployed = if verbose {
        let mut progress = |event| eprintln!("{event}");
        deploy_oci_with_progress(
            connection,
            &application.id,
            &artifact,
            None,
            Some(&public_configuration),
            &mut progress,
        )
    } else {
        deploy_oci(
            connection,
            &application.id,
            &artifact,
            None,
            Some(&public_configuration),
        )
    }
    .map_err(|source: DeployOciError| CliError::DeployOci {
        source: Box::new(source),
    })?;
    println!("{}", output::deployed(&application.name, &deployed));
    Ok(())
}

// Resolves and deploys the requested branch's published OCI artifact with host-configured paths.
pub(crate) fn run_deploy_branch(
    connection: &mut Connection,
    verbose: bool,
    application_name: &str,
    branch: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    let application = resolve_application(connection, application_name)?;
    let public_configuration = public_deployment_configuration();
    if verbose {
        log_verbose(
            verbose,
            format!(
                "deployment input: application {}, branch {branch}",
                application.name
            ),
        );
    } else {
        eprintln!("Deploying {}...", application.name);
    }
    let deployed = if verbose {
        let mut progress = |event| eprintln!("{event}");
        deploy_branch_with_progress(
            connection,
            &application.id,
            Some(branch),
            Some(&public_configuration),
            &mut progress,
        )
    } else {
        deploy_branch(
            connection,
            &application.id,
            Some(branch),
            Some(&public_configuration),
        )
    }
    .map_err(|source: DeployBranchError| CliError::DeployBranch {
        source: Box::new(source),
    })?;
    println!("{}", output::deployed(&application.name, &deployed));
    Ok(())
}

// Rolls back through the use case while supplying paths needed for public exposure effects.
pub(crate) fn run_rollback(
    connection: &mut Connection,
    verbose: bool,
    application_name: &str,
) -> Result<(), CliError> {
    log_verbose(
        verbose,
        format!("resolve application by name: {application_name}"),
    );
    let application = resolve_application(connection, application_name)?;
    log_verbose(
        verbose,
        format!("rolling back application {}", application.name),
    );
    let public_configuration = public_deployment_configuration();
    let rolled_back = rollback_deployment(connection, &application.id, Some(&public_configuration))
        .map_err(|source| CliError::Rollback { source })?;
    println!(
        "{}",
        output::rollback_result(&application.name, &rolled_back)
    );
    Ok(())
}

fn public_deployment_configuration() -> PublicDeploymentConfiguration {
    PublicDeploymentConfiguration {
        managed_caddy_directory: configured_path(
            CADDY_MANAGED_PATH_ENVIRONMENT_VARIABLE,
            DEFAULT_CADDY_MANAGED_PATH,
        ),
        caddyfile_path: configured_path(
            CADDYFILE_PATH_ENVIRONMENT_VARIABLE,
            DEFAULT_CADDYFILE_PATH,
        ),
    }
}
