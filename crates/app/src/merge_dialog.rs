#[derive(Clone)]
pub(crate) enum MergePhase {
    Checking {
        request: u64,
    },
    Assessed {
        assessment: gh::MergeAssessment,
        error: Option<String>,
    },
    Submitting {
        assessment: gh::MergeAssessment,
    },
    Failed {
        message: String,
    },
}

pub(crate) struct MergeDialog {
    pub(crate) item_id: u64,
    pub(crate) method: gh::MergeMethod,
    pub(crate) delete_branch: bool,
    pub(crate) phase: MergePhase,
}

impl MergeDialog {
    pub(crate) fn new(item_id: u64, request: u64) -> Self {
        Self {
            item_id,
            method: gh::MergeMethod::Squash,
            delete_branch: false,
            phase: MergePhase::Checking { request },
        }
    }

    pub(crate) fn apply_assessment(&mut self, request: u64, result: anyhow::Result<gh::MergeAssessment>) -> bool {
        if !matches!(self.phase, MergePhase::Checking { request: active } if active == request) {
            return false;
        }
        self.phase = match result {
            Ok(assessment) => MergePhase::Assessed {
                assessment,
                error: None,
            },
            Err(error) => MergePhase::Failed {
                message: format!("{error:#}"),
            },
        };
        true
    }

    pub(crate) fn begin_submit(&mut self) -> Option<(gh::MergeAssessment, gh::MergeMethod, bool)> {
        let MergePhase::Assessed { assessment, .. } = &self.phase else {
            return None;
        };
        if !assessment.can_attempt() {
            return None;
        }
        let values = (assessment.clone(), self.method, self.delete_branch);
        self.phase = MergePhase::Submitting {
            assessment: assessment.clone(),
        };
        Some(values)
    }

    pub(crate) fn submit_failed(&mut self, message: String) {
        if let MergePhase::Submitting { assessment } = &self.phase {
            self.phase = MergePhase::Assessed {
                assessment: assessment.clone(),
                error: Some(message),
            };
        }
    }

    pub(crate) fn is_submitting(&self) -> bool {
        matches!(self.phase, MergePhase::Submitting { .. })
    }
}

