use std::path::{Path, PathBuf};

use pneuma::manifest::{ManifestError, Visibility, load_manifest, parse_manifest};

const VALID_MANIFEST: &str = include_str!("fixtures/valid/pneuma.toml");

#[test]
fn loads_and_validates_a_repository_manifest() {
    let repository = fixture_path("valid");

    let manifest = load_manifest(&repository).expect("valid fixture should load");

    assert_eq!(manifest.schema_version, 1);
    assert_eq!(manifest.application.name, "personal-site");
    assert_eq!(manifest.source.branch, "main");
    assert_eq!(manifest.build.containerfile, Path::new("Containerfile"));
    assert_eq!(manifest.runtime.container_port, 8080);
    assert_eq!(manifest.runtime.healthcheck_path, "/healthz");
    assert_eq!(manifest.exposure.default_visibility, Visibility::Public);
    assert_eq!(
        manifest.exposure.domain.as_deref(),
        Some("vitoralmeida.tech")
    );
}

#[test]
fn reports_a_missing_manifest_with_its_path() {
    let repository = fixture_path("missing");

    let error = load_manifest(&repository).expect_err("missing fixture should fail");

    assert!(matches!(error, ManifestError::Read { .. }));
    assert!(error.to_string().contains("missing/pneuma.toml"));
}

#[test]
fn rejects_an_unsupported_schema_version() {
    let contents = VALID_MANIFEST.replace("schema_version = 1", "schema_version = 2");

    let error = parse_manifest(&contents).expect_err("schema version 2 should fail");

    assert!(matches!(
        error,
        ManifestError::UnsupportedSchemaVersion { found: 2 }
    ));
}

#[test]
fn rejects_invalid_manifest_fields() {
    let cases = [
        (
            "application.name",
            "name = \"personal-site\"",
            "name = \"Personal Site\"",
        ),
        (
            "application.name",
            "name = \"personal-site\"",
            "name = \"\"",
        ),
        (
            "application.name",
            "name = \"personal-site\"",
            "name = \"-personal-site\"",
        ),
        (
            "application.name",
            "name = \"personal-site\"",
            "name = \"personal-site-\"",
        ),
        (
            "source.repository",
            "repository = \"https://github.com/vitoraalmeida/vitoralmeida.tech\"",
            "repository = \"\"",
        ),
        (
            "source.repository",
            "repository = \"https://github.com/vitoraalmeida/vitoralmeida.tech\"",
            "repository = \" local/repository \"",
        ),
        (
            "source.branch",
            "branch = \"main\"",
            "branch = \"main branch\"",
        ),
        ("source.branch", "branch = \"main\"", "branch = \"\""),
        (
            "build.containerfile",
            "containerfile = \"Containerfile\"",
            "containerfile = \"/Containerfile\"",
        ),
        ("build.context", "context = \".\"", "context = \"../site\""),
        ("build.context", "context = \".\"", "context = \"\""),
        (
            "runtime.container_port",
            "container_port = 8080",
            "container_port = 0",
        ),
        (
            "runtime.healthcheck_path",
            "healthcheck_path = \"/healthz\"",
            "healthcheck_path = \"healthz\"",
        ),
        (
            "runtime.healthcheck_path",
            "healthcheck_path = \"/healthz\"",
            "healthcheck_path = \"/health check\"",
        ),
        (
            "runtime.expected_status",
            "expected_status = 200",
            "expected_status = 99",
        ),
        (
            "runtime.expected_status",
            "expected_status = 200",
            "expected_status = 600",
        ),
        (
            "exposure.domain",
            "domain = \"vitoralmeida.tech\"",
            "domain = \"-invalid.example\"",
        ),
        (
            "exposure.domain",
            "domain = \"vitoralmeida.tech\"",
            "domain = \"invalid-.example\"",
        ),
        (
            "exposure.domain",
            "domain = \"vitoralmeida.tech\"",
            "domain = \"\"",
        ),
        (
            "exposure.domain",
            "domain = \"vitoralmeida.tech\"",
            "domain = \"invalid..example\"",
        ),
        (
            "exposure.domain",
            "domain = \"vitoralmeida.tech\"",
            "domain = \"inválido.example\"",
        ),
    ];

    for (expected_field, valid, invalid) in cases {
        let contents = VALID_MANIFEST.replace(valid, invalid);
        assert_invalid_field(&contents, expected_field);
    }
}

