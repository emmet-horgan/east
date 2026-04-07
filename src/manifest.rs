use std::collections::BTreeMap;
use url::Url;

/// A single unresolved dependency — the common IR that both
/// east.toml and west.yml lower into before resolution.
#[derive(Debug, Clone)]
pub struct UnresolvedDep {
    pub name: String,
    pub url: Url,
    pub revision: String,
    pub path: String,
    pub import: ImportSpec,
}

/// What (if anything) to import from this dependency's repo.
#[derive(Debug, Clone)]
pub enum ImportSpec {
    /// Don't import anything.
    None,
    /// Import a single file (e.g. "west.yml").
    File(String),
    /// Import multiple files.
    Files(Vec<String>),
    /// Import with filtering.
    Filtered {
        file: String,
        name_allowlist: Option<Vec<String>>,
        name_blocklist: Option<Vec<String>>,
        path_allowlist: Option<Vec<String>>,
        path_blocklist: Option<Vec<String>>,
        path_prefix: Option<String>,
    },
}

/// The common manifest representation after parsing but before resolution.
#[derive(Debug)]
pub struct UnresolvedManifest {
    /// The "self" repo URL and revision (needed to resolve self.import).
    pub self_url: Option<Url>,
    pub self_revision: Option<String>,
    pub self_import: ImportSpec,
    /// Direct dependencies declared in the manifest.
    pub deps: BTreeMap<String, UnresolvedDep>,
}

/// A fully resolved dependency (post-import-resolution), ready for lockfile.
#[derive(Debug, Clone)]
pub struct ResolvedDep {
    pub name: String,
    pub url: Url,
    pub revision: String,
    pub path: String,
}

/// The resolved manifest — a flat set of all transitive dependencies.
#[derive(Debug)]
pub struct ResolvedManifest {
    pub deps: BTreeMap<String, ResolvedDep>,
}
