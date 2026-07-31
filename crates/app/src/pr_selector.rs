use super::{theme, ReviewApp};
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use gpui::{
    div, prelude::*, px, uniform_list, Context, Hsla, ScrollStrategy, SharedString,
    UniformListScrollHandle,
};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    IconName, Sizable as _,
};

const ROW_HEIGHT: f32 = 42.0;

#[derive(Debug, Clone, PartialEq, Eq)]
enum RefreshPhase {
    Idle,
    Loading { request: u64 },
    Failed { request: u64, message: SharedString },
}

pub(crate) struct PrSelector {
    rows: Vec<gh::UserPrSummary>,
    phase: RefreshPhase,
    next_request: u64,
    expanded: bool,
    scroll: UniformListScrollHandle,
}

impl PrSelector {
    pub(crate) fn new() -> Self {
        Self {
            rows: Vec::new(),
            phase: RefreshPhase::Idle,
            next_request: 0,
            expanded: true,
            scroll: UniformListScrollHandle::new(),
        }
    }

    pub(crate) fn begin_refresh(&mut self) -> u64 {
        self.next_request += 1;
        self.phase = RefreshPhase::Loading {
            request: self.next_request,
        };
        self.next_request
    }

    pub(crate) fn apply_result(
        &mut self,
        request: u64,
        result: anyhow::Result<Vec<gh::UserPrSummary>>,
    ) -> bool {
        if !matches!(self.phase, RefreshPhase::Loading { request: active } if active == request) {
            return false;
        }
        match result {
            Ok(rows) => {
                self.rows = rows;
                self.phase = RefreshPhase::Idle;
            }
            Err(error) => {
                self.phase = RefreshPhase::Failed {
                    request,
                    message: format!("{error:#}").into(),
                }
            }
        }
        true
    }

    pub(crate) fn remove(&mut self, repo: &str, number: u64) {
        self.rows
            .retain(|pr| pr.number != number || pr.repository.name_with_owner != repo);
    }

    pub(crate) fn is_loading(&self) -> bool {
        matches!(self.phase, RefreshPhase::Loading { .. })
    }
    pub(crate) fn query_changed(&self) {
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
    }
    fn error(&self) -> Option<SharedString> {
        match &self.phase {
            RefreshPhase::Failed { message, .. } => Some(message.clone()),
            _ => None,
        }
    }
}

pub(crate) fn fuzzy_indices<T>(
    items: &[T],
    query: &str,
    text: impl Fn(&T) -> String,
) -> Vec<usize> {
    let query = query.trim();
    if query.is_empty() {
        return (0..items.len()).collect();
    }
    let matcher = SkimMatcherV2::default();
    let mut scored: Vec<_> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            matcher
                .fuzzy_match(&text(item), query)
                .map(|score| (score, index))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, index)| index).collect()
}

fn filter(rows: &[gh::UserPrSummary], query: &str) -> Vec<usize> {
    fuzzy_indices(rows, query, |pr| {
        format!(
            "{} #{} {} {}",
            pr.repository.name_with_owner, pr.number, pr.title, pr.author.login
        )
    })
}

fn render_row(
    pr: &gh::UserPrSummary,
    pos: usize,
    entity: gpui::Entity<ReviewApp>,
) -> gpui::AnyElement {
    let repo = pr.repository.name_with_owner.clone();
    let number = pr.number;
    let dot = if pr.is_draft {
        theme::overlay0()
    } else {
        theme::green()
    };
    div()
        .id(("user-pr", pos))
        .mx_1()
        .px_2()
        .h(px(ROW_HEIGHT))
        .rounded_md()
        .flex()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .hover(|style| style.bg(Hsla::from(theme::surface0()).opacity(0.5)))
        .on_click(move |_, window, cx| {
            entity.update(cx, |app, cx| {
                app.open_or_activate_pr(&repo, number, window, cx)
            })
        })
        .child(
            div()
                .w(px(8.))
                .h(px(8.))
                .flex_shrink_0()
                .rounded_full()
                .bg(dot),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .truncate()
                        .text_color(theme::text())
                        .child(SharedString::from(pr.title.clone())),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(10.))
                        .text_color(theme::subtext())
                        .child(SharedString::from(format!(
                            "{}#{} · @{}",
                            pr.repository.name_with_owner, pr.number, pr.author.login
                        ))),
                ),
        )
        .into_any_element()
}

