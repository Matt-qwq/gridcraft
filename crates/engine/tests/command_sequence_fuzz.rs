//! Command-sequence fuzzing: reproducible random sequences of commands on the sample workbooks,
//! with invariants checked as they run. Per-command tests (`every_command_survives_empty_params`)
//! miss the bugs that need a sequence — insert rows under a spill, undo, sort, paste into a table —
//! and those are the ones that corrupt a workbook.
//!
//! Each seed picks a sample (sales, budget, grades) and runs `GRIDCRAFT_FUZZ_STEPS` commands. Most
//! come from a curated list of state-changing commands with hand-shaped params; the rest are drawn
//! from every registered command, with params generated from its `params` doc string.
//!
//! Invariants:
//! - no command panics (`Session::execute` reports a caught panic as [`EngineError::Internal`]),
//!   and saving, reloading or recalculating never panics either;
//! - a non-undoable command never changes cell content (it would be lost to undo);
//! - the structure stays valid: active sheet in range, sheet and table names valid and unique,
//!   merges and tables on the sheet and not overlapping, legacy arrays anchored at their
//!   top-left cell, the selection on the sheet;
//! - a full recalculation from scratch gives the values the incremental recalculation left
//!   (catches stale dependency graphs);
//! - every few steps and at the end, an XLSX save and reload (with the recalculation opening does)
//!   gives the same sheets, formulas and values;
//! - at the end, undoing everything gives back the starting workbook and redoing everything the
//!   final one (view state aside), and both keep the Watch Window list, which isn't an edit.
//!
//! Knobs (environment variables):
//! - `GRIDCRAFT_FUZZ_SEEDS` (default 6) and `GRIDCRAFT_FUZZ_STEPS` (default 40): the budget. Soak with
//!   `GRIDCRAFT_FUZZ_SEEDS=200 GRIDCRAFT_FUZZ_STEPS=200 cargo test --release -p gridcraft-engine --test command_sequence_fuzz`.
//! - `GRIDCRAFT_FUZZ_SEED_BUDGET_MS` (default 20000, `0` disables): the wall clock one seed may take.
//!   A seed that runs past it is cut *between* steps and reported `slow` with the step it was cut at
//!   and the time it took, instead of stalling the run (see "cutting a seed" below).
//! - `GRIDCRAFT_FUZZ_SEED=17`: replay that one seed (the failure message prints the command line).
//! - `GRIDCRAFT_FUZZ_REPLAY=<path>`: replay a capture file a failing run wrote, instead of generating
//!   sequences. Takes precedence over the seeds and steps, and a replay the budget cuts fails: a
//!   sequence that did not reach its end has proven nothing.
//! - `GRIDCRAFT_FUZZ_SHRINK=1`: on failure, shrink the sequence to a minimal list of commands that
//!   still fails the same way, printed as `execute` calls ready for a regression test.
//! - `GRIDCRAFT_FUZZ_KEEP_GOING=1`: report every failing seed instead of stopping at the first.
//! - `GRIDCRAFT_FUZZ_TRACE=1`: print each step before running it, so a hang names its command.
//!
//! Every run prints one summary line, whatever the outcome:
//! `soak: 30 seeds × 100 steps, 41.7 s wall, 0 invariant failures, 0 slow seeds, slowest seed 12 (3102 ms)`
//! so a green run that took two hours and cut eight seeds cannot pass for a green run that took
//! twenty minutes. (Test output is captured on a green test: `-- --nocapture` or
//! `RUST_TEST_NOCAPTURE=1` shows it.)
//!
//! A failure report writes the whole sequence to a capture file under the OS temp dir (the path is
//! printed) and shows only the first and last few steps inline: `#` metadata lines (seed, sample,
//! planned and captured steps, the invariant) followed by one step per line, `id json`, starting from
//! the workbook `file.new` opens for that sample.
//!
//! Seeds are not portable: a sequence is drawn from the registered commands in registration order
//! (`command_specs()`), so adding, renaming, removing or reordering a command changes every seed's
//! sequence, and `GRIDCRAFT_FUZZ_SEED=<n>` only reproduces a run at the commit that generated it. The
//! capture file is the durable artefact: `GRIDCRAFT_FUZZ_REPLAY=<path>` replays it verbatim against
//! the current tree, with the same checks and the same report, so a soak failure stays usable after
//! the command set changes.
//!
//! Cutting a seed: the budget is checked before each step, after the previous step's command and all
//! of its checks (structure, recalculation, the XLSX round-trip when due) have passed. It cannot turn
//! a failure into a pass at the step it cut at, and a failure in the step that crosses the budget is
//! still reported as that invariant — a real failure always wins over `slow`. A cut seed still runs
//! its end-of-episode checks (undo everything, redo everything, one save and reload), which cost what
//! the steps already run cost; a failure there is likewise reported as a failure, not as the cut.
//! What a cut does not check is the steps after it, so a `slow` seed is not an invariant failure (the
//! test stays green) but is counted apart in the summary and reported in full;
//! `GRIDCRAFT_FUZZ_SEED=<seed> GRIDCRAFT_FUZZ_SEED_BUDGET_MS=0` finishes a seed that was cut. The one
//! thing the budget cannot bound is a single step that runs longer than the whole budget: it is not
//! interrupted, because cutting inside a command would report a half-applied edit.
//! `GRIDCRAFT_FUZZ_TRACE=1` names that step, and the overshoot is one step rather than the hours a
//! pathological seed used to take.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gridcraft_core::{CellRef, MAX_COLS, MAX_ROWS, RangeRef, Value, col_to_letters};
use gridcraft_engine::calc::Calc;
use gridcraft_engine::model::{CalcMode, Formula, LineMap, Sheet, Style, StyleId, StyleTable, Workbook};
use gridcraft_engine::{EngineError, Session, command_specs, find_command, io};
use serde_json::{Value as Json, json};

mod fuzz_common;

use fuzz_common::{content_diff, has_content, watch_list, workbook_diff};

const SAMPLES: [&str; 3] = ["sales", "budget", "grades"];
/// Episodes end (undo/redo checks, then a fresh history) before the engine's 100-step undo limit
/// starts dropping the oldest entries.
/// Most undo entries an episode may hold before the harness ends it and starts a fresh one.
const EPISODE_UNDO_DEPTH: usize = 90;
/// The session's undo history is capped at 128 MB and drops its oldest entries past that, which
/// makes "undo all, redo all" inexact. End the episode well before the cap instead: a workbook
/// whose steps retain 48 MB between them is past what the undo test is about.
const EPISODE_UNDO_BYTES: usize = 48 * 1024 * 1024;
/// Save and reload every this many steps (and at the end of every episode).
const ROUNDTRIP_EVERY: usize = 10;
/// Default wall clock one seed may take (`GRIDCRAFT_FUZZ_SEED_BUDGET_MS`). A healthy seed runs 200
/// commands in well under a second; the seeds that took minutes (one grew past 5 GB) made the same
/// 400 × 200 soak take 22 min on one commit and two hours on another, so a stall now names itself.
const DEFAULT_SEED_BUDGET_MS: u64 = 20_000;
/// Steps of a failing sequence shown inline; the whole sequence goes to the capture file.
const INLINE_STEPS: usize = 5;

/// Commands the fuzzer never runs, and why.
const EXCLUDED: &[(&str, &str)] = &[
    ("file.new", "opens another document"),
    ("file.open", "opens another document"),
    ("file.close", "closes the document under test"),
    ("file.save", "writes files"),
    ("file.saveAs", "writes files"),
    ("file.exportCsv", "writes files"),
    ("file.exportHtml", "writes files"),
    ("file.exportPdf", "renders a PDF: slow, and may write files"),
    ("file.print", "renders a PDF: slow"),
    ("window.activate", "switches documents"),
    ("view.newWindow", "opens another document"),
    ("insert.picture", "reads files"),
    ("pageLayout.background", "reads files"),
    ("formulas.calculationOptions", "manual and iterative calculation differ from a full recalc by design"),
];

// ---------------------------------------------------------------- deterministic randomness

/// SplitMix64: tiny, deterministic and good enough to pick commands.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
    /// Inclusive range.
    fn int(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % ((hi - lo + 1).max(1) as u64)) as i64
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

// ---------------------------------------------------------------- harness

type Step = (String, Json);

/// What a seed cut short by `GRIDCRAFT_FUZZ_SEED_BUDGET_MS` is reported as: a failure `kind`, so the
/// report and the shrink filter can name it, counted apart from the invariant failures.
const SLOW: &str = "slow";

