//! Fetch PR data via the `gh` CLI, piggybacking on the user's `gh auth`.

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct PrLocator {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

impl PrLocator {
    pub fn repo_slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

/// Accepts `owner/repo#123`, `#123`, `123`, or a GitHub PR URL. Bare numbers
/// resolve the repo from the current directory's git remote (via `gh`).
pub fn resolve_pr_arg(arg: &str) -> Result<PrLocator> {
    if let Some(rest) = arg
        .strip_prefix("https://github.com/")
        .or_else(|| arg.strip_prefix("http://github.com/"))
        .or_else(|| arg.strip_prefix("github.com/"))
    {
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() >= 4 && parts[2] == "pull" {
            let digits: String = parts[3].chars().take_while(char::is_ascii_digit).collect();
            let number = digits
                .parse()
                .with_context(|| format!("no PR number in URL {arg}"))?;
            return Ok(PrLocator {
                owner: parts[0].to_string(),
                repo: parts[1].to_string(),
                number,
            });
        }
        bail!("unrecognized GitHub URL: {arg}");
    }

    if let Some((repo_part, number)) = arg.split_once('#') {
        let number = number
            .parse()
            .with_context(|| format!("invalid PR number in {arg}"))?;
        if repo_part.is_empty() {
            return locator_in_cwd_repo(number);
        }
        let (owner, repo) = repo_part
            .split_once('/')
            .with_context(|| format!("expected owner/repo before '#' in {arg}"))?;
        return Ok(PrLocator {
            owner: owner.to_string(),
            repo: repo.to_string(),
            number,
        });
    }

    if let Ok(number) = arg.parse() {
        return locator_in_cwd_repo(number);
    }

    bail!("could not parse {arg:?}; expected owner/repo#123, a PR URL, or a PR number")
}

fn locator_in_cwd_repo(number: u64) -> Result<PrLocator> {
    let out = gh(&[
        "repo",
        "view",
        "--json",
        "nameWithOwner",
        "--jq",
        ".nameWithOwner",
    ])
    .context("couldn't infer the repo from the current directory; use owner/repo#123")?;
    let (owner, repo) = out
        .trim()
        .split_once('/')
        .with_context(|| format!("unexpected gh repo view output: {out}"))?;
    Ok(PrLocator {
        owner: owner.to_string(),
        repo: repo.to_string(),
        number,
    })
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrMeta {
    pub number: u64,
    pub title: String,
    pub author: Author,
    pub state: String,
    pub url: String,
    /// PR description (markdown); empty when the PR has none. Used as chat
    /// context, not rendered in the UI.
    #[serde(default)]
    pub body: String,
    pub base_ref_name: String,
    pub head_ref_name: String,
    #[serde(default)]
    pub base_ref_oid: String,
    #[serde(default)]
    pub head_ref_oid: String,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    /// "APPROVED", "CHANGES_REQUESTED", "REVIEW_REQUIRED", or "" (no
    /// required reviews and none given).
    #[serde(default)]
    pub review_decision: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Author {
    pub login: String,
}

pub fn fetch_meta(loc: &PrLocator) -> Result<PrMeta> {
    let json = gh(&[
        "pr",
        "view",
        &loc.number.to_string(),
        "--repo",
        &loc.repo_slug(),
        "--json",
        "number,title,author,state,url,body,baseRefName,headRefName,baseRefOid,headRefOid,\
         additions,deletions,changedFiles,reviewDecision",
    ])?;
    serde_json::from_str(&json).context("unexpected gh pr view JSON")
}

/// One row of `gh pr list` output, for the PR picker.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrSummary {
    pub number: u64,
    pub title: String,
    pub author: Author,
    pub state: String,
    pub is_draft: bool,
    pub head_ref_name: String,
    pub updated_at: String,
}

/// Open PRs for a repo, most recently updated first (gh's default order).
pub fn list_prs(owner: &str, repo: &str) -> Result<Vec<PrSummary>> {
    let json = gh(&[
        "pr",
        "list",
        "--repo",
        &format!("{owner}/{repo}"),
        "--state",
        "open",
        "--limit",
        "200",
        "--json",
        "number,title,author,state,isDraft,headRefName,updatedAt",
    ])?;
    serde_json::from_str(&json).context("unexpected gh pr list JSON")
}

/// One row of `gh search prs` output for the signed-in user's sidebar list.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserPrSummary {
    pub number: u64,
    pub title: String,
    pub author: Author,
    pub is_draft: bool,
    pub updated_at: String,
    pub repository: Repository,
    pub url: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub name: String,
    pub name_with_owner: String,
}

fn search_user_prs(filter: &str) -> Result<Vec<UserPrSummary>> {
    let json = gh(&[
        "search", "prs", filter, "--state=open", "--limit=200",
        "--sort=updated", "--order=desc",
        "--json=number,title,author,isDraft,updatedAt,repository,url",
    ])?;
    serde_json::from_str(&json).context("unexpected gh search prs JSON")
}

fn merge_user_prs(
    authored: Vec<UserPrSummary>,
    review_requested: Vec<UserPrSummary>,
) -> Vec<UserPrSummary> {
    let mut by_id: HashMap<(String, u64), UserPrSummary> = HashMap::new();
    for pr in authored.into_iter().chain(review_requested) {
        let key = (pr.repository.name_with_owner.clone(), pr.number);
        match by_id.get(&key) {
            Some(existing) if existing.updated_at >= pr.updated_at => {}
            _ => {
                by_id.insert(key, pr);
            }
        }
    }
    let mut merged: Vec<_> = by_id.into_values().collect();
    merged.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.repository.name_with_owner.cmp(&b.repository.name_with_owner))
            .then_with(|| a.number.cmp(&b.number))
    });
    merged
}

