//! Reads worksheet rows straight from the XML events.
//!
//! The generic [`crate::xml::parse_streaming`] builds an `El` — a name string, an attribute vector
//! and a text string — for every element. A worksheet has one element per cell (plus one per
//! `<v>`), so a million-row sheet means tens of millions of them, and for a big file that cost
//! more than decoding the XML did. Rows are regular enough to decode directly: this walker reads
//! `<row>`/`<c>`/`<v>`/`<f>` into reused buffers and hands them to a callback, and keeps the `El`
//! tree only for everything outside `<sheetData>` (views, columns, merges, tables…) and for the
//! rare `<is>` inline string.

use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};

use crate::IoError;
use crate::xml::{self, El};

/// `<row>` attributes the reader uses.
#[derive(Debug, Default)]
pub struct RowAttrs<'a> {
    pub r: Option<&'a str>,
    pub ht: Option<&'a str>,
    pub hidden: bool,
    pub outline: Option<&'a str>,
    pub collapsed: bool,
    pub custom_height: bool,
    pub custom_format: bool,
    pub s: Option<&'a str>,
}

/// `<f>` attributes and text.
#[derive(Debug, Default)]
pub struct FormulaAttrs<'a> {
    /// `t`: `normal`, `shared`, `array`, `dataTable`.
    pub kind: Option<&'a str>,
    pub si: Option<&'a str>,
    pub range: Option<&'a str>,
    pub text: &'a str,
}

/// One `<c>` element.
#[derive(Debug, Default)]
pub struct CellAttrs<'a> {
    pub r: Option<&'a str>,
    /// `t`: `n`, `s`, `str`, `inlineStr`, `b`, `e`, `d`.
    pub t: Option<&'a str>,
    pub s: Option<&'a str>,
    pub vm: Option<&'a str>,
    /// A dynamic-array cell (`cm` attribute present).
    pub cm: bool,
    /// Text of `<v>`, when the cell has one (it may be empty).
    pub v: Option<&'a str>,
    pub f: Option<FormulaAttrs<'a>>,
    /// The `<is>` subtree, when the cell carries an inline string.
    pub inline: Option<&'a El>,
}

/// What the walker reports, in document order.
pub enum Item<'a> {
    RowStart(RowAttrs<'a>),
    Cell(CellAttrs<'a>),
    RowEnd,
}

/// Reused buffers: a row's attributes, a cell's attributes and its text.
#[derive(Default)]
struct Buffers {
    /// `r`, `ht`, `outlineLevel`, `s`.
    row: [String; 4],
    /// `r`, `t`, `s`, `vm`.
    cell: [String; 4],
    cell_cm: bool,
    cell_has_f: bool,
    cell_has_v: bool,
    /// `si`, `ref`, `t`.
    f: [String; 3],
    v: String,
    f_text: String,
    inline: Option<El>,
}

impl Buffers {
    fn start_cell(&mut self, e: &BytesStart<'_>) {
        for s in &mut self.cell {
            s.clear();
        }
        self.cell_cm = false;
        self.cell_has_f = false;
        self.cell_has_v = false;
        self.inline = None;
        self.v.clear();
        self.f_text.clear();
        for a in e.attributes().with_checks(false).flatten() {
            let slot = match a.key.as_ref() {
                b"r" => 0,
                b"t" => 1,
                b"s" => 2,
                b"vm" => 3,
                b"cm" => {
                    self.cell_cm = true;
                    continue;
                }
                _ => continue,
            };
            attr_into(&mut self.cell[slot], &a);
        }
    }

    fn start_formula(&mut self, e: &BytesStart<'_>) {
        for s in &mut self.f {
            s.clear();
        }
        self.cell_has_f = true;
        self.f_text.clear();
        for a in e.attributes().with_checks(false).flatten() {
            let slot = match a.key.as_ref() {
                b"si" => 0,
                b"ref" => 1,
                b"t" => 2,
                _ => continue,
            };
            attr_into(&mut self.f[slot], &a);
        }
    }

    fn cell_attrs(&self) -> CellAttrs<'_> {
        let text = opt;
        CellAttrs {
            r: text(self.cell[0].as_str()),
            t: text(self.cell[1].as_str()),
            s: text(self.cell[2].as_str()),
            vm: text(self.cell[3].as_str()),
            cm: self.cell_cm,
            v: self.cell_has_v.then_some(self.v.as_str()),
            f: self.cell_has_f.then(|| FormulaAttrs {
                kind: text(self.f[2].as_str()),
                si: text(self.f[0].as_str()),
                range: text(self.f[1].as_str()),
                text: self.f_text.as_str(),
            }),
            inline: self.inline.as_ref(),
        }
    }
}

