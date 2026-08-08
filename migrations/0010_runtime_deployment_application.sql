CREATE TRIGGER runtime_instances_deployment_application_insert
BEFORE INSERT ON runtime_instances
FOR EACH ROW
WHEN (SELECT application_id FROM deployments WHERE id = NEW.deployment_id) != NEW.application_id
BEGIN
    SELECT RAISE(ABORT, 'runtime deployment belongs to another application');
END;

CREATE TRIGGER runtime_instances_deployment_application_update
BEFORE UPDATE OF application_id, deployment_id ON runtime_instances
FOR EACH ROW
WHEN (SELECT application_id FROM deployments WHERE id = NEW.deployment_id) != NEW.application_id
BEGIN
    SELECT RAISE(ABORT, 'runtime deployment belongs to another application');
END;