#[derive(Debug)]
struct Failure {
    /// Index of the step that broke an invariant (`steps.len()` for the end-of-run checks, the first
    /// step the budget did not allow for a `slow` cut).
    step: usize,
    /// Short category, used to keep shrinking on the same bug.
    kind: &'static str,
    what: String,
}

fn env_num(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

/// The wall clock one seed may take, `None` when `GRIDCRAFT_FUZZ_SEED_BUDGET_MS=0` disables it.
fn seed_budget() -> Option<Duration> {
    match env_num("GRIDCRAFT_FUZZ_SEED_BUDGET_MS", DEFAULT_SEED_BUDGET_MS) {
        0 => None,
        ms => Some(Duration::from_millis(ms)),
    }
}

fn ms(d: Duration) -> u128 {
    d.as_millis()
}

/// What a run covered, as one line: the point of the line is that a green run that cut seeds and
/// took two hours cannot pass for a green run that took twenty minutes.
struct Summary {
    seeds: usize,
    steps: usize,
    wall: Duration,
    invariants: usize,
    slow: usize,
    slowest: Option<(u64, Duration)>,
}

impl Summary {
    fn line(&self) -> String {
        let slowest = match self.slowest {
            Some((seed, d)) => format!("seed {seed} ({} ms)", ms(d)),
            None => "none (0 ms)".to_string(),
        };
        format!(
            "soak: {} seeds × {} steps, {:.1} s wall, {} invariant failures, {} slow seeds, slowest {slowest}",
            self.seeds,
            self.steps,
            self.wall.as_secs_f64(),
            self.invariants,
            self.slow
        )
    }
}

#[test]
#[allow(clippy::disallowed_methods)] // the soak times itself; this test never runs on wasm
fn command_sequences_keep_the_workbook_consistent() {
    if let Some(path) = std::env::var_os("GRIDCRAFT_FUZZ_REPLAY") {
        replay_capture(Path::new(&path));
        return;
    }
    let seeds = env_num("GRIDCRAFT_FUZZ_SEEDS", 6);
    let steps = env_num("GRIDCRAFT_FUZZ_STEPS", 40) as usize;
    let budget = seed_budget();
    let keep_going = std::env::var_os("GRIDCRAFT_FUZZ_KEEP_GOING").is_some();
    let list: Vec<u64> = match std::env::var("GRIDCRAFT_FUZZ_SEED").ok().and_then(|v| v.trim().parse().ok()) {
        Some(one) => vec![one],
        None => (0..seeds).collect(),
    };
    let start = Instant::now();
    let mut summary = Summary { seeds: 0, steps, wall: Duration::ZERO, invariants: 0, slow: 0, slowest: None };
    let mut reports = Vec::new();
    for seed in list {
        let run = run_seed(seed, steps, budget);
        summary.seeds += 1;
        if summary.slowest.is_none_or(|(_, best)| run.elapsed > best) {
            summary.slowest = Some((seed, run.elapsed));
        }
        match &run.failure {
            // Not a failure — every step it ran passed every check — and not a pass either: a green
            // run has to say which seeds it did not finish, so the summary counts them apart.
            Some(f) if f.kind == SLOW => {
                summary.slow += 1;
                eprintln!("{}", report(Some(seed), run.sample, &run.sequence, f, Capture::Write { seed, planned: steps }));
            }
            Some(f) => {
                summary.invariants += 1;
                reports.push(report(Some(seed), run.sample, &run.sequence, f, Capture::Write { seed, planned: steps }));
                if !keep_going {
                    break;
                }
            }
            None => {}
        }
    }
    summary.wall = start.elapsed();
    println!("{}", summary.line());
    assert!(reports.is_empty(), "{} failing seed(s)\n\n{}", reports.len(), reports.join("\n\n"));
}

/// `GRIDCRAFT_FUZZ_REPLAY`: replay a captured sequence against the current tree, with the same checks
/// a generated run makes, and print the same result. A capture the budget cuts fails the replay: a
/// sequence that did not reach its end has proven nothing.
#[allow(clippy::disallowed_methods)] // the replay times itself; this test never runs on wasm
fn replay_capture(path: &Path) {
    let (sample, steps) = read_capture(path);
    assert!(
        SAMPLES.contains(&sample.as_str()),
        "{}: sample `{sample}` is not one of {SAMPLES:?}; the capture names it on its `# sample:` line",
        path.display()
    );
    let (failure, elapsed) = replay_budgeted(&sample, &steps, seed_budget());
    let result = match &failure {
        None => "no invariant failure".to_string(),
        Some(f) if f.kind == SLOW => format!("cut at step {} of {} by the budget", f.step, steps.len()),
        Some(f) => format!("invariant [{}] at step {}", f.kind, f.step),
    };
    println!("soak replay: {} (sample `{sample}`), {} steps, {:.1} s wall, {result}", path.display(), steps.len(), elapsed.as_secs_f64());
    if let Some(f) = &failure {
        eprintln!("{}", report(None, &sample, &steps, f, Capture::Existing(path)));
    }
    assert!(failure.is_none(), "replay of {} did not pass", path.display());
}

/// Reads a capture written by [`capture_file`], or by hand: `#` lines are metadata (`# sample:` names
/// the workbook) and every other non-blank line is one step, `id json`. A leading `file.new` names
/// the sample instead and is not run twice, so a capture may start with the command that opens it.
fn read_capture(path: &Path) -> (String, Vec<Step>) {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("GRIDCRAFT_FUZZ_REPLAY={}: {e}", path.display()));
    let mut sample = None;
    let mut steps = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(meta) = line.strip_prefix('#') {
            if let Some(name) = meta.trim().strip_prefix("sample:") {
                sample = Some(name.trim().to_string());
            }
            continue;
        }
        let (id, rest) = line.split_once(' ').unwrap_or_else(|| panic!("{}:{}: expected `id json`: {line}", path.display(), n + 1));
        let params: Json = serde_json::from_str(rest).unwrap_or_else(|e| panic!("{}:{}: {e}: {rest}", path.display(), n + 1));
        if steps.is_empty() && id == "file.new" && sample.is_none() {
            sample = params.get("sample").and_then(Json::as_str).map(str::to_string);
            if sample.is_some() {
                continue;
            }
        }
        steps.push((id.to_string(), params));
    }
    let sample = sample.unwrap_or_else(|| panic!("{}: no `# sample: <name>` line and no leading file.new with a sample", path.display()));
    (sample, steps)
}

/// What one seed did: the sample it ran on, the steps it ran, its wall clock and what stopped it.
struct SeedRun {
    sample: &'static str,
    sequence: Vec<Step>,
    elapsed: Duration,
    /// The first invariant failure, or `slow` when the budget cut the seed.
    failure: Option<Failure>,
}

/// Generates and runs one seed's sequence; `budget` cuts it between steps.
#[allow(clippy::disallowed_methods)] // the clock times the seed; this test never runs on wasm
fn run_seed(seed: u64, steps: usize, budget: Option<Duration>) -> SeedRun {
    let start = Instant::now();
    let mut rng = Rng(seed.wrapping_mul(0xA24B_AED4_963E_E407) ^ 0x5EED);
    let sample = SAMPLES[(seed % SAMPLES.len() as u64) as usize];
    let mut h = Harness::new(sample);
    let mut sequence = Vec::new();
    let trace = std::env::var_os("GRIDCRAFT_FUZZ_TRACE").is_some();
    for i in 0..steps {
        // Between steps, never inside one: the previous step's command and every check it ran have
        // passed, so the cut cannot turn a failure into a pass.
        let elapsed = start.elapsed();
        if let Some(b) = budget
            && elapsed > b
        {
            let cut = format!("cut after {i} of {steps} steps at {} ms (budget {} ms)", ms(elapsed), ms(b));
            // The end-of-episode checks cost what the steps already run cost, so they still run: an
            // invariant failure there is a real failure and is reported instead of the cut.
            let failure = h.finish(i).err().unwrap_or_else(|| fail(i, SLOW, cut));
            return SeedRun { sample, sequence, elapsed: start.elapsed(), failure: Some(failure) };
        }
        let (id, params) = generate(&mut rng, &h.s);
        if trace {
            eprintln!("{i} start {id} {params}");
        }
        let t = Instant::now();
        let step = h.step(i, &id, &params);
        if trace {
            let undo = h.s.doc().map(|d| d.undo.len()).unwrap_or(0);
            eprintln!("{i} done {:?} {id} undo={undo}", t.elapsed());
        }
        sequence.push((id.clone(), params.clone()));
        if let Err(f) = step {
            return SeedRun { sample, sequence, elapsed: start.elapsed(), failure: Some(f) };
        }
    }
    let failure = h.finish(steps).err();
    SeedRun { sample, sequence, elapsed: start.elapsed(), failure }
}

