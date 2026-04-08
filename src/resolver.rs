pub mod git;

use std::collections::{BTreeMap, HashSet};

use crate::dep::*;
use self::git::{GitResolver, GitResolverError};

// ── Errors ────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("git resolver error: {0}")]
    Git(#[from] GitResolverError),

    #[error("failed to parse sub-manifest fetched from {url} @ {rev}, path {path}: {reason}")]
    ParseError {
        url: String,
        rev: String,
        path: String,
        reason: String,
    },

    #[error("import cycle detected: {0}")]
    CycleDetected(String),
}

// ── Manifest parser trait ─────────────────────────────────────────────

/// Converts raw file content (e.g. a west.yml) into the common `Manifest` IR.
///
/// This is the seam between the resolver (which is format-agnostic) and
/// the frontend parsers (west.rs, config.rs).  When the resolver fetches
/// a sub-manifest via `GitResolver::fetch_file`, it hands the content to
/// this trait to obtain more `Dep`s to resolve.
pub trait ManifestParser {
    fn parse(&self, content: &str) -> Result<Manifest, ResolveError>;
}

// ── DAG builder ───────────────────────────────────────────────────────

/// Recursively resolves a `Manifest` into a flat `ResolvedManifest` (DAG).
///
/// The algorithm:
/// 1. For each `Dep`, pin its git ref to a commit SHA via `GitResolver::resolve`.
/// 2. If the dep declares an `ImportSpec`, fetch the referenced file(s)
///    from the now-pinned repo, parse them via `ManifestParser`, and merge
///    the resulting deps (first-seen-wins) into the graph.
/// 3. Recurse until no new imports remain.
///
/// Cycle detection is based on `(url, commit, path)` fetch keys.
pub struct DependencyResolver<'a, P: ManifestParser> {
    git: &'a mut dyn GitResolver,
    parser: &'a P,
    /// Tracks `url@commit:path` strings we have already fetched to detect
    /// import cycles / avoid re-processing.
    seen_fetches: HashSet<String>,
}

impl<'a, P: ManifestParser> DependencyResolver<'a, P> {
    pub fn new(git: &'a mut dyn GitResolver, parser: &'a P) -> Self {
        Self {
            git,
            parser,
            seen_fetches: HashSet::new(),
        }
    }

    /// Resolve a manifest into a flat dependency graph.
    ///
    /// Uses a two-pass approach so that direct deps always take priority
    /// over transitive deps discovered through imports (first-seen-wins):
    ///   1. Pin and insert every direct dep.
    ///   2. Walk imports for each dep, skipping any name already resolved.
    pub fn resolve(&mut self, manifest: &Manifest) -> Result<ResolvedManifest, ResolveError> {
        let mut resolved: BTreeMap<String, ResolvedDep> = BTreeMap::new();

        // Pass 1: pin all direct deps so they claim their names first.
        let mut pinned: Vec<(String, Dep, ResolvedGit)> = Vec::new();
        for (name, dep) in &manifest.deps {
            let rg = self.resolve_git_source(&dep.source)?;
            resolved.insert(name.clone(), ResolvedDep {
                name: name.clone(),
                path: dep.path.clone(),
                source: ResolvedSource::Git(rg.clone()),
                deps: Vec::new(),
            });
            pinned.push((name.clone(), dep.clone(), rg));
        }

        // Handle self-imports (manifest repo importing its own sub-manifests).
        // Done between pass 1 and pass 2 so direct deps beat self-imports,
        // but self-imports beat transitive imports.
        if let Some(self_dep) = &manifest.self_dep {
            let resolved_git = self.resolve_git_source(&self_dep.source)?;
            self.process_imports(&self_dep.import, &resolved_git, &mut resolved)?;
        }

        // Pass 2: process imports for each direct dep.
        for (name, dep, rg) in &pinned {
            let imported = self.process_imports(&dep.import, rg, &mut resolved)?;
            if let Some(entry) = resolved.get_mut(name) {
                entry.deps = imported;
            }
        }

        Ok(ResolvedManifest { deps: resolved })
    }

