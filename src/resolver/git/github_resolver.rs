use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};

use crate::dep::{Git, ResolvedGit};

use super::{GitResolver, GitResolverError};

/// A [`GitResolver`] implementation backed by the GitHub REST API.
///
/// Avoids cloning for `resolve`, `fetch_file`, and `list_dir` operations,
/// making them significantly faster than git2 for GitHub-hosted repos.
///
/// Downloaded tarballs are cached as compressed `.tar.gz` files in `cache_dir`,
/// keyed by `{owner}_{repo}_{commit}.tar.gz`. Subsequent calls for the same
/// commit are served entirely from disk with no network traffic.
///
/// An optional personal access token extends the unauthenticated rate limit
/// (60 req/hr) to 5,000 req/hr and enables access to private repositories.
pub struct GitHubResolver {
    client: Client,
    token: Option<String>,
    cache_dir: PathBuf,
}

impl GitHubResolver {
    pub fn new(cache_dir: impl Into<PathBuf>, token: Option<String>) -> Self {
        Self {
            client: Client::new(),
            token,
            cache_dir: cache_dir.into(),
        }
    }

    /// Filesystem-safe tarball cache path for a given URL + commit.
    fn tarball_cache_path(&self, url: &url::Url, commit: &str) -> Result<PathBuf, GitResolverError> {
        let (owner, repo) = Self::parse_github_url(url)?;
        let filename = format!("{}_{}_{}.tar.gz", owner, repo, commit);
        Ok(self.cache_dir.join(filename))
    }

    /// Return the cached tarball path if it exists on disk.
    fn check_cache(&self, url: &url::Url, commit: &str) -> Option<PathBuf> {
        if let Ok(p) = self.tarball_cache_path(url, commit) {
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    /// Download the tarball for `source` and write the raw `.tar.gz` bytes to
    /// the cache directory. Returns the path of the cached file.
    fn download_to_cache(&self, source: &ResolvedGit) -> Result<PathBuf, GitResolverError> {
        let (owner, repo) = Self::parse_github_url(&source.url)?;
        let cache_path = self.tarball_cache_path(&source.url, &source.commit)?;

        // Ensure cache directory exists.
        std::fs::create_dir_all(&self.cache_dir)
            .map_err(|e| GitResolverError::HttpError(format!("create cache dir: {}", e)))?;

        let tarball_url = format!(
            "https://api.github.com/repos/{}/{}/tarball/{}",
            owner, repo, source.commit
        );

        // Build a one-off client with gzip decompression disabled so we
        // receive the raw `.tar.gz` bytes without reqwest trying to decode
        // the gzip transport layer on top.
        let raw_client = Client::builder()
            .no_gzip()
            .build()
            .map_err(|e| GitResolverError::HttpError(format!("build http client: {}", e)))?;

        let mut req = raw_client
            .get(&tarball_url)
            .header(USER_AGENT, "east")
            .header(ACCEPT, "application/vnd.github+json");

        if let Some(token) = &self.token {
            req = req.header(AUTHORIZATION, format!("Bearer {}", token));
        }

        let mut resp = req
            .send()
            .map_err(|e| GitResolverError::HttpError(e.to_string()))?;

        if let Some(err) = Self::check_rate_limit(&resp) {
            return Err(err);
        }

        if !resp.status().is_success() {
            return Err(GitResolverError::HttpError(format!(
                "GitHub tarball download returned {}",
                resp.status()
            )));
        }

        // Stream the response body directly to the cache file.
        let mut file = File::create(&cache_path)
            .map_err(|e| GitResolverError::HttpError(format!("create cache file: {}", e)))?;

        std::io::copy(&mut resp, &mut file)
            .map_err(|e| GitResolverError::HttpError(format!("writing cache file: {}", e)))?;

        Ok(cache_path)
    }

    /// Return the cached tarball path, downloading first if necessary.
    fn ensure_cached(&self, source: &ResolvedGit) -> Result<PathBuf, GitResolverError> {
        if let Some(p) = self.check_cache(&source.url, &source.commit) {
            return Ok(p);
        }
        self.download_to_cache(source)
    }

    /// Extract the full tarball into `dest`, stripping the top-level directory
    /// prefix that GitHub adds (e.g. `owner-repo-sha1234/`).
    fn extract_tarball(tarball: &Path, dest: &Path) -> Result<(), GitResolverError> {
        let file = File::open(tarball)
            .map_err(|e| GitResolverError::HttpError(format!("open cache file: {}", e)))?;
        let gz = flate2::read::GzDecoder::new(BufReader::new(file));
        let mut archive = tar::Archive::new(gz);

        std::fs::create_dir_all(dest)
            .map_err(|e| GitResolverError::HttpError(format!("create dir: {}", e)))?;

        for entry in archive
            .entries()
            .map_err(|e| GitResolverError::HttpError(format!("tar entries: {}", e)))?
        {
            let mut entry =
                entry.map_err(|e| GitResolverError::HttpError(format!("tar entry: {}", e)))?;

            let entry_path = entry
                .path()
                .map_err(|e| GitResolverError::HttpError(format!("tar path: {}", e)))?
                .into_owned();

            // Strip the top-level directory prefix.
            let mut components = entry_path.components();
            components.next();
            let stripped: PathBuf = components.collect();
            if stripped.as_os_str().is_empty() {
                continue;
            }

            let target = dest.join(&stripped);
            if entry.header().entry_type().is_dir() {
                std::fs::create_dir_all(&target)
                    .map_err(|e| GitResolverError::HttpError(format!("mkdir: {}", e)))?;
            } else {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| GitResolverError::HttpError(format!("mkdir: {}", e)))?;
                }
                entry
                    .unpack(&target)
                    .map_err(|e| GitResolverError::HttpError(format!("unpack: {}", e)))?;
            }
        }
        Ok(())
    }

