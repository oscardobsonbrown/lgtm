//! Fetch pull request data through the authenticated `gh` CLI.

macro_rules! remote_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum $name {
            $($variant,)+
            Unknown(String),
        }

        impl Default for $name {
            fn default() -> Self {
                Self::Unknown(String::new())
            }
        }

        impl $name {
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $value,)+
                    Self::Unknown(value) => value,
                }
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                match value.as_str() {
                    $($value => Self::$variant,)+
                    _ => Self::Unknown(value),
                }
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                Ok(<String as serde::Deserialize>::deserialize(deserializer)?.into())
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

mod command;
mod merge;
mod model;
mod preview;
mod review;
mod search;

pub use merge::{
    fetch_merge_assessment, merge_pr, CheckState, CheckSummary, MergeAssessment, MergeMethod, MergeState, Mergeability,
};
pub use model::{fetch_meta, resolve_pr_arg, Author, PrLocator, PrMeta, PrState, ReviewDecision};
pub use preview::{
    cache_key, encode_path, fetch_file_at, fetch_file_at_in, fetch_patch, fetch_patch_locally, is_diff_too_large_error,
};
pub use review::{fetch_review_comments, post_reply, post_review_comment, submit_review, ReviewComment, ReviewVerdict};
pub use search::{list_prs, list_user_prs, PrSummary, Repository, UserPrSummary};
