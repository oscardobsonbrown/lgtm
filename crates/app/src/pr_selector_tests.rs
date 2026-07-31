use super::*;

fn row(number: u64) -> gh::UserPrSummary {
    gh::UserPrSummary {
        number,
        title: "selector".into(),
        author: gh::Author { login: "alice".into() },
        is_draft: false,
        updated_at: "now".into(),
        repository: gh::Repository {
            name: "r".into(),
            name_with_owner: "a/r".into(),
        },
        url: "u".into(),
    }
}

fn opened(id: u64, source: Source) -> super::super::ReviewItem {
    super::super::ReviewItem {
        id,
        source,
        state: ItemState::Loading,
        reloading: false,
        refresh_error: None,
        upgrade_gen: 0,
        preview: None,
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
    assert!(matches!(selector.phase, RefreshPhase::Failed { .. }));
}

#[test]
fn loaded_empty_and_filter_fields_are_explicit() {
    let mut selector = PrSelector::new();
    let request = selector.begin_refresh();
    selector.apply_result(request, Ok(Vec::new()));
    assert!(selector.rows.is_empty());
    assert_eq!(fuzzy_indices(&[row(10)], "#10", pr_search), vec![0]);
    assert_eq!(fuzzy_indices(&[row(10)], "alice", pr_search), vec![0]);
}

#[test]
fn opened_sources_join_the_selector_without_duplicate_prs() {
    let items = vec![
        opened(
            7,
            Source::Pr(gh::PrLocator {
                owner: "a".into(),
                repo: "r".into(),
                number: 10,
            }),
        ),
        opened(
            8,
            Source::Local(git::LocalSource {
                repo_root: "/tmp/project".into(),
                branch: "feature".into(),
                base_label: "main".into(),
                base_oid: None,
            }),
        ),
    ];
    let entries = selector_entries(&[row(10)], &items, 0);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].open_item, Some(7));
    assert!(entries[0].active);
    assert!(matches!(entries[1].action, EntryAction::OpenItem { id: 8 }));
    assert_eq!(
        fuzzy_indices(&entries, "feature", |entry| entry.search.clone()),
        vec![1]
    );
}
