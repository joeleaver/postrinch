//! The rule editor's pickers: a label of the account's, or an inbox
//! category.

use postrinch_rules::Category;
use rinch::prelude::*;

use crate::rules_ui::RulesUi;

pub fn label_select(
    __scope: &mut RenderScope,
    ui: RulesUi,
    value: Signal<String>,
    set: impl Fn(String) + 'static,
) -> NodeHandle {
    let labels: Vec<SelectOption> = ui
        .labels
        .get()
        .into_iter()
        .map(|l| SelectOption::new(l.label, l.name))
        .collect();
    rsx! {
        Select { data: labels, placeholder: "Pick a label", value_fn: move || value.get(), onchange: move |v: String| set(v) }
    }
}

pub fn category_select(
    __scope: &mut RenderScope,
    value: Signal<String>,
    set: impl Fn(String) + 'static,
) -> NodeHandle {
    let items: Vec<SelectOption> = Category::ALL
        .iter()
        .map(|c| SelectOption::new(c.name(), c.name()))
        .collect();
    rsx! {
        Select { data: items, placeholder: "Pick a category", value_fn: move || value.get(), onchange: move |v: String| set(v) }
    }
}
