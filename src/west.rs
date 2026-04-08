use serde::Deserialize;
use url::Url;

use crate::config::{
    Config,
    Module,
    Workspace
};

#[derive(Debug, Deserialize)]
pub struct Remote {
    pub(crate) name: String,
    #[serde(rename = "url-base")]
    pub(crate) url_base: Url 
}

#[derive(Debug, Deserialize)]
pub struct Defaults {
    #[serde(default)]
    pub(crate) remote: Option<String>,
    #[serde(default = "default_revision")]
    pub(crate) revision: String
}

fn default_revision() -> String {
    "master".to_string()
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            remote: None,
            revision: "master".to_string()
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Import {
    Bool(bool),
    RelPath(String),
    Mapping {
        #[serde(default = "default_mapping_file")]
        file: String,
        #[serde(rename = "name-allowlist")]
        #[serde(default)]
        name_allow_list: Option<OneOrSeq<String>>,
        #[serde(rename = "path-allowlist")]
        #[serde(default)]
        path_allow_list: Option<OneOrSeq<String>>,
        #[serde(rename = "name-blocklist")]
        #[serde(default)]
        name_block_list: Option<OneOrSeq<String>>,
        #[serde(rename = "path-blocklist")]
        #[serde(default)]
        path_block_list: Option<OneOrSeq<String>>,
        #[serde(rename = "path-prefix")]
        #[serde(default)]
        path_prefix: Option<String>,
    },
    Seq(Vec<String>)
}

impl Default for Import {
    fn default() -> Self {
        Self::Bool(false)
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum OneOrSeq<T> {
    One(T),
    Seq(Vec<T>),
}

fn default_mapping_file() -> String {
    "west.yml".to_string()
}

#[derive(Debug, Deserialize)]
pub struct Project {
    pub(crate) name: String,

    #[serde(default)]
    pub(crate) remote: Option<String>,

    #[serde(rename = "repo-path")]
    #[serde(default)]
    pub(crate) repo_path: Option<String>,

    #[serde(default)]
    pub(crate) url: Option<Url>,

    #[serde(default)]
    pub(crate) path: Option<String>,

    #[serde(default)]
    pub(crate) revision: Option<String>,

    #[serde(default)]
    pub(crate) import: Import,

    #[serde(default)]
    pub(crate) groups: Vec<String>,

    #[serde(rename = "west-commands")]
    #[serde(default)]
    pub(crate) west_commands: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ManifestSelf {
    #[serde(default)]
    pub(crate) path: Option<String>,

    #[serde(default)]
    pub(crate) import: Import,

    #[serde(rename = "west-commands")]
    #[serde(default)]
    pub(crate) west_commands: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub(crate) defaults: Defaults,

    #[serde(default)]
    pub(crate) remotes: Vec<Remote>,

    #[serde(default)]
    pub(crate) projects: Vec<Project>,

    #[serde(rename = "self")]
    #[serde(default)]
    pub(crate) manifest_self: ManifestSelf,

    #[serde(rename = "group-filter")]
    #[serde(default)]
    pub(crate) group_filter: Vec<String>,
}

impl Manifest {
    pub fn remote(&self, name: &str) -> Option<&Remote> {
        self.remotes.iter().find(|r| r.name.as_str() == name)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WestError {
    #[error("invalid west project '{}': {}", .prj, .msg)]
    InvalidPrjError{
        prj: String,
        msg: String
    },
    #[error("default remote '{0}' does not exist")]
    DefaultRemoteError(String),
    #[error("unable to join '{}' onto url '{}'", .joinee, .url)]
    BadUrlJoin {
        url: String,
        joinee: String
    },
    #[error("multiple projects with the name '{}' were found", .prj)]
    DuplicatePrjError {
        prj: String
    }
}

#[derive(Debug, Deserialize)]
pub struct West {
    manifest: Manifest
}

impl West {
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn validate(self) -> Result<Config, WestError> {
        use std::collections::BTreeMap;
        // Check that self.import does not import itself
        // [x] Check that all projects have a valid remote
        // Check that all remotes are uniquely named
        // [x] Check that all projects are uniquely named

        let workspace = Workspace::new(
            "",
            semver::Version::parse("0.0.0+east").unwrap()
        );
        let mut modules = BTreeMap::new();

        for prj in &self.manifest.projects {
            let default_remote = self.manifest.defaults.remote.as_ref();

            let url = match (&prj.url, &prj.remote, default_remote) {
                (Some(l), None, _) => l.clone(),
                (None, Some(r), _) | (None, None, Some(r)) => {
                    let remote = self.manifest.remote(r.as_str())
                        .ok_or(WestError::DefaultRemoteError(r.to_string()))?;
                    let url = remote.url_base.clone();
                    if let Some(p) = prj.repo_path.as_ref() {
                        url.join(p)
                            .map_err(|_| WestError::BadUrlJoin { url: url.to_string(), joinee: p.clone() })?
                    } else {
                        url.join(&prj.name)
                            .map_err(|_| WestError::BadUrlJoin { url: url.to_string(), joinee: prj.name.clone() })?
                    }
                },
                (Some(_), Some(_), _) => return Err(
                    WestError::InvalidPrjError { 
                        prj: prj.name.clone(), 
                        msg: "cannot have a url and a remote".to_string()  
                    }
                ),
                (None, None, None) => return Err(
                    WestError::InvalidPrjError { 
                        prj: prj.name.clone(), 
                        msg: "a url or remote must be specified".to_string()  
                    }
                ),
            };
            let rev = match prj.revision.as_ref()  {
                Some(r) => r.clone(),
                None => self.manifest.defaults.revision.clone()

            };
            // TODO: fix import
            if let Some(_) = modules.insert(prj.name.clone(), Module::new(url, rev)) {
                return Err(WestError::DuplicatePrjError { prj: prj.name.clone() })
            }
        }

        Ok(Config { workspace, modules })
    }

//    /// Lower a parsed west.yml into the common `UnresolvedManifest` IR.
//    /// `self_url` and `self_rev` identify the repo containing this west.yml
//    /// (needed for resolving `self: import:`).
//    pub fn into_unresolved(
//        &self,
//        self_url: Option<Url>,
//        self_rev: Option<String>,
//    ) -> Result<UnresolvedManifest, WestError> {
//        let manifest = self.manifest();
//        let mut deps = std::collections::BTreeMap::new();
//
//        for prj in &manifest.projects {
//            let default_remote = manifest.defaults.remote.as_ref();
//
//            let url = match (&prj.url, &prj.remote, default_remote) {
//                (Some(u), None, _) => u.clone(),
//                (None, Some(r), _) | (None, None, Some(r)) => {
//                    let remote = manifest
//                        .remote(r.as_str())
//                        .ok_or(WestError::DefaultRemoteError(r.to_string()))?;
//                    let base = &remote.url_base;
//                    let repo = prj.repo_path.as_deref().unwrap_or(&prj.name);
//                    base.join(repo).map_err(|_| WestError::BadUrlJoin {
//                        url: base.to_string(),
//                        joinee: repo.to_string(),
//                    })?
//                }
//                (Some(_), Some(_), _) => {
//                    return Err(WestError::InvalidPrjError {
//                        prj: prj.name.clone(),
//                        msg: "cannot have both url and remote".into(),
//                    })
//                }
//                (None, None, None) => {
//                    return Err(WestError::InvalidPrjError {
//                        prj: prj.name.clone(),
//                        msg: "no url or remote and no default remote".into(),
//                    })
//                }
//            };
//
//            let revision = prj
//                .revision
//                .clone()
//                .unwrap_or_else(|| manifest.defaults.revision.clone());
//
//            let path = prj.path.clone().unwrap_or_else(|| prj.name.clone());
//            let import = import_to_spec(&prj.import);
//
//            deps.insert(
//                prj.name.clone(),
//                UnresolvedDep { name: prj.name.clone(), url, revision, path, import },
//            );
//        }
//
//        let self_import = import_to_spec(&manifest.manifest_self.import);
//
//        Ok(UnresolvedManifest {
//            self_url,
//            self_revision: self_rev,
//            self_import,
//            deps,
//        })
//    }
}

fn one_or_seq_to_vec(o: &OneOrSeq<String>) -> Vec<String> {
    match o {
        OneOrSeq::One(s) => vec![s.clone()],
        OneOrSeq::Seq(v) => v.clone(),
    }
}

//fn import_to_spec(import: &Import) -> ImportSpec {
//    match import {
//        Import::Bool(false) => ImportSpec::None,
//        Import::Bool(true) => ImportSpec::File("west.yml".into()),
//        Import::RelPath(p) => ImportSpec::File(p.clone()),
//        Import::Seq(paths) => ImportSpec::Files(paths.clone()),
//        Import::Mapping {
//            file,
//            name_allow_list,
//            name_block_list,
//            path_allow_list,
//            path_block_list,
//            path_prefix,
//        } => ImportSpec::Filtered {
//            file: file.clone(),
//            name_allowlist: name_allow_list.as_ref().map(one_or_seq_to_vec),
//            name_blocklist: name_block_list.as_ref().map(one_or_seq_to_vec),
//            path_allowlist: path_allow_list.as_ref().map(one_or_seq_to_vec),
//            path_blocklist: path_block_list.as_ref().map(one_or_seq_to_vec),
//            path_prefix: path_prefix.clone(),
//        },
//    }
//}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_zephyr_west_yml() -> &'static str {
        include_str!("../tests/zephyr.west.yml")
    }

    #[test]
    fn zephyr_west_yml_parse() {
        let test_cfg = get_zephyr_west_yml();

        let _parsed: West = serde_yaml::from_str(&test_cfg)
            .expect("parsing failed");

        println!("{:#?}", _parsed);
    }
}