//! Renders the whole GridCraft window offscreen (wgpu, no window) to a PNG.
//!
//! `cargo run --release -p gridcraft-ui-egui --example snapshot -- [--sample sales] [--in file.xlsx]
//!  [--size 1440x900] [--scale 2] [--cmd 'id={json}']... [--tab Insert] [--dark] [--language en|es|zh|ja|ko|ru|pt|de|cs|fr|tr]
//!  [--dialog 'name={json}'] [--hf-edit header|footer] [--pane formatChart] [--chart <id>|last]
//!  [--drag se=120x60] out.png
//!
//! `--drag` leaves a chart resize in progress (the handle pressed and moved, not released), so a
//! snapshot can show what a drag looks like.`

use gridcraft_engine::Session;
use gridcraft_ui_egui::SheetApp;
use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut sample = None;
    let mut input = None;
    let mut size = (1440.0f32, 900.0f32);
    let mut scale = 2.0f32;
    let mut cmds: Vec<String> = vec![];
    let mut tab = None;
    let mut dark = false;
    let mut pane: Option<String> = None;
    let mut chart: Option<u32> = None;
    let mut chart_last = false;
    let mut drag: Option<String> = None;
    let mut dialog: Option<String> = None;
    let mut language: Option<String> = None;
    let mut hf_edit: Option<String> = None;
    let mut out = "snapshot.png".to_string();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--sample" => {
                i += 1;
                sample = args.get(i).cloned();
            }
            "--in" => {
                i += 1;
                input = args.get(i).cloned();
            }
            "--size" => {
                i += 1;
                if let Some((w, h)) = args.get(i).and_then(|s| s.split_once('x')) {
                    size = (w.parse().unwrap_or(1440.0), h.parse().unwrap_or(900.0));
                }
            }
            "--scale" => {
                i += 1;
                scale = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(2.0);
            }
            "--cmd" => {
                i += 1;
                if let Some(c) = args.get(i) {
                    cmds.push(c.clone());
                }
            }
            "--tab" => {
                i += 1;
                tab = args.get(i).cloned();
            }
            "--dark" => dark = true,
            "--pane" => {
                i += 1;
                pane = args.get(i).cloned();
            }
            "--chart" => {
                i += 1;
                match args.get(i).map(String::as_str) {
                    Some("last") => chart_last = true,
                    other => chart = other.and_then(|s| s.parse().ok()),
                }
            }
            "--drag" => {
                i += 1;
                drag = args.get(i).cloned();
            }
            "--dialog" => {
                i += 1;
                dialog = args.get(i).cloned();
            }
            "--hf-edit" => {
                i += 1;
                hf_edit = args.get(i).cloned();
            }
            "--language" => {
                i += 1;
                language = args.get(i).cloned();
            }
            other => out = other.to_string(),
        }
        i += 1;
    }
    let mut session = Session::new();
    if let Some(p) = &input {
        session.execute("file.open", json!({"path": p})).expect("open");
    } else if let Some(s) = &sample {
        session.execute("file.new", json!({"sample": s})).expect("sample");
    } else {
        session.new_workbook();
    }
    for c in &cmds {
        let (id, p) = c.split_once('=').map(|(a, b)| (a.to_string(), serde_json::from_str(b).unwrap_or(json!({})))).unwrap_or((c.clone(), json!({})));
        if let Err(e) = session.execute(&id, p) {
            eprintln!("{id}: {e}");
        }
    }
    let mut harness =
        egui_kittest::Harness::builder().with_size(egui::vec2(size.0, size.1)).with_pixels_per_point(scale).with_max_steps(12).wgpu().build_ui_state(
            move |ui, app: &mut SheetApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            {
                let mut app = SheetApp::new(session, Default::default());
                app.ui.dark = dark;
                if let Some(code) = &language
                    && let Some(l) = gridcraft_ui_egui::i18n::Language::parse(code)
                {
                    app.ui.language = l;
                }
                if let Some(t) = tab {
                    app.ui.ribbon_tab = t;
                }
                app.grid.pane = pane;
                app.selected_chart = chart;
                if let Some(d) = &dialog {
                    let (name, p) =
                        d.split_once('=').map(|(a, b)| (a, serde_json::from_str(b).unwrap_or(json!({})))).unwrap_or((d.as_str(), json!({})));
                    app.open_dialog(name, p);
                }
                // Page Layout view: edit the first page's centre header (or footer) section.
                let active = app.session.active().and_then(|d| d.wb.active().cloned().map(|s| (d.wb.active_sheet, s)));
                if let (Some(part), Some((si, sh))) = (&hf_edit, active) {
                    let zone = gridcraft_engine::paper::HfZone { footer: part == "footer", section: 1 };
                    gridcraft_ui_egui::paper_view::begin(&mut app, si, &sh, (0, 0), zone);
                }
                app
            },
        );
    harness.run_steps(8);
    if chart_last {
        harness.state_mut().selected_chart =
            harness.state().session.active().and_then(|d| d.wb.active()).and_then(|sh| sh.charts.iter().map(|c| c.id).max());
        harness.run_steps(2);
    }
    if let Some(spec) = drag {
        resize_drag(&mut harness, &spec);
    }
    let img = harness.render().expect("render");
    img.save(&out).expect("save png");
    eprintln!("wrote {out} ({}x{})", img.width(), img.height());
}

