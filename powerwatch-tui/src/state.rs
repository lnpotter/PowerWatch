#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunState {
    #[default]
    Running,
    Paused,
}

impl RunState {
    pub fn toggle(self) -> Self {
        match self {
            RunState::Running => RunState::Paused,
            RunState::Paused => RunState::Running,
        }
    }

    pub fn is_paused(self) -> bool {
        self == RunState::Paused
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoggingState {
    #[default]
    Idle,
    Logging,
}

impl LoggingState {
    pub fn toggle(self) -> Self {
        match self {
            LoggingState::Idle => LoggingState::Logging,
            LoggingState::Logging => LoggingState::Idle,
        }
    }

    pub fn is_logging(self) -> bool {
        self == LoggingState::Logging
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SuggestionsState {
    #[default]
    Closed,
    Open {
        selected: usize,
    },
    Confirming {
        id: u64,
        selected: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionsKey {
    Toggle,
    Next,
    Previous,
    Confirm,
    Apply,
    Cancel,
    Dismiss,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionsEffect {
    Nothing,
    Apply(u64),
    Dismiss(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuggestionItem {
    pub id: u64,
    pub has_action: bool,
}

impl SuggestionsState {
    pub fn is_open(self) -> bool {
        !matches!(self, SuggestionsState::Closed)
    }

    pub fn selected(self) -> Option<usize> {
        match self {
            SuggestionsState::Closed => None,
            SuggestionsState::Open { selected } | SuggestionsState::Confirming { selected, .. } => {
                Some(selected)
            }
        }
    }

    pub fn synced(self, items: &[SuggestionItem]) -> Self {
        let last = items.len().saturating_sub(1);
        match self {
            SuggestionsState::Closed => SuggestionsState::Closed,
            SuggestionsState::Open { selected } => SuggestionsState::Open {
                selected: selected.min(last),
            },
            SuggestionsState::Confirming { id, selected } => {
                match items.iter().position(|item| item.id == id) {
                    Some(position) => SuggestionsState::Confirming {
                        id,
                        selected: position,
                    },
                    None => SuggestionsState::Open {
                        selected: selected.min(last),
                    },
                }
            }
        }
    }

    pub fn handle(
        self,
        key: SuggestionsKey,
        items: &[SuggestionItem],
    ) -> (Self, SuggestionsEffect) {
        let last = items.len().saturating_sub(1);
        match (self, key) {
            (SuggestionsState::Closed, SuggestionsKey::Toggle) => (
                SuggestionsState::Open { selected: 0 },
                SuggestionsEffect::Nothing,
            ),
            (_, SuggestionsKey::Toggle) => (SuggestionsState::Closed, SuggestionsEffect::Nothing),
            (SuggestionsState::Open { selected }, SuggestionsKey::Next) => (
                SuggestionsState::Open {
                    selected: (selected + 1).min(last),
                },
                SuggestionsEffect::Nothing,
            ),
            (SuggestionsState::Open { selected }, SuggestionsKey::Previous) => (
                SuggestionsState::Open {
                    selected: selected.saturating_sub(1),
                },
                SuggestionsEffect::Nothing,
            ),
            (SuggestionsState::Open { selected }, SuggestionsKey::Confirm) => {
                match items.get(selected) {
                    Some(item) if item.has_action => (
                        SuggestionsState::Confirming {
                            id: item.id,
                            selected,
                        },
                        SuggestionsEffect::Nothing,
                    ),
                    _ => (self, SuggestionsEffect::Nothing),
                }
            }
            (SuggestionsState::Open { selected }, SuggestionsKey::Dismiss) => {
                match items.get(selected) {
                    Some(item) => (self, SuggestionsEffect::Dismiss(item.id)),
                    None => (self, SuggestionsEffect::Nothing),
                }
            }
            (SuggestionsState::Confirming { id, selected }, SuggestionsKey::Apply) => {
                let still_there = items.iter().any(|item| item.id == id);
                let effect = if still_there {
                    SuggestionsEffect::Apply(id)
                } else {
                    SuggestionsEffect::Nothing
                };
                (SuggestionsState::Open { selected }, effect)
            }
            (SuggestionsState::Confirming { selected, .. }, SuggestionsKey::Cancel) => (
                SuggestionsState::Open { selected },
                SuggestionsEffect::Nothing,
            ),
            _ => (self, SuggestionsEffect::Nothing),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_running_by_default() {
        assert_eq!(RunState::default(), RunState::Running);
        assert!(!RunState::default().is_paused());
    }

    #[test]
    fn toggling_running_gives_paused() {
        assert_eq!(RunState::Running.toggle(), RunState::Paused);
    }

    #[test]
    fn toggling_paused_gives_running() {
        assert_eq!(RunState::Paused.toggle(), RunState::Running);
    }

    #[test]
    fn is_paused_reflects_the_current_state() {
        assert!(RunState::Paused.is_paused());
        assert!(!RunState::Running.is_paused());
    }

    #[test]
    fn logging_starts_idle_by_default() {
        assert_eq!(LoggingState::default(), LoggingState::Idle);
        assert!(!LoggingState::default().is_logging());
    }

    #[test]
    fn toggling_idle_gives_logging() {
        assert_eq!(LoggingState::Idle.toggle(), LoggingState::Logging);
    }

    #[test]
    fn toggling_logging_gives_idle() {
        assert_eq!(LoggingState::Logging.toggle(), LoggingState::Idle);
    }

    #[test]
    fn suggestions_state_starts_closed() {
        assert_eq!(SuggestionsState::default(), SuggestionsState::Closed);
        assert!(!SuggestionsState::default().is_open());
    }

    #[test]
    fn is_open_returns_true_for_open_and_confirming() {
        assert!(SuggestionsState::Open { selected: 0 }.is_open());
        assert!(SuggestionsState::Confirming { id: 1, selected: 0 }.is_open());
        assert!(!SuggestionsState::Closed.is_open());
    }

    fn item(id: u64, has_action: bool) -> SuggestionItem {
        SuggestionItem { id, has_action }
    }

    fn open(selected: usize) -> SuggestionsState {
        SuggestionsState::Open { selected }
    }

    #[test]
    fn s_opens_the_panel_on_the_first_suggestion_and_closes_it_again() {
        let items = [item(1, true)];

        let (state, _) = SuggestionsState::Closed.handle(SuggestionsKey::Toggle, &items);
        assert_eq!(state, open(0));

        let (state, _) = state.handle(SuggestionsKey::Toggle, &items);
        assert_eq!(state, SuggestionsState::Closed);
    }

    #[test]
    fn the_selection_moves_down_and_up_but_stops_at_the_ends() {
        let items = [item(1, true), item(2, true), item(3, true)];

        let (state, _) = open(0).handle(SuggestionsKey::Previous, &items);
        assert_eq!(state, open(0));

        let (state, _) = state.handle(SuggestionsKey::Next, &items);
        let (state, _) = state.handle(SuggestionsKey::Next, &items);
        let (state, _) = state.handle(SuggestionsKey::Next, &items);
        assert_eq!(state, open(2));

        let (state, _) = state.handle(SuggestionsKey::Previous, &items);
        assert_eq!(state, open(1));
    }

    #[test]
    fn moving_the_selection_with_no_suggestions_stays_on_zero() {
        let (state, _) = open(0).handle(SuggestionsKey::Next, &[]);
        assert_eq!(state, open(0));
    }

    #[test]
    fn enter_confirms_the_selected_suggestion_not_the_first_one() {
        let items = [item(7, true), item(9, true)];

        let (state, effect) = open(1).handle(SuggestionsKey::Confirm, &items);

        assert_eq!(state, SuggestionsState::Confirming { id: 9, selected: 1 });
        assert_eq!(effect, SuggestionsEffect::Nothing);
    }

    #[test]
    fn enter_does_nothing_on_an_info_only_suggestion() {
        let items = [item(1, false)];

        let (state, effect) = open(0).handle(SuggestionsKey::Confirm, &items);

        assert_eq!(state, open(0));
        assert_eq!(effect, SuggestionsEffect::Nothing);
    }

    #[test]
    fn y_applies_exactly_the_suggestion_that_was_confirmed() {
        let items = [item(7, true), item(9, true)];
        let confirming = SuggestionsState::Confirming { id: 9, selected: 1 };

        let (state, effect) = confirming.handle(SuggestionsKey::Apply, &items);

        assert_eq!(state, open(1));
        assert_eq!(effect, SuggestionsEffect::Apply(9));
    }

    #[test]
    fn y_applies_nothing_if_the_confirmed_suggestion_disappeared() {
        let items = [item(7, true)];
        let confirming = SuggestionsState::Confirming { id: 9, selected: 1 };

        let (state, effect) = confirming.handle(SuggestionsKey::Apply, &items);

        assert_eq!(state, open(1));
        assert_eq!(effect, SuggestionsEffect::Nothing);
    }

    #[test]
    fn y_does_nothing_unless_a_confirmation_is_open() {
        let items = [item(1, true)];

        let (state, effect) = open(0).handle(SuggestionsKey::Apply, &items);

        assert_eq!(state, open(0));
        assert_eq!(effect, SuggestionsEffect::Nothing);
    }

    #[test]
    fn n_cancels_the_confirmation_and_keeps_the_suggestion() {
        let items = [item(1, true)];
        let confirming = SuggestionsState::Confirming { id: 1, selected: 0 };

        let (state, effect) = confirming.handle(SuggestionsKey::Cancel, &items);

        assert_eq!(state, open(0));
        assert_eq!(effect, SuggestionsEffect::Nothing);
    }

    #[test]
    fn d_dismisses_the_selected_suggestion() {
        let items = [item(7, true), item(9, false)];

        let (state, effect) = open(1).handle(SuggestionsKey::Dismiss, &items);

        assert_eq!(state, open(1));
        assert_eq!(effect, SuggestionsEffect::Dismiss(9));
    }

    #[test]
    fn d_does_nothing_while_confirming_or_with_an_empty_list() {
        let confirming = SuggestionsState::Confirming { id: 1, selected: 0 };

        let (state, effect) = confirming.handle(SuggestionsKey::Dismiss, &[item(1, true)]);
        assert_eq!(state, confirming);
        assert_eq!(effect, SuggestionsEffect::Nothing);

        let (_, effect) = open(0).handle(SuggestionsKey::Dismiss, &[]);
        assert_eq!(effect, SuggestionsEffect::Nothing);
    }

    #[test]
    fn syncing_pulls_the_selection_back_when_the_list_shrinks() {
        let items = [item(1, true)];

        assert_eq!(open(3).synced(&items), open(0));
        assert_eq!(open(3).synced(&[]), open(0));
        assert_eq!(
            SuggestionsState::Closed.synced(&items),
            SuggestionsState::Closed
        );
    }

    #[test]
    fn syncing_follows_a_confirmed_suggestion_when_the_list_reorders() {
        let confirming = SuggestionsState::Confirming { id: 9, selected: 1 };
        let items = [item(9, true), item(11, true)];

        assert_eq!(
            confirming.synced(&items),
            SuggestionsState::Confirming { id: 9, selected: 0 }
        );
    }

    #[test]
    fn syncing_drops_a_confirmation_whose_suggestion_went_away() {
        let confirming = SuggestionsState::Confirming { id: 9, selected: 1 };
        let items = [item(7, true)];

        assert_eq!(confirming.synced(&items), open(0));
    }
}