/// Runs a recorded sequence with the same checks; `budget` cuts it between steps like a seed.
#[allow(clippy::disallowed_methods)] // the clock times the replay; this test never runs on wasm
fn replay_budgeted(sample: &str, steps: &[Step], budget: Option<Duration>) -> (Option<Failure>, Duration) {
    let start = Instant::now();
    let mut h = Harness::new(sample);
    for (i, (id, p)) in steps.iter().enumerate() {
        let elapsed = start.elapsed();
        if let Some(b) = budget
            && elapsed > b
        {
            let cut = format!("cut after {i} of {} steps at {} ms (budget {} ms)", steps.len(), ms(elapsed), ms(b));
            // A failure in the checks the seed's own steps reached is a real failure, not a cut.
            return (Some(h.finish(i).err().unwrap_or_else(|| fail(i, SLOW, cut))), start.elapsed());
        }
        if let Err(f) = h.step(i, id, p) {
            return (Some(f), start.elapsed());
        }
    }
    (h.finish(steps.len()).err(), start.elapsed())
}

/// Runs a recorded sequence unbounded: what shrinking and the fixed-regression test use.
fn replay(sample: &str, steps: &[Step]) -> Option<Failure> {
    replay_budgeted(sample, steps, None).0
}

/// Greedy delta debugging: drop chunks of steps while the run still fails the same way.
fn shrink(sample: &str, steps: &[Step], kind: &str) -> Vec<Step> {
    let mut cur = steps.to_vec();
    let mut chunk = cur.len().div_ceil(2).max(1);
    loop {
        let mut i = 0;
        let mut removed = false;
        while i < cur.len() {
            let mut trial = cur.clone();
            trial.drain(i..(i + chunk).min(trial.len()));
            if replay(sample, &trial).is_some_and(|f| f.kind == kind) {
                cur = trial;
                removed = true;
            } else {
                i += chunk;
            }
        }
        if chunk == 1 && !removed {
            return cur;
        }
        if !removed {
            chunk = chunk.div_ceil(2);
        }
    }
}

// ---------------------------------------------------------------- reporting

/// Where the whole sequence can be replayed from: a file the caller already has (`GRIDCRAFT_FUZZ_REPLAY`
/// read it) or one the report writes now.
enum Capture<'a> {
    Existing(&'a Path),
    Write { seed: u64, planned: usize },
}

/// The report of one failing or cut seed: what stopped it, where the whole sequence lives, the first
/// and last few steps inline, and how to replay it. The file — not the seed — is what outlives the
/// commit, so its path is printed with the command that replays it.
fn report(seed: Option<u64>, sample: &str, sequence: &[Step], f: &Failure, capture: Capture<'_>) -> String {
    let planned = match &capture {
        Capture::Write { planned, .. } => *planned,
        Capture::Existing(_) => sequence.len(),
    };
    let who = match seed {
        Some(seed) => format!("seed {seed} (sample `{sample}`)"),
        None => format!("sample `{sample}`"),
    };
    let at = match (f.kind, sequence.get(f.step)) {
        (SLOW, _) => format!("cut before step {} of {planned}", f.step),
        (_, Some((id, p))) => format!("step {} of {planned}: `{id}` {p}", f.step),
        (_, None) => format!("end-of-run checks after {} steps", sequence.len()),
    };
    let what = if f.kind == SLOW { "slow" } else { "failure" };
    let mut out = format!("command-sequence fuzz {what}: {who}, {at}\n");
    out.push_str(&format!("  {} [{}]: {}\n", if f.kind == SLOW { "budget" } else { "invariant" }, f.kind, f.what));
    let file = match &capture {
        Capture::Existing(p) => Ok((*p).to_path_buf()),
        Capture::Write { seed, planned } => capture_file(*seed, sample, *planned, sequence, f),
    };
    match file {
        Ok(path) => {
            out.push_str(&format!("  whole sequence ({} steps) in {}\n", sequence.len(), path.display()));
            out.push_str(&format!(
                "  replay the file: GRIDCRAFT_FUZZ_REPLAY={} cargo test -p gridcraft-engine --test command_sequence_fuzz\n",
                path.display()
            ));
        }
        // A capture that cannot be written is reported beside the invariant, never instead of it.
        Err(e) => out.push_str(&format!("  whole sequence ({} steps) could not be written: {e}\n", sequence.len())),
    }
    if let Capture::Write { seed, planned } = capture {
        out.push_str(&format!(
            "  replay the seed: GRIDCRAFT_FUZZ_SEED={seed} GRIDCRAFT_FUZZ_STEPS={planned} cargo test -p gridcraft-engine --test command_sequence_fuzz\n    (a seed's sequence depends on the registered command set: that line is only valid at the commit that generated it)\n"
        ));
    }
    out.push_str(&steps_inline(sequence));
    if f.kind != SLOW {
        if std::env::var_os("GRIDCRAFT_FUZZ_SHRINK").is_some() {
            let end = (f.step + 1).min(sequence.len());
            let min = shrink(sample, &sequence[..end], f.kind);
            out.push_str(&format!("  minimal sequence ({} steps) from file.new {{\"sample\": \"{sample}\"}}:\n", min.len()));
            for (id, p) in &min {
                out.push_str(&format!("    s.execute(\"{id}\", json!({p})).ok();\n"));
            }
            if let Some(f) = replay(sample, &min) {
                out.push_str(&format!("  which fails with [{}]: {}\n", f.kind, f.what));
            }
        } else {
            out.push_str("  (set GRIDCRAFT_FUZZ_SHRINK=1 for a minimal sequence)\n");
        }
    }
    out
}

/// The first and last few steps, with the count of what sits between them in the file.
fn steps_inline(sequence: &[Step]) -> String {
    let n = sequence.len();
    if n == 0 {
        return "  (no steps ran)\n".to_string();
    }
    let head = n.min(INLINE_STEPS);
    let tail = INLINE_STEPS.min(n - head);
    let mut out = format!("  first {head} steps:\n");
    for (i, (id, p)) in sequence.iter().take(head).enumerate() {
        out.push_str(&format!("    {i:4} {id} {p}\n"));
    }
    if n > head {
        out.push_str(&format!("    ... {} more in the file ...\n", n - head - tail));
    }
    if tail > 0 {
        out.push_str(&format!("  last {tail} steps:\n"));
        for (i, (id, p)) in sequence.iter().enumerate().skip(n - tail) {
            out.push_str(&format!("    {i:4} {id} {p}\n"));
        }
    }
    out
}

/// Writes the whole sequence to a capture file under the OS temp dir and returns its path. Only the
/// steps that ran are written (a cut seed stops early), and the file starts from the workbook
/// `file.new` opens for `# sample:`.
#[allow(clippy::disallowed_methods)] // the capture is stamped with the clock; this test never runs on wasm
fn capture_file(seed: u64, sample: &str, planned: usize, sequence: &[Step], f: &Failure) -> Result<PathBuf, String> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = std::env::temp_dir().join(format!("gridcraft-soak-seed-{seed}-{stamp}.txt"));
    let mut text = String::from("# GridCraft command-sequence soak capture: replay it with GRIDCRAFT_FUZZ_REPLAY=<this file>.\n");
    text.push_str("# Steps are `id json`, one per line, from the workbook `file.new` opens for `# sample:`.\n");
    text.push_str(&format!("# seed: {seed}\n# sample: {sample}\n# planned steps: {planned}\n# captured steps: {}\n", sequence.len()));
    let stopped = if f.kind == SLOW { "cut" } else { "failure" };
    text.push_str(&format!("# {stopped} at step {} [{}]: {}\n", f.step, f.kind, f.what.replace('\n', " ")));
    for (id, p) in sequence {
        text.push_str(&format!("{id} {p}\n"));
    }
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

struct Harness {
    s: Session,
    /// The workbook at the start of the current episode (what undoing everything gives back).
    base: Arc<Workbook>,
    /// `DocState::history_dropped` when the episode started: past that value the byte cap has
    /// dropped entries, so undoing everything can no longer reach `base`.
    dropped_at_start: u64,
}

fn fail(step: usize, kind: &'static str, what: impl Into<String>) -> Failure {
    Failure { step, kind, what: what.into() }
}

/// Runs `f`, turning a panic into a failure (for checks that run outside `Session::execute`).
fn guarded<T>(step: usize, kind: &'static str, f: impl FnOnce() -> Result<T, String>) -> Result<T, Failure> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(fail(step, kind, e)),
        Err(p) => {
            let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
            Err(fail(step, kind, format!("panicked: {msg}")))
        }
    }
}

