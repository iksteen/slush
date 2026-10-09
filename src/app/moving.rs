//! Pane moves retain the original terminal, process, selection, and shader state.
use super::App;
use crate::layout::{Direction, Id};

impl App {
    pub(super) fn move_pane(
        &mut self,
        id: Id,
        destination: Id,
        beside: Id,
        direction: Direction,
        take_focus: bool,
    ) -> bool {
        let Some(id) = self.resolve(id) else {
            return false;
        };
        if !self
            .layout
            .move_leaf(id, destination, beside, direction, take_focus)
        {
            return false;
        }
        // The destination needs real geometry even when it is in the background.
        let dest = self
            .layout
            .tabs
            .iter()
            .position(|t| t.id == destination)
            .unwrap();
        self.sync_tab(dest);
        self.sync_layout();
        true
    }
    pub(super) fn move_pane_to_new_tab(&mut self, id: Id, name: impl AsRef<[u8]>) -> Option<Id> {
        let id = self.resolve(id)?;
        let tab = self.layout.move_leaf_to_new_tab(id, name)?;
        self.sync_tab(self.layout.tabs.len() - 1);
        self.sync_layout();
        Some(tab)
    }
    pub(super) fn push_pane_tab(&mut self, forward: bool) {
        let count = self.layout.tabs.len();
        if count < 2 {
            self.toast("only one tab to move it to");
            return;
        }
        let destination = (self.layout.current + if forward { 1 } else { count - 1 }) % count;
        let tab = self.layout.tabs[destination].id;
        let name = self.layout.tabs[destination].name.clone();
        if !self.move_pane(0, tab, 0, Direction::Cols, true) {
            self.toast("cannot move it there");
            return;
        }
        let index = self.layout.tabs.iter().position(|t| t.id == tab).unwrap() + 1;
        self.toast(&format!(
            "moved to tab {index}{}",
            if name.is_empty() {
                String::new()
            } else {
                format!(" ({})", String::from_utf8_lossy(&name))
            }
        ));
    }
}