impl ReviewApp {
    pub(super) fn render_merge(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let empty = || div().into_any_element();
        let Some(dialog) = &self.merge else {
            return empty();
        };
        let Some(item) = self.items.get(self.active).filter(|item| item.id == dialog.item_id) else {
            return empty();
        };
        let ItemState::Ready(data) = &item.state else {
            return empty();
        };
        let Some(meta) = &data.pr_meta else {
            return empty();
        };
        let selected = dialog.method;
        let submitting = dialog.is_submitting();
        let error = match &dialog.phase {
            MergePhase::Assessed { error, .. } => error.clone(),
            _ => None,
        };
        let method = |label: &'static str, value: gh::MergeMethod| {
            div()
                .id(label)
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .cursor_pointer()
                .when(value == selected, |row| {
                    row.bg(Hsla::from(theme::mauve()).opacity(0.15))
                        .border_color(theme::mauve())
                        .text_color(theme::mauve())
                })
                .when(value != selected, |row| {
                    row.border_color(theme::surface0()).text_color(theme::subtext())
                })
                .on_click(cx.listener(move |app, _, _, cx| {
                    if let Some(dialog) = &mut app.merge {
                        dialog.method = value;
                        cx.notify();
                    }
                }))
                .child(label)
        };
        let (can_attempt, status) = match &dialog.phase {
            MergePhase::Checking { .. } => (
                false,
                div()
                    .flex()
                    .gap_2()
                    .text_color(theme::overlay0())
                    .child(Spinner::new().xsmall())
                    .child("checking GitHub merge state…")
                    .into_any_element(),
            ),
            MergePhase::Failed { message } => (
                false,
                div()
                    .text_color(theme::red())
                    .child(SharedString::from(message.clone()))
                    .into_any_element(),
            ),
            MergePhase::Assessed { assessment, .. } | MergePhase::Submitting { assessment } => {
                let passing = assessment
                    .checks
                    .iter()
                    .filter(|check| check.state == gh::CheckState::Passing)
                    .count();
                let mut area = div().flex().flex_col().gap_1().child(
                    div()
                        .text_color(if assessment.merge_state == gh::MergeState::Clean {
                            theme::green()
                        } else {
                            theme::peach()
                        })
                        .child(SharedString::from(format!(
                            "GitHub state: {} · mergeability: {} · {passing} checks passed",
                            assessment.merge_state.as_str().to_lowercase(),
                            assessment.mergeability.as_str().to_lowercase()
                        ))),
                );
                for check in assessment
                    .checks
                    .iter()
                    .filter(|check| check.state != gh::CheckState::Passing)
                    .take(8)
                {
                    area = area.child(
                        div()
                            .pl_2()
                            .text_color(match check.state {
                                gh::CheckState::Failing => theme::red(),
                                _ => theme::peach(),
                            })
                            .child(SharedString::from(format!(
                                "{}: {}",
                                match check.state {
                                    gh::CheckState::Failing => "failed",
                                    gh::CheckState::Pending => "waiting",
                                    _ => "unknown",
                                },
                                check.name
                            ))),
                    );
                }
                if assessment.merge_state != gh::MergeState::Clean && assessment.merge_state != gh::MergeState::HasHooks
                {
                    area = area.child(
                        div()
                            .text_color(theme::overlay0())
                            .child("GitHub will make the final merge decision when you submit"),
                    );
                }
                (assessment.can_attempt(), area.into_any_element())
            }
        };
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(120.))
            .bg(theme::palette_backdrop())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|app, _, window, cx| {
                    cx.stop_propagation();
                    app.close_merge(window, cx);
                }),
            )
            .child(
                div()
                    .w(px(560.))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_action(cx.listener(|app, _: &InputEscape, window, cx| app.close_merge(window, cx)))
                    .rounded_lg()
                    .border_1()
                    .border_color(theme::surface0())
                    .bg(theme::mantle())
                    .shadow_lg()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .text_size(px(12.))
                    .child(div().text_color(theme::overlay0()).child("Merge pull request"))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(SharedString::from(format!("#{} {}", meta.number, meta.title))),
                    )
                    .child(div().text_color(theme::subtext()).child(SharedString::from(format!(
                        "{} → {}",
                        meta.head_ref_name, meta.base_ref_name
                    ))))
                    .child(status)
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(method("Merge commit", gh::MergeMethod::Merge))
                            .child(method("Squash", gh::MergeMethod::Squash))
                            .child(method("Rebase", gh::MergeMethod::Rebase)),
                    )
                    .child(
                        div()
                            .id("merge-delete-branch")
                            .flex()
                            .gap_2()
                            .cursor_pointer()
                            .on_click(cx.listener(|app, _, _, cx| {
                                if let Some(dialog) = &mut app.merge {
                                    dialog.delete_branch = !dialog.delete_branch;
                                    cx.notify();
                                }
                            }))
                            .child(if dialog.delete_branch { "☑" } else { "☐" })
                            .child("Delete branch after merge"),
                    )
                    .when_some(error, |card, error| {
                        card.child(div().text_color(theme::red()).child(error))
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .text_color(theme::overlay0())
                                    .child("GitHub will enforce repository policy"),
                            )
                            .child(
                                Button::new("merge-cancel")
                                    .label("Cancel")
                                    .ghost()
                                    .small()
                                    .disabled(submitting)
                                    .on_click(cx.listener(|app, _, window, cx| app.close_merge(window, cx))),
                            )
                            .child(
                                Button::new("merge-confirm")
                                    .label("Merge pull request")
                                    .primary()
                                    .small()
                                    .disabled(!can_attempt || submitting)
                                    .loading(submitting)
                                    .on_click(cx.listener(|app, _, window, cx| app.submit_merge(window, cx))),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assessment(decision: gh::ReviewDecision, head: &str) -> gh::MergeAssessment {
        gh::MergeAssessment {
            is_draft: false,
            mergeability: gh::Mergeability::Mergeable,
            merge_state: gh::MergeState::Clean,
            review_decision: decision,
            head_oid: head.into(),
            checks: Vec::new(),
        }
    }

    #[test]
    fn reopened_dialog_rejects_the_prior_request() {
        let mut dialog = MergeDialog::new(1, 2);
        assert!(!dialog.apply_assessment(1, Ok(assessment(gh::ReviewDecision::Approved, "old"))));
        assert!(matches!(dialog.phase, MergePhase::Checking { .. }));
        assert!(dialog.apply_assessment(2, Ok(assessment(gh::ReviewDecision::Approved, "current"))));
        assert!(matches!(dialog.phase, MergePhase::Assessed { .. }));
    }

    #[test]
    fn submit_failure_returns_to_assessed_for_retry() {
        let mut dialog = MergeDialog::new(1, 1);
        dialog.apply_assessment(1, Ok(assessment(gh::ReviewDecision::Approved, "a")));
        assert!(dialog.begin_submit().is_some());
        assert!(dialog.is_submitting());
        dialog.submit_failed("blocked".into());
        assert!(matches!(dialog.phase, MergePhase::Assessed { error: Some(_), .. }));
    }

    #[test]
    fn revoked_approval_or_missing_head_prevents_attempt() {
        let mut dialog = MergeDialog::new(1, 1);
        dialog.apply_assessment(1, Ok(assessment(gh::ReviewDecision::ChangesRequested, "a")));
        assert!(dialog.begin_submit().is_none());
        let mut dialog = MergeDialog::new(1, 1);
        dialog.apply_assessment(1, Ok(assessment(gh::ReviewDecision::Approved, "")));
        assert!(dialog.begin_submit().is_none());
    }
}
use super::{theme, ItemState, ReviewApp};
use gpui::{div, prelude::*, px, Context, Hsla, MouseButton, SharedString};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    input::Escape as InputEscape,
    spinner::Spinner,
    Disableable as _, Sizable as _,
};
