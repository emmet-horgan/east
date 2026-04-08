use std::path::Path;

use crate::dep::{Git, ResolvedGit};

pub mod git2_resolver;
pub mod github_resolver;

#[derive(Debug, thiserror::Error)]
pub enum GitResolverError {
    #[error("failed to clone repository '{repo}'")]
    CloneError {
        repo: url::Url
    },
    #[error(transparent)]
    Git2Error(#[from] git2::Error),
    #[error("{0}")]
    HttpError(String),
    #[error("rate limited by {host} (retry after {retry_after_secs}s)")]
    RateLimited {
        host: String,
        retry_after_secs: u64,
    },
    #[error("could not find file '{rel_path}' in '{repo}'")]
    MissingFileError {
        repo: url::Url,
        rel_path: String
    },
    #[error("could not find directory '{rel_path}' in '{repo}'")]
    MissingDirectoryError {
        repo: url::Url,
        rel_path: String
    },
    #[error("could not find reference '{rev}' in '{repo}'")]
    InvalidRevError {
        repo: url::Url,
        rev: String
    }
}

impl GitResolverError {
    /// Returns `true` if this error indicates a rate limit was hit,
    /// signalling the caller should fall back to an alternative resolver.
    pub fn is_rate_limited(&self) -> bool {
        matches!(self, GitResolverError::RateLimited { .. })
    }
}


pub(crate) fn cached_name_resolver(url: &url::Url, oid: &git2::Oid) -> String {
    let host = url.host_str().unwrap_or("");
    let repo_dir_name = format!(
        "{}_{}_{}", 
        host, 
        url.path().replace('/', "_"),
        &oid.to_string()
    );
    repo_dir_name
}

pub trait GitResolver {
    fn resolve(&mut self, source: &Git) -> Result<ResolvedGit, GitResolverError>;
    fn fetch(&mut self, source: &ResolvedGit, path: &Path) -> Result<(), GitResolverError>;
    fn fetch_file(&mut self, source: &ResolvedGit, rel_path: String) -> Result<String, GitResolverError>;
    fn list_dir(&mut self, source: &ResolvedGit, rel_path: String) -> Result<Vec<String>, GitResolverError>;
}