/// The local name of an element (strict packages prefix theirs: `<x:row>`).
fn local(q: &[u8]) -> &[u8] {
    match q.iter().rposition(|&b| b == b':') {
        Some(i) => q.get(i + 1..).unwrap_or(q),
        None => q,
    }
}

/// An attribute value, or `None` when the element did not carry it.
fn opt(s: &str) -> Option<&str> {
    Some(s).filter(|v| !v.is_empty())
}

fn attr_into(out: &mut String, a: &quick_xml::events::attributes::Attribute<'_>) {
    out.clear();
    match a.normalized_value(XmlVersion::Explicit1_0) {
        Ok(v) => out.push_str(&v),
        Err(_) => out.push_str(&String::from_utf8_lossy(&a.value)),
    }
}

fn flag(e: &BytesStart<'_>, key: &[u8]) -> bool {
    e.attributes().with_checks(false).flatten().any(|a| a.key.as_ref() == key && matches!(a.value.as_ref(), b"1" | b"true" | b"on" | b"TRUE"))
}

fn row_attrs<'a>(b: &'a mut Buffers, e: &BytesStart<'_>) -> RowAttrs<'a> {
    for s in &mut b.row {
        s.clear();
    }
    for a in e.attributes().with_checks(false).flatten() {
        let slot = match a.key.as_ref() {
            b"r" => 0,
            b"ht" => 1,
            b"outlineLevel" => 2,
            b"s" => 3,
            _ => continue,
        };
        attr_into(&mut b.row[slot], &a);
    }
    let text = opt;
    RowAttrs {
        r: text(b.row[0].as_str()),
        ht: text(b.row[1].as_str()),
        hidden: flag(e, b"hidden"),
        outline: text(b.row[2].as_str()),
        collapsed: flag(e, b"collapsed"),
        custom_height: flag(e, b"customHeight"),
        custom_format: flag(e, b"customFormat"),
        s: text(b.row[3].as_str()),
    }
}

/// Appends the text of an element, resolving character and entity references.
fn append_text(out: &mut String, t: &quick_xml::events::BytesText<'_>) -> Result<(), IoError> {
    match t.xml_content(XmlVersion::Explicit1_0) {
        Ok(s) => {
            out.push_str(&s);
            Ok(())
        }
        Err(e) => Err(IoError::Xml(e.to_string())),
    }
}

fn append_ref(out: &mut String, r: &quick_xml::events::BytesRef<'_>) {
    match r.resolve_char_ref() {
        Ok(Some(c)) => out.push(c),
        _ => {
            let name = String::from_utf8_lossy(r).into_owned();
            match xml::named_entity(&name) {
                Some(c) => out.push(c),
                None => {
                    out.push('&');
                    out.push_str(&name);
                    out.push(';');
                }
            }
        }
    }
}