    /// Read a single file from a cached tarball without extracting everything.
    fn read_file_from_tarball(tarball: &Path, rel_path: &str) -> Result<Option<String>, GitResolverError> {
        let file = File::open(tarball)
            .map_err(|e| GitResolverError::HttpError(format!("open cache file: {}", e)))?;
        let gz = flate2::read::GzDecoder::new(BufReader::new(file));
        let mut archive = tar::Archive::new(gz);

        let normalised = rel_path.trim_start_matches("./");

        for entry in archive
            .entries()
            .map_err(|e| GitResolverError::HttpError(format!("tar entries: {}", e)))?
        {
            let mut entry =
                entry.map_err(|e| GitResolverError::HttpError(format!("tar entry: {}", e)))?;

            let entry_path = entry
                .path()
                .map_err(|e| GitResolverError::HttpError(format!("tar path: {}", e)))?
                .into_owned();

            // Strip top-level dir
            let mut components = entry_path.components();
            components.next();
            let stripped: PathBuf = components.collect();

            if stripped == Path::new(normalised) && !entry.header().entry_type().is_dir() {
                let mut content = String::new();
                entry
                    .read_to_string(&mut content)
                    .map_err(|e| GitResolverError::HttpError(format!("read entry: {}", e)))?;
                return Ok(Some(content));
            }
        }
        Ok(None)
    }

    /// List immediate children of a directory inside a cached tarball.
    fn list_dir_from_tarball(tarball: &Path, rel_path: &str) -> Result<Option<Vec<String>>, GitResolverError> {
        let file = File::open(tarball)
            .map_err(|e| GitResolverError::HttpError(format!("open cache file: {}", e)))?;
        let gz = flate2::read::GzDecoder::new(BufReader::new(file));
        let mut archive = tar::Archive::new(gz);

        let normalised = rel_path.trim_start_matches("./");
        let prefix = if normalised.is_empty() || normalised.ends_with('/') {
            normalised.to_string()
        } else {
            format!("{}/", normalised)
        };

        let mut names = std::collections::BTreeSet::new();
        let mut found_any = false;

        for entry in archive
            .entries()
            .map_err(|e| GitResolverError::HttpError(format!("tar entries: {}", e)))?
        {
            let entry =
                entry.map_err(|e| GitResolverError::HttpError(format!("tar entry: {}", e)))?;

            let entry_path = entry
                .path()
                .map_err(|e| GitResolverError::HttpError(format!("tar path: {}", e)))?
                .into_owned();

            // Strip top-level dir
            let mut components = entry_path.components();
            components.next();
            let stripped: PathBuf = components.collect();
            let stripped_str = stripped.to_string_lossy();

            if let Some(remainder) = stripped_str.strip_prefix(&prefix) {
                if remainder.is_empty() {
                    // This is the directory entry itself — confirms it exists.
                    found_any = true;
                    continue;
                }
                found_any = true;
                // Take only the first path component after the prefix (immediate child).
                if let Some(name) = remainder.split('/').next() {
                    if !name.is_empty() {
                        names.insert(name.to_string());
                    }
                }
            }
        }

        if found_any {
            Ok(Some(names.into_iter().collect()))
        } else {
            Ok(None)
        }
    }

