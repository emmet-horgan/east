use std::path::{Path, PathBuf};
use std::fs;

use git2::{build::RepoBuilder, FetchOptions, ObjectType};

use crate::config::{Config, Module};
use crate::lock::{LockFile, LockedModule, Source, GitSource};

#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("failed to create directory '{}'", .0.display())]
    CreateEastDirError(PathBuf),
    #[error("failed to find the east configuration file at '{}'", .0.display())]
    FindConfigError(PathBuf),
    #[error("failed to read the east configuration file at '{}'", .0.display())]
    ReadConfigError(PathBuf),
    #[error("failed to parse the east configuration file {0}")]
    ConfigParseError(#[from] toml::de::Error),
    #[error("zephyr rtos not present in module list")]
    NoZephyrError,
    #[error("failed to create module directory '{}'", .0.display())]
    CreateModDirError(PathBuf),
    #[error("failed to clone repository for module '{}'\n{}", .module, .source)]
    RepoCloneError {
        module: String,
        source: git2::Error
    },
    #[error("failed to find reference '{}' for module '{}'", .revision, .module)]
    RepoRefError {
        module: String,
        revision: String
    },
    #[error("failed to checkout '{}' for module '{}'", .revision, .module)]
    RepoCheckoutError {
        module: String,
        revision: String,
    }
}

pub fn init(dir: &PathBuf) -> Result<LockFile, InitError> {
    let config_file = dir.join("east.toml");
    if !config_file.exists() {
        return Err(InitError::FindConfigError(config_file.clone()))
    }

    let config = fs::read_to_string(&config_file)
        .map_err(|_| InitError::ReadConfigError(config_file.clone()))?;

    let config: Config = toml::from_str(&config)?;

    // quick zephyr check
    let _zephyr_present = config
        .modules()
        .iter()
        .filter_map(|(n, _)| if n.as_str() == "zephyr" {Some(())} else {None})
        .next()
        .ok_or(InitError::NoZephyrError)?;

    let east_dir = dir.join(".east");
    if !east_dir.exists() {
        fs::create_dir_all(&east_dir)
            .map_err(|_| InitError::CreateEastDirError(east_dir.clone()))?;
    }

    let modules_dir = east_dir.join("modules");
    if !modules_dir.exists() {
        fs::create_dir_all(&east_dir)
            .map_err(|_| InitError::CreateModDirError(modules_dir.clone()))?;
    }
    let mut locked_mods = Vec::with_capacity(config.modules().len());
    for (n, m) in config.modules().iter() {
        locked_mods.push(
            // Should probably be recursive
            setup_module(&modules_dir,n.as_str(),m)?
        );
    }

    Ok(LockFile::new(config.workspace().version().clone(), locked_mods))
}

fn setup_module(
    modules_dir: &Path, 
    name: &str, 
    module: &Module
) -> Result<LockedModule, InitError> {
    let mod_path = modules_dir.join(name);
    let mut depth = 1;
    let mut fetch_opts = FetchOptions::new();
    fetch_opts.depth(depth);

    let repo = RepoBuilder::new()
        .fetch_options(fetch_opts)
        .clone(module.git().as_str(), &mod_path)
        .map_err(|e| InitError::RepoCloneError { 
            module: name.to_string(), 
            source: e 
        })?;
    let commit = {
        let obj = repo.revparse_single(module.rev())
            .map_err(|_| InitError::RepoRefError { 
                module: name.to_string(), 
                revision: module.rev().to_string() 
            })?;
        repo.checkout_tree(&obj, None)
            .map_err(|_| InitError::RepoCheckoutError { 
                module: name.to_string(), 
                revision: module.rev().to_string() 
            })?;
        match obj.as_commit() {
            Some(commit) => {
                repo.set_head_detached(commit.id())
                    .map_err(|_| InitError::RepoCheckoutError {
                        module: name.to_string(), 
                        revision: module.rev().to_string() 
                    })?;
                commit.id().to_string()
            },
            None => {
                let commit = obj.peel_to_commit()
                    .map_err(|_| InitError::RepoCheckoutError { 
                        module: name.to_string(), 
                        revision: module.rev().to_string() 
                    })?;
                repo.set_head_detached(commit.id())
                    .map_err(|_| InitError::RepoCheckoutError { 
                        module: name.to_string(), 
                        revision: module.rev().to_string() 
                    })?;
                commit.id().to_string()
            }
        }
    };

    Ok(LockedModule{
        name: name.to_string(),
        source: Source::Git(GitSource {
            url: module.git().clone(),
            rev: module.rev().to_string(),
            commit: commit,
            // hardcode for now
            dependencies: vec![]
        })
    })
}