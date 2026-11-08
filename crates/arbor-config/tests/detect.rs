use std::fs;

use arbor_config::{
    Checks, Config, Detection, FILTER_PLACEHOLDER, PROJECT_TYPES, detect_project_type,
    detect_project_type_in,
};
use tempfile::tempdir;

fn detect(root_files: &[&str]) -> Detection {
    detect_project_type(|name| root_files.contains(&name))
}

fn found(manifest: &str) -> Detection {
    let project_type = PROJECT_TYPES
        .iter()
        .find(|project_type| project_type.manifest == manifest)
        .unwrap();
    Detection::Found(project_type)
}

#[test]
fn one_manifest_names_the_project_type() {
    for (root_files, manifest) in [
        (&["Cargo.toml", "README.md"][..], "Cargo.toml"),
        (&["go.mod", "go.sum"], "go.mod"),
        (&["pyproject.toml"], "pyproject.toml"),
        (&["package.json", "tsconfig.json"], "package.json"),
    ] {
        assert_eq!(detect(root_files), found(manifest), "{root_files:?}");
    }
}

#[test]
fn no_known_manifest_is_detected_as_none() {
    assert_eq!(detect(&[]), Detection::NoManifest);
    assert_eq!(detect(&["Makefile", "README.md"]), Detection::NoManifest);
}

#[test]
fn manifests_for_several_project_types_assume_none() {
    let Detection::Several(found) = detect(&["Cargo.toml", "package.json"]) else {
        panic!("expected several project types");
    };
    let names: Vec<&str> = found.iter().map(|project_type| project_type.name).collect();
    assert_eq!(names, ["Rust", "JavaScript"]);
}

#[test]
fn every_project_type_has_checks_arbor_toml_accepts() {
    for project_type in PROJECT_TYPES {
        let checks = project_type.checks();
        assert!(checks.scaffold.is_some() && checks.unit.is_some());
        if let Some(template) = &checks.unit_filtered {
            assert!(template.contains(FILTER_PLACEHOLDER), "{template}");
            let text = format!("[checks]\nunit_filtered = {template:?}\n");
            assert!(Config::from_toml(None, Some(&text)).is_ok(), "{text}");
        }
    }
    assert_eq!(
        PROJECT_TYPES[0].checks(),
        Checks {
            scaffold: Some("cargo check --all-targets".into()),
            unit: Some("cargo test".into()),
            unit_filtered: Some("cargo test {filter}".into()),
        }
    );
}

#[test]
fn detection_looks_only_at_files_in_the_project_root() {
    let dir = tempdir().unwrap();
    assert_eq!(detect_project_type_in(dir.path()), Detection::NoManifest);
    fs::create_dir_all(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join("sub").join("go.mod"), "module x\n").unwrap();
    fs::create_dir_all(dir.path().join("Cargo.toml")).unwrap();
    assert_eq!(detect_project_type_in(dir.path()), Detection::NoManifest);
    fs::write(dir.path().join("go.mod"), "module x\n").unwrap();
    assert_eq!(detect_project_type_in(dir.path()), found("go.mod"));
}

#[test]
fn a_detection_describes_itself_for_the_scaffolder() {
    assert_eq!(
        found("Cargo.toml").to_string(),
        "Detected a Rust project from Cargo.toml."
    );
    let none = Detection::NoManifest.to_string();
    assert!(none.contains("Cargo.toml (Rust), go.mod (Go)"), "{none}");
    assert_eq!(
        detect(&["go.mod", "pyproject.toml"]).to_string(),
        "The repository root has manifests for several project types: go.mod (Go), \
         pyproject.toml (Python)."
    );
}