/// Open PRs authored by the signed-in user or requesting their review,
/// across every repository visible to their `gh` login.
pub fn list_user_prs() -> Result<Vec<UserPrSummary>> {
    let authored = search_user_prs("--author=@me")?;
    let review_requested = search_user_prs("--review-requested=@me")?;
    Ok(merge_user_prs(authored, review_requested))
}

/// One PR review comment from the REST pulls/comments API. Unlike `gh pr
/// view --json` (camelCase), this endpoint returns snake_case field names, so
/// no `rename_all` here.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    pub path: String,
    /// Anchor line on `side` against the *current* diff; None = outdated
    /// (the code the comment was written against has since changed).
    pub line: Option<u64>,
    /// "RIGHT" (new side) or "LEFT" (old side).
    pub side: Option<String>,
    /// Multi-line comments start here and anchor at `line` (the end), like
    /// GitHub's own UI.
    pub start_line: Option<u64>,
    pub body: String,
    pub user: Author,
    pub created_at: String,
    pub in_reply_to_id: Option<u64>,
}

/// Every review comment on the PR. `--paginate --slurp` (gh ≥ 2.66; we
/// require it) wraps each page's JSON array into one array-of-arrays —
/// without `--slurp`, `--paginate` concatenates the arrays back-to-back
/// ("[…][…]"), which serde can't parse.
pub fn fetch_review_comments(loc: &PrLocator) -> Result<Vec<ReviewComment>> {
    let json = gh(&[
        "api",
        "--paginate",
        "--slurp",
        &format!(
            "repos/{}/{}/pulls/{}/comments?per_page=100",
            loc.owner, loc.repo, loc.number
        ),
    ])?;
    let pages: Vec<Vec<ReviewComment>> =
        serde_json::from_str(&json).context("unexpected gh pulls/comments JSON")?;
    Ok(pages.into_iter().flatten().collect())
}