/// Presses a selected chart's resize handle and moves the pointer, leaving the drag in progress:
/// `se=120x60`, `nw=-40x-20`, `e=60x0`.
fn resize_drag(harness: &mut egui_kittest::Harness<'static, SheetApp>, spec: &str) {
    use gridcraft_ui_egui::chartview::{Handle, handle_centres};
    let (which, by) = spec.split_once('=').unwrap_or((spec, "120x60"));
    let (dx, dy) = by.split_once('x').and_then(|(a, b)| a.trim().parse::<f32>().ok().zip(b.trim().parse::<f32>().ok())).unwrap_or((120.0, 60.0));
    let handle = match which.trim().to_ascii_lowercase().as_str() {
        "nw" => Handle::Nw,
        "n" => Handle::N,
        "ne" => Handle::Ne,
        "e" => Handle::E,
        "s" => Handle::S,
        "sw" => Handle::Sw,
        "w" => Handle::W,
        _ => Handle::Se,
    };
    let start = {
        let app = harness.state();
        let Some(sh) = app.session.active().and_then(|d| d.wb.active()) else { return };
        let Some(rect) = app.grid.rect else { return };
        let Some(id) = app.selected_chart else { return };
        let Some(ch) = sh.charts.iter().find(|c| c.id == id) else { return };
        let geo = gridcraft_ui_egui::grid::Geo::new(sh, rect, app.view().scroll);
        let a = &ch.anchor;
        let min = egui::pos2(geo.x(sh, a.cell.col) + a.dx * geo.z, geo.y(sh, a.cell.row) + a.dy * geo.z);
        let r = egui::Rect::from_min_size(min, egui::vec2(a.width.max(1.0), a.height.max(1.0)) * geo.z);
        let centres = handle_centres(r);
        centres[match handle {
            Handle::Nw => 0,
            Handle::N => 1,
            Handle::Ne => 2,
            Handle::E => 3,
            Handle::Se => 4,
            Handle::S => 5,
            Handle::Sw => 6,
            Handle::W => 7,
        }]
    };
    harness.input_mut().events.push(egui::Event::PointerMoved(start));
    harness.run_steps(2);
    harness.input_mut().events.push(egui::Event::PointerButton {
        pos: start,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(2);
    harness.input_mut().events.push(egui::Event::PointerMoved(start + egui::vec2(dx, dy)));
    harness.run_steps(3);
}