    /// Extract `(owner, repo)` from a `github.com` URL.
    ///
    /// Accepts URLs like `https://github.com/owner/repo[.git]`.
    fn parse_github_url(url: &url::Url) -> Result<(String, String), GitResolverError> {
        let host = url.host_str().unwrap_or("");
        if host != "github.com" {
            return Err(GitResolverError::HttpError(format!(
                "not a GitHub URL: {}",
                url
            )));
        }
        let segments: Vec<&str> = url.path().trim_matches('/').split('/').collect();
        if segments.len() < 2 {
            return Err(GitResolverError::HttpError(format!(
                "cannot parse owner/repo from URL: {}",
                url
            )));
        }
        let owner = segments[0].to_string();
        let repo = segments[1].trim_end_matches(".git").to_string();
        Ok((owner, repo))
    }

    /// Build a GET request with standard headers (user-agent, auth).
    fn request(&self, url: &str) -> reqwest::blocking::RequestBuilder {
        let mut req = self.client.get(url).header(USER_AGENT, "east");
        if let Some(token) = &self.token {
            req = req.header(AUTHORIZATION, format!("Bearer {}", token));
        }
        req
    }

    /// Inspect a response for GitHub rate-limit signals.
    ///
    /// GitHub returns 429 (Too Many Requests) for secondary rate limits and
    /// 403 with `x-ratelimit-remaining: 0` for primary rate limits.
    /// Returns `Some(RateLimited)` when either is detected.
    fn check_rate_limit(resp: &reqwest::blocking::Response) -> Option<GitResolverError> {
        let status = resp.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(60);
            return Some(GitResolverError::RateLimited {
                host: "github.com".into(),
                retry_after_secs: retry_after,
            });
        }

        if status == reqwest::StatusCode::FORBIDDEN {
            let remaining = resp
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());

