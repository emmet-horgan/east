use std::path::{Path, PathBuf};

use crate::dep::{Git, ResolvedGit};

#[derive(Debug, thiserror::Error)]
pub enum GitResolverError {
    #[error("failed to clone repository '{repo}'")]
    CloneError {
        repo: url::Url
    },
    #[error(transparent)]
    Git2Error(#[from] git2::Error),
    #[error("could not find file '{rel_path}' in '{repo}'")]
    MissingFileError {
        repo: url::Url,
        rel_path: String
    },
    #[error("could not find directory '{rel_path}' in '{repo}'")]
    MissingDirectoryError {
        repo: url::Url,
        rel_path: String
    }
}
pub trait GitResolver {
    fn resolve(&mut self, source: &Git) -> Result<ResolvedGit, GitResolverError>;
    fn fetch(&mut self, source: &ResolvedGit, path: &Path) -> Result<(), GitResolverError>;
    fn fetch_file(&mut self, source: &ResolvedGit, rel_path: String) -> Result<String, GitResolverError>;
    fn list_dir(&mut self, source: &ResolvedGit, rel_path: String) -> Result<Vec<String>, GitResolverError>;
}

#[derive(Debug, Clone)]
pub struct Git2Resolver {
    cache_dir: PathBuf,
    cache: std::collections::BTreeMap<url::Url, String>
}

impl Git2Resolver {
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self { cache_dir: cache_dir.into(), cache: Default::default() }
    }

    pub fn repo_dir_name(url: &url::Url, rev: &str) -> String {
        let host = url.host_str().unwrap_or("");
        let repo_dir_name = format!(
            "{}_{}_{}", 
            host, 
            url.path().replace('/', "_"),
            rev
        );
        repo_dir_name
    }
}

impl GitResolver for Git2Resolver {
    /// The only way to resolve with the git2 resolver implementation is to actually
    /// fetch the repo contents and check the sha at the requested revision. This is
    /// what we would hope to do for the fetch implementation but there is no other
    /// option
    fn resolve(&mut self, source: &Git) -> Result<ResolvedGit, GitResolverError> {
        if let Some((url, sha)) = self.cache.get_key_value(&source.url) {
            return Ok(ResolvedGit {
                url: url.clone(),
                rev: source.rev.clone(),
                commit: sha.clone()
            });
        }
        let clone_path = self.cache_dir.join(Self::repo_dir_name(&source.url, &source.rev));
        let repo = git2::Repository::clone(
            &source.url.to_string(),
            clone_path
        ).map_err(|_| GitResolverError::CloneError { repo: source.url.clone() })?;

        let commit = {
            let obj = repo.revparse_single(&source.rev)?;
            repo.checkout_tree(&obj, None)?;
            match obj.as_commit() {
                Some(commit) => {
                    repo.set_head_detached(commit.id())?;
                    commit.id().to_string()
                },
                None => {
                    let commit = obj.peel_to_commit()?;
                    repo.set_head_detached(commit.id())?;
                    commit.id().to_string()
                }
            }
        };
        if self.cache.insert(source.url.clone(), commit.clone()).is_some() {
            panic!("cache invariant violated");
        }

        Ok(ResolvedGit {
            url: source.url.clone(),
            rev: source.rev.clone(),
            commit
        })
    }

    fn fetch(&mut self, source: &ResolvedGit, path: &Path) -> Result<(), GitResolverError> {
        let git = Git { url: source.url.clone(), rev: source.rev.clone() };
        self.resolve(&git)?;
        
        let source = self.cache_dir.join(Self::repo_dir_name(&source.url, &source.rev));
        let opts = fs_extra::dir::CopyOptions::new();
        fs_extra::copy_items(&[source], path, &opts)
            .expect("dircopy error");
        Ok(())
    }

    fn fetch_file(&mut self, source: &ResolvedGit, rel_path: String) -> Result<String, GitResolverError> {
        let git = Git { url: source.url.clone(), rev: source.rev.clone() };
        self.resolve(&git)?;

        let p = self.cache_dir
            .join(Self::repo_dir_name(
                &source.url,
                &source.rev
            ))
            .join(&rel_path);
        let f = std::fs::read_to_string(&p)
            .map_err(|_| GitResolverError::MissingFileError { repo: source.url.clone(), rel_path: rel_path.clone() })?;
        Ok(f)
    }

    fn list_dir(&mut self, source: &ResolvedGit, rel_path: String) -> Result<Vec<String>, GitResolverError> {
        let git = Git { url: source.url.clone(), rev: source.rev.clone() };
        self.resolve(&git)?;

        let p = self.cache_dir
            .join(Self::repo_dir_name(
                &source.url,
                &source.rev
            ))
            .join(&rel_path);

        let entries = std::fs::read_dir(&p)
            .map_err(|_| GitResolverError::MissingDirectoryError { repo: source.url.clone(), rel_path: rel_path.clone() })?
            .filter_map(|entry| {
                let e = entry.ok()?;
                Some(
                    e.file_name()
                        .to_string_lossy()
                        .to_string()
                )
                
            })
            .collect::<Vec<_>>();
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics_on_zephyr() {
        let cache = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".test");
        let url = "https://github.com/zephyrproject-rtos/zephyr.git";

        let mut resolver = Git2Resolver::new(cache);
        let git = Git {
            url: url::Url::parse(url).expect("bad url"),
            rev: "main".to_string()
        };
        let resolved = resolver.resolve(&git)
            .expect("resolution failed");
        let westyml = resolver.fetch_file(&resolved, "./west.yml".to_string())
            .expect("west.yml fetch failed");
        println!("{}", &westyml);
        let scripts = resolver.list_dir(&resolved, "./scripts".to_string())
            .expect("list dir scripts failed");
        println!("{:#?}", &scripts);
    }
}