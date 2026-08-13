# ADR-0003 - Rootless Podman, Quadlet, and systemd

**Status:** Accepted (retrospective)

## Context

Pneuma must make deployment decisions without becoming the process supervisor.
Applications must continue after the CLI exits and return after host reboot while
reducing runtime privilege.

## Decision

Pneuma uses rootless Podman materialized through Quadlet files and supervised by
the `pneuma` user's systemd manager. Units bind application ports to loopback,
use `Restart=on-failure`, and contain `WantedBy=default.target`. Bootstrap enables
user linger. Pneuma writes and starts a candidate unit before health verification;
it does not call `systemctl --user enable`.

## Alternatives Considered

- **Pneuma daemon supervising child containers:** rejected because application
  availability would depend on a resident Pneuma process.
- **Rootful containers:** rejected because the current host model can use a
  narrower runtime privilege boundary.
- **Direct Podman without systemd/Quadlet:** rejected because reboot and
  long-lived service ownership would be less explicit.

## Consequences

systemd owns long-lived runtime supervision after promotion. Rootless Podman
reduces privilege but is not hostile-workload or multi-tenant isolation.

## References

- [`../architecture/architecture.md`](../architecture/architecture.md)
- [`../architecture/security-model.md`](../architecture/security-model.md)
- [`../operations/dev-vm-tutorial.md`](../operations/dev-vm-tutorial.md)