#[test]
fn accepts_name_domain_and_status_boundaries() {
    let maximum_name = "a".repeat(63);
    let maximum_domain = format!("{}.example", "a".repeat(63));

    for expected_status in [100, 599] {
        let contents = VALID_MANIFEST
            .replace(
                "name = \"personal-site\"",
                &format!("name = \"{maximum_name}\""),
            )
            .replace(
                "domain = \"vitoralmeida.tech\"",
                &format!("domain = \"{maximum_domain}\""),
            )
            .replace(
                "expected_status = 200",
                &format!("expected_status = {expected_status}"),
            );

        let manifest = parse_manifest(&contents).expect("boundary values should be valid");

        assert_eq!(manifest.application.name, maximum_name);
        assert_eq!(
            manifest.exposure.domain.as_deref(),
            Some(maximum_domain.as_str())
        );
        assert_eq!(manifest.runtime.expected_status, expected_status);
    }
}

#[test]
fn rejects_overlong_names_and_domains() {
    let overlong_name = "a".repeat(64);
    let contents = VALID_MANIFEST.replace(
        "name = \"personal-site\"",
        &format!("name = \"{overlong_name}\""),
    );
    assert_invalid_field(&contents, "application.name");

    let overlong_label = "a".repeat(64);
    let contents = VALID_MANIFEST.replace(
        "domain = \"vitoralmeida.tech\"",
        &format!("domain = \"{overlong_label}.example\""),
    );
    assert_invalid_field(&contents, "exposure.domain");

    let overlong_domain = vec!["a"; 128].join(".");
    let contents = VALID_MANIFEST.replace(
        "domain = \"vitoralmeida.tech\"",
        &format!("domain = \"{overlong_domain}\""),
    );
    assert_invalid_field(&contents, "exposure.domain");
}

#[test]
fn rejects_container_ports_above_the_supported_range() {
    let contents = VALID_MANIFEST.replace("container_port = 8080", "container_port = 65536");

    let error = parse_manifest(&contents).expect_err("port above u16 range should fail");

    assert!(matches!(error, ManifestError::Parse { .. }));
}

#[test]
fn requires_a_domain_for_public_exposure() {
    let contents = VALID_MANIFEST.replace("domain = \"vitoralmeida.tech\"\n", "");

    let error = parse_manifest(&contents).expect_err("public exposure needs a domain");

    assert!(matches!(
        error,
        ManifestError::InvalidField {
            field: "exposure.domain",
            ..
        }
    ));
}

#[test]
fn allows_internal_exposure_without_a_domain() {
    let contents = VALID_MANIFEST
        .replace(
            "default_visibility = \"public\"",
            "default_visibility = \"internal\"",
        )
        .replace("domain = \"vitoralmeida.tech\"\n", "");

    let manifest = parse_manifest(&contents).expect("internal exposure should not need a domain");

    assert_eq!(manifest.exposure.default_visibility, Visibility::Internal);
    assert_eq!(manifest.exposure.domain, None);
}

#[test]
fn rejects_unknown_fields() {
    let contents = VALID_MANIFEST.replace(
        "container_port = 8080",
        "container_port = 8080\nprivileged = true",
    );

    let error = parse_manifest(&contents).expect_err("unknown runtime field should fail");

    assert!(matches!(error, ManifestError::Parse { .. }));
    assert!(error.to_string().contains("unknown field `privileged`"));
}

#[test]
fn reports_invalid_toml() {
    let error =
        parse_manifest("schema_version = [").expect_err("syntactically invalid TOML should fail");

    assert!(matches!(error, ManifestError::Parse { .. }));
    assert!(error.to_string().contains("invalid manifest TOML"));
}

fn assert_invalid_field(contents: &str, expected_field: &'static str) {
    let error = parse_manifest(contents).expect_err(expected_field);

    assert!(
        matches!(
            error,
            ManifestError::InvalidField { field, .. } if field == expected_field
        ),
        "unexpected error for {expected_field}: {error}"
    );
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