impl Harness {
    fn new(sample: &str) -> Harness {
        let mut s = Session::new();
        s.execute("file.new", json!({"sample": sample})).expect("sample opens");
        let base = s.doc().expect("document").wb.clone();
        Harness { s, base, dropped_at_start: 0 }
    }

    fn wb(&self) -> Arc<Workbook> {
        self.s.doc().expect("document").wb.clone()
    }

    fn step(&mut self, i: usize, id: &str, params: &Json) -> Result<(), Failure> {
        let before = self.wb();
        let r = self.s.execute(id, params.clone());
        self.s.take_ui_requests();
        if let Err(EngineError::Internal(_, msg)) = &r {
            return Err(fail(i, "panic", format!("`{id}` panicked (caught by Session::execute): {msg}")));
        }
        if self.s.documents().len() != 1 {
            return Err(fail(i, "documents", format!("`{id}` changed the number of open documents; exclude it in EXCLUDED")));
        }
        let after = self.wb();
        let spec = find_command(id).expect("generated ids are registered");
        if !spec.undoable
            && !matches!(id, "edit.undo" | "edit.redo")
            && let Some(diff) = content_diff(&before, &after)
        {
            return Err(fail(i, "not undoable", format!("non-undoable `{id}` changed cell content: {diff}")));
        }
        self.check_state(i)?;
        if (i + 1).is_multiple_of(ROUNDTRIP_EVERY) {
            if let Some(e) = cells_len_diff(&after) {
                return Err(fail(i, "cells", e));
            }
            guarded(i, "xlsx roundtrip", || roundtrip_diff(&after))?;
        }
        let over = {
            let d = self.s.doc().expect("document");
            d.undo.len() >= EPISODE_UNDO_DEPTH || d.undo.iter().map(|e| e.retained_bytes).sum::<usize>() > EPISODE_UNDO_BYTES
        };
        if over {
            self.end_episode(i)?;
        }
        Ok(())
    }

    /// Structure and recalculation invariants on the current state.
    fn check_state(&self, i: usize) -> Result<(), Failure> {
        let d = self.s.doc().expect("document");
        if let Some(e) = structure_problem(&d.wb, &d.selection) {
            return Err(fail(i, "structure", e));
        }
        let wb = d.wb.clone();
        guarded(i, "recalc", || recalc_diff(&wb))
    }

    /// Undo everything, compare with the start, redo everything, compare with the end, then save
    /// and reload; the next episode starts from here with an empty history.
    fn end_episode(&mut self, i: usize) -> Result<(), Failure> {
        let end = self.wb();
        if std::env::var_os("GRIDCRAFT_FUZZ_TRACE").is_some() {
            let d = self.s.doc().expect("document");
            eprintln!(
                "episode ends at step {i}: {} undo, {} redo, base sheets {:?}",
                d.undo.len(),
                d.redo.len(),
                self.base.sheets.iter().map(|s| s.name.as_str()).collect::<Vec<_>>()
            );
        }
        let mut n = 0;
        while !self.s.doc().expect("document").undo.is_empty() {
            if let Err(EngineError::Internal(_, msg)) = self.s.execute("edit.undo", json!({})) {
                return Err(fail(i, "panic", format!("`edit.undo` panicked while undoing everything: {msg}")));
            }
            n += 1;
            if n > 1_000 {
                return Err(fail(i, "undo", "undo never empties its history"));
            }
        }
        let undone = self.wb();
        // A history trimmed at the byte cap can no longer replay the whole episode, and the two
        // comparisons below are then meaningless. The rest of the checks still run. A redo stack
        // shorter than the steps undone catches a failed redo; the dropped counter catches
        // entries the cap removed *before* the episode ended, which the redo length cannot.
        let trimmed = self.s.doc().expect("document").redo.len() < n || self.s.doc().expect("document").history_dropped > self.dropped_at_start;
        if !trimmed && let Some(diff) = workbook_diff(&self.base, &undone) {
            return Err(fail(i, "undo", format!("undoing all {n} steps doesn't give back the starting workbook: {diff}")));
        }
        if watch_list(&end) != watch_list(&undone) {
            return Err(fail(i, "undo", format!("undoing all {n} steps changed the Watch Window list (not an edit)")));
        }
        self.check_state(i)?;
        let have = self.s.doc().expect("document").redo.len();
        // Only the steps undone here: an `edit.undo {steps}` in the sequence may have left more.
        for _ in 0..n {
            if let Err(EngineError::Internal(_, msg)) = self.s.execute("edit.redo", json!({})) {
                return Err(fail(i, "panic", format!("`edit.redo` panicked while redoing everything: {msg}")));
            }
        }
        let left = self.s.doc().expect("document").redo.len();
        if !trimmed && let Some(diff) = workbook_diff(&end, &self.wb()) {
            return Err(fail(
                i,
                "redo",
                format!("redoing all {n} steps (redo stack {have}, {left} left) doesn't give back the final workbook: {diff}"),
            ));
        }
        self.check_state(i)?;
        guarded(i, "xlsx roundtrip", || roundtrip_diff(&end))?;
        let d = self.s.active_mut().expect("document");
        d.undo.clear();
        d.redo.clear();
        self.base = d.wb.clone();
        self.dropped_at_start = d.history_dropped;
        Ok(())
    }

    fn finish(&mut self, steps: usize) -> Result<(), Failure> {
        self.end_episode(steps)
    }
}

// ---------------------------------------------------------------- invariants

fn overlaps(a: &RangeRef, b: &RangeRef) -> bool {
    a.intersection(b).is_some()
}

fn structure_problem(wb: &Workbook, sel: &gridcraft_engine::Selection) -> Option<String> {
    if wb.sheets.is_empty() {
        return Some("no sheets left".into());
    }
    if wb.active_sheet >= wb.sheets.len() {
        return Some(format!("active sheet {} of {}", wb.active_sheet, wb.sheets.len()));
    }
    let mut tables = BTreeSet::new();
    for (i, sh) in wb.sheets.iter().enumerate() {
        if let Err(e) = wb.check_sheet_name(&sh.name, Some(i)) {
            return Some(format!("sheet name `{}`: {e}", sh.name));
        }
        let on_sheet = |r: &RangeRef| r.start.is_valid() && r.end.is_valid() && r.start.row <= r.end.row && r.start.col <= r.end.col;
        if let Some(r) = sh.merges.iter().chain(sh.tables.iter().map(|t| &t.range)).find(|r| !on_sheet(r)) {
            return Some(format!("{}: merge or table range {r:?} isn't on the sheet", sh.name));
        }
        // A legacy array formula lives in the top-left cell of its range.
        for (c, cell) in sh.cells.iter() {
            if let Some(a) = cell.formula.as_ref().and_then(|f| f.array)
                && (a.start != c || !on_sheet(&a))
            {
                return Some(format!("{}!{c}: array formula covers {a}", sh.name));
            }
        }
        for (k, m) in sh.merges.iter().enumerate() {
            if let Some(o) = sh.merges.iter().skip(k + 1).find(|o| overlaps(m, o)) {
                return Some(format!("{}: merges {m} and {o} overlap", sh.name));
            }
        }
        for (k, t) in sh.tables.iter().enumerate() {
            if !tables.insert(t.name.to_ascii_lowercase()) {
                return Some(format!("two tables named `{}`", t.name));
            }
            if t.columns.len() as u32 != t.range.end.col - t.range.start.col + 1 {
                return Some(format!("table `{}` at {} has {} columns", t.name, t.range, t.columns.len()));
            }
            if let Some(o) = sh.tables.iter().skip(k + 1).find(|o| overlaps(&t.range, &o.range)) {
                return Some(format!("{}: tables `{}` ({}) and `{}` ({}) overlap", sh.name, t.name, t.range, o.name, o.range));
            }
        }
    }
    if sel.ranges.is_empty() || !sel.active.is_valid() || sel.ranges.iter().any(|r| !r.start.is_valid() || !r.end.is_valid()) {
        return Some(format!("selection {sel:?}"));
    }
    None
}

