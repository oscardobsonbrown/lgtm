use crate::command::{checked, run_gh, run_git, spawn};
use crate::model::{PrLocator, PrMeta};
use anyhow::{bail, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_BLOB_BYTES: usize = 1024 * 1024;

pub fn fetch_patch(loc: &PrLocator) -> Result<String> {
    run_gh(&["pr", "diff", &loc.number.to_string(), "--repo", &loc.repo_slug()])
}

pub fn is_diff_too_large_error(err: &anyhow::Error) -> bool {
    let message = format!("{err:#}").to_lowercase();
    message.contains("diff too large")
        || message.contains("diff is too large")
        || message.contains("diff too big")
        || message.contains("diff exceeded the maximum")
        || message.contains("pullrequest.diff too_large")
}

/// Builds a local patch in a unique app-owned destination. The caller owns
/// creation and cleanup; this function never resets or removes the directory.
pub fn fetch_patch_locally(loc: &PrLocator, meta: &PrMeta, destination: &Path) -> Result<String> {
    if destination.exists() {
        bail!("preview destination already exists: {}", destination.display());
    }
    let repo_path = destination.to_string_lossy().into_owned();
    let mut command = Command::new("gh");
    command.args([
        "repo",
        "clone",
        &loc.repo_slug(),
        &repo_path,
        "--",
        "--bare",
        "--filter=blob:none",
        "--single-branch",
        "--no-tags",
    ]);
    checked(command, &format!("gh repo clone {}", loc.repo_slug()))?;
    let base_ref = format!("+refs/heads/{}:refs/lgtm/base", meta.base_ref_name);
    let head_ref = format!("+refs/pull/{}/head:refs/lgtm/head", loc.number);
    run_git(destination, &["fetch", "--no-tags", "origin", &base_ref, &head_ref])?;
    let merge_base = run_git(destination, &["merge-base", "refs/lgtm/base", "refs/lgtm/head"])?;
    run_git(
        destination,
        &[
            "diff",
            "-M",
            "--no-color",
            "--no-ext-diff",
            merge_base.trim(),
            "refs/lgtm/head",
        ],
    )
}

pub fn fetch_file_at(loc: &PrLocator, commit_oid: &str, path: &str) -> Result<Option<String>> {
    fetch_file_at_with_cache(loc, commit_oid, path, None)
}

pub fn fetch_file_at_in(loc: &PrLocator, commit_oid: &str, path: &str, cache_root: &Path) -> Result<Option<String>> {
    fetch_file_at_with_cache(loc, commit_oid, path, Some(cache_root))
}

fn fetch_file_at_with_cache(
    loc: &PrLocator,
    commit_oid: &str,
    path: &str,
    root: Option<&Path>,
) -> Result<Option<String>> {
    let cache = cache_path(root, &loc.repo_slug(), commit_oid, path);
    if let Some(cache) = &cache {
        if cache.with_extension("absent").exists() {
            return Ok(None);
        }
        if let Ok(bytes) = std::fs::read(cache) {
            return Ok(String::from_utf8(bytes).ok());
        }
    }
    let endpoint = format!(
        "repos/{}/{}/contents/{}?ref={}",
        loc.owner,
        loc.repo,
        encode_path(path),
        commit_oid
    );
    let mut command = Command::new("gh");
    command.args(["api", "-H", "Accept: application/vnd.github.raw+json", &endpoint]);
    let output = spawn(command, "gh api")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("404") {
            mark_absent(cache);
            return Ok(None);
        }
        bail!("gh api {endpoint} failed: {}", stderr.trim());
    }
    if output.stdout.len() > MAX_BLOB_BYTES {
        mark_absent(cache);
        return Ok(None);
    }
    let Ok(text) = String::from_utf8(output.stdout) else {
        mark_absent(cache);
        return Ok(None);
    };
    if let Some(cache) = cache {
        let _ = std::fs::write(cache, &text);
    }
    Ok(Some(text))
}

fn mark_absent(cache: Option<PathBuf>) {
    if let Some(cache) = cache {
        let _ = std::fs::write(cache.with_extension("absent"), b"");
    }
}

fn cache_path(root: Option<&Path>, repo: &str, oid: &str, path: &str) -> Option<PathBuf> {
    let dir = root
        .map(Path::to_path_buf)
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".cache/lgtm/blobs")))?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(cache_key(repo, oid, path)))
}

pub fn cache_key(repo: &str, oid: &str, path: &str) -> String {
    let mut hasher = Sha256::new();
    for value in [repo, oid, path] {
        hasher.update(value.as_bytes());
        hasher.update([0]);
    }
    hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_large_diff_errors() {
        assert!(is_diff_too_large_error(&anyhow::anyhow!("PullRequest.diff too_large")));
        assert!(!is_diff_too_large_error(&anyhow::anyhow!("bad credentials")));
    }

    #[test]
    fn encodes_paths_and_stable_keys() {
        assert_eq!(encode_path("a b/c#d"), "a%20b/c%23d");
        assert_eq!(cache_key("a/r", "oid", "p"), cache_key("a/r", "oid", "p"));
    }

    #[test]
    fn local_preview_refuses_owned_destination() {
        let root = std::env::temp_dir().join(format!("lgtm-owned-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let loc = PrLocator {
            owner: "a".into(),
            repo: "r".into(),
            number: 1,
        };
        let meta: PrMeta = serde_json::from_str(r#"{"number":1,"title":"x","author":{"login":"a"},"state":"OPEN","url":"u","baseRefName":"main","headRefName":"x","additions":0,"deletions":0,"changedFiles":0}"#).unwrap();
        assert!(fetch_patch_locally(&loc, &meta, &root).is_err());
        assert!(root.exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