            if remaining == Some(0) {
                let retry_after = resp
                    .headers()
                    .get("x-ratelimit-reset")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .and_then(|reset| {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .ok()?
                            .as_secs();
                        Some(reset.saturating_sub(now))
                    })
                    .unwrap_or(60);
                return Some(GitResolverError::RateLimited {
                    host: "github.com".into(),
                    retry_after_secs: retry_after,
                });
            }
        }

        None
    }

    /// Fetch a single file via the GitHub Contents API.
    fn fetch_file_api(
        &self,
        source: &ResolvedGit,
        rel_path: &str,
    ) -> Result<String, GitResolverError> {
        let (owner, repo) = Self::parse_github_url(&source.url)?;
        let path = rel_path.trim_start_matches("./");

        // With the raw media type the response body IS the file content.
        let api_url = format!(
            "https://api.github.com/repos/{}/{}/contents/{}?ref={}",
            owner, repo, path, source.commit
        );

        let resp = self
            .request(&api_url)
            .header(ACCEPT, "application/vnd.github.raw+json")
            .send()
            .map_err(|e| GitResolverError::HttpError(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(GitResolverError::MissingFileError {
                repo: source.url.clone(),
                rel_path: rel_path.to_string(),
            });
        }

        if let Some(err) = Self::check_rate_limit(&resp) {
            return Err(err);
        }

        if !resp.status().is_success() {
            return Err(GitResolverError::HttpError(format!(
                "GitHub API returned {} for {}",
                resp.status(),
                api_url
            )));
        }

        resp.text()
            .map_err(|e| GitResolverError::HttpError(e.to_string()))
    }

    /// List a directory via the GitHub Contents API.
    ///
    /// Note: directories with more than 1,000 entries require the Git Trees API
    /// instead; this is not currently handled.
    fn list_dir_api(
        &self,
        source: &ResolvedGit,
        rel_path: &str,
    ) -> Result<Vec<String>, GitResolverError> {
        let (owner, repo) = Self::parse_github_url(&source.url)?;
        let path = rel_path.trim_start_matches("./");

        let api_url = format!(
            "https://api.github.com/repos/{}/{}/contents/{}?ref={}",
            owner, repo, path, source.commit
        );

        let resp = self
            .request(&api_url)
            .header(ACCEPT, "application/vnd.github+json")
            .send()
            .map_err(|e| GitResolverError::HttpError(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(GitResolverError::MissingDirectoryError {
                repo: source.url.clone(),
                rel_path: rel_path.to_string(),
            });
        }

        if let Some(err) = Self::check_rate_limit(&resp) {
            return Err(err);
        }

        if !resp.status().is_success() {
            return Err(GitResolverError::HttpError(format!(
                "GitHub API returned {} for {}",
                resp.status(),
                api_url
            )));
        }

        let json: Vec<serde_json::Value> = resp
            .json()
            .map_err(|e| GitResolverError::HttpError(e.to_string()))?;

        let names = json
            .iter()
            .filter_map(|entry| entry["name"].as_str().map(|s| s.to_string()))
            .collect();

        Ok(names)
    }
}

impl GitResolver for GitHubResolver {
    fn resolve(&mut self, source: &Git) -> Result<ResolvedGit, GitResolverError> {
        // If the rev is already a 40-char hex SHA, return directly.
        if source.rev.len() == 40 && source.rev.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(ResolvedGit {
                url: source.url.clone(),
                rev: source.rev.clone(),
                commit: source.rev.clone(),
            });
        }

        let (owner, repo) = Self::parse_github_url(&source.url)?;

        // GET /repos/{owner}/{repo}/commits/{ref} resolves branches, tags, and SHAs.
        let api_url = format!(
            "https://api.github.com/repos/{}/{}/commits/{}",
            owner, repo, source.rev
        );

        let resp = self
            .request(&api_url)
            .header(ACCEPT, "application/vnd.github+json")
            .send()
            .map_err(|e| GitResolverError::HttpError(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND
            || resp.status() == reqwest::StatusCode::UNPROCESSABLE_ENTITY
        {
            return Err(GitResolverError::InvalidRevError {
                repo: source.url.clone(),
                rev: source.rev.clone(),
            });
        }

        if let Some(err) = Self::check_rate_limit(&resp) {
            return Err(err);
        }

        if !resp.status().is_success() {
            return Err(GitResolverError::HttpError(format!(
                "GitHub API returned {} for {}",
                resp.status(),
                api_url
            )));
        }

        let json: serde_json::Value = resp
            .json()
            .map_err(|e| GitResolverError::HttpError(e.to_string()))?;

        let sha = json["sha"].as_str().ok_or_else(|| {
            GitResolverError::InvalidRevError {
                repo: source.url.clone(),
                rev: source.rev.clone(),
            }
        })?;

        Ok(ResolvedGit {
            url: source.url.clone(),
            rev: source.rev.clone(),
            commit: sha.to_string(),
        })
    }

    /// Download the repo tarball (or use the cached copy) and extract it to `path`.
    fn fetch(&mut self, source: &ResolvedGit, path: &Path) -> Result<(), GitResolverError> {
        let tarball = self.ensure_cached(source)?;
        Self::extract_tarball(&tarball, path)
    }

    /// Fetch a single file's contents.
    ///
    /// If the tarball is already cached, reads directly from it (no network).
    /// Otherwise hits the GitHub Contents API for just that one file — much
    /// faster than downloading the entire tarball during resolution.
    fn fetch_file(
        &mut self,
        source: &ResolvedGit,
        rel_path: String,
    ) -> Result<String, GitResolverError> {
        // Fast path: tarball already on disk from a prior `fetch()`.
        if let Some(tarball) = self.check_cache(&source.url, &source.commit) {
            return match Self::read_file_from_tarball(&tarball, &rel_path)? {
                Some(content) => Ok(content),
                None => Err(GitResolverError::MissingFileError {
                    repo: source.url.clone(),
                    rel_path,
                }),
            };
        }

        // No cached tarball — use the Contents API (single HTTP call).
        self.fetch_file_api(source, &rel_path)
    }

    /// List the entries in a directory.
    ///
    /// If the tarball is already cached, reads directly from it (no network).
    /// Otherwise hits the GitHub Contents API for just that listing.
    fn list_dir(
        &mut self,
        source: &ResolvedGit,
        rel_path: String,
    ) -> Result<Vec<String>, GitResolverError> {
        // Fast path: tarball already on disk from a prior `fetch()`.
        if let Some(tarball) = self.check_cache(&source.url, &source.commit) {
            return match Self::list_dir_from_tarball(&tarball, &rel_path)? {
                Some(entries) => Ok(entries),
                None => Err(GitResolverError::MissingDirectoryError {
                    repo: source.url.clone(),
                    rel_path,
                }),
            };
        }

        // No cached tarball — use the Contents API (single HTTP call).
        self.list_dir_api(source, &rel_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dep::Git;

    fn test_resolver() -> GitHubResolver {
        let cache = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".test_github_cache");
        GitHubResolver::new(cache, None)
    }

    #[test]
    fn resolve_branch() {
        let mut resolver = test_resolver();
        let git = Git {
            url: url::Url::parse("https://github.com/zephyrproject-rtos/zephyr.git").unwrap(),
            rev: "main".to_string(),
        };
        let resolved = resolver.resolve(&git).unwrap();
        assert_eq!(resolved.rev, "main");
        assert_eq!(resolved.commit.len(), 40);
    }

    #[test]
    fn resolve_sha_passthrough() {
        let mut resolver = test_resolver();
        let sha = "c1f2a9b6b04509953ccab078225d48477ce2a808";
        let git = Git {
            url: url::Url::parse("https://github.com/zephyrproject-rtos/zephyr.git").unwrap(),
            rev: sha.to_string(),
        };
        let resolved = resolver.resolve(&git).unwrap();
        assert_eq!(resolved.commit, sha);
    }

    #[test]
    fn fetch_file_west_yml() {
        let mut resolver = test_resolver();
        let source = ResolvedGit {
            url: url::Url::parse("https://github.com/zephyrproject-rtos/zephyr.git").unwrap(),
            rev: "main".to_string(),
            commit: "c1f2a9b6b04509953ccab078225d48477ce2a808".to_string(),
        };
        let content = resolver
            .fetch_file(&source, "./west.yml".to_string())
            .unwrap();
        assert!(content.contains("manifest:"));
        println!("{}", &content);
    }

    #[test]
    fn list_dir_scripts() {
        let mut resolver = test_resolver();
        let source = ResolvedGit {
            url: url::Url::parse("https://github.com/zephyrproject-rtos/zephyr.git").unwrap(),
            rev: "main".to_string(),
            commit: "c1f2a9b6b04509953ccab078225d48477ce2a808".to_string(),
        };
        let entries = resolver
            .list_dir(&source, "./scripts".to_string())
            .unwrap();
        assert!(!entries.is_empty());
        assert!(entries.contains(&"west-commands.yml".to_string()));
        println!("{:#?}", &entries);
    }

    #[test]
    fn fetch_file_cache_hit() {
        let cache = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".test_github_cache");
        // Ensure no cached tarball to start — fetch_file should use the API.
        let _ = std::fs::remove_dir_all(&cache);

        let mut resolver = GitHubResolver::new(&cache, None);
        let source = ResolvedGit {
            url: url::Url::parse("https://github.com/zephyrproject-rtos/zephyr.git").unwrap(),
            rev: "main".to_string(),
            commit: "c1f2a9b6b04509953ccab078225d48477ce2a808".to_string(),
        };

        // No tarball cached — this goes via the Contents API (fast).
        assert!(resolver.check_cache(&source.url, &source.commit).is_none());
        let content_api = resolver
            .fetch_file(&source, "./west.yml".to_string())
            .unwrap();
        // Still no tarball — individual file fetches don't cache tarballs.
        assert!(resolver.check_cache(&source.url, &source.commit).is_none());

        // Now do a full fetch to populate the tarball cache.
        let extract_dir = cache.join("_extract_test");
        let _ = std::fs::remove_dir_all(&extract_dir);
        resolver.fetch(&source, &extract_dir).unwrap();
        assert!(resolver.check_cache(&source.url, &source.commit).is_some());

        // This call should now read from the cached tarball (no network).
        let content_cached = resolver
            .fetch_file(&source, "./west.yml".to_string())
            .unwrap();
        assert_eq!(content_api, content_cached);

        // Clean up extracted dir (leave tarball for other tests).
        let _ = std::fs::remove_dir_all(&extract_dir);
    }

    //#[test]
    //fn full_fetch() {
    //    // 78dcc5e7ce43f107c7e68c4a3efac4489f3c4806
    //    let cache = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".test");
    //    let mut resolver = GitHubResolver::new(&cache, None);
//
    //    let source = ResolvedGit {
    //        url: url::Url::parse("https://github.com/zephyrproject-rtos/zephyr.git").unwrap(),
    //        rev: "78dcc5e7ce43f107c7e68c4a3efac4489f3c4806".to_string(),
    //        commit: "78dcc5e7ce43f107c7e68c4a3efac4489f3c4806".to_string(),
    //    };
    //    assert!(resolver.fetch(&source, &PathBuf::from(".")).is_ok())
    //}
}
