use std::fmt;
use std::path::Path;

use crate::config::Checks;

/// A kind of project Arbor recognises by a manifest at the repository root, with the Checks such
/// projects usually use.
#[derive(Debug, PartialEq, Eq)]
pub struct ProjectType {
    pub name: &'static str,
    pub manifest: &'static str,
    pub scaffold: &'static str,
    pub unit: &'static str,
    /// `None` where the test runner has no portable way to select tests by name.
    pub unit_filtered: Option<&'static str>,
}

pub const PROJECT_TYPES: &[ProjectType] = &[
    ProjectType {
        name: "Rust",
        manifest: "Cargo.toml",
        scaffold: "cargo check --all-targets",
        unit: "cargo test",
        unit_filtered: Some("cargo test {filter}"),
    },
    ProjectType {
        name: "Go",
        manifest: "go.mod",
        // `go vet` compiles the tests as well as the code, which `go build` skips.
        scaffold: "go vet ./...",
        unit: "go test ./...",
        unit_filtered: Some("go test ./... -run {filter}"),
    },
    ProjectType {
        name: "Python",
        manifest: "pyproject.toml",
        // Collecting imports every test module, the nearest Python has to a build.
        scaffold: "python -m pytest --collect-only -q",
        unit: "python -m pytest",
        unit_filtered: Some("python -m pytest -k {filter}"),
    },
    ProjectType {
        name: "JavaScript",
        manifest: "package.json",
        scaffold: "npm run build --if-present",
        unit: "npm test",
        unit_filtered: None,
    },
];

impl ProjectType {
    pub fn checks(&self) -> Checks {
        Checks {
            scaffold: Some(self.scaffold.to_owned()),
            unit: Some(self.unit.to_owned()),
            unit_filtered: self.unit_filtered.map(str::to_owned),
        }
    }
}

/// What the manifests at the repository root say about the project.
#[derive(Debug, PartialEq, Eq)]
pub enum Detection {
    Found(&'static ProjectType),
    NoManifest,
    /// Manifests for more than one project type, so none is assumed.
    Several(Vec<&'static ProjectType>),
}

/// Detects the project type from which manifests `has_file` says are at the repository root.
pub fn detect_project_type(has_file: impl Fn(&str) -> bool) -> Detection {
    let mut found: Vec<&'static ProjectType> = PROJECT_TYPES
        .iter()
        .filter(|project_type| has_file(project_type.manifest))
        .collect();
    match found.len() {
        0 => Detection::NoManifest,
        1 => Detection::Found(found.remove(0)),
        _ => Detection::Several(found),
    }
}

pub fn detect_project_type_in(project_root: &Path) -> Detection {
    detect_project_type(|manifest| project_root.join(manifest).is_file())
}

impl fmt::Display for Detection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let describe = |project_type: &ProjectType| {
            format!("{} ({})", project_type.manifest, project_type.name)
        };
        match self {
            Self::Found(project_type) => write!(
                f,
                "Detected a {} project from {}.",
                project_type.name, project_type.manifest
            ),
            Self::NoManifest => {
                let known: Vec<String> = PROJECT_TYPES.iter().map(describe).collect();
                write!(
                    f,
                    "No manifest Arbor recognises is at the repository root. It looks for {}.",
                    known.join(", ")
                )
            }
            Self::Several(found) => {
                let found: Vec<String> = found.iter().map(|found| describe(found)).collect();
                write!(
                    f,
                    "The repository root has manifests for several project types: {}.",
                    found.join(", ")
                )
            }
        }
    }
}
