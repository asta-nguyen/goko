//! Goko's one select. Every "choose a value" control is rune-kit's searchable
//! `Combobox` with its field trigger; every "run a command" dropdown is a
//! `rune_kit::Menu` anchored to the button that opened it. This module holds
//! the shared pieces of the select.

/// The narrowest a select's open list gets: room for its search field and a
/// readable option, whatever its trigger's width.
pub const MIN_LIST_WIDTH: f32 = 240.0;

/// Options whose id is their label, for a select over plain labels.
pub fn options<S: AsRef<str>>(labels: &[S]) -> Vec<rune_kit::ComboboxOption> {
    labels
        .iter()
        .map(|label| rune_kit::ComboboxOption::new(label.as_ref(), label.as_ref()))
        .collect()
}

/// The open list's width for a trigger `width` wide: the trigger's own, or
/// [`MIN_LIST_WIDTH`] when that is narrower.
pub fn list_width(width: f32) -> f32 {
    width.max(MIN_LIST_WIDTH)
}
