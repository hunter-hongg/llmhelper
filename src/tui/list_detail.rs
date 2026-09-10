use ratatui::widgets::TableState;

/// A two-state view enum (List / Detail) that the shared list+detail state can
/// flip without knowing the concrete variant names.
pub trait ViewSwitcher {
    fn is_detail(&self) -> bool;
    fn show_list(&mut self);
    fn show_detail(&mut self);
}

/// Shared list + detail state machine for the `sessions` and `search` TUIs.
///
/// Captures everything the two list+detail viewers used to replicate: the item
/// list, the open detail, the ratatui table selection and the List/Detail view
/// switch, plus selection bookkeeping (clamping, offset follow, first/last).
/// A concrete state feeds `apply_items` with its own identity predicate, so a
/// refresh can keep a detail open only when the same item survives and
/// otherwise closes it.
pub struct ListDetail<T, V> {
    pub running: bool,
    pub view: V,
    pub items: Vec<T>,
    pub detail: Option<T>,
    pub table_state: TableState,
}

impl<T: Clone, V: Default + ViewSwitcher> Default for ListDetail<T, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone, V: Default + ViewSwitcher> ListDetail<T, V> {
    /// How many rows stay visible above or below the selection, used to scroll
    /// the table so the selected row never leaves the viewport.
    pub const VISIBLE_ROWS: usize = 15;

    pub fn new() -> Self {
        Self {
            running: false,
            view: V::default(),
            items: Vec::new(),
            detail: None,
            table_state: TableState::default(),
        }
    }

    /// Open the detail view for the selected item, if any.
    pub fn open_detail(&mut self) {
        if self.view.is_detail() {
            return;
        }
        let Some(selected) = self.table_state.selected() else {
            return;
        };
        let Some(item) = self.items.get(selected) else {
            return;
        };
        self.detail = Some(item.clone());
        self.view.show_detail();
    }

    /// Return to the list, keeping the selection where it was.
    pub fn close_detail(&mut self) {
        if !self.view.is_detail() {
            return;
        }
        self.view.show_list();
        self.detail = None;
    }

    pub fn quit(&mut self) {
        self.running = false;
    }

    /// Replace the item list. The selection is clamped to the new length; an
    /// open detail is re-anchored via `same` when the item survives the
    /// refresh and closed when it disappeared. Returns the new index of a
    /// surviving detail so a concrete state can reset its own detail scratch
    /// state (scroll position, re-wrap).
    pub fn apply_items(&mut self, items: Vec<T>, same: impl Fn(&T, &T) -> bool) -> Option<usize> {
        let selected = self.table_state.selected();
        let detail = self.detail.take();

        self.items = items;
        self.sync_selection(selected);

        if let Some(detail) = detail {
            let Some(index) = self.items.iter().position(|item| same(item, &detail)) else {
                self.view.show_list();
                self.detail = None;
                return None;
            };
            self.detail = Some(self.items[index].clone());
            self.view.show_detail();
            self.set_selection(index);
            return Some(index);
        }
        None
    }

    pub fn select_next(&mut self) {
        if self.view.is_detail() {
            return;
        }
        let count = self.items.len();
        if count > 0 {
            let i = match self.table_state.selected() {
                Some(i) => std::cmp::min(i + 1, count - 1),
                None => 0,
            };
            self.set_selection(i);
        }
    }

    pub fn select_previous(&mut self) {
        if self.view.is_detail() {
            return;
        }
        let count = self.items.len();
        if count > 0 {
            let i = match self.table_state.selected() {
                Some(i) => i.saturating_sub(1),
                None => 0,
            };
            self.set_selection(i);
        }
    }

    pub fn select_first(&mut self) {
        if self.view.is_detail() || self.items.is_empty() {
            return;
        }
        self.set_selection(0);
    }

    pub fn select_last(&mut self) {
        if self.view.is_detail() || self.items.is_empty() {
            return;
        }
        self.set_selection(self.items.len() - 1);
    }

    /// Establish or re-clamp the list selection after the items change.
    /// Constructors that seed `items` directly call this with `None`, so the
    /// first draw always has a selection (`TableState::default()` starts with
    /// none, which would make Enter and the arrow keys no-ops).
    pub fn sync_selection(&mut self, selected: Option<usize>) {
        let count = self.items.len();
        if count == 0 {
            self.table_state.select(None);
            *self.table_state.offset_mut() = 0;
            return;
        }
        let selected = selected.unwrap_or(0).min(count - 1);
        self.set_selection(selected);
    }

    fn set_selection(&mut self, selected: usize) {
        self.table_state.select(Some(selected));
        Self::adjust_offset(&mut self.table_state, selected);
    }

    fn adjust_offset(table_state: &mut TableState, selected: usize) {
        let visible = Self::VISIBLE_ROWS;
        let offset = table_state.offset();
        if selected >= offset + visible {
            *table_state.offset_mut() = selected - visible + 1;
        } else if selected < offset {
            *table_state.offset_mut() = selected;
        }
    }
}
