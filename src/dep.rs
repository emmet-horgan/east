use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct Dep {
    name: String,
    import: ImportSpec,
    source: Source
}

// What (if anything) to import from this dependency's repo.
#[derive(Debug, Clone)]
pub enum ImportSpec {
    /// Don't import anything.
    None,
    /// Import a single file (e.g. "west.yml") or a directory of yaml files.
    Path(String),
    /// Import multiple files.
    Paths(Vec<String>),
    /// Import from a directory path.
    
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

#[derive(Debug, Clone)]
pub enum Source {
    Git(Git)
}

#[derive(Debug, Clone)]
pub struct Git {
    pub(crate) url: url::Url,
    pub(crate) rev: String
}

#[derive(Debug, Clone)]
pub struct Manifest {
    pub(crate) deps: BTreeMap<String, Dep>
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResolvedDep {
    pub(crate) name: String,
    pub(crate) source: ResolvedSource,
    pub(crate) deps: Vec<String>
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ResolvedSource {
    Git(ResolvedGit)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResolvedGit {
    pub(crate) url: url::Url,
    pub(crate) rev: String,
    pub(crate) commit: String
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResolvedManifest {
    pub(crate) deps: BTreeMap<String, ResolvedDep>
}