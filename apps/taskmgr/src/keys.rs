//! Keyboard navigation for tables: Arrow keys, PageUp/PageDown, Home/End, and type-ahead.

use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListNav<'a> {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Prefix(&'a str),
}

/// Pure logic for table keyboard navigation: takes the row count `n`, current `cursor`,
/// navigation command, and name accessor for type-ahead matching.
/// Returns the new selected row index, or None if no movement was made.
pub fn list_keys_pure(
    n: usize,
    cursor: Option<usize>,
    nav: ListNav<'_>,
    names: impl Fn(usize) -> String,
) -> Option<usize> {
    if n == 0 {
        return None;
    }
    match nav {
        ListNav::Up => Some(cursor.map_or(0, |c| c.saturating_sub(1))),
        ListNav::Down => Some(cursor.map_or(0, |c| (c + 1).min(n - 1))),
        ListNav::PageUp => Some(cursor.map_or(0, |c| c.saturating_sub(20))),
        ListNav::PageDown => Some(cursor.map_or(0, |c| (c + 20).min(n - 1))),
        ListNav::Home => Some(0),
        ListNav::End => Some(n - 1),
        ListNav::Prefix(prefix) => {
            if prefix.is_empty() {
                return None;
            }
            let prefix_lower = prefix.to_lowercase();
            // One letter steps to the next match; more letters refine the current one, as Windows lists do.
            let next = prefix.chars().nth(1).is_none();
            let start = cursor.map_or(0, |c| if next { (c + 1) % n } else { c.min(n - 1) });
            for step in 0..n {
                let i = (start + step) % n;
                if names(i).to_lowercase().starts_with(&prefix_lower) {
                    return Some(i);
                }
            }
            None
        }
    }
}

#[derive(Clone, Default)]
struct TypeAhead {
    prefix: String,
    last_typed: Option<Instant>,
}

/// Keyboard navigation for every table. Only handles keys when no text field has focus.
pub fn list_keys(ui: &egui::Ui, n: usize, cursor: Option<usize>, names: impl Fn(usize) -> String) -> Option<usize> {
    if ui.ctx().memory(|m| m.focused().is_some()) {
        return None;
    }
    if n == 0 {
        return None;
    }

    let id = egui::Id::new("taskmgr_list_keys_typeahead");

    if ui.input(|i| i.pointer.any_click()) {
        clear_typeahead(ui, id);
    }

    if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
        clear_typeahead(ui, id);
        return list_keys_pure(n, cursor, ListNav::Up, names);
    }
    if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
        clear_typeahead(ui, id);
        return list_keys_pure(n, cursor, ListNav::Down, names);
    }
    if ui.input(|i| i.key_pressed(egui::Key::PageUp)) {
        clear_typeahead(ui, id);
        return list_keys_pure(n, cursor, ListNav::PageUp, names);
    }
    if ui.input(|i| i.key_pressed(egui::Key::PageDown)) {
        clear_typeahead(ui, id);
        return list_keys_pure(n, cursor, ListNav::PageDown, names);
    }
    if ui.input(|i| i.key_pressed(egui::Key::Home)) {
        clear_typeahead(ui, id);
        return list_keys_pure(n, cursor, ListNav::Home, names);
    }
    if ui.input(|i| i.key_pressed(egui::Key::End)) {
        clear_typeahead(ui, id);
        return list_keys_pure(n, cursor, ListNav::End, names);
    }

    // Type-ahead: collect typed alphanumeric characters
    let mut typed = String::new();
    ui.input(|i| {
        if !i.modifiers.ctrl && !i.modifiers.alt && !i.modifiers.command {
            for ev in &i.events {
                if let egui::Event::Text(t) = ev {
                    for ch in t.chars() {
                        if ch.is_alphanumeric() {
                            typed.push(ch);
                        }
                    }
                }
            }
        }
    });

    if typed.is_empty() {
        return None;
    }

    let prefix = ui.data_mut(|d| {
        let state = d.get_temp_mut_or_default::<TypeAhead>(id);
        let now = Instant::now();
        if let Some(last) = state.last_typed
            && now.duration_since(last) > Duration::from_millis(900)
        {
            state.prefix.clear();
        }
        state.prefix.push_str(&typed);
        state.last_typed = Some(now);
        state.prefix.clone()
    });

    list_keys_pure(n, cursor, ListNav::Prefix(&prefix), names)
}

