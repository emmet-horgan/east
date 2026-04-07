use std::collections::BTreeMap;

use serde::Deserialize;
use semver::Version;
use url::Url;

use crate::manifest::{UnresolvedManifest, UnresolvedDep, ImportSpec};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub(crate) workspace: Workspace,
    pub(crate) modules: BTreeMap<String, Module>
}

impl Config {
    pub fn modules(&self) -> &BTreeMap<String, Module> {
        &self.modules
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// Lower an east.toml into the common `UnresolvedManifest` IR.
    pub fn into_unresolved(&self) -> UnresolvedManifest {
        let deps = self
            .modules
            .iter()
            .map(|(name, m)| {
                let dep = UnresolvedDep {
                    name: name.clone(),
                    url: m.git.clone(),
                    revision: m.rev.clone(),
                    path: name.clone(),
                    import: ImportSpec::None, // east.toml is already flat
                };
                (name.clone(), dep)
            })
            .collect();

        UnresolvedManifest {
            self_url: None,
            self_revision: None,
            self_import: ImportSpec::None,
            deps,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Workspace {
    name: String,
    version: Version
}

impl Workspace {

    pub fn new(name: impl Into<String>, version: impl Into<Version>) -> Self {
        let name = name.into();
        let version = version.into();

        Self { name, version }
    }

    pub fn version(&self) -> &Version {
        &self.version
    }

    pub fn name(&self) -> &str {
        self.name.as_str()
    }
}

#[derive(Debug, Deserialize)]
pub struct Module {
    git: Url,
    rev: String,
    import: ImportSpec
}

impl Module {
    pub fn git(&self) -> &Url {
        &self.git
    }

    pub fn rev(&self) -> &str {
        &self.rev
    }

    pub fn new(git: Url, rev: String, import: ImportSpec) -> Self {
        Self { git, rev, import }
    }
}