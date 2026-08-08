CREATE TRIGGER deployments_release_application_insert
BEFORE INSERT ON deployments
FOR EACH ROW
WHEN (SELECT application_id FROM releases WHERE id = NEW.release_id) != NEW.application_id
BEGIN
    SELECT RAISE(ABORT, 'deployment release belongs to another application');
END;

CREATE TRIGGER deployments_release_application_update
BEFORE UPDATE OF application_id, release_id ON deployments
FOR EACH ROW
WHEN (SELECT application_id FROM releases WHERE id = NEW.release_id) != NEW.application_id
BEGIN
    SELECT RAISE(ABORT, 'deployment release belongs to another application');
END;
