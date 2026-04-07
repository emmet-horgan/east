use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use url::Url;

use crate::manifest::*;

#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub url: Url,
    pub revision: String,
    pub path: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("fetch failed for {url} @ {rev}, path {path}: {reason}")]
    FetchFailed { url: String, rev: String, path: String, reason: String },
    #[error("parse error in {url} @ {rev}, path {path}: {reason}")]
    ParseError { url: String, rev: String, path: String, reason: String },
    #[error("cycle detected: {0}")]
    CycleDetected(String),
    #[error(transparent)]
    West(#[from] crate::west::WestError),
}

/// Abstracts fetching a file from a git repo at a revision.
pub trait ManifestFetcher {
    fn fetch(&self, request: &FetchRequest) -> Result<String, ResolveError>;
}

pub struct Resolver<F: ManifestFetcher> {
    fetcher: F,
    seen: HashSet<String>,
}

impl<F: ManifestFetcher> Resolver<F> {
    pub fn new(fetcher: F) -> Self {
        Self { fetcher, seen: HashSet::new() }
    }

    /// Resolve an `UnresolvedManifest` into a flat `ResolvedManifest`.
    pub fn resolve(&mut self, manifest: &UnresolvedManifest) -> Result<ResolvedManifest, ResolveError> {
        let mut resolved: BTreeMap<String, ResolvedDep> = BTreeMap::new();

        // 1. Resolve self.import (if any)
        if let (Some(url), Some(rev)) = (&manifest.self_url, &manifest.self_revision) {
            self.resolve_import(&manifest.self_import, url, rev, &mut resolved)?;
        }

        // 2. Resolve each direct dependency and its transitive imports
        for (name, dep) in &manifest.deps {
            if !resolved.contains_key(name) {
                resolved.insert(name.clone(), ResolvedDep {
                    name: dep.name.clone(),
                    url: dep.url.clone(),
                    revision: dep.revision.clone(),
                    path: dep.path.clone(),
                });
            }
            self.resolve_import(&dep.import, &dep.url, &dep.revision, &mut resolved)?;
        }

        Ok(ResolvedManifest { deps: resolved })
    }

    fn resolve_import(
        &mut self,
        import: &ImportSpec,
        url: &Url,
        rev: &str,
        resolved: &mut BTreeMap<String, ResolvedDep>,
    ) -> Result<(), ResolveError> {
        match import {
            ImportSpec::None => Ok(()),
            ImportSpec::File(path) => self.fetch_and_merge(url, rev, path, resolved),
            ImportSpec::Files(paths) => {
                for path in paths {
                    self.fetch_and_merge(url, rev, path, resolved)?;
                }
                Ok(())
            }
            ImportSpec::Filtered {
                file,
                name_allowlist,
                name_blocklist,
                path_allowlist,
                path_blocklist,
                path_prefix,
            } => {
                let content = self.fetch_file(url, rev, file)?;
                let west: crate::west::West = serde_yaml::from_str(&content)
                    .map_err(|e| ResolveError::ParseError {
                        url: url.to_string(), rev: rev.to_string(),
                        path: file.clone(), reason: e.to_string(),
                    })?;

                let sub = west.into_unresolved(None, None)
                    .map_err(ResolveError::West)?;

                for (name, dep) in &sub.deps {
                    if !passes_filter(
                        name, &dep.path,
                        name_allowlist.as_deref(),
                        name_blocklist.as_deref(),
                        path_allowlist.as_deref(),
                        path_blocklist.as_deref(),
                    ) {
                        continue;
                    }

                    let final_path = match path_prefix {
                        Some(pfx) => format!("{}/{}", pfx, dep.path),
                        None => dep.path.clone(),
                    };

                    if !resolved.contains_key(name) {
                        resolved.insert(name.clone(), ResolvedDep {
                            name: dep.name.clone(),
                            url: dep.url.clone(),
                            revision: dep.revision.clone(),
                            path: final_path,
                        });
                    }

                    self.resolve_import(&dep.import, &dep.url, &dep.revision, resolved)?;
                }
                Ok(())
            }
        }
    }

    fn fetch_file(&mut self, url: &Url, rev: &str, path: &str) -> Result<String, ResolveError> {
        let key = format!("{}@{}:{}", url, rev, path);
        if !self.seen.insert(key.clone()) {
            return Err(ResolveError::CycleDetected(key));
        }
        self.fetcher.fetch(&FetchRequest {
            url: url.clone(),
            revision: rev.to_string(),
            path: path.to_string(),
        })
    }

    fn fetch_and_merge(
        &mut self,
        url: &Url,
        rev: &str,
        path: &str,
        resolved: &mut BTreeMap<String, ResolvedDep>,
    ) -> Result<(), ResolveError> {
        let content = self.fetch_file(url, rev, path)?;
        let west: crate::west::West = serde_yaml::from_str(&content)
            .map_err(|e| ResolveError::ParseError {
                url: url.to_string(), rev: rev.to_string(),
                path: path.to_string(), reason: e.to_string(),
            })?;

        let sub = west.into_unresolved(None, None)
            .map_err(ResolveError::West)?;

        for (name, dep) in &sub.deps {
            if !resolved.contains_key(name) {
                resolved.insert(name.clone(), ResolvedDep {
                    name: dep.name.clone(),
                    url: dep.url.clone(),
                    revision: dep.revision.clone(),
                    path: dep.path.clone(),
                });
            }
            self.resolve_import(&dep.import, &dep.url, &dep.revision, resolved)?;
        }
        Ok(())
    }
}

fn passes_filter(
    name: &str,
    path: &str,
    name_allow: Option<&[String]>,
    name_block: Option<&[String]>,
    path_allow: Option<&[String]>,
    path_block: Option<&[String]>,
) -> bool {
    if let Some(allow) = name_allow {
        if !allow.iter().any(|a| a == name) { return false; }
    }
    if let Some(block) = name_block {
        if block.iter().any(|b| b == name) { return false; }
    }
    if let Some(allow) = path_allow {
        if !allow.iter().any(|a| a == path) { return false; }
    }
    if let Some(block) = path_block {
        if block.iter().any(|b| b == path) { return false; }
    }
    true
}
