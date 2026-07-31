use crate::command::run_gh;
use crate::model::{Author, PrLocator};
use anyhow::{Context, Result};

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReviewComment {
    pub id: u64,
    pub path: String,
    pub line: Option<u64>,
    pub side: Option<String>,
    pub start_line: Option<u64>,
    pub body: String,
    pub user: Author,
    pub created_at: String,
    pub in_reply_to_id: Option<u64>,
}

pub fn fetch_review_comments(loc: &PrLocator) -> Result<Vec<ReviewComment>> {
    let json = run_gh(&[
        "api",
        "--paginate",
        "--slurp",
        &format!(
            "repos/{}/{}/pulls/{}/comments?per_page=100",
            loc.owner, loc.repo, loc.number
        ),
    ])?;
    let pages: Vec<Vec<ReviewComment>> = serde_json::from_str(&json).context("unexpected gh pulls/comments JSON")?;
    Ok(pages.into_iter().flatten().collect())
}

pub fn post_review_comment(
    loc: &PrLocator,
    commit_id: &str,
    path: &str,
    side: &str,
    line: u64,
    body: &str,
) -> Result<()> {
    run_gh(&[
        "api",
        "-X",
        "POST",
        &format!("repos/{}/{}/pulls/{}/comments", loc.owner, loc.repo, loc.number),
        "-f",
        &format!("body={body}"),
        "-f",
        &format!("commit_id={commit_id}"),
        "-f",
        &format!("path={path}"),
        "-f",
        &format!("side={side}"),
        "-F",
        &format!("line={line}"),
    ])?;
    Ok(())
}

pub fn post_reply(loc: &PrLocator, comment_id: u64, body: &str) -> Result<()> {
    run_gh(&[
        "api",
        "-X",
        "POST",
        &format!(
            "repos/{}/{}/pulls/{}/comments/{comment_id}/replies",
            loc.owner, loc.repo, loc.number
        ),
        "-f",
        &format!("body={body}"),
    ])?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewVerdict {
    Approve,
    RequestChanges,
    Comment,
}

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
        args.extend(["--body", body]);
    }
    run_gh(&args)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_paginated_comments() {
        let json = r#"[[{"id":1,"path":"a.rs","line":2,"side":"RIGHT","start_line":null,"body":"x","user":{"login":"a"},"created_at":"now","in_reply_to_id":null}]]"#;
        let pages: Vec<Vec<ReviewComment>> = serde_json::from_str(json).unwrap();
        assert_eq!(pages[0][0].path, "a.rs");
    }
}
