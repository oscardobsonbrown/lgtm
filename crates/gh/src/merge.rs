use crate::command::run_gh;
use crate::model::{PrLocator, ReviewDecision};
use anyhow::{anyhow, bail, Context, Result};
use std::process::Command;

macro_rules! remote_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum $name { $($variant,)+ Unknown(String) }
        impl $name {
            pub fn as_str(&self) -> &str {
                match self { $(Self::$variant => $value,)+ Self::Unknown(value) => value }
            }
        }
        impl From<String> for $name {
            fn from(value: String) -> Self {
                match value.as_str() { $($value => Self::$variant,)+ _ => Self::Unknown(value) }
            }
        }
    };
}

remote_enum!(Mergeability {
    Mergeable => "MERGEABLE",
    Conflicting => "CONFLICTING",
});

remote_enum!(MergeState {
    Behind => "BEHIND",
    Blocked => "BLOCKED",
    Clean => "CLEAN",
    Dirty => "DIRTY",
    Draft => "DRAFT",
    HasHooks => "HAS_HOOKS",
    Unstable => "UNSTABLE",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Passing,
    Pending,
    Failing,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckSummary {
    pub name: String,
    pub state: CheckState,
}

#[derive(Debug, Clone)]
pub struct MergeAssessment {
    pub is_draft: bool,
    pub mergeability: Mergeability,
    pub merge_state: MergeState,
    pub review_decision: ReviewDecision,
    pub head_oid: String,
    pub checks: Vec<CheckSummary>,
}

impl MergeAssessment {
    pub fn can_attempt(&self) -> bool {
        self.review_decision == ReviewDecision::Approved && !self.head_oid.is_empty()
    }

    pub fn checks_with(&self, state: CheckState) -> impl Iterator<Item = &CheckSummary> {
        self.checks.iter().filter(move |check| check.state == state)
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAssessment {
    is_draft: bool,
    mergeable: String,
    merge_state_status: String,
    #[serde(default)]
    review_decision: ReviewDecision,
    head_ref_oid: String,
    #[serde(default)]
    status_check_rollup: Vec<RawCheck>,
}

#[derive(serde::Deserialize)]
#[serde(tag = "__typename")]
enum RawCheck {
    CheckRun {
        name: String,
        status: String,
        conclusion: Option<String>,
    },
    StatusContext {
        context: String,
        state: String,
    },
    #[serde(other)]
    Unknown,
}

fn normalize(raw: RawAssessment) -> MergeAssessment {
    let checks = raw
        .status_check_rollup
        .into_iter()
        .map(|check| match check {
            RawCheck::CheckRun {
                name,
                status,
                conclusion,
            } => {
                let state = if status != "COMPLETED" {
                    CheckState::Pending
                } else if matches!(
                    conclusion.as_deref(),
                    Some("SUCCESS" | "NEUTRAL" | "SKIPPED")
                ) {
                    CheckState::Passing
                } else {
                    CheckState::Failing
                };
                CheckSummary { name, state }
            }
            RawCheck::StatusContext { context, state } => CheckSummary {
                name: context,
                state: match state.as_str() {
                    "SUCCESS" => CheckState::Passing,
                    "PENDING" | "EXPECTED" => CheckState::Pending,
                    "ERROR" | "FAILURE" => CheckState::Failing,
                    _ => CheckState::Unknown,
                },
            },
            RawCheck::Unknown => CheckSummary {
                name: "unknown check".into(),
                state: CheckState::Unknown,
            },
        })
        .collect();
    MergeAssessment {
        is_draft: raw.is_draft,
        mergeability: raw.mergeable.into(),
        merge_state: raw.merge_state_status.into(),
        review_decision: raw.review_decision,
        head_oid: raw.head_ref_oid,
        checks,
    }
}

pub fn fetch_merge_assessment(loc: &PrLocator) -> Result<MergeAssessment> {
    let json = run_gh(&[
        "pr",
        "view",
        &loc.number.to_string(),
        "--repo",
        &loc.repo_slug(),
        "--json",
        "isDraft,mergeable,mergeStateStatus,reviewDecision,headRefOid,statusCheckRollup",
    ])?;
    let raw = serde_json::from_str(&json).context("unexpected gh merge assessment JSON")?;
    Ok(normalize(raw))
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

fn merge_args(
    loc: &PrLocator,
    method: MergeMethod,
    delete_branch: bool,
    head_oid: &str,
) -> Vec<String> {
    let mut args = vec![
        "pr".into(),
        "merge".into(),
        loc.number.to_string(),
        "--repo".into(),
        loc.repo_slug(),
        method.flag().into(),
        "--match-head-commit".into(),
        head_oid.into(),
    ];
    if delete_branch {
        args.push("--delete-branch".into());
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
    let output = Command::new("gh")
        .args(&args)
        .current_dir(std::env::temp_dir())
        .output()
        .map_err(|err| anyhow!("failed to run gh: {err}"))?;
    if !output.status.success() {
        bail!(
            "gh {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(state: &str, decision: &str, checks: &str) -> MergeAssessment {
        let json = format!(
            r#"{{"isDraft":false,"mergeable":"MERGEABLE","mergeStateStatus":"{state}","reviewDecision":"{decision}","headRefOid":"abc","statusCheckRollup":{checks}}}"#
        );
        normalize(serde_json::from_str(&json).unwrap())
    }

    #[test]
    fn parses_every_documented_merge_state() {
        for (raw, expected) in [
            ("BEHIND", MergeState::Behind),
            ("BLOCKED", MergeState::Blocked),
            ("CLEAN", MergeState::Clean),
            ("DIRTY", MergeState::Dirty),
            ("DRAFT", MergeState::Draft),
            ("HAS_HOOKS", MergeState::HasHooks),
            ("UNSTABLE", MergeState::Unstable),
        ] {
            assert_eq!(parse(raw, "APPROVED", "[]").merge_state, expected);
        }
        assert_eq!(
            parse("FUTURE", "APPROVED", "[]").merge_state,
            MergeState::Unknown("FUTURE".into())
        );
    }

    #[test]
    fn parses_both_check_shapes_without_values() {
        let status = parse(
            "BLOCKED",
            "APPROVED",
            r#"[{"__typename":"CheckRun","name":"test","status":"COMPLETED","conclusion":"SUCCESS"},{"__typename":"StatusContext","context":"deploy","state":"FAILURE"}]"#,
        );
        assert_eq!(status.checks[0].state, CheckState::Passing);
        assert_eq!(status.checks[1].state, CheckState::Failing);
        assert!(status.can_attempt());
    }

    #[test]
    fn attempts_require_current_approval_and_head() {
        assert!(!parse("CLEAN", "CHANGES_REQUESTED", "[]").can_attempt());
        let mut status = parse("CLEAN", "APPROVED", "[]");
        status.head_oid.clear();
        assert!(!status.can_attempt());
    }

    #[test]
    fn arguments_keep_head_guard() {
        let loc = PrLocator {
            owner: "a".into(),
            repo: "r".into(),
            number: 8,
        };
        assert_eq!(
            merge_args(&loc, MergeMethod::Squash, true, "abc"),
            vec![
                "pr",
                "merge",
                "8",
                "--repo",
                "a/r",
                "--squash",
                "--match-head-commit",
                "abc",
                "--delete-branch"
            ]
        );
    }
}
