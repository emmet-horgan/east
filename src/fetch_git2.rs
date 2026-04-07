use std::path::{Path, PathBuf};

use git2::Repository;

use crate::resolve::{FetchRequest, ManifestFetcher, ResolveError};

/// Fetches manifest files by cloning/opening bare repos into a cache directory.
pub struct Git2Fetcher {
    cache_dir: PathBuf,
}

impl Git2Fetcher {
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            cache_dir: cache_dir.into(),
        }
    }

    fn repo_cache_path(&self, url: &url::Url) -> PathBuf {
        // Turn URL into a filesystem-safe directory name
        let sanitized = url
            .to_string()
            .replace("://", "_")
            .replace('/', "_")
            .replace(':', "_");
        self.cache_dir.join(sanitized)
    }

    fn open_or_clone(&self, url: &url::Url) -> Result<Repository, ResolveError> {
        let path = self.repo_cache_path(url);

        if path.exists() {
            Repository::open_bare(&path).map_err(|e| ResolveError::FetchFailed {
                url: url.to_string(),
                rev: String::new(),
                path: path.display().to_string(),
                reason: format!("failed to open cached repo: {}", e),
            })
        } else {
            std::fs::create_dir_all(&path).map_err(|e| ResolveError::FetchFailed {
                url: url.to_string(),
                rev: String::new(),
                path: path.display().to_string(),
                reason: format!("failed to create cache dir: {}", e),
            })?;

            Repository::clone_recurse(url.as_str(), &path).map_err(|e| {
                // Clean up on failure
                let _ = std::fs::remove_dir_all(&path);
                ResolveError::FetchFailed {
                    url: url.to_string(),
                    rev: String::new(),
                    path: path.display().to_string(),
                    reason: format!("clone failed: {}", e),
                }
            })
        }
    }
}

impl ManifestFetcher for Git2Fetcher {
    fn fetch(&self, request: &FetchRequest) -> Result<String, ResolveError> {
        let repo = self.open_or_clone(&request.url)?;

        // Resolve the revision to an oid (could be a branch, tag, or sha)
        let obj = repo
            .revparse_single(&request.revision)
            .map_err(|e| ResolveError::FetchFailed {
                url: request.url.to_string(),
                rev: request.revision.clone(),
                path: request.path.clone(),
                reason: format!("cannot resolve rev '{}': {}", request.revision, e),
            })?;

        let commit = obj.peel_to_commit().map_err(|e| ResolveError::FetchFailed {
            url: request.url.to_string(),
            rev: request.revision.clone(),
            path: request.path.clone(),
            reason: format!("rev '{}' does not point to a commit: {}", request.revision, e),
        })?;

        let tree = commit.tree().map_err(|e| ResolveError::FetchFailed {
            url: request.url.to_string(),
            rev: request.revision.clone(),
            path: request.path.clone(),
            reason: format!("cannot get tree: {}", e),
        })?;

        let entry = tree
            .get_path(Path::new(&request.path))
            .map_err(|e| ResolveError::FetchFailed {
                url: request.url.to_string(),
                rev: request.revision.clone(),
                path: request.path.clone(),
                reason: format!("path '{}' not found in tree: {}", request.path, e),
            })?;

        let blob = repo
            .find_blob(entry.id())
            .map_err(|e| ResolveError::FetchFailed {
                url: request.url.to_string(),
                rev: request.revision.clone(),
                path: request.path.clone(),
                reason: format!("cannot read blob: {}", e),
            })?;

        let content = std::str::from_utf8(blob.content()).map_err(|e| ResolveError::FetchFailed {
            url: request.url.to_string(),
            rev: request.revision.clone(),
            path: request.path.clone(),
            reason: format!("file is not valid UTF-8: {}", e),
        })?;

        Ok(content.to_string())
    }
}