/// Reads a worksheet's rows: `cb` sees every `<row>` and every cell of the rows region, in order,
/// plus the `<is>` inline strings it needs. Nothing outside the rows is kept — the caller parses
/// the rest of the worksheet (views, columns, merges…) on its own, which is what lets a sheet be
/// read in windows.
pub fn parse(xml: &[u8], cb: &mut dyn FnMut(Item<'_>) -> Result<(), IoError>) -> Result<(), IoError> {
    let data = xml::to_utf8(xml);
    let mut reader = Reader::from_reader(data.as_ref());
    {
        let cfg = reader.config_mut();
        cfg.check_end_names = false;
        cfg.allow_unmatched_ends = true;
        cfg.allow_dangling_amp = true;
        cfg.trim_text_start = false;
        cfg.trim_text_end = false;
    }

    // Only `<is>` subtrees build elements, so this stack stays small.
    let mut stack: Vec<El> = Vec::new();
    let mut buf = Vec::new();
    let mut b = Buffers::default();
    let mut in_row = false;
    let mut in_v = false;
    let mut in_f = false;
    // `<is>` nesting depth inside a cell (0 = not in one).
    let mut in_is = 0usize;

    // A damaged sheet ends the loop and keeps the rows read so far.
    while let Ok(ev) = reader.read_event_into(&mut buf) {
        match ev {
            Event::Start(e) => {
                let qname = e.name();
                let name = local(qname.as_ref());
                if in_is > 0 {
                    if stack.len() >= xml::MAX_DEPTH {
                        return Err(IoError::Xml("XML nested too deeply".into()));
                    }
                    in_is += 1;
                    stack.push(xml::element(&e));
                    continue;
                }
                match name {
                    b"row" => {
                        in_row = true;
                        let attrs = row_attrs(&mut b, &e);
                        cb(Item::RowStart(attrs))?;
                    }
                    b"c" => b.start_cell(&e),
                    b"v" => {
                        in_v = true;
                        b.cell_has_v = true;
                    }
                    b"f" => {
                        in_f = true;
                        b.start_formula(&e);
                    }
                    b"is" => {
                        in_is = 1;
                        stack.push(xml::element(&e));
                    }
                    _ => {}
                }
            }
            Event::Empty(e) => {
                let qname = e.name();
                let name = local(qname.as_ref());
                if in_is > 0 {
                    let el = xml::element(&e);
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(el);
                    }
                    continue;
                }
                match name {
                    b"row" => {
                        let attrs = row_attrs(&mut b, &e);
                        cb(Item::RowStart(attrs))?;
                        cb(Item::RowEnd)?;
                    }
                    b"c" => {
                        b.start_cell(&e);
                        cb(Item::Cell(b.cell_attrs()))?;
                        b.inline = None;
                    }
                    b"f" => b.start_formula(&e),
                    b"v" => b.cell_has_v = true,
                    b"is" => b.inline = Some(El::default()),
                    _ => {}
                }
            }
            Event::End(e) => {
                let qname = e.name();
                let name = local(qname.as_ref());
                if in_is > 0 {
                    in_is -= 1;
                    if let Some(el) = stack.pop() {
                        if in_is == 0 {
                            b.inline = Some(el);
                        } else if let Some(parent) = stack.last_mut() {
                            parent.children.push(el);
                        }
                    }
                    continue;
                }
                match name {
                    b"v" => in_v = false,
                    b"f" => in_f = false,
                    b"c" => {
                        cb(Item::Cell(b.cell_attrs()))?;
                        b.inline = None;
                    }
                    b"row" => {
                        in_row = false;
                        cb(Item::RowEnd)?;
                    }
                    _ => {}
                }
            }
            Event::Text(t) => {
                if in_v {
                    append_text(&mut b.v, &t)?;
                } else if in_f {
                    append_text(&mut b.f_text, &t)?;
                } else if let Some(top) = stack.last_mut() {
                    append_text(&mut top.text, &t)?;
                }
            }
            Event::CData(t) => {
                if in_v {
                    b.v.push_str(&String::from_utf8_lossy(&t));
                } else if in_f {
                    b.f_text.push_str(&String::from_utf8_lossy(&t));
                } else if let Some(top) = stack.last_mut() {
                    top.text.push_str(&String::from_utf8_lossy(&t));
                }
            }
            Event::GeneralRef(r) => {
                if in_v {
                    append_ref(&mut b.v, &r);
                } else if in_f {
                    append_ref(&mut b.f_text, &r);
                } else if let Some(top) = stack.last_mut() {
                    append_ref(&mut top.text, &r);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    if in_row {
        cb(Item::RowEnd)?;
    }
    Ok(())
}
