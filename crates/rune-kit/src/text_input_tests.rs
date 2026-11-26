//! A multiline field's own scroll viewport.

use super::*;
use rune::{Env, Runtime};

struct Doc {
    edit: TextEdit,
}

fn view(s: &Doc, _: &mut rune::Cx<Doc>) -> El {
    let k = Kit::neutral(true);
    col().key("root").size(320.0, 200.0).child(
        TextArea::new(&s.edit)
            .label("Notes")
            .width(None)
            .max_height(160.0)
            .on_edit(|s: &mut Doc| &mut s.edit)
            .build(&k),
    )
}

fn rect(rt: &Runtime<Doc>, key: &str) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key} is mounted")).rect
}

/// A held press that leaves the field's bottom edge scrolls the field and
/// keeps extending the selection, instead of stopping at the last line shown.
#[test]
fn a_held_drag_past_the_bottom_edge_scrolls_a_multiline_field() {
    let text = (0..80).map(|i| format!("line {i:03} words here")).collect::<Vec<_>>().join("\n");
    let mut rt = Runtime::new(Doc { edit: TextEdit::new(text) }, view, (320.0, 200.0), Env::new(true, 1.0));
    for _ in 0..10 {
        rt.frame(1.0 / 60.0);
    }
    let (scroll, input) = (rect(&rt, "scroll"), rect(&rt, "input"));
    assert!(
        input[3] > scroll[3] + 100.0,
        "the field is taller than its viewport: field {input:?}, viewport {scroll:?}"
    );
    let x = input[0] + 20.0;
    rt.pointer_down(x, input[1] + 8.0);
    rt.frame(1.0 / 60.0);
    let anchor = rt.state.edit.anchor;
    rt.pointer_move(x, scroll[1] + scroll[3] + 20.0);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let moved = rect(&rt, "input");
    assert!(moved[1] < scroll[1], "the viewport scrolled under the held drag: top {} -> {}", input[1], moved[1]);
    assert_eq!(rt.state.edit.anchor, anchor, "the press's anchor stays put");
    let lines = rt.state.edit.text[..rt.state.edit.focus].matches('\n').count();
    assert!(lines > 0, "the selection reaches past the first line: focus {}", rt.state.edit.focus);
    rt.pointer_up(x, scroll[1] + scroll[3] + 20.0);
}
