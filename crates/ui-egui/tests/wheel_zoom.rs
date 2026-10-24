//! Ctrl+wheel zoom steps along a ladder of percentages that always comes back to 100%.

use egui::{Event, Modifiers, MouseWheelUnit, TouchPhase, vec2};
use gridcraft_engine::Session;
use gridcraft_ui_egui::SheetApp;
use serde_json::json;

type Harness = egui_kittest::Harness<'static, SheetApp>;

fn harness() -> Harness {
    let mut session = Session::new();
    session.new_workbook();
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SheetApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        SheetApp::new(session, Default::default()),
    );
    h.run_steps(4);
    let over_grid = h.state().grid.rect.unwrap().center();
    h.input_mut().events.push(Event::PointerMoved(over_grid));
    h.run_steps(2);
    h
}

fn zoom(h: &Harness) -> u16 {
    h.state().session.active().unwrap().wb.active().unwrap().zoom
}

fn set_zoom(h: &mut Harness, percent: u16) {
    h.state_mut().run("view.zoom", json!({"percent": percent})).unwrap();
    h.run_steps(2);
}

/// Turns the wheel `turns` notches at a time with Ctrl held (up zooms in), each turn `frames`
/// frames after the last, then lets egui's easing play out.
fn wheel_turns(h: &mut Harness, turns: &[f32], frames: usize) {
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::COMMAND));
    for &t in turns {
        let delta = vec2(0.0, t);
        h.input_mut().events.push(Event::MouseWheel { unit: MouseWheelUnit::Line, delta, phase: TouchPhase::Move, modifiers: Modifiers::COMMAND });
        h.run_steps(frames);
    }
    h.run_steps(60);
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
}

fn wheel(h: &mut Harness, notches: f32) {
    wheel_turns(h, &[notches], 1);
}

#[test]
fn a_notch_steps_at_once() {
    let mut h = harness();
    h.input_mut().events.push(Event::ModifiersChanged(Modifiers::COMMAND));
    h.input_mut().events.push(Event::MouseWheel {
        unit: MouseWheelUnit::Line,
        delta: vec2(0.0, 1.0),
        phase: TouchPhase::Move,
        modifiers: Modifiers::COMMAND,
    });
    let seen: Vec<u16> = (0..20)
        .map(|_| {
            h.step();
            zoom(&h)
        })
        .collect();
    assert!(seen.iter().all(|&z| z == 120 || z == 100), "no percentage in between: {seen:?}");
    assert!(seen.iter().take(3).any(|&z| z == 120), "within a couple of frames: {seen:?}");
}

#[test]
fn a_notch_out_returns_to_the_zoom_before_the_notch_in() {
    let mut h = harness();
    assert_eq!(zoom(&h), 100);
    // Multiplying the rounded zoom by egui's eased factor every frame drifted: 100 → 122 → 101 → 123 → 102.
    for _ in 0..4 {
        wheel(&mut h, 1.0);
        assert_eq!(zoom(&h), 120);
        wheel(&mut h, -1.0);
        assert_eq!(zoom(&h), 100);
    }
    for expect in [120, 140, 170] {
        wheel(&mut h, 1.0);
        assert_eq!(zoom(&h), expect);
    }
    wheel(&mut h, -3.0);
    assert_eq!(zoom(&h), 100);
    wheel(&mut h, -2.0);
    assert_eq!(zoom(&h), 70);
    // Notches turned quickly, each before the last one has eased in.
    wheel_turns(&mut h, &[1.0, 1.0, 1.0, 1.0], 3);
    assert_eq!(zoom(&h), 140);
    wheel_turns(&mut h, &[-1.0, -1.0], 3);
    assert_eq!(zoom(&h), 100);
}

#[test]
fn parts_of_a_notch_add_up_to_whole_steps() {
    // A fine-grained wheel (or a Windows touchpad pinch) sends a notch in pieces, not always evenly.
    let mut h = harness();
    wheel_turns(&mut h, &[0.25, 0.25, 0.25], 4);
    assert_eq!(zoom(&h), 120, "three quarters of a notch step");
    wheel_turns(&mut h, &[-0.3, -0.4, -0.3], 4);
    assert_eq!(zoom(&h), 100);
    wheel_turns(&mut h, &[0.25, 0.25, 0.25, 0.25, 0.3, 0.4, 0.3], 4);
    assert_eq!(zoom(&h), 140, "one step per notch, however it's divided");
    wheel_turns(&mut h, &[-0.5, -0.25, -0.25, -0.25, -0.25, -0.5], 4);
    assert_eq!(zoom(&h), 100);
    wheel_turns(&mut h, &[0.5], 4);
    assert_eq!(zoom(&h), 100, "half a notch doesn't step");
    // What's left of a notch is forgotten after a pause (`wheel_turns` waits a second) rather
    // than adding up with the next turn.
    wheel_turns(&mut h, &[0.4], 4);
    assert_eq!(zoom(&h), 100);
}

#[test]
fn from_a_zoom_set_another_way_a_notch_goes_to_the_next_step_or_100() {
    let mut h = harness();
    for (from, notches, to) in
        [(112, 1.0, 120), (112, -1.0, 100), (121, -1.0, 100), (99, 1.0, 100), (101, -1.0, 100), (385, 1.0, 400), (400, 1.0, 400), (11, -1.0, 10)]
    {
        set_zoom(&mut h, from);
        wheel(&mut h, notches);
        assert_eq!(zoom(&h), to, "{from}% by {notches}");
    }
    // Two notches out from 121%: 100%, then 85%.
    set_zoom(&mut h, 121);
    wheel(&mut h, -2.0);
    assert_eq!(zoom(&h), 85);
}
