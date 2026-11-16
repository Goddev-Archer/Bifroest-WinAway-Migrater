use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use walkdir::WalkDir;

use crate::model::GitRepoInfo;

pub fn scan_git_repos(roots: &[PathBuf], max_depth: usize) -> Vec<GitRepoInfo> {
    let mut repos = Vec::new();
    let mut seen_paths = HashSet::new();

    for root in roots {
        if !root.exists() || !root.is_dir() {
            continue;
        }

        let mut it = WalkDir::new(root)
            .max_depth(max_depth)
            .into_iter()
            .filter_entry(|entry| {
                let name = entry.file_name().to_string_lossy();
                // Ignoriere typische Riesen-Ordner & Caches
                if entry.file_type().is_dir() {
                    if name == "node_modules" 
                        || name == "target" 
                        || name == ".cargo" 
                        || name == ".rustup" 
                        || name == "AppData" 
                        || name == ".vscode" 
                        || name == ".nuget" {
                        return false;
                    }
                }
                true
            });

        while let Some(Ok(entry)) = it.next() {
            if entry.file_type().is_dir() && entry.file_name() == ".git" {
                if let Some(repo_root) = entry.path().parent() {
                    let canonical = fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
                    if seen_paths.insert(canonical) {
                        let repo_info = inspect_repo(repo_root);
                        repos.push(repo_info);
                    }
                }
                it.skip_current_dir();
            }
        }
    }

    repos
}

fn inspect_repo(repo_path: &Path) -> GitRepoInfo {
    let repo_name = repo_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unnamed_repo".to_string());

    let branch = read_current_branch(repo_path).unwrap_or_else(|| "detached".to_string());
    let (is_dirty, uncommitted_count) = check_dirty_status(repo_path);

    GitRepoInfo {
        path: repo_path.to_path_buf(),
        name: repo_name,
        current_branch: branch,
        is_dirty,
        uncommitted_files_count: uncommitted_count,
        selected: true,
    }
}

fn read_current_branch(repo_path: &Path) -> Option<String> {
    let head_path = repo_path.join(".git").join("HEAD");
    if let Ok(content) = fs::read_to_string(head_path) {
        let trimmed = content.trim();
        if let Some(rest) = trimmed.strip_prefix("ref: refs/heads/") {
            return Some(rest.to_string());
        } else if trimmed.len() >= 7 {
            return Some(trimmed[..7].to_string());
        }
    }
    None
}

fn check_dirty_status(repo_path: &Path) -> (bool, usize) {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo_path)
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
            let count = lines.len();
            return (count > 0, count);
        }
    }

    (false, 0)
}