    /// Resolve a single dep, inserting it into `resolved` if not already present,
    /// then recursively processing its imports.
    fn resolve_dep(
        &mut self,
        name: &str,
        dep: &Dep,
        resolved: &mut BTreeMap<String, ResolvedDep>,
    ) -> Result<(), ResolveError> {
        // First-seen-wins: skip if already resolved.
        if resolved.contains_key(name) {
            return Ok(());
        }

        let resolved_git = self.resolve_git_source(&dep.source)?;

        // Insert the dep *before* processing imports so that any circular
        // reference through imports sees us as already-present.
        resolved.insert(name.to_string(), ResolvedDep {
            name: name.to_string(),
            path: dep.path.clone(),
            source: ResolvedSource::Git(resolved_git.clone()),
            deps: Vec::new(),
        });

        // Process imports and record which deps were pulled in.
        let imported = self.process_imports(&dep.import, &resolved_git, resolved)?;

        // Patch the deps list now that we know what was imported.
        if let Some(entry) = resolved.get_mut(name) {
            entry.deps = imported;
        }

        Ok(())
    }

    /// Walk an `ImportSpec`, fetching sub-manifests and merging their deps.
    /// Returns the list of dependency names that were discovered.
    fn process_imports(
        &mut self,
        import: &ImportSpec,
        source: &ResolvedGit,
        resolved: &mut BTreeMap<String, ResolvedDep>,
    ) -> Result<Vec<String>, ResolveError> {
        match import {
            ImportSpec::None => Ok(Vec::new()),

            ImportSpec::Path(path) => {
                self.fetch_and_merge(source, path, resolved)
            }

            ImportSpec::Paths(paths) => {
                let mut all_names = Vec::new();
                for path in paths {
                    let names = self.fetch_and_merge(source, path, resolved)?;
                    all_names.extend(names);
                }
                Ok(all_names)
            }

            ImportSpec::Filtered {
                path,
                name_allowlist,
                name_blocklist,
                path_allowlist,
                path_blocklist,
                path_prefix,
            } => {
                let sub = self.fetch_sub_manifest(source, path)?;
                let mut imported = Vec::new();

                for (dep_name, dep) in &sub.deps {
                    if !passes_filter(
                        dep_name,
                        &dep.path,
                        name_allowlist.as_deref(),
                        name_blocklist.as_deref(),
                        path_allowlist.as_deref(),
                        path_blocklist.as_deref(),
                    ) {
                        continue;
                    }

                    // Apply path prefix if configured.
                    let mut dep = dep.clone();
                    if let Some(pfx) = path_prefix {
                        dep.path = format!("{}/{}", pfx, dep.path);
                    }

                    imported.push(dep_name.clone());
                    self.resolve_dep(dep_name, &dep, resolved)?;
                }

                Ok(imported)
            }
        }
    }

    /// Fetch a single sub-manifest file, parse it, and merge its deps.
    fn fetch_and_merge(
        &mut self,
        source: &ResolvedGit,
        path: &str,
        resolved: &mut BTreeMap<String, ResolvedDep>,
    ) -> Result<Vec<String>, ResolveError> {
        let sub = self.fetch_sub_manifest(source, path)?;
        let mut imported = Vec::new();

        // First resolve self-imports of the sub-manifest.
        if let Some(self_dep) = &sub.self_dep {
            let rg = self.resolve_git_source(&self_dep.source)?;
            let self_imported = self.process_imports(&self_dep.import, &rg, resolved)?;
            imported.extend(self_imported);
        }

        for (dep_name, dep) in &sub.deps {
            imported.push(dep_name.clone());
            self.resolve_dep(dep_name, dep, resolved)?;
        }

        Ok(imported)
    }

    /// Fetch file content and parse it into a `Manifest` via the parser.
    /// Returns `Err(CycleDetected)` if we've already fetched this exact
    /// `(url, commit, path)` tuple.
    fn fetch_sub_manifest(
        &mut self,
        source: &ResolvedGit,
        path: &str,
    ) -> Result<Manifest, ResolveError> {
        let key = format!("{}@{}:{}", source.url, source.commit, path);
        if !self.seen_fetches.insert(key.clone()) {
            return Err(ResolveError::CycleDetected(key));
        }

        let content = self.git.fetch_file(source, path.to_string())?;

        self.parser.parse(&content).map_err(|e| match e {
            // Rewrap parse errors with fetch context if not already.
            ResolveError::ParseError { .. } => e,
            other => ResolveError::ParseError {
                url: source.url.to_string(),
                rev: source.rev.clone(),
                path: path.to_string(),
                reason: other.to_string(),
            },
        })
    }

