//! Writes one small PDF per script (and the mixed-bidi cases on separate pages) with a face the
//! platform provides, for checking the writer's shaped output with an independent extractor:
//!
//! ```text
//! cargo run -p gridcraft-pdf --example text_scripts -- target/pdf-text
//! pdftotext -enc UTF-8 target/pdf-text/arabic.pdf -
//! ```
//!
//! A script the machine has no font for is reported and skipped (AGENTS.md: no font binaries are
//! committed here; set `CRAFT_FONTS_DIR` to a directory of Noto faces in a bare container). Each
//! sample sits alone on its page so the extractor's layout heuristics cannot merge lines.

use std::error::Error;
use std::path::PathBuf;

use gridcraft_core::fonts::{self, Script};
use gridcraft_pdf::{Direction, Face, Font, PdfDoc};

fn main() -> Result<(), Box<dyn Error>> {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "target/pdf-text".to_string()));
    std::fs::create_dir_all(&dir)?;
    let scripts = [
        ("cyrillic", Script::Cyrillic, "Привет, мир 2024"),
        ("arabic", Script::Arabic, "مرحبا بالعالم — لا 2024"),
        ("hebrew", Script::Hebrew, "שלום עולם 2024"),
        ("devanagari", Script::Devanagari, "नमस्ते दुनिया 2024"),
        ("thai", Script::Thai, "สวัสดีชาวโลก 2024"),
    ];
    for (name, script, sample) in scripts {
        let font = match fonts::resolve_for_script(script) {
            Ok(font) => font,
            Err(e) => {
                println!("{name}: skipped ({e})");
                continue;
            }
        };
        let face = match Face::parse(font.data) {
            Ok(face) => face,
            Err(e) => {
                println!("{name}: skipped ({} does not parse: {e})", font.path.display());
                continue;
            }
        };
        let mut doc = PdfDoc::new();
        doc.set_font(face);
        doc.info.title = format!("GridCraft {name} sample");
        let page = doc.add_page(420.0, 100.0);
        page.text(20.0, 55.0, 20.0, Font::Helvetica, [0, 0, 0], sample);
        let path = dir.join(format!("{name}.pdf"));
        std::fs::write(&path, doc.finish())?;
        println!("{name}: {} ({})", path.display(), font.path.display());
    }

    // The same Arabic face for the mixed-direction cases: an LTR-base line, an RTL-base line (a
    // right-to-left sheet passes Direction::Rtl) and neutral text under the RTL base direction.
    if let Ok(font) = fonts::resolve_for_script(Script::Arabic)
        && let Ok(face) = Face::parse(font.data)
    {
        let mut doc = PdfDoc::new();
        doc.set_font(face);
        let cases = [(Direction::Auto, "Hello مرحبا 2024"), (Direction::Rtl, "مرحبا Hello 2024"), (Direction::Rtl, "(2024)")];
        for (direction, text) in cases {
            let page = doc.add_page(420.0, 100.0);
            page.text_dir(20.0, 55.0, 20.0, Font::Helvetica, [0, 0, 0], text, direction);
        }
        let path = dir.join("bidi.pdf");
        std::fs::write(&path, doc.finish())?;
        println!("bidi: {} ({})", path.display(), font.path.display());
    }
    Ok(())
}
