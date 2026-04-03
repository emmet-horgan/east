use std::collections::BTreeMap;

use serde::Deserialize;
use semver::Version;
use url::Url;

#[derive(Debug, Deserialize)]
pub struct Config {
    workspace: Workspace,
    modules: BTreeMap<String, Module>
}

impl Config {
    pub fn modules(&self) -> &BTreeMap<String, Module> {
        &self.modules
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
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
    rev: String
}

impl Module {
    pub fn git(&self) -> &Url {
        &self.git
    }

    pub fn rev(&self) -> &str {
        &self.rev
    }
}