fn same_value(a: &Value, b: &Value, file: bool) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x == y || (x.is_nan() && y.is_nan()),
        // What the file format cannot store: `#SPILL!`/`#CALC!` are cached as `#VALUE!` and
        // `#CIRC!` as 0 (see the writer; Excel refuses those error codes in cached values). A
        // full recalc must still agree exactly; only the save/reload comparison is tolerant.
        (Value::Error(a), Value::Error(b)) if file => {
            let file_value = |e: &gridcraft_core::CellError| matches!(e, gridcraft_core::CellError::Value);
            let unrepresentable = |e: &gridcraft_core::CellError| matches!(e, gridcraft_core::CellError::Spill | gridcraft_core::CellError::Calc);
            a == b || (unrepresentable(a) && file_value(b)) || (unrepresentable(b) && file_value(a))
        }
        (Value::Error(gridcraft_core::CellError::Circ), Value::Number(n)) if file => *n == 0.0,
        (Value::Number(n), Value::Error(gridcraft_core::CellError::Circ)) if file => *n == 0.0,
        _ => a == b,
    }
}

/// The first cell whose displayed value (own value or spilled) differs between two workbooks.
fn value_diff(a: &Workbook, b: &Workbook, what: &str, file: bool) -> Option<String> {
    for (x, y) in a.sheets.iter().zip(&b.sheets) {
        let mut keys: BTreeSet<CellRef> = BTreeSet::new();
        for s in [x, y] {
            keys.extend(s.cells.iter().filter(|(_, c)| has_content(c)).map(|(c, _)| c));
            keys.extend(s.spill.keys().copied());
        }
        for c in keys {
            let (va, vb) = (x.value(c), y.value(c));
            if !same_value(&va, &vb, file) {
                let f = x
                    .cell(c)
                    .and_then(|c| c.formula)
                    .map(|f| format!(" (formula ={})", f.text))
                    .or_else(|| {
                        // A spilled cell: name the array it came from, which is what has a formula.
                        let (anchor, _) = x.spill_ranges.iter().find(|(_, r)| r.contains(c))?;
                        let f = x.cell(*anchor).and_then(|c| c.formula)?;
                        Some(format!(" (spilled from {} ={})", anchor.a1(), f.text))
                    })
                    .unwrap_or_default();
                return Some(format!("{}!{c}{f}: {va:?} in memory, {vb:?} after {what}", x.name));
            }
        }
    }
    None
}

/// `CellStore::len` must count exactly the cells `iter` yields. A shift that is not paired with a
/// take leaves a cell no logical position reaches, which compaction and paging both assume away;
/// walking the logical cells is the only check that sees it.
fn cells_len_diff(wb: &Workbook) -> Option<String> {
    wb.sheets.iter().find_map(|sh| {
        let count = sh.cells.iter().count();
        (sh.cells.len() != count).then(|| format!("{}: cells.len() is {} but iter() yields {count}", sh.name, sh.cells.len()))
    })
}

/// A full recalculation from scratch must agree with the incrementally maintained values.
fn recalc_diff(wb: &Workbook) -> Result<(), String> {
    if wb.calc.mode != CalcMode::Automatic || wb.calc.iterative {
        return Ok(());
    }
    let mut fresh = wb.clone();
    Calc::new().recalc_all(&mut fresh);
    value_diff(wb, &fresh, "a full recalc", false).map_or(Ok(()), Err)
}

fn normalized_formula(f: &Formula) -> String {
    let text = Formula::new(&f.text).text;
    if f.array.is_some() { format!("{{={text}}}") } else { format!("={text}") }
}

/// A line as the file keeps it: line, hidden, outline, collapsed, style, user-set size flag and
/// the size in the file's own precision.
type KeptLine = (u32, bool, u8, bool, Option<Style>, bool, i64);

/// The line metadata a save and reload gives back. The format is lossier than the model in four
/// ways, all of them in `sheet_write`/`sheet_read`:
/// - a line whose settings are all default is not written at all;
/// - a row height the user did not set is auto-fitted from content and re-fitted on open
///   (`fit_imported_rows`), so it is derived, not content;
/// - a height is stored in points (rounded to 0.01) and a width in whole pixels;
/// - a line whose style is the default style is written as `style="0"` and reads back unstyled.
///
/// Sizes compare in the file's units, styles by content (the style table may be renumbered).
fn kept_lines(map: &LineMap, styles: &StyleTable, rows: bool) -> Vec<KeptLine> {
    map.iter()
        .filter_map(|(line, info)| {
            let style = info.style.filter(|s| *s != StyleId::DEFAULT).map(|s| styles.get(s).clone());
            let outline = info.outline.min(7);
            let custom = rows && info.custom && info.size.is_some();
            let size = match info.size {
                _ if rows && !custom => -1,
                Some(px) if rows => (f64::from(px.clamp(0.0, 546.0)) * 75.0).round() as i64,
                Some(px) => px.clamp(0.0, 1790.0).round() as i64,
                None => -1,
            };
            let content = info.hidden || outline > 0 || info.collapsed || style.is_some() || custom || size >= 0;
            content.then_some((line, info.hidden, outline, info.collapsed, style, custom, size))
        })
        .collect()
}

