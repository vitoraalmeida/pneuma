use rusqlite::Connection;

use pneuma::domain::deployment::{DeploymentFailureEvidence, DeploymentLifecycle};
use pneuma::domain::release::OciArtifact;
use pneuma::use_cases::deployment::{
    DeployBranchError, DeployOciError, DeploymentResult, PublicDeploymentConfiguration,
    deploy_branch, deploy_branch_with_progress, deploy_oci, deploy_oci_with_progress,
    list_deployments, rollback_deployment,
};

use super::error::CliError;
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
    if deployments.is_empty() {
        println!("No deployments for {}", application.name);
    } else {
        println!("Deployments for {}:", application.name);
        println!("DEPLOYMENT\tTYPE\tRELEASE\tSOURCE\tSTATUS\tSTARTED\tFINISHED\tACTIVE\tFAILURE");
        for deployment in deployments {
            let source = deployment
                .deployment
                .source_revision
                .as_ref()
                .map_or("-", pneuma::domain::deployment::SourceRevision::as_str);
            let (finished_at, failure) = match &deployment.deployment.lifecycle {
                DeploymentLifecycle::Succeeded { finished_at } => {
                    (finished_at.as_str(), "-".to_owned())
                }
                DeploymentLifecycle::Failed {
                    evidence: DeploymentFailureEvidence::Complete(failure),
                } => (
                    failure.finished_at.as_str(),
                    format!("{}:{}:{}", failure.code, failure.stage, failure.message),
                ),
                DeploymentLifecycle::Failed {
                    evidence: DeploymentFailureEvidence::Incomplete,
                } => ("-", "incomplete".to_owned()),
                DeploymentLifecycle::Pending
                | DeploymentLifecycle::Starting
                | DeploymentLifecycle::Verifying
                | DeploymentLifecycle::Activating => ("-", "-".to_owned()),
            };
            println!(
                "{}\t{:?}\t{}\t{}\t{:?}\t{}\t{}\t{}\t{}",
                deployment.deployment.id,
                deployment.deployment.deployment_type,
                deployment.release.artifact.digest(),
                source,
                deployment.deployment.status(),
                deployment.deployment.started_at.as_deref().unwrap_or("-"),
                finished_at,
                if deployment.is_active { "yes" } else { "no" },
                failure,
            );
        }
    }
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
    print_deployed(&application.name, &deployed);
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
    print_deployed(&application.name, &deployed);
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
    println!("Rolled back {}", application.name);
    println!("Image: {}", rolled_back.artifact.reference());
    if let Some(source_revision) = rolled_back.source_revision {
        println!("Source revision: {source_revision}");
    }
    println!("Deployment: {}", rolled_back.deployment_id);
    println!("Runtime: {}", rolled_back.runtime_id);
    println!("Status: Succeeded");
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

fn print_deployed(
    application_name: &pneuma::domain::application::ApplicationName,
    deployed: &DeploymentResult,
) {
    println!("Deployed {application_name}");
    println!("Image: {}", deployed.artifact.reference());
    if let Some(source_revision) = &deployed.source_revision {
        println!("Source revision: {source_revision}");
    }
    println!("Deployment: {}", deployed.deployment_id);
    println!("Runtime: {}", deployed.runtime_id);
    println!("Container: {}", deployed.container_name);
    println!("Status: Succeeded");
}
