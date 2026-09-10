use ratatui::widgets::TableState;

/// How many rows stay visible above or below the selection, used to scroll a
/// table so the selected row never leaves the viewport.
pub const VISIBLE_ROWS: usize = 15;

/// Move the selection and, when it would leave the viewport, scroll the table
/// offset so the row stays visible.
pub fn set_selected(table_state: &mut TableState, selected: usize) {
    table_state.select(Some(selected));
    adjust_offset(table_state, selected);
}

/// Establish or re-clamp a table selection after its item count changes.
/// A `None` selection (or an out-of-range one) resolves to the first row; an
/// empty table clears both selection and offset.
pub fn sync_selection(table_state: &mut TableState, count: usize, selected: Option<usize>) {
    if count == 0 {
        table_state.select(None);
        *table_state.offset_mut() = 0;
        return;
    }
    let selected = selected.unwrap_or(0).min(count - 1);
    set_selected(table_state, selected);
}

/// Move down one row, clamped to the last row.
pub fn select_next(table_state: &mut TableState, count: usize) {
    if count == 0 {
        return;
    }
    let selected = match table_state.selected() {
        Some(i) => std::cmp::min(i + 1, count - 1),
        None => 0,
    };
    set_selected(table_state, selected);
}

/// Move up one row, clamped to the first row.
pub fn select_previous(table_state: &mut TableState, count: usize) {
    if count == 0 {
        return;
    }
    let selected = match table_state.selected() {
        Some(i) => i.saturating_sub(1),
        None => 0,
    };
    set_selected(table_state, selected);
}

/// Jump to the first row.
pub fn select_first(table_state: &mut TableState, count: usize) {
    if count > 0 {
        set_selected(table_state, 0);
    }
}

/// Jump to the last row.
pub fn select_last(table_state: &mut TableState, count: usize) {
    if count > 0 {
        set_selected(table_state, count - 1);
    }
}

fn adjust_offset(table_state: &mut TableState, selected: usize) {
    let offset = table_state.offset();
    if selected >= offset + VISIBLE_ROWS {
        *table_state.offset_mut() = selected - VISIBLE_ROWS + 1;
    } else if selected < offset {
        *table_state.offset_mut() = selected;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_selection_empty_clears_selection_and_offset() {
        let mut state = TableState::default();
        state.select(Some(3));
        *state.offset_mut() = 2;
        sync_selection(&mut state, 0, Some(3));
        assert_eq!(state.selected(), None);
        assert_eq!(state.offset(), 0);
    }

    #[test]
    fn sync_selection_clamps_out_of_range_and_starts_at_first() {
        let mut state = TableState::default();
        sync_selection(&mut state, 3, None);
        assert_eq!(state.selected(), Some(0));
        sync_selection(&mut state, 3, Some(9));
        assert_eq!(state.selected(), Some(2));
    }

    #[test]
    fn select_next_and_previous_clamp_at_bounds() {
        let mut state = TableState::default();
        select_previous(&mut state, 2);
        assert_eq!(state.selected(), Some(0));
        select_next(&mut state, 2);
        select_next(&mut state, 2);
        assert_eq!(state.selected(), Some(1));
        select_previous(&mut state, 2);
        select_previous(&mut state, 2);
        assert_eq!(state.selected(), Some(0));
    }

    #[test]
    fn navigation_scrolls_selection_into_view() {
        let mut state = TableState::default();
        sync_selection(&mut state, 40, None);
        for _ in 0..VISIBLE_ROWS {
            select_next(&mut state, 40);
        }
        assert_eq!(state.selected(), Some(VISIBLE_ROWS));
        assert_eq!(state.offset(), 1);
        select_last(&mut state, 40);
        assert_eq!(state.selected(), Some(39));
        assert_eq!(state.offset(), 25);
        select_first(&mut state, 40);
        assert_eq!(state.selected(), Some(0));
        assert_eq!(state.offset(), 0);
    }

    #[test]
    fn empty_count_is_noop() {
        let mut state = TableState::default();
        for op in [
            |s: &mut TableState| select_next(s, 0),
            |s: &mut TableState| select_previous(s, 0),
            |s: &mut TableState| select_first(s, 0),
            |s: &mut TableState| select_last(s, 0),
        ] {
            op(&mut state);
        }
        assert_eq!(state.selected(), None);
        assert_eq!(state.offset(), 0);
    }
}