impl ReviewApp {
    pub(super) fn render_user_pr_selector(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let query = self.open_input.read(cx).value().to_string();
        let filtered = filter(&self.pr_selector.rows, &query);
        let count = filtered.len();
        let entity = cx.entity();
        let expanded = self.pr_selector.expanded;
        let loading = self.pr_selector.is_loading();
        let error = self.pr_selector.error();
        let total = self.pr_selector.rows.len();
        let header = div()
            .h(px(30.))
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id("toggle-user-prs")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.pr_selector.expanded = !app.pr_selector.expanded;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(12.))
                            .text_color(theme::overlay0())
                            .child(SharedString::from(if expanded { "▾" } else { "▸" })),
                    )
                    .child(
                        div()
                            .truncate()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme::subtext())
                            .child(SharedString::from(format!("Pull requests ({total})"))),
                    ),
            )
            .child(
                Button::new("refresh-user-prs")
                    .icon(IconName::Redo)
                    .loading(loading)
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(|app, _, _, cx| app.refresh_user_prs(cx))),
            );
        let body = if !expanded {
            div().into_any_element()
        } else if self.pr_selector.rows.is_empty() && loading {
            div()
                .px_3()
                .py_2()
                .text_color(theme::overlay0())
                .child("loading pull requests…")
                .into_any_element()
        } else if self.pr_selector.rows.is_empty() {
            error.clone().map_or_else(
                || {
                    div()
                        .px_3()
                        .py_2()
                        .text_color(theme::overlay0())
                        .child("no open pull requests")
                        .into_any_element()
                },
                |error| {
                    div()
                        .px_3()
                        .py_2()
                        .text_size(px(11.))
                        .text_color(theme::red())
                        .child(error)
                        .into_any_element()
                },
            )
        } else if count == 0 {
            div()
                .px_3()
                .py_2()
                .text_color(theme::overlay0())
                .child("no matching pull requests")
                .into_any_element()
        } else {
            div()
                .h(px(count.min(5) as f32 * ROW_HEIGHT))
                .child(
                    uniform_list("user-pr-list", count, move |range, _, cx| {
                        let app = entity.read(cx);
                        range
                            .filter_map(|pos| {
                                Some((pos, app.pr_selector.rows.get(*filtered.get(pos)?)?))
                            })
                            .map(|(pos, pr)| render_row(pr, pos, entity.clone()))
                            .collect()
                    })
                    .track_scroll(self.pr_selector.scroll.clone())
                    .h_full(),
                )
                .into_any_element()
        };
        div()
            .flex_shrink_0()
            .border_b_1()
            .border_color(theme::surface0())
            .child(header)
            .child(body)
            .when(expanded && !self.pr_selector.rows.is_empty(), |section| {
                section.when_some(error, |section, error| {
                    section.child(
                        div()
                            .px_3()
                            .pb_1()
                            .truncate()
                            .text_size(px(10.))
                            .text_color(theme::red())
                            .child(error),
                    )
                })
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(number: u64) -> gh::UserPrSummary {
        gh::UserPrSummary {
            number,
            title: "selector".into(),
            author: gh::Author {
                login: "alice".into(),
            },
            is_draft: false,
            updated_at: "now".into(),
            repository: gh::Repository {
                name: "r".into(),
                name_with_owner: "a/r".into(),
            },
            url: "u".into(),
        }
    }

    #[test]
    fn refresh_state_keeps_rows_on_failure_and_discards_stale_results() {
        let mut selector = PrSelector::new();
        let first = selector.begin_refresh();
        let second = selector.begin_refresh();
        assert!(!selector.apply_result(first, Ok(vec![row(1)])));
        assert!(selector.apply_result(second, Ok(vec![row(2)])));
        let third = selector.begin_refresh();
        assert!(selector.apply_result(third, Err(anyhow::anyhow!("offline"))));
        assert_eq!(selector.rows[0].number, 2);
        assert!(selector.error().is_some());
    }

    #[test]
    fn loaded_empty_and_filter_fields_are_explicit() {
        let mut selector = PrSelector::new();
        let request = selector.begin_refresh();
        selector.apply_result(request, Ok(Vec::new()));
        assert!(selector.rows.is_empty());
        assert_eq!(filter(&[row(10)], "#10"), vec![0]);
        assert_eq!(filter(&[row(10)], "alice"), vec![0]);
    }
}
