use crate::command::run_gh;
use crate::model::Author;
use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;

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

pub fn list_prs(owner: &str, repo: &str) -> Result<Vec<PrSummary>> {
    let json = run_gh(&[
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

fn search_user_prs(filter: &str) -> Result<Vec<UserPrSummary>> {
    let json = run_gh(&[
        "search",
        "prs",
        filter,
        "--state=open",
        "--limit=200",
        "--sort=updated",
        "--order=desc",
        "--json=number,title,author,isDraft,updatedAt,repository,url",
    ])?;
    serde_json::from_str(&json).context("unexpected gh search prs JSON")
}

fn merge_user_prs(left: Vec<UserPrSummary>, right: Vec<UserPrSummary>) -> Vec<UserPrSummary> {
    let mut by_id = HashMap::new();
    for pr in left.into_iter().chain(right) {
        let key = (pr.repository.name_with_owner.clone(), pr.number);
        if by_id
            .get(&key)
            .is_none_or(|old: &UserPrSummary| old.updated_at < pr.updated_at)
        {
            by_id.insert(key, pr);
        }
    }
    let mut prs: Vec<_> = by_id.into_values().collect();
    prs.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| {
                a.repository
                    .name_with_owner
                    .cmp(&b.repository.name_with_owner)
            })
            .then_with(|| a.number.cmp(&b.number))
    });
    prs
}

pub fn list_user_prs() -> Result<Vec<UserPrSummary>> {
    list_user_prs_with(search_user_prs)
}

fn list_user_prs_with(
    search: impl Fn(&str) -> Result<Vec<UserPrSummary>> + Sync,
) -> Result<Vec<UserPrSummary>> {
    let (authored, requested) = std::thread::scope(|scope| {
        let authored = scope.spawn(|| search("--author=@me"));
        let requested = scope.spawn(|| search("--review-requested=@me"));
        (authored.join(), requested.join())
    });
    let authored = authored.map_err(|_| anyhow!("authored PR search panicked"))??;
    let requested = requested.map_err(|_| anyhow!("review-requested PR search panicked"))??;
    Ok(merge_user_prs(authored, requested))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(repo: &str, number: u64, updated: &str) -> UserPrSummary {
        UserPrSummary {
            number,
            title: format!("PR {number}"),
            author: Author { login: "a".into() },
            is_draft: false,
            updated_at: updated.into(),
            repository: Repository {
                name: repo.rsplit('/').next().unwrap().into(),
                name_with_owner: repo.into(),
            },
            url: format!("https://github.com/{repo}/pull/{number}"),
        }
    }

    #[test]
    fn merges_deduplicates_and_sorts() {
        let got = merge_user_prs(
            vec![pr("a/r", 1, "2026-01-01"), pr("a/r", 2, "2026-01-03")],
            vec![pr("a/r", 1, "2026-01-02"), pr("b/r", 3, "2026-01-04")],
        );
        let ids: Vec<_> = got
            .iter()
            .map(|pr| (pr.repository.name_with_owner.as_str(), pr.number))
            .collect();
        assert_eq!(ids, vec![("b/r", 3), ("a/r", 2), ("a/r", 1)]);
    }

    #[test]
    fn parses_nested_repository() {
        let pr: UserPrSummary = serde_json::from_str(r#"{"number":8,"title":"x","author":{"login":"a"},"isDraft":false,"updatedAt":"x","repository":{"name":"r","nameWithOwner":"a/r"},"url":"u"}"#).unwrap();
        assert_eq!(pr.repository.name_with_owner, "a/r");
    }

    #[test]
    fn user_searches_start_concurrently() {
        let barrier = std::sync::Barrier::new(2);
        let result = list_user_prs_with(|filter| {
            barrier.wait();
            Ok(vec![pr(
                if filter.contains("author") {
                    "a/r"
                } else {
                    "b/r"
                },
                1,
                "now",
            )])
        })
        .unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn either_search_failure_fails_the_atomic_refresh() {
        let error = list_user_prs_with(|filter| {
            if filter.contains("review-requested") {
                Err(anyhow::anyhow!("offline"))
            } else {
                Ok(vec![pr("a/r", 1, "now")])
            }
        })
        .unwrap_err();
        assert!(format!("{error:#}").contains("offline"));
    }
}
