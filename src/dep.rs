use std::collections::BTreeMap;

/// An unresolved dependency: a named source with an optional import spec
/// indicating sub-manifests to pull in transitively.
#[derive(Debug, Clone)]
pub struct Dep {
    pub(crate) name: String,
    /// On-disk path for this dependency (e.g. "modules/hal/adi").
    pub(crate) path: String,
    /// What (if anything) to import from this dep's repo.
    pub(crate) import: ImportSpec,
    pub(crate) source: Source,
}

impl Dep {
    pub fn new(name: impl Into<String>, path: impl Into<String>, source: Source) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            import: ImportSpec::None,
            source,
        }
    }

    pub fn with_import(mut self, import: ImportSpec) -> Self {
        self.import = import;
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn import(&self) -> &ImportSpec {
        &self.import
    }

    pub fn source(&self) -> &Source {
        &self.source
    }
}

/// What (if anything) to import from a dependency's repository.
#[derive(Debug, Clone)]
pub enum ImportSpec {
    /// Don't import anything.
    None,
    /// Import a single file (e.g. "west.yml").
    Path(String),
    /// Import multiple files.
    Paths(Vec<String>),
    /// Import with filtering.
    Filtered {
        path: String,
        name_allowlist: Option<Vec<String>>,
        name_blocklist: Option<Vec<String>>,
        path_allowlist: Option<Vec<String>>,
        path_blocklist: Option<Vec<String>>,
        path_prefix: Option<String>,
    },
}

impl Default for ImportSpec {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone)]
pub enum Source {
    Git(Git),
}

#[derive(Debug, Clone)]
pub struct Git {
    pub(crate) url: url::Url,
    pub(crate) rev: String,
}

impl Git {
    pub fn new(url: url::Url, rev: impl Into<String>) -> Self {
        Self { url, rev: rev.into() }
    }

    pub fn url(&self) -> &url::Url {
        &self.url
    }

    pub fn rev(&self) -> &str {
        &self.rev
    }
}

/// The unresolved manifest IR that both config.rs (east.toml) and
/// west.rs (west.yml) lower into.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// Optional "self" dependency representing the manifest repo itself.
    /// Used when the manifest repo's own sub-manifests need importing
    /// (e.g. `manifest.self.import: submanifests` in west.yml).
    pub(crate) self_dep: Option<Dep>,
    /// Named dependencies to resolve.
    pub(crate) deps: BTreeMap<String, Dep>,
}

impl Manifest {
    pub fn new(deps: BTreeMap<String, Dep>) -> Self {
        Self { self_dep: None, deps }
    }

    pub fn with_self_dep(mut self, dep: Dep) -> Self {
        self.self_dep = Some(dep);
        self
    }

    pub fn deps(&self) -> &BTreeMap<String, Dep> {
        &self.deps
    }

    pub fn self_dep(&self) -> Option<&Dep> {
        self.self_dep.as_ref()
    }
}

// ── Resolved (output) types ───────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResolvedDep {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) source: ResolvedSource,
    /// Names of dependencies that were imported from this dep's repo.
    pub(crate) deps: Vec<String>,
}

impl ResolvedDep {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn source(&self) -> &ResolvedSource {
        &self.source
    }

    pub fn deps(&self) -> &[String] {
        &self.deps
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ResolvedSource {
    Git(ResolvedGit),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResolvedGit {
    pub(crate) url: url::Url,
    pub(crate) rev: String,
    pub(crate) commit: String,
}

impl ResolvedGit {
    pub fn url(&self) -> &url::Url {
        &self.url
    }

    pub fn rev(&self) -> &str {
        &self.rev
    }

    pub fn commit(&self) -> &str {
        &self.commit
    }
}

/// Fully resolved, flat dependency graph.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResolvedManifest {
    pub(crate) deps: BTreeMap<String, ResolvedDep>,
}

impl ResolvedManifest {
    pub fn deps(&self) -> &BTreeMap<String, ResolvedDep> {
        &self.deps
    }

    pub fn get(&self, name: &str) -> Option<&ResolvedDep> {
        self.deps.get(name)
    }

    pub fn len(&self) -> usize {
        self.deps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.deps.is_empty()
    }
}
