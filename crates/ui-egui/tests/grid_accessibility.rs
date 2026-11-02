//! A screen reader walks a sheet: the cells on screen are AccessKit nodes under a grid.

use egui::accesskit::Role;
use egui::vec2;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use gridcraft_engine::Session;
use gridcraft_ui_egui::SheetApp;
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

fn fixture() -> Harness {
    let mut session = Session::new();
    session.new_workbook();
    session.execute("range.setValues", json!({"range": "A1", "values": [["Region", "Sales"], ["North", 42], ["South", 7]]})).unwrap();
    session.execute("cell.set", json!({"cell": "C2", "input": "=B2*2"})).unwrap();
    session.execute("selection.set", json!({"cell": "B2"})).unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut SheetApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        SheetApp::new(session, Default::default()),
    );
    h.run_steps(4);
    h
}

#[test]
fn the_cells_on_screen_are_accesskit_nodes_under_a_grid() {
    let h = fixture();
    // The grid names its sheet and reports the active cell.
    let grid = h.get_by_role(Role::Grid);
    let node = grid.accesskit_node();
    assert_eq!(node.label().as_deref(), Some("Sheet1"), "the grid is labelled with the sheet");
    assert!(
        node.description().is_some_and(|d| d.contains("active cell B2") && d.contains("42")),
        "the grid says which cell is active and what it holds: {:?}",
        node.description()
    );

    // Each visible cell has its own node, with its value as the label/value.
    let cells: Vec<_> = h.get_all_by_role(Role::GridCell).map(|n| n.accesskit_node()).collect();
    assert!(cells.len() >= 6, "the cells on screen: {}", cells.len());
    let value_of = |address: &str| cells.iter().find(|c| c.description().is_some_and(|d| d.starts_with(address))).and_then(|c| c.value());
    assert_eq!(value_of("A2").as_deref(), Some("North"), "text cell");
    assert_eq!(value_of("B2").as_deref(), Some("42"), "number cell");
    assert_eq!(value_of("C2").as_deref(), Some("84"), "a formula reads as its result");
    // A formula's address and text are in the description, so a reader can hear the formula too.
    let formula = cells.iter().find(|c| c.description().is_some_and(|d| d.starts_with("C2"))).unwrap();
    assert_eq!(formula.description().as_deref(), Some("C2, =B2*2"), "the formula travels with the cell");
    // The active cell is the selected one, and the grid names it as its active descendant.
    let selected: Vec<String> = cells.iter().filter(|c| c.is_selected() == Some(true)).filter_map(|c| c.description()).collect();
    assert_eq!(selected, vec!["B2".to_string()], "one selected cell: {selected:?}");
    let descendant = node.active_descendant().expect("the grid names its active cell");
    let cell = cells.iter().find(|c| c.description().as_deref() == Some("B2")).expect("the active cell's node");
    assert_eq!(descendant.id(), cell.id(), "the active descendant is the active cell");
}

#[test]
fn the_grid_follows_the_selection_and_the_scroll() {
    let mut h = fixture();
    // Scroll the sheet down: the tree follows what is on screen.
    h.state_mut().run("selection.set", json!({"cell": "A400"})).unwrap();
    h.state_mut().grid.ensure_visible = true;
    h.run_steps(6);
    let cells: Vec<_> = h.get_all_by_role(Role::GridCell).map(|n| n.accesskit_node()).collect();
    let addresses: Vec<String> = cells.iter().filter_map(|c| c.description().map(|d| d.split(',').next().unwrap_or("").to_string())).collect();
    assert!(addresses.iter().any(|a| a == "A400"), "scrolling brought the cell into the tree: {addresses:?}");
    assert!(!addresses.iter().any(|a| a == "A1"), "the top of the sheet scrolled away: {addresses:?}");
    let grid = h.get_by_role(Role::Grid).accesskit_node();
    assert!(grid.description().is_some_and(|d| d.contains("active cell A400")), "the grid follows the selection");
}
