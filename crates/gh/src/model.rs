use crate::command::run_gh;
use anyhow::{bail, Context, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
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

remote_enum!(PrState {
    Open => "OPEN",
    Closed => "CLOSED",
    Merged => "MERGED",
});

remote_enum!(ReviewDecision {
    Approved => "APPROVED",
    ChangesRequested => "CHANGES_REQUESTED",
    ReviewRequired => "REVIEW_REQUIRED",
});

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrMeta {
    pub number: u64,
    pub title: String,
    pub author: Author,
    pub state: PrState,
    #[serde(default)]
    pub is_draft: bool,
    pub url: String,
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
    #[serde(default)]
    pub review_decision: ReviewDecision,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Author {
    pub login: String,
}

pub fn resolve_pr_arg(arg: &str) -> Result<PrLocator> {
    if let Some(rest) = arg
        .strip_prefix("https://github.com/")
        .or_else(|| arg.strip_prefix("http://github.com/"))
        .or_else(|| arg.strip_prefix("github.com/"))
    {
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() >= 4 && parts[2] == "pull" {
            let digits: String = parts[3].chars().take_while(char::is_ascii_digit).collect();
            return Ok(PrLocator {
                owner: parts[0].to_string(),
                repo: parts[1].to_string(),
                number: digits.parse().with_context(|| format!("no PR number in URL {arg}"))?,
            });
        }
        bail!("unrecognized GitHub URL: {arg}");
    }
    if let Some((repo, number)) = arg.split_once('#') {
        let number = number.parse().with_context(|| format!("invalid PR number in {arg}"))?;
        if repo.is_empty() {
            return locator_in_cwd_repo(number);
        }
        let (owner, repo) = repo.split_once('/').context("expected owner/repo before '#'")?;
        return Ok(PrLocator {
            owner: owner.into(),
            repo: repo.into(),
            number,
        });
    }
    if let Ok(number) = arg.parse() {
        return locator_in_cwd_repo(number);
    }
    bail!("could not parse {arg:?}; expected owner/repo#123, a PR URL, or a PR number")
}

fn locator_in_cwd_repo(number: u64) -> Result<PrLocator> {
    let out = run_gh(&["repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"])
        .context("couldn't infer the repo from the current directory; use owner/repo#123")?;
    let (owner, repo) = out.trim().split_once('/').context("unexpected gh repo view output")?;
    Ok(PrLocator {
        owner: owner.into(),
        repo: repo.into(),
        number,
    })
}

pub fn fetch_meta(loc: &PrLocator) -> Result<PrMeta> {
    let json = run_gh(&[
        "pr", "view", &loc.number.to_string(), "--repo", &loc.repo_slug(), "--json",
        "number,title,author,state,isDraft,url,body,baseRefName,headRefName,baseRefOid,headRefOid,additions,deletions,changedFiles,reviewDecision",
    ])?;
    serde_json::from_str(&json).context("unexpected gh pr view JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_locators() {
        assert_eq!(resolve_pr_arg("ellie/lgtm#8").unwrap().number, 8);
        assert_eq!(
            resolve_pr_arg("https://github.com/ellie/lgtm/pull/9").unwrap().repo,
            "lgtm"
        );
        assert!(resolve_pr_arg("garbage").is_err());
    }

    #[test]
    fn typed_values_keep_unknown_remote_states() {
        let approved: ReviewDecision = serde_json::from_str("\"APPROVED\"").unwrap();
        let future: PrState = serde_json::from_str("\"FUTURE\"").unwrap();
        assert_eq!(approved, ReviewDecision::Approved);
        assert_eq!(future, PrState::Unknown("FUTURE".into()));
    }
}