/// Save to XLSX, reload (recalculating like opening does) and compare sheets, formulas and values.
fn roundtrip_diff(wb: &Workbook) -> Result<(), String> {
    let bytes = io::save_bytes(wb, "fuzz.xlsx").map_err(|e| format!("save failed: {e}"))?;
    let (mut back, _) = io::open_bytes("fuzz.xlsx", &bytes).map_err(|e| format!("reload failed: {e}"))?;
    Calc::new().recalc_all(&mut back);
    let names = |wb: &Workbook| wb.sheets.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
    if names(wb) != names(&back) {
        return Err(format!("sheets {:?} reload as {:?}", names(wb), names(&back)));
    }
    for (x, y) in wb.sheets.iter().zip(&back.sheets) {
        let formulas = |s: &Sheet| s.cells.iter().filter_map(|(c, cell)| Some((c, normalized_formula(cell.formula.as_ref()?)))).collect::<Vec<_>>();
        let (fx, fy) = (formulas(x), formulas(y));
        if fx != fy {
            let first = fx.iter().zip(&fy).find(|(a, b)| a != b).map(|(a, b)| format!("{a:?} vs {b:?}"));
            let first = first.unwrap_or_else(|| format!("{} formulas vs {}", fx.len(), fy.len()));
            return Err(format!("{}: formulas differ after reload: {first}", x.name));
        }
        // Line metadata maps through physical ids now; the file still stores logical lines, so a
        // mapped sheet must reload with the same rows and columns.
        if kept_lines(&x.rows, &wb.styles, true) != kept_lines(&y.rows, &back.styles, true) {
            return Err(format!("{}: rows differ after reload", x.name));
        }
        if kept_lines(&x.cols, &wb.styles, false) != kept_lines(&y.cols, &back.styles, false) {
            return Err(format!("{}: cols differ after reload", x.name));
        }
    }
    if wb.calc.mode == CalcMode::Automatic && !wb.calc.iterative {
        value_diff(wb, &back, "an XLSX save and reload", true).map_or(Ok(()), Err)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- generation

fn letter(c: u32) -> String {
    col_to_letters(c)
}

/// A row/column index: mostly near the data, sometimes further out, rarely at the sheet's edge.
fn coord(rng: &mut Rng, near: i64, far: i64, max: u32) -> u32 {
    match rng.below(20) {
        0 => max - 1 - rng.int(0, 2) as u32,
        1 | 2 => rng.int(near, far) as u32,
        _ => rng.int(0, near) as u32,
    }
}

fn cell(rng: &mut Rng) -> CellRef {
    CellRef::new(coord(rng, 24, 120, MAX_ROWS), coord(rng, 11, 40, MAX_COLS))
}

/// A cell near the data (formula references stay small).
fn near_cell(rng: &mut Rng) -> CellRef {
    CellRef::new(rng.int(0, 22) as u32, rng.int(0, 10) as u32)
}

fn near_range(rng: &mut Rng) -> RangeRef {
    let a = near_cell(rng);
    let b = CellRef::new(a.row + rng.int(0, 7) as u32, a.col + rng.int(0, 4) as u32);
    RangeRef::new(a, b)
}

fn range(rng: &mut Rng) -> String {
    match rng.below(20) {
        0 | 1 => {
            let c = rng.int(0, 10) as u32;
            format!("{}:{}", letter(c), letter(c + rng.int(0, 2) as u32))
        }
        2 | 3 => {
            let r = rng.int(1, 25);
            format!("{r}:{}", r + rng.int(0, 2))
        }
        4 => cell(rng).a1(),
        5 => {
            let a = cell(rng);
            RangeRef::new(a, a.offset_clamped(rng.int(0, 3), rng.int(0, 3))).a1()
        }
        _ => near_range(rng).a1(),
    }
}

fn rows_spec(rng: &mut Rng) -> String {
    let r = coord(rng, 24, 120, MAX_ROWS) as i64 + 1;
    format!("{r}:{}", (r + rng.int(0, 2)).min(MAX_ROWS as i64))
}

fn cols_spec(rng: &mut Rng) -> String {
    let c = coord(rng, 11, 40, MAX_COLS);
    format!("{}:{}", letter(c), letter((c + rng.int(0, 1) as u32).min(MAX_COLS - 1)))
}

fn quoted(name: &str) -> String {
    format!("'{}'", name.replace('\'', "''"))
}

fn wb_of(s: &Session) -> &Workbook {
    &s.doc().expect("document").wb
}

fn spill_anchors(wb: &Workbook) -> Vec<(usize, CellRef)> {
    wb.sheets.iter().enumerate().flat_map(|(i, sh)| sh.spill_ranges.keys().map(move |c| (i, *c))).collect()
}

fn tables(wb: &Workbook) -> Vec<(usize, &gridcraft_engine::model::Table)> {
    wb.sheets.iter().enumerate().flat_map(|(i, sh)| sh.tables.iter().map(move |t| (i, t))).collect()
}

fn other_sheet(rng: &mut Rng, wb: &Workbook) -> String {
    quoted(&wb.sheets[rng.below(wb.sheets.len())].name)
}

fn formula(rng: &mut Rng, wb: &Workbook) -> String {
    let c = near_cell(rng).a1();
    let c2 = near_cell(rng).a1();
    let r = near_range(rng).a1();
    let n = rng.int(1, 6);
    let col = letter(rng.int(0, 9) as u32);
    let tabs = tables(wb);
    match rng.below(30) {
        0 => format!("={c}+{n}"),
        1 => format!("=SUM({r})"),
        2 => format!("={c}*{c2}"),
        3 => format!("={}!{c}*2", other_sheet(rng, wb)),
        4 => format!("=SEQUENCE({n},{})", rng.int(1, 3)),
        5 | 6 if !tabs.is_empty() => {
            let (_, t) = rng.pick(&tabs);
            let column = t.columns.get(rng.below(t.columns.len())).map(|c| c.name.clone()).unwrap_or_default();
            match rng.below(4) {
                0 => format!("=SUM({}[{column}])", t.name),
                1 => format!("=ROWS({})", t.name),
                2 => format!("={}[@{column}]", t.name),
                _ => format!("=COUNTA({}[#All])", t.name),
            }
        }
        7 => format!("=INDIRECT(\"{c}\")"),
        8 => format!("=OFFSET({c},1,1)"),
        9 => format!("=IF({c}>{n},\"big\",{c})"),
        10 => {
            let anchors = spill_anchors(wb);
            match anchors.get(rng.below(anchors.len().max(1))) {
                Some((si, a)) => format!("=SUM({}!{a}#)", quoted(&wb.sheets[*si].name)),
                None => format!("={c}#"),
            }
        }
        11 => format!("=SORT({r})"),
        12 => format!("=UNIQUE({r})"),
        13 => format!("=TRANSPOSE({r})"),
        14 => format!("=COUNTIF({col}:{col},\">0\")"),
        15 => format!("=SUM({}:{}!{c})", other_sheet(rng, wb), other_sheet(rng, wb)),
        16 => format!("=LET(x,{c},x*2)"),
        17 => format!("=XLOOKUP({c},{col}1:{col}20,{col}1:{col}20)"),
        18 => format!("=FILTER({col}1:{col}20,{col}1:{col}20>{n})"),
        19 => match wb.names.get(rng.below(wb.names.len().max(1))) {
            Some(nm) => format!("=SUM({})", nm.name),
            None => format!("={c}"),
        },
        20 => "=ROW()+COLUMN()".into(),
        21 => format!("={c}&\"-\"&{c2}"),
        22 => format!("=AVERAGE({r})*{n}"),
        23 => format!("=MAX({r})-MIN({r})"),
        24 => rng.pick(&["=1/0", "=NA()", "=#REF!+1", "=NOSUCHFN(1)", "=SUM(", "=\"text\""]).to_string(),
        25 => format!("={r}"),
        26 => format!("=INDEX({r},{n},1)"),
        27 => format!("=VLOOKUP({c},{r},2,FALSE)"),
        28 => format!("=SEQUENCE({n})*{c}"),
        _ => format!("=TEXT({c},\"0.00\")"),
    }
}

fn literal(rng: &mut Rng) -> String {
    match rng.below(14) {
        0..=3 => rng.int(-50, 500).to_string(),
        4 => format!("{}.{}", rng.int(-9, 99), rng.int(0, 99)),
        5 => rng.pick(&["North", "South", "East", "West", "abc", "Total", "x y"]).to_string(),
        6 => rng.pick(&["TRUE", "FALSE"]).to_string(),
        7 => format!("{}%", rng.int(0, 100)),
        8 => format!("2026-{:02}-{:02}", rng.int(1, 12), rng.int(1, 28)),
        9 => String::new(),
        10 => "'007".into(),
        11 => "$1,234.50".into(),
        12 => "#N/A".into(),
        _ => "1e3".into(),
    }
}

fn input(rng: &mut Rng, wb: &Workbook) -> String {
    if rng.chance(55) { formula(rng, wb) } else { literal(rng) }
}

fn json_input(rng: &mut Rng, wb: &Workbook) -> Json {
    match rng.below(10) {
        0..=1 => json!(rng.int(-20, 200)),
        2 => Json::Null,
        3 => json!(rng.chance(50)),
        _ => json!(input(rng, wb)),
    }
}

fn values_2d(rng: &mut Rng, wb: &Workbook) -> Json {
    let (h, w) = (rng.int(1, 4), rng.int(1, 3));
    Json::Array((0..h).map(|_| Json::Array((0..w).map(|_| json_input(rng, wb)).collect())).collect())
}

fn sheet_param(rng: &mut Rng, wb: &Workbook) -> Json {
    match rng.below(10) {
        0 => json!(wb.sheets.len() + rng.below(2)),
        1..=4 => json!(wb.sheets[rng.below(wb.sheets.len())].name),
        _ => json!(rng.below(wb.sheets.len())),
    }
}

fn new_name(rng: &mut Rng) -> String {
    rng.pick(&["Data", "Q4 2026", "Summary", "Sheet9", "x", "Sales", "Long name of a sheet 31 chars!", "it's", "Grades"]).to_string()
}

fn color(rng: &mut Rng) -> String {
    rng.pick(&["#FF0000", "#00B050", "#FFFF00", "#1F4E79", "none"]).to_string()
}

fn table_param(rng: &mut Rng, wb: &Workbook) -> Json {
    let tabs = tables(wb);
    match tabs.get(rng.below(tabs.len() + 1)) {
        Some((_, t)) => json!(t.name),
        None => json!("NoSuchTable"),
    }
}

fn chart_ids(wb: &Workbook) -> Vec<u32> {
    wb.sheets.iter().flat_map(|s| s.charts.iter().map(|c| c.id)).collect()
}

/// Curated state-changing commands and their weights.
const CURATED: &[(&str, usize)] = &[
    ("cell.set", 22),
    ("range.setValues", 5),
    ("range.fill", 2),
    ("selection.set", 8),
    ("edit.undo", 6),
    ("edit.redo", 3),
    ("edit.copy", 3),
    ("edit.cut", 2),
    ("edit.paste", 4),
    ("edit.pasteSpecial", 2),
    ("edit.clearContents", 2),
    ("edit.clearAll", 1),
    ("edit.fillDown", 2),
    ("edit.fillRight", 1),
    ("edit.autoFill", 2),
    ("edit.fillSeries", 1),
    ("home.bold", 1),
    ("home.fillColor", 1),
    ("home.numberFormat", 1),
    ("home.borders", 1),
    ("home.mergeCenter", 2),
    ("home.mergeCells", 1),
    ("home.unmergeCells", 1),
    ("home.insertRows", 4),
    ("home.insertColumns", 3),
    ("home.deleteRows", 4),
    ("home.deleteColumns", 3),
    ("home.insertCells", 2),
    ("home.deleteCells", 2),
    ("home.hideRows", 1),
    ("home.unhideRows", 1),
    ("home.insertSheet", 2),
    ("home.deleteSheet", 2),
    ("sheet.rename", 2),
    ("sheet.move", 2),
    ("sheet.activate", 3),
    ("insert.table", 3),
    ("table.totalRow", 1),
    ("table.resize", 1),
    ("table.convertToRange", 1),
    ("insert.chart", 2),
    ("chart.delete", 1),
    ("data.sortAscending", 2),
    ("data.sort", 2),
    ("data.filter", 2),
    ("data.filterBy", 2),
    ("data.clearFilter", 1),
    ("data.removeDuplicates", 1),
    ("formulas.defineName", 2),
    ("formulas.deleteName", 1),
    ("edit.replace", 1),
];

fn generate(rng: &mut Rng, s: &Session) -> Step {
    let wb = wb_of(s);
    if rng.chance(82) {
        let total: usize = CURATED.iter().map(|(_, w)| w).sum();
        let mut roll = rng.below(total);
        for (id, w) in CURATED {
            if roll < *w {
                return (id.to_string(), curated_params(rng, wb, id));
            }
            roll -= w;
        }
    }
    let specs: Vec<_> = command_specs().iter().filter(|c| !EXCLUDED.iter().any(|(x, _)| *x == c.id)).collect();
    let spec = specs[rng.below(specs.len())];
    (spec.id.to_string(), doc_params(rng, wb, spec.params))
}

fn curated_params(rng: &mut Rng, wb: &Workbook, id: &str) -> Json {
    let maybe_range = |rng: &mut Rng| if rng.chance(70) { json!({"range": range(rng)}) } else { json!({}) };
    match id {
        "cell.set" => {
            let mut p = json!({"cell": cell(rng).a1(), "input": input(rng, wb)});
            if rng.chance(10) {
                p["sheet"] = sheet_param(rng, wb);
            }
            if rng.chance(4) {
                p["array"] = json!(true);
            }
            p
        }
        "range.setValues" => json!({"range": near_cell(rng).a1(), "values": values_2d(rng, wb)}),
        "range.fill" => json!({"range": near_range(rng).a1(), "input": input(rng, wb)}),
        "selection.set" => {
            let mut p = json!({"range": range(rng)});
            if rng.chance(15) {
                p["range"] = json!(format!("{},{}", near_range(rng).a1(), near_cell(rng).a1()));
            }
            p
        }
        "edit.undo" | "edit.redo" => json!({"steps": rng.int(1, 3)}),
        "edit.copy" | "edit.cut" => maybe_range(rng),
        "edit.paste" => {
            if rng.chance(15) {
                json!({"at": near_cell(rng).a1(), "text": "1\t2\nx\t=A1"})
            } else {
                json!({"at": cell(rng).a1()})
            }
        }
        "edit.pasteSpecial" => json!({
            "what": rng.pick(&["all", "formulas", "values", "formats", "allExceptBorders", "columnWidths", "valuesAndNumberFormats"]),
            "operation": rng.pick(&["none", "add", "multiply"]),
            "skipBlanks": rng.chance(30),
            "transpose": rng.chance(30),
        }),
        "edit.autoFill" => {
            let src = near_range(rng);
            let tgt = RangeRef::new(src.start, CellRef::new(src.end.row + rng.int(0, 6) as u32, src.end.col + rng.int(0, 1) as u32));
            json!({"source": src.a1(), "target": tgt.a1(), "mode": rng.pick(&["series", "copy", "alternate", "formats", "values"])})
        }
        "edit.fillSeries" => json!({"range": near_range(rng).a1(), "type": rng.pick(&["linear", "growth", "autofill"]), "step": rng.int(-2, 3)}),
        "home.bold" => json!({"range": range(rng)}),
        "home.fillColor" => json!({"range": range(rng), "color": color(rng)}),
        "home.numberFormat" => json!({"range": range(rng), "format": rng.pick(&["Currency", "Percentage", "Short Date", "Text", "General"])}),
        "home.borders" => json!({"range": range(rng), "preset": rng.pick(&["all", "outside", "none", "thickBottom"])}),
        "home.insertRows" | "home.deleteRows" | "home.hideRows" | "home.unhideRows" => {
            let mut p = json!({"rows": rows_spec(rng)});
            if id == "home.insertRows" && rng.chance(20) {
                p["count"] = json!(rng.int(1, 4));
            }
            p
        }
        "home.insertColumns" | "home.deleteColumns" => json!({"cols": cols_spec(rng)}),
        "home.insertCells" => json!({"range": near_range(rng).a1(), "shift": rng.pick(&["right", "down", "row", "column"])}),
        "home.deleteCells" => json!({"range": near_range(rng).a1(), "shift": rng.pick(&["left", "up", "row", "column"])}),
        "home.insertSheet" => {
            let mut p = json!({});
            if rng.chance(40) {
                p["name"] = json!(new_name(rng));
            }
            if rng.chance(40) {
                p["before"] = json!(rng.below(wb.sheets.len() + 1));
            }
            p
        }
        "home.deleteSheet" | "sheet.activate" => json!({"sheet": sheet_param(rng, wb)}),
        "sheet.rename" => json!({"sheet": sheet_param(rng, wb), "name": new_name(rng)}),
        "sheet.move" => json!({"sheet": sheet_param(rng, wb), "to": rng.below(wb.sheets.len() + 1), "copy": rng.chance(35)}),
        "insert.table" => {
            let mut p = json!({"range": near_range(rng).a1(), "header": rng.chance(80)});
            if rng.chance(40) {
                p["name"] = json!(rng.pick(&["T1", "Sales", "Data", "Grades"]));
            }
            p
        }
        "table.totalRow" => json!({"table": table_param(rng, wb), "on": rng.chance(50)}),
        "table.resize" => json!({"table": table_param(rng, wb), "range": near_range(rng).a1()}),
        "table.convertToRange" => json!({"table": table_param(rng, wb)}),
        "insert.chart" => {
            json!({"range": near_range(rng).a1(), "type": rng.pick(&["column", "line", "pie", "scatter", "bar"]), "at": cell(rng).a1()})
        }
        "chart.delete" => {
            let ids = chart_ids(wb);
            match ids.get(rng.below(ids.len() + 1)) {
                Some(id) => json!({"chart": id}),
                None => json!({}),
            }
        }
        "data.sortAscending" => {
            let mut p = maybe_range(rng);
            if rng.chance(50) {
                p["column"] = json!(letter(rng.int(0, 9) as u32));
            }
            p
        }
        "data.sort" => json!({
            "range": near_range(rng).a1(),
            "header": rng.chance(50),
            "keys": [{"column": letter(rng.int(0, 9) as u32), "order": rng.pick(&["asc", "desc"])}],
            "orientation": if rng.chance(15) { "columns" } else { "rows" },
        }),
        "data.filter" => maybe_range(rng),
        "data.filterBy" => {
            let column = letter(rng.int(0, 9) as u32);
            match rng.below(4) {
                0 => json!({"column": column, "values": [literal(rng), literal(rng)]}),
                1 => json!({"column": column, "custom": {"op": ">", "value": rng.int(0, 60000).to_string()}}),
                2 => json!({"column": column, "top": {"count": rng.int(1, 5)}}),
                _ => json!({"column": column, "clear": true}),
            }
        }
        "data.removeDuplicates" => json!({"range": near_range(rng).a1(), "header": rng.chance(50)}),
        "formulas.defineName" => {
            let sheet = other_sheet(rng, wb);
            let r = near_range(rng);
            let abs = format!("${}${}:${}${}", letter(r.start.col), r.start.row + 1, letter(r.end.col), r.end.row + 1);
            json!({"name": rng.pick(&["Rate", "Totals", "my_list", "Q"]), "refersTo": format!("={sheet}!{abs}")})
        }
        "formulas.deleteName" => match wb.names.get(rng.below(wb.names.len() + 1)) {
            Some(n) => json!({"name": n.name}),
            None => json!({"name": "Nope"}),
        },
        "edit.replace" => json!({"what": rng.pick(&["North", "1", "A", "Sales"]), "with": rng.pick(&["South", "2", "B", ""]), "all": true}),
        _ => maybe_range(rng),
    }
}

/// Top-level keys of a params doc (`{range?, shift: right|down, count?}`): (key, optional, hint).
fn doc_keys(doc: &str) -> Vec<(String, bool, String)> {
    let Some(start) = doc.find('{') else { return vec![] };
    let mut parts = Vec::new();
    let (mut depth, mut in_str, mut cur) = (0i32, false, String::new());
    for ch in doc[start + 1..].chars() {
        match ch {
            '"' => {
                in_str = !in_str;
                cur.push(ch);
            }
            _ if in_str => cur.push(ch),
            '{' | '[' | '(' => {
                depth += 1;
                cur.push(ch);
            }
            '}' | ']' | ')' if depth == 0 => break,
            '}' | ']' | ')' => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if depth == 0 => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    parts.push(cur);
    let mut out = Vec::new();
    for part in parts {
        let part = part.trim();
        let key: String = part.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        if key.is_empty() {
            continue;
        }
        let rest = &part[key.len()..];
        let optional = rest.starts_with('?');
        let hint = rest.trim_start_matches('?').trim_start();
        let hint = hint.strip_prefix(':').unwrap_or(hint).trim().to_string();
        out.push((key, optional, hint));
    }
    out
}

/// Bare-word alternatives in a hint (`asc|desc`, `"Good"|"Bad"`), if it is an enumeration.
fn alternatives(hint: &str) -> Option<Vec<String>> {
    if !hint.contains('|') {
        return None;
    }
    let alts: Vec<String> = hint
        .split('|')
        .map(|a| a.split(" (").next().unwrap_or(a).trim().trim_matches('"').trim_end_matches('…').to_string())
        .filter(|a| !a.is_empty() && a.chars().all(|c| c.is_ascii_alphanumeric() || " .".contains(c)))
        .collect();
    (!alts.is_empty()).then_some(alts)
}

/// Params from a command's params doc: values chosen by key name, then by the hint's shape.
fn doc_params(rng: &mut Rng, wb: &Workbook, doc: &str) -> Json {
    let mut p = serde_json::Map::new();
    for (key, optional, hint) in doc_keys(doc) {
        if optional && rng.chance(40) {
            continue;
        }
        if let Some(v) = doc_value(rng, wb, &key, &hint) {
            p.insert(key, v);
        }
    }
    Json::Object(p)
}

fn doc_value(rng: &mut Rng, wb: &Workbook, key: &str, hint: &str) -> Option<Json> {
    let lower = key.to_ascii_lowercase();
    let v = match lower.as_str() {
        "path" | "base64" | "password" | "html" => return None,
        "range" | "source" | "target" | "destination" | "criteria" | "copyto" | "printarea" | "location" | "resultcells" | "changing"
        | "rowinput" | "columninput" | "add" | "remove" | "apply" => json!(range(rng)),
        "refersto" => json!(format!("={}!$A$1:$B$4", other_sheet(rng, wb))),
        "cell" | "at" | "active" | "fromcell" | "set" | "reference" => json!(cell(rng).a1()),
        "rows" | "titlerows" if !hint.starts_with('[') => json!(rows_spec(rng)),
        "cols" | "titlecols" if !hint.starts_with('[') => json!(cols_spec(rng)),
        "sheet" => sheet_param(rng, wb),
        "input" | "formula" | "formula1" | "formula2" | "value" | "value2" | "with" => json!(input(rng, wb)),
        "table" => table_param(rng, wb),
        "chart" | "id" => {
            let ids = chart_ids(wb);
            json!(ids.get(rng.below(ids.len() + 1)).copied().unwrap_or(rng.int(0, 5) as u32))
        }
        "ids" => json!(chart_ids(wb)),
        "color" => json!(color(rng)),
        "values" if hint.starts_with("[[") => values_2d(rng, wb),
        "keys" => json!([{"column": letter(rng.int(0, 9) as u32), "order": rng.pick(&["asc", "desc"])}]),
        "column" | "groupby" => json!(letter(rng.int(0, 9) as u32)),
        "columns" | "sources" if !hint.contains('|') => json!([letter(rng.int(0, 9) as u32)]),
        "points" => json!([[10, 10], [40, 25], [70, 5]]),
        "commands" => json!([{"command": "cell.set", "params": {"cell": near_cell(rng).a1(), "input": input(rng, wb)}}]),
        "char" => json!("€"),
        "name" | "oldname" if alternatives(hint).is_none() => json!(new_name(rng)),
        "text" | "title" | "comment" | "author" | "word" | "header" | "footer" | "what" if alternatives(hint).is_none() => {
            json!(rng.pick(&["North", "note", "&CPage &P", "x"]))
        }
        _ => return Some(hint_value(rng, wb, hint)),
    };
    Some(v)
}

fn hint_value(rng: &mut Rng, wb: &Workbook, hint: &str) -> Json {
    if let Some(alts) = alternatives(hint) {
        return json!(rng.pick(&alts));
    }
    if hint.contains("bool") || hint == "true" || hint == "false" {
        return json!(rng.chance(50));
    }
    if let Some((a, b)) = hint.split_once("..")
        && let (Ok(a), Ok(b)) = (a.trim().parse::<i64>(), b.trim().split(' ').next().unwrap_or("").parse::<i64>())
    {
        return json!(rng.int(a, b));
    }
    if let Ok(n) = hint.split(' ').next().unwrap_or("").parse::<f64>() {
        return json!((n * rng.int(0, 3) as f64).round());
    }
    if hint.starts_with('[') {
        let heads: Vec<String> = tables(wb).iter().flat_map(|(_, t)| t.columns.iter().map(|c| c.name.clone())).collect();
        return match heads.get(rng.below(heads.len() + 1)) {
            Some(h) => json!([h]),
            None => json!([]),
        };
    }
    if let Some(ex) = hint.strip_prefix('"').and_then(|h| h.split('"').next())
        && rng.chance(60)
    {
        return json!(ex);
    }
    match rng.below(4) {
        0 => json!(rng.chance(50)),
        1 => json!(rng.int(0, 5)),
        2 => json!(near_range(rng).a1()),
        _ => json!(literal(rng)),
    }
}

#[test]
fn params_docs_parse_into_keys() {
    let keys = doc_keys("{range?, shift: right|down|row|column, input: \"text, number or =formula\"}");
    let names: Vec<_> = keys.iter().map(|(k, o, _)| (k.as_str(), *o)).collect();
    assert_eq!(names, [("range", true), ("shift", false), ("input", false)]);
    assert_eq!(alternatives(&keys[1].2), Some(vec!["right".into(), "down".into(), "row".into(), "column".into()]));
    // Every registered command's doc parses, and the fuzzer's exclusions name real commands.
    for spec in command_specs() {
        let _ = doc_keys(spec.params);
    }
    for (id, _) in EXCLUDED {
        assert!(find_command(id).is_some(), "EXCLUDED names unknown command `{id}`");
    }
    for (id, _) in CURATED {
        assert!(find_command(id).is_some(), "CURATED names unknown command `{id}`");
    }
}

/// Fixed sequences the soak turned up, replayed verbatim, because a seed's generated sequence changes
/// whenever the command set changes. Seed 295: a legacy array placed over the selection cleared cells
/// inside `B11`'s precedent range without marking them as changes, so the incremental pass kept
/// 1914.35 where a full recalc said 1755.
#[test]
fn fixed_regressions_from_the_soak() {
    let steps: &[Step] = &[
        ("table.resize".to_string(), json!({"range":"G3:K5","table":"NoSuchTable"})),
        ("edit.copy".to_string(), json!({})),
        ("cell.set".to_string(), json!({"cell":"C14","input":"=MAX(J10:K16)-MIN(J10:K16)"})),
        ("data.filter".to_string(), json!({"range":"C:E"})),
        ("cell.set".to_string(), json!({"cell":"K10","input":"=OFFSET(A21,1,1)"})),
        ("edit.undo".to_string(), json!({"steps":2})),
        ("data.sortAscending".to_string(), json!({"column":"C","range":"J9:M9"})),
        ("edit.autoFill".to_string(), json!({"mode":"series","source":"H16:H21","target":"H16:I27"})),
        ("edit.paste".to_string(), json!({"at":"E1"})),
        ("formulas.errorChecking".to_string(), json!({})),
        ("cell.set".to_string(), json!({"cell":"AO1","input":"=I6*C3"})),
        ("home.insertRows".to_string(), json!({"count":1,"rows":"19:19"})),
        ("edit.undo".to_string(), json!({"steps":2})),
        ("selection.column".to_string(), json!({})),
        ("edit.copy".to_string(), json!({})),
        ("home.mergeCenter".to_string(), json!({})),
        ("cell.set".to_string(), json!({"cell":"B11","input":"=MAX(A3:E10)-MIN(A3:E10)"})),
        ("cell.set".to_string(), json!({"cell":"L15","input":"=B9*D21"})),
        ("cell.set".to_string(), json!({"cell":"XFB1048574","input":"'007"})),
        ("range.setValues".to_string(), json!({"range":"G2","values":[[true,"#N/A","=UNIQUE(A22:B28)"],["1e3","86.55",null]]})),
        ("data.filter".to_string(), json!({"range":"5:7"})),
        ("chart.selectData".to_string(), json!({"categories":false,"chart":5,"range":"I4:K10"})),
        ("view.normal".to_string(), json!({})),
        ("cell.set".to_string(), json!({"array":true,"input":"=XLOOKUP(K4,D1:D20,D1:D20)"})),
    ];
    if let Some(f) = replay("budget", steps) {
        panic!("seed 295 regression: invariant [{}]: {}", f.kind, f.what);
    }
}
