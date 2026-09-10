use ratatui::widgets::TableState;

use super::table;

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
            table::set_selected(&mut self.table_state, index);
            return Some(index);
        }
        None
    }

    pub fn select_next(&mut self) {
        if self.view.is_detail() {
            return;
        }
        table::select_next(&mut self.table_state, self.items.len());
    }

    pub fn select_previous(&mut self) {
        if self.view.is_detail() {
            return;
        }
        table::select_previous(&mut self.table_state, self.items.len());
    }

    pub fn select_first(&mut self) {
        if self.view.is_detail() {
            return;
        }
        table::select_first(&mut self.table_state, self.items.len());
    }

    pub fn select_last(&mut self) {
        if self.view.is_detail() {
            return;
        }
        table::select_last(&mut self.table_state, self.items.len());
    }

    pub fn sync_selection(&mut self, selected: Option<usize>) {
        table::sync_selection(&mut self.table_state, self.items.len(), selected);
    }
}