    /// Pin a `Source` to a resolved git commit.
    fn resolve_git_source(&mut self, source: &Source) -> Result<ResolvedGit, ResolveError> {
        match source {
            Source::Git(git) => Ok(self.git.resolve(git)?),
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────

fn passes_filter(
    name: &str,
    path: &str,
    name_allow: Option<&[String]>,
    name_block: Option<&[String]>,
    path_allow: Option<&[String]>,
    path_block: Option<&[String]>,
) -> bool {
    if let Some(allow) = name_allow {
        if !allow.iter().any(|a| a == name) {
            return false;
        }
    }
    if let Some(block) = name_block {
        if block.iter().any(|b| b == name) {
            return false;
        }
    }
    if let Some(allow) = path_allow {
        if !allow.iter().any(|a| a == path) {
            return false;
        }
    }
    if let Some(block) = path_block {
        if block.iter().any(|b| b == path) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // ── Mock GitResolver ──────────────────────────────────────────────

    /// A mock git resolver that records resolve calls and serves
    /// pre-configured file content.
    struct MockGitResolver {
        /// Map of "url#rev" → commit SHA to return from resolve().
        commits: BTreeMap<String, String>,
        /// Map of "url#commit#path" → file content.
        files: BTreeMap<String, String>,
    }

    impl MockGitResolver {
        fn new() -> Self {
            Self {
                commits: BTreeMap::new(),
                files: BTreeMap::new(),
            }
        }

        fn add_commit(&mut self, url: &str, rev: &str, commit: &str) {
            self.commits.insert(format!("{}#{}", url, rev), commit.to_string());
        }

        fn add_file(&mut self, url: &str, commit: &str, path: &str, content: &str) {
            self.files.insert(format!("{}#{}#{}", url, commit, path), content.to_string());
        }
    }

    impl GitResolver for MockGitResolver {
        fn resolve(&mut self, source: &Git) -> Result<ResolvedGit, GitResolverError> {
            let key = format!("{}#{}", source.url, source.rev);
            let commit = self.commits.get(&key)
                .ok_or_else(|| GitResolverError::InvalidRevError {
                    repo: source.url.clone(),
                    rev: source.rev.clone(),
                })?;
            Ok(ResolvedGit {
                url: source.url.clone(),
                rev: source.rev.clone(),
                commit: commit.clone(),
            })
        }

        fn fetch(
            &mut self, _source: &ResolvedGit, _path: &std::path::Path,
        ) -> Result<(), GitResolverError> {
            Ok(())
        }

        fn fetch_file(
            &mut self, source: &ResolvedGit, rel_path: String,
        ) -> Result<String, GitResolverError> {
            let key = format!("{}#{}#{}", source.url, source.commit, rel_path);
            self.files.get(&key).cloned().ok_or_else(|| {
                GitResolverError::MissingFileError {
                    repo: source.url.clone(),
                    rel_path,
                }
            })
        }

        fn list_dir(
            &mut self, _source: &ResolvedGit, _rel_path: String,
        ) -> Result<Vec<String>, GitResolverError> {
            Ok(Vec::new())
        }
    }

    // ── Mock ManifestParser ───────────────────────────────────────────

    /// Trivial parser that treats each non-empty line as:
    /// `name url rev [import_path]`
    struct SimpleParser;

    impl ManifestParser for SimpleParser {
        fn parse(&self, content: &str) -> Result<Manifest, ResolveError> {
            let mut deps = BTreeMap::new();
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let parts: Vec<&str> = line.splitn(4, ' ').collect();
                if parts.len() < 3 {
                    return Err(ResolveError::ParseError {
                        url: String::new(),
                        rev: String::new(),
                        path: String::new(),
                        reason: format!("bad line: {}", line),
                    });
                }
                let name = parts[0];
                let url: url::Url = parts[1].parse().map_err(|e: url::ParseError| {
                    ResolveError::ParseError {
                        url: String::new(),
                        rev: String::new(),
                        path: String::new(),
                        reason: e.to_string(),
                    }
                })?;
                let rev = parts[2];
                let import = if parts.len() == 4 {
                    ImportSpec::Path(parts[3].to_string())
                } else {
                    ImportSpec::None
                };
                let dep = Dep::new(name, name, Source::Git(Git::new(url, rev)))
                    .with_import(import);
                deps.insert(name.to_string(), dep);
            }
            Ok(Manifest::new(deps))
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────

    #[test]
    fn flat_manifest_no_imports() {
        let mut git = MockGitResolver::new();
        git.add_commit("https://github.com/a/foo", "main", "aaa111");
        git.add_commit("https://github.com/a/bar", "v1.0", "bbb222");

        let parser = SimpleParser;
        let mut resolver = DependencyResolver::new(&mut git, &parser);

        let mut deps = BTreeMap::new();
        deps.insert("foo".into(), Dep::new(
            "foo", "libs/foo",
            Source::Git(Git::new("https://github.com/a/foo".parse().unwrap(), "main")),
        ));
        deps.insert("bar".into(), Dep::new(
            "bar", "libs/bar",
            Source::Git(Git::new("https://github.com/a/bar".parse().unwrap(), "v1.0")),
        ));

        let manifest = Manifest::new(deps);
        let resolved = resolver.resolve(&manifest).unwrap();

        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved.get("foo").unwrap().path(), "libs/foo");
        assert_eq!(
            match &resolved.get("foo").unwrap().source {
                ResolvedSource::Git(g) => g.commit(),
            },
            "aaa111"
        );
        assert_eq!(resolved.get("bar").unwrap().path(), "libs/bar");
        assert!(resolved.get("foo").unwrap().deps().is_empty());
    }

    #[test]
    fn single_import_resolves_transitive_deps() {
        let mut git = MockGitResolver::new();
        git.add_commit("https://github.com/a/root", "main", "aaaa");
        git.add_commit("https://github.com/a/child1", "v1", "cccc");
        git.add_commit("https://github.com/a/child2", "v2", "dddd");

        // root's west.yml defines two children.
        git.add_file(
            "https://github.com/a/root", "aaaa", "west.yml",
            "child1 https://github.com/a/child1 v1\nchild2 https://github.com/a/child2 v2\n",
        );

        let parser = SimpleParser;
        let mut resolver = DependencyResolver::new(&mut git, &parser);

        let mut deps = BTreeMap::new();
        deps.insert("root".into(), Dep::new(
            "root", "root",
            Source::Git(Git::new("https://github.com/a/root".parse().unwrap(), "main")),
        ).with_import(ImportSpec::Path("west.yml".into())));

        let manifest = Manifest::new(deps);
        let resolved = resolver.resolve(&manifest).unwrap();

        assert_eq!(resolved.len(), 3); // root + child1 + child2
        assert!(resolved.get("root").is_some());
        assert!(resolved.get("child1").is_some());
        assert!(resolved.get("child2").is_some());

        // root should list its imported deps.
        let root = resolved.get("root").unwrap();
        assert_eq!(root.deps(), &["child1", "child2"]);
    }

    #[test]
    fn first_seen_wins() {
        let mut git = MockGitResolver::new();
        git.add_commit("https://github.com/a/root", "main", "aaaa");
        git.add_commit("https://github.com/a/shared", "v1", "1111");
        git.add_commit("https://github.com/a/shared", "v2", "2222");

        // root imports a sub-manifest that also defines "shared" at v2,
        // but "shared" was already in the initial manifest at v1.
        git.add_file(
            "https://github.com/a/root", "aaaa", "west.yml",
            "shared https://github.com/a/shared v2\n",
        );

        let parser = SimpleParser;
        let mut resolver = DependencyResolver::new(&mut git, &parser);

        let mut deps = BTreeMap::new();
        // "shared" declared directly at v1.
        deps.insert("shared".into(), Dep::new(
            "shared", "libs/shared",
            Source::Git(Git::new("https://github.com/a/shared".parse().unwrap(), "v1")),
        ));
        // "root" imports a file that also declares "shared".
        deps.insert("root".into(), Dep::new(
            "root", "root",
            Source::Git(Git::new("https://github.com/a/root".parse().unwrap(), "main")),
        ).with_import(ImportSpec::Path("west.yml".into())));

        let manifest = Manifest::new(deps);
        let resolved = resolver.resolve(&manifest).unwrap();

        // "shared" should be at v1 (first-seen from the direct deps).
        let shared = resolved.get("shared").unwrap();
        assert_eq!(
            match &shared.source {
                ResolvedSource::Git(g) => g.commit(),
            },
            "1111"
        );
    }

    #[test]
    fn cycle_detection() {
        let mut git = MockGitResolver::new();
        git.add_commit("https://github.com/a/root", "main", "aaaa");

        // root's west.yml imports itself → cycle.
        git.add_file(
            "https://github.com/a/root", "aaaa", "west.yml",
            "", // content doesn't matter — the fetch key cycle triggers first
        );
        // But a second fetch of the same (url, commit, path) is a cycle.
        // Actually, the first fetch succeeds. We need a manifest that
        // re-imports the same file:
        git.add_file(
            "https://github.com/a/root", "aaaa", "west.yml",
            "child https://github.com/a/root main west.yml\n",
        );
        git.add_commit("https://github.com/a/root", "main", "aaaa");

        let parser = SimpleParser;
        let mut resolver = DependencyResolver::new(&mut git, &parser);

        let mut deps = BTreeMap::new();
        deps.insert("root".into(), Dep::new(
            "root", "root",
            Source::Git(Git::new("https://github.com/a/root".parse().unwrap(), "main")),
        ).with_import(ImportSpec::Path("west.yml".into())));

        let manifest = Manifest::new(deps);
        let result = resolver.resolve(&manifest);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, ResolveError::CycleDetected(_)),
            "expected CycleDetected, got: {:?}", err
        );
    }

    #[test]
    fn filtered_import_respects_allowlist() {
        let mut git = MockGitResolver::new();
        git.add_commit("https://github.com/a/root", "main", "aaaa");
        git.add_commit("https://github.com/a/keep", "v1", "1111");
        git.add_commit("https://github.com/a/skip", "v1", "2222");

        git.add_file(
            "https://github.com/a/root", "aaaa", "west.yml",
            "keep https://github.com/a/keep v1\nskip https://github.com/a/skip v1\n",
        );

        let parser = SimpleParser;
        let mut resolver = DependencyResolver::new(&mut git, &parser);

        let mut deps = BTreeMap::new();
        deps.insert("root".into(), Dep::new(
            "root", "root",
            Source::Git(Git::new("https://github.com/a/root".parse().unwrap(), "main")),
        ).with_import(ImportSpec::Filtered {
            path: "west.yml".into(),
            name_allowlist: Some(vec!["keep".into()]),
            name_blocklist: None,
            path_allowlist: None,
            path_blocklist: None,
            path_prefix: None,
        }));

        let manifest = Manifest::new(deps);
        let resolved = resolver.resolve(&manifest).unwrap();

        assert_eq!(resolved.len(), 2); // root + keep
        assert!(resolved.get("keep").is_some());
        assert!(resolved.get("skip").is_none());
    }

    #[test]
    fn diamond_imports_converge() {
        // A diamond: root → {left, right}, both left and right import
        // the same sub-manifest from "shared" repo. The deps from that
        // sub-manifest should appear only once (first-seen-wins,
        // and the second fetch is skipped by first-seen on the dep name).
        let mut git = MockGitResolver::new();
        git.add_commit("https://github.com/a/root", "main", "rrrr");
        git.add_commit("https://github.com/a/left", "main", "llll");
        git.add_commit("https://github.com/a/right", "main", "riri");
        git.add_commit("https://github.com/a/bottom", "v1", "bbbb");

        // root's west.yml: left and right.
        git.add_file(
            "https://github.com/a/root", "rrrr", "west.yml",
            "left https://github.com/a/left main west.yml\nright https://github.com/a/right main west.yml\n",
        );
        // Both left and right point to "bottom".
        git.add_file(
            "https://github.com/a/left", "llll", "west.yml",
            "bottom https://github.com/a/bottom v1\n",
        );
        git.add_file(
            "https://github.com/a/right", "riri", "west.yml",
            "bottom https://github.com/a/bottom v1\n",
        );

        let parser = SimpleParser;
        let mut resolver = DependencyResolver::new(&mut git, &parser);

        let mut deps = BTreeMap::new();
        deps.insert("root".into(), Dep::new(
            "root", "root",
            Source::Git(Git::new("https://github.com/a/root".parse().unwrap(), "main")),
        ).with_import(ImportSpec::Path("west.yml".into())));

        let manifest = Manifest::new(deps);
        let resolved = resolver.resolve(&manifest).unwrap();

        // root, left, right, bottom = 4.
        assert_eq!(resolved.len(), 4);
        assert!(resolved.get("bottom").is_some());
    }
}