fn clear_typeahead(ui: &egui::Ui, id: egui::Id) {
    ui.data_mut(|d| {
        d.remove_temp::<TypeAhead>(id);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_empty() {
        assert_eq!(list_keys_pure(0, None, ListNav::Down, |_| String::new()), None);
        assert_eq!(list_keys_pure(0, Some(0), ListNav::Up, |_| String::new()), None);
        assert_eq!(list_keys_pure(0, None, ListNav::Prefix("a"), |_| String::new()), None);
    }

    #[test]
    fn pure_arrows() {
        let names = |_| String::new();
        assert_eq!(list_keys_pure(3, None, ListNav::Down, names), Some(0));
        assert_eq!(list_keys_pure(3, Some(0), ListNav::Down, names), Some(1));
        assert_eq!(list_keys_pure(3, Some(2), ListNav::Down, names), Some(2));
        assert_eq!(list_keys_pure(3, Some(2), ListNav::Up, names), Some(1));
        assert_eq!(list_keys_pure(3, Some(0), ListNav::Up, names), Some(0));
        assert_eq!(list_keys_pure(3, None, ListNav::Up, names), Some(0));
    }

    #[test]
    fn pure_page_keys() {
        let names = |_| String::new();
        assert_eq!(list_keys_pure(50, Some(5), ListNav::PageDown, names), Some(25));
        assert_eq!(list_keys_pure(50, Some(40), ListNav::PageDown, names), Some(49));
        assert_eq!(list_keys_pure(50, Some(25), ListNav::PageUp, names), Some(5));
        assert_eq!(list_keys_pure(50, Some(10), ListNav::PageUp, names), Some(0));
    }

    #[test]
    fn pure_home_end() {
        let names = |_| String::new();
        assert_eq!(list_keys_pure(50, Some(25), ListNav::Home, names), Some(0));
        assert_eq!(list_keys_pure(50, Some(25), ListNav::End, names), Some(49));
    }

    #[test]
    fn pure_type_ahead() {
        let list = ["Calculator", "Chrome", "Command Prompt", "Edge", "Explorer"];
        let names = |i: usize| list[i].to_string();

        // From None, typing "c" jumps to first matching row after start (index 0)
        assert_eq!(list_keys_pure(list.len(), None, ListNav::Prefix("c"), names), Some(0));
        // From row 0 (Calculator), typing "c" wraps to Chrome (1)
        assert_eq!(list_keys_pure(list.len(), Some(0), ListNav::Prefix("c"), names), Some(1));
        // From row 1 (Chrome), typing "c" moves to Command Prompt (2)
        assert_eq!(list_keys_pure(list.len(), Some(1), ListNav::Prefix("c"), names), Some(2));
        // Multi-character prefix "ch" jumps to Chrome
        assert_eq!(list_keys_pure(list.len(), Some(0), ListNav::Prefix("ch"), names), Some(1));
        // Typing on from Chrome keeps it while it still matches
        assert_eq!(list_keys_pure(list.len(), Some(1), ListNav::Prefix("chr"), names), Some(1));
        // Multi-character prefix "ed" jumps to Edge
        assert_eq!(list_keys_pure(list.len(), Some(0), ListNav::Prefix("ed"), names), Some(3));
        // Non-matching prefix returns None
        assert_eq!(list_keys_pure(list.len(), Some(0), ListNav::Prefix("xyz"), names), None);
    }
}
