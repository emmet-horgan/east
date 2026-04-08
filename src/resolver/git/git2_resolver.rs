use std::path::{Path, PathBuf};

use crate::dep::{Git, ResolvedGit};

use super::{GitResolver, GitResolverError, cached_name_resolver};


#[derive(Debug, Clone)]
pub struct Git2Resolver {
    cache_dir: PathBuf,
    ref_cache: std::collections::BTreeMap<url::Url, String>
}

impl Git2Resolver {
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self { cache_dir: cache_dir.into(), ref_cache: Default::default() }
    }

    fn check_cache(&self, url: &url::Url, oid: &git2::Oid) -> Option<PathBuf> {
        let dirname = cached_name_resolver(url, oid);

        let p = self.cache_dir.join(&dirname);
        if p.exists() {
            Some(p)
        } else {
            None
        }
    }

    fn fetch_to_cache(&mut self, source: &ResolvedGit) -> Result<PathBuf, GitResolverError> {
        use git2::{build::CheckoutBuilder, FetchOptions, Oid, Repository};
        let oid = Oid::from_str(&source.commit)?;
        let path = self.cache_dir.join(cached_name_resolver(&source.url, &oid));
        
        let repo = Repository::init(&path)?;
        let mut remote = repo.remote("origin", source.url.as_str())?;

        let mut fo = FetchOptions::new();
        fo.depth(1);

        // Tiered fetch strategy (mirrors west's approach):
        //
        // 1. Shallow fetch by exact SHA (fastest, needs allowReachableSHA1InWant)
        // 2. Shallow fetch by branch/tag name (if rev is a named ref)
        // 3. Full fetch of all refs (always works, slowest)
        let fetched = remote.fetch(&[&source.commit], Some(&mut fo), None).is_ok();

        if !fetched {
            // Try named refs (only useful when rev != commit, i.e. rev is a branch/tag name)
            let refspecs = [
                format!("+refs/heads/{}:refs/remotes/origin/{}", source.rev, source.rev),
                format!("+refs/tags/{}:refs/tags/{}", source.rev, source.rev),
            ];
            let specs: Vec<&str> = refspecs.iter().map(|s| s.as_str()).collect();
            let named_ok = remote.fetch(&specs, Some(&mut fo), None).is_ok();

            if !named_ok {
                // Last resort: full fetch (no depth limit) to guarantee the commit is reachable.
                // This handles self-hosted servers without allowReachableSHA1InWant.
                remote.fetch(&["refs/heads/*:refs/remotes/origin/*"], None, None)?;
            }
        }

        let oid = Oid::from_str(&source.commit)?;
        let commit = repo.find_commit(oid)?;

        let mut checkout = CheckoutBuilder::new();
        checkout.force();
        repo.checkout_tree(commit.as_object(), Some(&mut checkout))?;
        repo.set_head_detached(oid)?;

        Ok(path)
    }
}

fn resolve_ref(refs: &[git2::RemoteHead], name: &str) -> Option<git2::Oid> {
    let tag_ref = format!("refs/tags/{}", name);
    let peeled_tag_ref = format!("{}^{{}}", tag_ref);
    let branch_ref = format!("refs/heads/{}", name);

    // 1. Prefer peeled tag
    if let Some(r) = refs.iter().find(|r| r.name() == peeled_tag_ref) {
        return Some(r.oid());
    }

    // 2. Then direct tag (lightweight)
    if let Some(r) = refs.iter().find(|r| r.name() == tag_ref) {
        return Some(r.oid());
    }

    // 3. Then branch
    if let Some(r) = refs.iter().find(|r| r.name() == branch_ref) {
        return Some(r.oid());
    }

    None
}

fn resolve(source: &Git) -> Result<ResolvedGit, GitResolverError> {
    use git2::{Oid, Repository, Direction};

    // Check if the ref is itself a commit and return early if so
    if let Ok(oid) = Oid::from_str(&source.rev) {
        let resolved = ResolvedGit {
            url: source.url.clone(),
            rev: source.rev.clone(),
            commit: oid.to_string()
        };
        return Ok(resolved);
    }

    let tmp = tempdir::TempDir::new("")
        .expect("could not get a temporary directory");
    let repo = Repository::init_bare(tmp.path())?;
    let mut remote = repo.remote_anonymous(source.url.as_str())?;
    remote.connect(Direction::Fetch)?;
    let refs = remote.list()?;
    
    let commit = resolve_ref(refs, &source.rev)
        .ok_or(GitResolverError::InvalidRevError { repo: source.url.clone(), rev: source.rev.clone() })?;

    Ok(ResolvedGit { url: source.url.clone(), rev: source.rev.clone(), commit: commit.to_string() })
}

impl GitResolver for Git2Resolver {

    fn resolve(&mut self, source: &Git) -> Result<ResolvedGit, GitResolverError> {
        let res = resolve(source)?;
        self.ref_cache.insert(source.url.clone(), res.commit.clone());
        Ok(res)
    }

    fn fetch(&mut self, source: &ResolvedGit, path: &Path) -> Result<(), GitResolverError> {
        use git2::Oid;

        let oid = Oid::from_str(&source.commit)?;
        if let Some(p) = self.check_cache(&source.url, &oid) {
            let opts = fs_extra::dir::CopyOptions::new();
            fs_extra::copy_items(&[p], path, &opts)
                .expect("dircopy error");
            return Ok(());
        }

        let cache_path = self.fetch_to_cache(source)?;
        let opts = fs_extra::dir::CopyOptions::new();
        fs_extra::copy_items(&[cache_path], path, &opts)
            .expect("dircopy error");

        Ok(())
    }

    fn fetch_file(&mut self, source: &ResolvedGit, rel_path: String) -> Result<String, GitResolverError> {
        use git2::Oid;

        let oid = Oid::from_str(&source.commit)?;

        let p = match self.check_cache(&source.url, &oid) {
            Some(p) => { p.join(&rel_path) },
            _ => { self.fetch_to_cache(source)?.join(&rel_path) }
        };

        let f = std::fs::read_to_string(&p)
            .map_err(|_| GitResolverError::MissingFileError { repo: source.url.clone(), rel_path: rel_path.clone() })?;
        Ok(f)
    }

    fn list_dir(&mut self, source: &ResolvedGit, rel_path: String) -> Result<Vec<String>, GitResolverError> {
        use git2::Oid;

        let oid = Oid::from_str(&source.commit)?;

        let p = match self.check_cache(&source.url, &oid) {
            Some(p) => { p.join(&rel_path) },
            _ => { self.fetch_to_cache(source)?.join(&rel_path) }
        };

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
    use url::Url;

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

    #[test]
    fn basics_on_zephyr_with_sha_ref() {
        let cache = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".test");
        let url = "https://github.com/zephyrproject-rtos/zephyr.git";

        let mut resolver = Git2Resolver::new(cache);
        let git = Git {
            url: url::Url::parse(url).expect("bad url"),
            rev: "c1f2a9b6b04509953ccab078225d48477ce2a808".to_string()
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

    #[test]
    fn git2_resolve() {
        let source = Git { url: Url::parse("https://github.com/zephyrproject-rtos/zephyr.git").unwrap(), rev: "main".to_string() };
        resolve(&source).unwrap();
    }
}