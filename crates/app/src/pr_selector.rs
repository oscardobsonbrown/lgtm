use super::{find_open_pr_item, theme, ItemState, ReviewApp, Source};
use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use gpui::{div, prelude::*, px, uniform_list, Context, Hsla, ScrollStrategy, SharedString, UniformListScrollHandle};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    IconName, Sizable as _,
};

const ROW_HEIGHT: f32 = 42.0;

#[derive(Debug, Clone, PartialEq, Eq)]
enum RefreshPhase {
    Idle,
    Loading { request: u64 },
    Failed { message: SharedString },
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

    pub(crate) fn apply_result(&mut self, request: u64, result: anyhow::Result<Vec<gh::UserPrSummary>>) -> bool {
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

    pub(crate) fn query_changed(&self) {
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
    }
}

pub(crate) fn fuzzy_indices<T>(items: &[T], query: &str, text: impl Fn(&T) -> String) -> Vec<usize> {
    let query = query.trim();
    if query.is_empty() {
        return (0..items.len()).collect();
    }
    let matcher = SkimMatcherV2::default();
    let mut scored: Vec<_> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| matcher.fuzzy_match(&text(item), query).map(|score| (score, index)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, index)| index).collect()
}

#[derive(Clone)]
enum EntryAction {
    PullRequest { repo: String, number: u64 },
    OpenItem { id: u64 },
}

#[derive(Clone)]
enum EntryStatus {
    Ready { additions: u32, deletions: u32 },
    Loading,
    Failed,
}

#[derive(Clone)]
struct SelectorEntry {
    title: SharedString,
    subtitle: SharedString,
    search: String,
    dot: gpui::Rgba,
    action: EntryAction,
    open_item: Option<u64>,
    active: bool,
    status: Option<EntryStatus>,
}

fn item_status(state: &ItemState) -> EntryStatus {
    match state {
        ItemState::Ready(data) => EntryStatus::Ready {
            additions: data.additions,
            deletions: data.deletions,
        },
        ItemState::Loading => EntryStatus::Loading,
        ItemState::Failed(_) => EntryStatus::Failed,
    }
}

fn pr_search(pr: &gh::UserPrSummary) -> String {
    format!(
        "{} #{} {} {}",
        pr.repository.name_with_owner, pr.number, pr.title, pr.author.login
    )
}

fn selector_entries(
    pull_requests: &[gh::UserPrSummary],
    items: &[super::ReviewItem],
    active: usize,
) -> Vec<SelectorEntry> {
    let mut entries = Vec::with_capacity(pull_requests.len() + items.len());
    let mut represented_items = std::collections::HashSet::new();
    for pr in pull_requests {
        let open_ix = find_open_pr_item(items, &pr.repository.name_with_owner, pr.number);
        let open = open_ix.map(|ix| &items[ix]);
        if let Some(item) = open {
            represented_items.insert(item.id);
        }
        entries.push(SelectorEntry {
            title: pr.title.clone().into(),
            subtitle: format!("{}#{} · @{}", pr.repository.name_with_owner, pr.number, pr.author.login).into(),
            search: pr_search(pr),
            dot: open.map_or_else(
                || {
                    if pr.is_draft {
                        theme::overlay0()
                    } else {
                        theme::green()
                    }
                },
                |item| item.dot_color(),
            ),
            action: EntryAction::PullRequest {
                repo: pr.repository.name_with_owner.clone(),
                number: pr.number,
            },
            open_item: open.map(|item| item.id),
            active: open_ix == Some(active),
            status: open.map(|item| item_status(&item.state)),
        });
    }
    for (ix, item) in items.iter().enumerate() {
        if represented_items.contains(&item.id) {
            continue;
        }
        let primary = item.primary();
        let secondary = item.secondary();
        let (title, subtitle) = match &item.source {
            Source::Pr(_) if !secondary.is_empty() => (secondary.clone(), primary.clone()),
            _ => (primary.clone(), secondary.clone()),
        };
        entries.push(SelectorEntry {
            title,
            subtitle,
            search: format!("{primary} {secondary}"),
            dot: item.dot_color(),
            action: EntryAction::OpenItem { id: item.id },
            open_item: Some(item.id),
            active: ix == active,
            status: Some(item_status(&item.state)),
        });
    }
    entries
}

impl ReviewApp {
    fn selector_entries(&self) -> Vec<SelectorEntry> {
        selector_entries(&self.pr_selector.rows, &self.items, self.active)
    }
}

