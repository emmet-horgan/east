use serde::{Serialize, Deserialize};
use semver::Version;
use url::Url;

#[derive(Debug, Serialize, Deserialize)]
pub struct LockFile {
    version: Version,
    #[serde(rename = "module")]
    modules: Vec<LockedModule>
}

impl LockFile {
    pub fn new(version: impl Into<Version>, modules: impl Into<Vec<LockedModule>>) -> Self {
        let version = version.into();
        let modules = modules.into();
        Self {
            version,
            modules
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LockedModule {
    pub(crate) name: String,
    pub(crate) source: Source
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Source {
    Git(GitSource)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GitSource {
    pub(crate) url: Url,
    pub(crate) rev: String,
    pub(crate) commit: String,
    // Check this with custom check
    pub(crate) dependencies: Vec<String>
}