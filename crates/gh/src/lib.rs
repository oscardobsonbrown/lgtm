//! Fetch pull request data through the authenticated `gh` CLI.

mod command;
mod merge;
mod model;
mod preview;
mod review;
mod search;

pub use merge::{
    fetch_merge_assessment, merge_pr, CheckState, CheckSummary, MergeAssessment, MergeMethod,
    MergeState, Mergeability,
};
pub use model::{fetch_meta, resolve_pr_arg, Author, PrLocator, PrMeta, PrState, ReviewDecision};
pub use preview::{
    cache_key, encode_path, fetch_file_at, fetch_file_at_in, fetch_patch, fetch_patch_locally,
    is_diff_too_large_error,
};
pub use review::{
    fetch_review_comments, post_reply, post_review_comment, submit_review, ReviewComment,
    ReviewVerdict,
};
pub use search::{list_prs, list_user_prs, PrSummary, Repository, UserPrSummary};