fn render_status(status: &EntryStatus) -> gpui::AnyElement {
    match status {
        EntryStatus::Ready { additions, deletions } => div()
            .flex()
            .items_center()
            .gap_1()
            .flex_shrink_0()
            .text_size(px(10.))
            .child(
                div()
                    .text_color(theme::green())
                    .child(SharedString::from(format!("+{additions}"))),
            )
            .child(
                div()
                    .text_color(theme::red())
                    .child(SharedString::from(format!("−{deletions}"))),
            )
            .into_any_element(),
        EntryStatus::Loading => div()
            .flex_shrink_0()
            .text_size(px(10.))
            .text_color(theme::overlay0())
            .child("loading…")
            .into_any_element(),
        EntryStatus::Failed => div()
            .flex_shrink_0()
            .text_size(px(10.))
            .text_color(theme::red())
            .child("failed")
            .into_any_element(),
    }
}

fn render_row(entry: &SelectorEntry, pos: usize, entity: gpui::Entity<ReviewApp>) -> gpui::AnyElement {
    let action = entry.action.clone();
    let open_item = entry.open_item;
    let activate_entity = entity.clone();
    div()
        .id(("user-pr", pos))
        .group("selector-row")
        .mx_1()
        .px_2()
        .h(px(ROW_HEIGHT))
        .rounded_md()
        .flex()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .when(entry.active, |row| row.bg(theme::surface0()))
        .hover(|style| style.bg(Hsla::from(theme::surface0()).opacity(0.5)))
        .on_click(move |_, window, cx| {
            activate_entity.update(cx, |app, cx| match &action {
                EntryAction::PullRequest { repo, number } => app.open_or_activate_pr(repo, *number, window, cx),
                EntryAction::OpenItem { id } => {
                    if let Some(ix) = app.items.iter().position(|item| item.id == *id) {
                        app.activate(ix, window, cx);
                    }
                }
            })
        })
        .child(div().w(px(8.)).h(px(8.)).flex_shrink_0().rounded_full().bg(entry.dot))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().truncate().text_color(theme::text()).child(entry.title.clone()))
                .when(!entry.subtitle.is_empty(), |column| {
                    column.child(
                        div()
                            .truncate()
                            .text_size(px(10.))
                            .text_color(theme::subtext())
                            .child(entry.subtitle.clone()),
                    )
                }),
        )
        .when_some(entry.status.as_ref(), |row, status| row.child(render_status(status)))
        .when_some(open_item, |row, id| {
            row.child(
                div()
                    .flex_shrink_0()
                    .opacity(0.)
                    .group_hover("selector-row", |style| style.opacity(1.))
                    .child(
                        Button::new(("close-selector-item", id))
                            .icon(IconName::Close)
                            .ghost()
                            .xsmall()
                            .on_click(move |_, _, cx| {
                                entity.update(cx, |app, cx| {
                                    if let Some(ix) = app.items.iter().position(|item| item.id == id) {
                                        app.close_item(ix, cx);
                                    }
                                })
                            }),
                    ),
            )
        })
        .into_any_element()
}

impl ReviewApp {
    pub(super) fn render_user_pr_selector(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let query = self.open_input.read(cx).value().to_string();
        let entries = self.selector_entries();
        let filtered = fuzzy_indices(&entries, &query, |entry| entry.search.clone());
        let count = filtered.len();
        let has_entries = !entries.is_empty();
        let entity = cx.entity();
        let expanded = self.pr_selector.expanded;
        let loading = matches!(self.pr_selector.phase, RefreshPhase::Loading { .. });
        let error = match &self.pr_selector.phase {
            RefreshPhase::Failed { message, .. } => Some(message.clone()),
            _ => None,
        };
        let total = entries.len();
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
                            .child(SharedString::from(format!("Changes ({total})"))),
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
        } else if !has_entries && loading {
            div()
                .px_3()
                .py_2()
                .text_color(theme::overlay0())
                .child("loading pull requests…")
                .into_any_element()
        } else if !has_entries {
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
                    uniform_list("user-pr-list", count, move |range, _, _cx| {
                        range
                            .filter_map(|pos| Some((pos, entries.get(*filtered.get(pos)?)?)))
                            .map(|(pos, entry)| render_row(entry, pos, entity.clone()))
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
            .when(expanded && has_entries, |section| {
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
#[path = "pr_selector_tests.rs"]
mod tests;