/// Post a new top-level review comment anchored at (path, side, line) against
/// `commit_id` (the PR's head oid). Errors carry gh's stderr — a 403 usually
/// means a missing token scope, a 422 an unanchorable line.
pub fn post_review_comment(
    loc: &PrLocator,
    commit_id: &str,
    path: &str,
    side: &str,
    line: u64,
    body: &str,
) -> Result<()> {
    gh(&[
        "api",
        "-X",
        "POST",
        &format!(
            "repos/{}/{}/pulls/{}/comments",
            loc.owner, loc.repo, loc.number
        ),
        "-f",
        &format!("body={body}"),
        "-f",
        &format!("commit_id={commit_id}"),
        "-f",
        &format!("path={path}"),
        "-f",
        &format!("side={side}"),
        // -F, not -f: line must be a JSON integer, not a string.
        "-F",
        &format!("line={line}"),
    ])?;
    Ok(())
}

/// The verdict of a top-level PR review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewVerdict {
    Approve,
    RequestChanges,
    Comment,
}

/// Submit a top-level review on the PR. GitHub requires a non-empty body for
/// request-changes and comment reviews (the caller enforces this so the
/// error surfaces before a network round-trip); approvals may be bodyless.
pub fn submit_review(loc: &PrLocator, verdict: ReviewVerdict, body: &str) -> Result<()> {
    let number = loc.number.to_string();
    let slug = loc.repo_slug();
    let flag = match verdict {
        ReviewVerdict::Approve => "--approve",
        ReviewVerdict::RequestChanges => "--request-changes",
        ReviewVerdict::Comment => "--comment",
    };
    let mut args = vec!["pr", "review", &number, "--repo", &slug, flag];
    if !body.is_empty() {
        args.push("--body");
        args.push(body);
    }
    gh(&args)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    fn flag(self) -> &'static str {
        match self {
            Self::Merge => "--merge",
            Self::Squash => "--squash",
            Self::Rebase => "--rebase",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MergeStatus {
    pub is_draft: bool,
    pub mergeable: String,
    pub merge_state_status: String,
    pub review_decision: String,
    pub head_ref_oid: String,
    pub passing_checks: usize,
    pub pending_checks: Vec<String>,
    pub failing_checks: Vec<String>,
}

impl MergeStatus {
    pub fn blockers(&self) -> Vec<String> {
        let mut blockers = Vec::new();
        if self.is_draft {
            blockers.push("pull request is still a draft".to_string());
        }
        if self.review_decision != "APPROVED" {
            blockers.push("pull request does not have an approval".to_string());
        }
        match self.mergeable.as_str() {
            "MERGEABLE" => {}
            "CONFLICTING" => blockers.push("branch has merge conflicts".to_string()),
            _ => blockers.push("GitHub has not confirmed mergeability".to_string()),
        }
        if !self.pending_checks.is_empty() {
            blockers.push(format!(
                "{} check{} still running",
                self.pending_checks.len(),
                if self.pending_checks.len() == 1 { "" } else { "s" }
            ));
        }
        if !self.failing_checks.is_empty() {
            blockers.push(format!(
                "{} check{} failed",
                self.failing_checks.len(),
                if self.failing_checks.len() == 1 { "" } else { "s" }
            ));
        }
        blockers
    }

    pub fn can_merge(&self) -> bool {
        self.blockers().is_empty()
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMergeStatus {
    is_draft: bool,
    mergeable: String,
    merge_state_status: String,
    #[serde(default)]
    review_decision: String,
    head_ref_oid: String,
    #[serde(default)]
    status_check_rollup: Vec<serde_json::Value>,
}

fn normalize_merge_status(raw: RawMergeStatus) -> MergeStatus {
    let mut passing_checks = 0;
    let mut pending_checks = Vec::new();
    let mut failing_checks = Vec::new();
    for check in raw.status_check_rollup {
        let kind = check.get("__typename").and_then(|v| v.as_str()).unwrap_or("");
        let name = check
            .get("name")
            .or_else(|| check.get("context"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown check")
            .to_string();
        match kind {
            "CheckRun" => {
                let status = check.get("status").and_then(|v| v.as_str()).unwrap_or("");
                let conclusion =
                    check.get("conclusion").and_then(|v| v.as_str()).unwrap_or("");
                if status != "COMPLETED" {
                    pending_checks.push(name);
                } else if matches!(conclusion, "SUCCESS" | "NEUTRAL" | "SKIPPED") {
                    passing_checks += 1;
                } else {
                    failing_checks.push(name);
                }
            }
            "StatusContext" => match check.get("state").and_then(|v| v.as_str()).unwrap_or("") {
                "SUCCESS" => passing_checks += 1,
                "PENDING" | "EXPECTED" => pending_checks.push(name),
                _ => failing_checks.push(name),
            },
            _ => pending_checks.push(name),
        }
    }
    MergeStatus {
        is_draft: raw.is_draft,
        mergeable: raw.mergeable,
        merge_state_status: raw.merge_state_status,
        review_decision: raw.review_decision,
        head_ref_oid: raw.head_ref_oid,
        passing_checks,
        pending_checks,
        failing_checks,
    }
}

pub fn fetch_merge_status(loc: &PrLocator) -> Result<MergeStatus> {
    let json = gh(&[
        "pr",
        "view",
        &loc.number.to_string(),
        "--repo",
        &loc.repo_slug(),
        "--json",
        "isDraft,mergeable,mergeStateStatus,reviewDecision,headRefOid,statusCheckRollup",
    ])?;
    let raw: RawMergeStatus =
        serde_json::from_str(&json).context("unexpected gh pr merge-status JSON")?;
    Ok(normalize_merge_status(raw))
}

fn merge_args(
    loc: &PrLocator,
    method: MergeMethod,
    delete_branch: bool,
    head_oid: &str,
) -> Vec<String> {
    let mut args = vec![
        "pr".to_string(),
        "merge".to_string(),
        loc.number.to_string(),
        "--repo".to_string(),
        loc.repo_slug(),
        method.flag().to_string(),
        "--match-head-commit".to_string(),
        head_oid.to_string(),
    ];
    if delete_branch {
        args.push("--delete-branch".to_string());
    }
    args
}

pub fn merge_pr(
    loc: &PrLocator,
    method: MergeMethod,
    delete_branch: bool,
    head_oid: &str,
) -> Result<()> {
    let args = merge_args(loc, method, delete_branch, head_oid);
    // Run outside the app's source workspace. With --delete-branch, gh may
    // also delete a matching local branch when its current directory is a
    // clone of the target repository.
    let output = Command::new("gh")
        .args(&args)
        .current_dir(std::env::temp_dir())
        .output()
        .map_err(|err| anyhow!("failed to run gh (is the GitHub CLI installed?): {err}"))?;
    if !output.status.success() {
        bail!(
            "gh {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Reply to the review thread rooted at `comment_id`.
pub fn post_reply(loc: &PrLocator, comment_id: u64, body: &str) -> Result<()> {
    gh(&[
        "api",
        "-X",
        "POST",
        &format!(
            "repos/{}/{}/pulls/{}/comments/{}/replies",
            loc.owner, loc.repo, loc.number, comment_id
        ),
        "-f",
        &format!("body={body}"),
    ])?;
    Ok(())
}

pub fn fetch_patch(loc: &PrLocator) -> Result<String> {
    gh(&[
        "pr",
        "diff",
        &loc.number.to_string(),
        "--repo",
        &loc.repo_slug(),
    ])
}

/// GitHub refuses to serve one combined patch after its hosted diff limits
/// are reached. Keep other `gh pr diff` failures on their normal error path.
pub fn is_diff_too_large_error(err: &anyhow::Error) -> bool {
    let message = format!("{err:#}").to_lowercase();
    message.contains("diff too large")
        || message.contains("diff is too large")
        || message.contains("diff too big")
        || message.contains("diff exceeded the maximum")
        || message.contains("pullrequest.diff too_large")
}

/// Fetch a large PR through Git into an app-owned bare partial clone, then
/// build the same merge-base-to-head patch locally. `preview_root` must be
/// unique to one open app item; the app owns its cleanup.
pub fn fetch_patch_locally(
    loc: &PrLocator,
    meta: &PrMeta,
    preview_root: &std::path::Path,
) -> Result<String> {
    if preview_root.exists() {
        std::fs::remove_dir_all(preview_root)
            .with_context(|| format!("couldn't reset preview {}", preview_root.display()))?;
    }
    std::fs::create_dir_all(preview_root)
        .with_context(|| format!("couldn't create preview {}", preview_root.display()))?;
    let repo = preview_root.join("repo.git");
    let repo_path = repo.to_string_lossy().into_owned();
    let clone = Command::new("gh")
        .args([
            "repo",
            "clone",
            &loc.repo_slug(),
            &repo_path,
            "--",
            "--bare",
            "--filter=blob:none",
            "--single-branch",
            "--no-tags",
        ])
        .output()
        .map_err(|err| anyhow!("failed to run gh (is the GitHub CLI installed?): {err}"))?;
    if !clone.status.success() {
        bail!(
            "gh repo clone {} failed: {}",
            loc.repo_slug(),
            String::from_utf8_lossy(&clone.stderr).trim()
        );
    }

    let base_ref = format!("+refs/heads/{}:refs/lgtm/base", meta.base_ref_name);
    let head_ref = format!("+refs/pull/{}/head:refs/lgtm/head", loc.number);
    git(&repo, &["fetch", "--no-tags", "origin", &base_ref, &head_ref])?;
    let merge_base = git(&repo, &["merge-base", "refs/lgtm/base", "refs/lgtm/head"])?;
    git(
        &repo,
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

/// Blob-size cap: PR review never needs multi-megabyte files, and the raw
/// contents API happily serves up to 100 MB.
const MAX_BLOB_BYTES: usize = 1024 * 1024;

/// Full contents of `path` at `commit_oid`, via the raw contents API.
/// `Ok(None)` means "leave this file un-upgraded": absent on that side (404),
/// non-UTF-8 (binary), or larger than [`MAX_BLOB_BYTES`]. A path at a commit
/// is immutable, so results — including negative ones — are cached on disk in
/// `~/.cache/lgtm/blobs/` (sha256 of `repo\0oid\0path`, with an `.absent`
/// sidecar marking negative entries).
pub fn fetch_file_at(loc: &PrLocator, commit_oid: &str, path: &str) -> Result<Option<String>> {
    fetch_file_at_with_cache(loc, commit_oid, path, None)
}

/// The same immutable blob fetch, but with an item-owned cache directory that
/// can be removed when its preview closes.
pub fn fetch_file_at_in(
    loc: &PrLocator,
    commit_oid: &str,
    path: &str,
    cache_root: &std::path::Path,
) -> Result<Option<String>> {
    fetch_file_at_with_cache(loc, commit_oid, path, Some(cache_root))
}

fn fetch_file_at_with_cache(
    loc: &PrLocator,
    commit_oid: &str,
    path: &str,
    cache_root: Option<&std::path::Path>,
) -> Result<Option<String>> {
    let cache = cache_path(cache_root, &loc.repo_slug(), commit_oid, path);
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
    let output = Command::new("gh")
        .args(["api", "-H", "Accept: application/vnd.github.raw+json", &endpoint])
        .output()
        .map_err(|err| anyhow!("failed to run gh (is the GitHub CLI installed?): {err}"))?;
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

/// `~/.cache/lgtm/blobs/<key>`, creating the directory; None when HOME is
/// unset or the directory can't be created (cache disabled, fetch still works).
fn cache_path(root: Option<&std::path::Path>, repo: &str, oid: &str, path: &str) -> Option<PathBuf> {
    let dir = match root {
        Some(root) => root.to_path_buf(),
        None => PathBuf::from(std::env::var_os("HOME")?)
            .join(".cache")
            .join("lgtm")
            .join("blobs"),
    };
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join(cache_key(repo, oid, path)))
}

/// Stable cache key: hex sha256 of `repo\0oid\0path`.
pub fn cache_key(repo: &str, oid: &str, path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(repo.as_bytes());
    hasher.update([0]);
    hasher.update(oid.as_bytes());
    hasher.update([0]);
    hasher.update(path.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Percent-encode a repo path for the contents API, keeping `/` separators:
/// every byte outside RFC 3986 unreserved is encoded, per segment.
pub fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn gh(args: &[&str]) -> Result<String> {
    let output = Command::new("gh")
        .args(args)
        .output()
        .map_err(|err| anyhow!("failed to run gh (is the GitHub CLI installed?): {err}"))?;
    if !output.status.success() {
        bail!(
            "gh {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("gh output was not UTF-8")
}

fn git(repo: &std::path::Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|err| anyhow!("failed to run git (is git installed?): {err}"))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("git output was not UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_pr(repo: &str, number: u64, updated_at: &str) -> UserPrSummary {
        UserPrSummary {
            number,
            title: format!("PR {number}"),
            author: Author { login: "alice".to_string() },
            is_draft: false,
            updated_at: updated_at.to_string(),
            repository: Repository {
                name: repo.rsplit('/').next().unwrap().to_string(),
                name_with_owner: repo.to_string(),
            },
            url: format!("https://github.com/{repo}/pull/{number}"),
        }
    }

    #[test]
    fn parses_slug_form() {
        let loc = resolve_pr_arg("zed-industries/zed#12345").unwrap();
        assert_eq!(loc.owner, "zed-industries");
        assert_eq!(loc.repo, "zed");
        assert_eq!(loc.number, 12345);
    }

    #[test]
    fn parses_url_form() {
        let loc = resolve_pr_arg("https://github.com/rust-lang/rust/pull/99999/files").unwrap();
        assert_eq!(loc.owner, "rust-lang");
        assert_eq!(loc.repo, "rust");
        assert_eq!(loc.number, 99999);
    }

    #[test]
    fn rejects_garbage() {
        assert!(resolve_pr_arg("not-a-pr").is_err());
    }

    #[test]
    fn recognizes_only_large_diff_errors_for_local_fallback() {
        assert!(is_diff_too_large_error(&anyhow!(
            "gh pr diff failed: HTTP 406: Diff too large"
        )));
        assert!(is_diff_too_large_error(&anyhow!("diff is too large to render")));
        assert!(is_diff_too_large_error(&anyhow!(
            "HTTP 406: Sorry, the diff exceeded the maximum number of lines (20000)\n\
             PullRequest.diff too_large"
        )));
        assert!(!is_diff_too_large_error(&anyhow!("HTTP 401: Bad credentials")));
        assert!(!is_diff_too_large_error(&anyhow!("network connection failed")));
    }

    #[test]
    fn merge_status_normalizes_checks_and_reports_blockers() {
        let json = r#"{
            "isDraft": false,
            "mergeable": "MERGEABLE",
            "mergeStateStatus": "BLOCKED",
            "reviewDecision": "APPROVED",
            "headRefOid": "abc123",
            "statusCheckRollup": [
                {"__typename":"CheckRun","name":"test","status":"COMPLETED","conclusion":"SUCCESS"},
                {"__typename":"CheckRun","name":"lint","status":"IN_PROGRESS","conclusion":""},
                {"__typename":"StatusContext","context":"deploy","state":"FAILURE"}
            ]
        }"#;
        let raw: RawMergeStatus = serde_json::from_str(json).unwrap();
        let status = normalize_merge_status(raw);
        assert_eq!(status.passing_checks, 1);
        assert_eq!(status.pending_checks, vec!["lint"]);
        assert_eq!(status.failing_checks, vec!["deploy"]);
        assert_eq!(
            status.blockers(),
            vec!["1 check still running", "1 check failed"]
        );
        assert!(!status.can_merge());
    }

    #[test]
    fn merge_status_blocks_drafts_unapproved_and_conflicting_prs() {
        let status = MergeStatus {
            is_draft: true,
            mergeable: "CONFLICTING".to_string(),
            merge_state_status: "DIRTY".to_string(),
            review_decision: String::new(),
            head_ref_oid: "abc123".to_string(),
            passing_checks: 2,
            pending_checks: Vec::new(),
            failing_checks: Vec::new(),
        };
        assert_eq!(
            status.blockers(),
            vec![
                "pull request is still a draft",
                "pull request does not have an approval",
                "branch has merge conflicts",
            ]
        );
    }

    #[test]
    fn merge_arguments_include_method_head_guard_and_optional_delete() {
        let loc = PrLocator {
            owner: "ellie".to_string(),
            repo: "lgtm".to_string(),
            number: 8,
        };
        assert_eq!(
            merge_args(&loc, MergeMethod::Squash, true, "abc123"),
            vec![
                "pr", "merge", "8", "--repo", "ellie/lgtm", "--squash",
                "--match-head-commit", "abc123", "--delete-branch",
            ]
        );
    }

    #[test]
    fn encodes_paths_per_segment_keeping_slashes() {
        assert_eq!(encode_path("src/main.rs"), "src/main.rs");
        assert_eq!(
            encode_path("dir with space/naïve+file#1.rs"),
            "dir%20with%20space/na%C3%AFve%2Bfile%231.rs"
        );
        assert_eq!(encode_path("a?b&c=d/e%f"), "a%3Fb%26c%3Dd/e%25f");
        assert_eq!(encode_path("A-Z_a.z~0/9"), "A-Z_a.z~0/9");
    }

    #[test]
    fn cache_key_is_stable() {
        // Pinned: changing this constant silently invalidates every user's
        // on-disk cache. The components are NUL-separated so `("a/b", "c")`
        // and `("a", "b/c")` can't collide.
        assert_eq!(
            cache_key(
                "BurntSushi/ripgrep",
                "f16ea0a8cfd0fbb0328b8348972356d532b921d0",
                "crates/core/main.rs"
            ),
            "1b21e76f45d6d948cf7b44c696608c64bdaeee714ac61b21dce21750ad9cb6bc"
        );
        assert_ne!(
            cache_key("o/r", "oid", "a/b"),
            cache_key("o/r/a", "oid", "b")
        );
    }

    #[test]
    fn deserializes_pr_meta_with_oids() {
        let json = r#"{
            "number": 1, "title": "t", "author": {"login": "a"}, "state": "OPEN",
            "url": "https://github.com/o/r/pull/1",
            "baseRefName": "main", "headRefName": "feat",
            "baseRefOid": "abc123", "headRefOid": "def456",
            "additions": 1, "deletions": 2, "changedFiles": 3, "reviewDecision": "CHANGES_REQUESTED"
        }"#;
        let meta: PrMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.base_ref_oid, "abc123");
        assert_eq!(meta.head_ref_oid, "def456");
        assert_eq!(meta.review_decision, "CHANGES_REQUESTED");

        // Older gh output without the field still deserializes.
        let json = json.replace(r#", "reviewDecision": "CHANGES_REQUESTED""#, "");
        let meta: PrMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta.review_decision, "");
    }

    #[test]
    fn deserializes_user_pr_with_repository_identity() {
        let json = r#"{
            "number": 8,
            "title": "Add selector",
            "author": {"login": "alice"},
            "isDraft": true,
            "updatedAt": "2026-07-28T10:48:41Z",
            "repository": {"name": "lgtm", "nameWithOwner": "ellie/lgtm"},
            "url": "https://github.com/ellie/lgtm/pull/8"
        }"#;
        let pr: UserPrSummary = serde_json::from_str(json).unwrap();
        assert_eq!(pr.repository.name, "lgtm");
        assert_eq!(pr.repository.name_with_owner, "ellie/lgtm");
        assert_eq!(pr.number, 8);
        assert!(pr.is_draft);
    }

    #[test]
    fn merges_user_prs_without_duplicates_and_sorts_latest_first() {
        let authored = vec![
            user_pr("ellie/lgtm", 1, "2026-07-20T00:00:00Z"),
            user_pr("ellie/lgtm", 2, "2026-07-25T00:00:00Z"),
        ];
        let requested = vec![
            user_pr("ellie/lgtm", 1, "2026-07-21T00:00:00Z"),
            user_pr("zed-industries/zed", 3, "2026-07-30T00:00:00Z"),
        ];
        let merged = merge_user_prs(authored, requested);
        let ids: Vec<_> = merged.iter()
            .map(|pr| (pr.repository.name_with_owner.as_str(), pr.number))
            .collect();
        assert_eq!(
            ids,
            vec![
                ("zed-industries/zed", 3),
                ("ellie/lgtm", 2),
                ("ellie/lgtm", 1),
            ]
        );
        assert_eq!(merged[2].updated_at, "2026-07-21T00:00:00Z");
    }

    #[test]
    fn deserializes_review_comments() {
        // Shaped like `gh api --paginate --slurp`: one array per page. The
        // REST payload is snake_case and carries fields we ignore.
        let json = r#"[[
            {
                "id": 100,
                "node_id": "x",
                "path": "src/main.rs",
                "line": 42,
                "side": "RIGHT",
                "start_line": 40,
                "start_side": "RIGHT",
                "body": "top-level comment",
                "user": {"login": "alice", "id": 1},
                "created_at": "2026-07-01T12:00:00Z",
                "in_reply_to_id": null
            },
            {
                "id": 101,
                "path": "src/main.rs",
                "line": 42,
                "side": "RIGHT",
                "start_line": null,
                "body": "a reply",
                "user": {"login": "bob"},
                "created_at": "2026-07-02T08:30:00Z",
                "in_reply_to_id": 100
            }
        ], [
            {
                "id": 102,
                "path": "old.rs",
                "line": null,
                "side": null,
                "start_line": null,
                "body": "outdated",
                "user": {"login": "carol"},
                "created_at": "2026-06-01T00:00:00Z"
            }
        ]]"#;
        let pages: Vec<Vec<ReviewComment>> = serde_json::from_str(json).unwrap();
        let comments: Vec<ReviewComment> = pages.into_iter().flatten().collect();
        assert_eq!(comments.len(), 3);
        let top = &comments[0];
        assert_eq!(top.id, 100);
        assert_eq!(top.path, "src/main.rs");
        assert_eq!(top.line, Some(42));
        assert_eq!(top.side.as_deref(), Some("RIGHT"));
        assert_eq!(top.start_line, Some(40));
        assert_eq!(top.user.login, "alice");
        assert_eq!(top.in_reply_to_id, None);
        let reply = &comments[1];
        assert_eq!(reply.in_reply_to_id, Some(100));
        assert_eq!(reply.start_line, None);
        // Outdated: null line/side, and a missing in_reply_to_id key.
        let outdated = &comments[2];
        assert_eq!(outdated.line, None);
        assert_eq!(outdated.side, None);
        assert_eq!(outdated.in_reply_to_id, None);
    }

    #[test]
    fn deserializes_pr_list_json() {
        let json = r#"[
            {
                "number": 3468,
                "title": "printer: add --field-name-terminator flag",
                "author": {"id": "x", "is_bot": false, "login": "alice", "name": "Alice"},
                "state": "OPEN",
                "isDraft": false,
                "headRefName": "field-name-terminator",
                "updatedAt": "2026-07-01T12:34:56Z"
            },
            {
                "number": 3470,
                "title": "wip: experiment",
                "author": {"login": "bob"},
                "state": "OPEN",
                "isDraft": true,
                "headRefName": "bob/wip",
                "updatedAt": "2026-06-30T08:00:00Z"
            }
        ]"#;
        let prs: Vec<PrSummary> = serde_json::from_str(json).unwrap();
        assert_eq!(prs.len(), 2);
        assert_eq!(prs[0].number, 3468);
        assert_eq!(prs[0].author.login, "alice");
        assert_eq!(prs[0].state, "OPEN");
        assert!(!prs[0].is_draft);
        assert_eq!(prs[0].head_ref_name, "field-name-terminator");
        assert_eq!(prs[0].updated_at, "2026-07-01T12:34:56Z");
        assert!(prs[1].is_draft);
    }
}
