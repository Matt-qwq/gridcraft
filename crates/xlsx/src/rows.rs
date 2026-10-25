//! Reads worksheet rows straight from the XML events.
//!
//! The generic [`crate::xml::parse_streaming`] builds an `El` — a name string, an attribute vector
//! and a text string — for every element. A worksheet has one element per cell (plus one per
//! `<v>`), so a million-row sheet means tens of millions of them, and for a big file that cost
//! more than decoding the XML did. Rows are regular enough to decode directly: this walker reads
//! `<row>`/`<c>`/`<v>`/`<f>` into reused buffers and hands them to a callback, and keeps the `El`
//! tree only for everything outside `<sheetData>` (views, columns, merges, tables…) and for the
//! rare `<is>` inline string.

#[cfg(test)]
use quick_xml::Reader;
#[cfg(test)]
use quick_xml::XmlVersion;
#[cfg(test)]
use quick_xml::events::{BytesStart, Event};

use crate::IoError;
use crate::xml::{self, El};

/// `<row>` attributes the reader uses.
#[derive(Debug, Default, PartialEq)]
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
#[derive(Debug, Default, PartialEq)]
pub struct FormulaAttrs<'a> {
    /// `t`: `normal`, `shared`, `array`, `dataTable`.
    pub kind: Option<&'a str>,
    pub si: Option<&'a str>,
    pub range: Option<&'a str>,
    pub text: &'a str,
}

/// One `<c>` element.
#[derive(Debug, Default, PartialEq)]
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
#[derive(Debug, PartialEq)]
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
    hidden: bool,
    collapsed: bool,
    custom_height: bool,
    custom_format: bool,
}

impl Buffers {
    #[cfg(test)]
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

    #[cfg(test)]
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

    fn row_attrs(&self) -> RowAttrs<'_> {
        let text = opt;
        RowAttrs {
            r: text(self.row[0].as_str()),
            ht: text(self.row[1].as_str()),
            hidden: self.hidden,
            outline: text(self.row[2].as_str()),
            collapsed: self.collapsed,
            custom_height: self.custom_height,
            custom_format: self.custom_format,
            s: text(self.row[3].as_str()),
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

#[cfg(test)]
fn attr_into(out: &mut String, a: &quick_xml::events::attributes::Attribute<'_>) {
    out.clear();
    match a.normalized_value(XmlVersion::Explicit1_0) {
        Ok(v) => out.push_str(&v),
        Err(_) => out.push_str(&String::from_utf8_lossy(&a.value)),
    }
}

#[cfg(test)]
fn flag(e: &BytesStart<'_>, key: &[u8]) -> bool {
    e.attributes().with_checks(false).flatten().any(|a| a.key.as_ref() == key && matches!(a.value.as_ref(), b"1" | b"true" | b"on" | b"TRUE"))
}

#[cfg(test)]
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
#[cfg(test)]
fn append_text(out: &mut String, t: &quick_xml::events::BytesText<'_>) -> Result<(), IoError> {
    match t.xml_content(XmlVersion::Explicit1_0) {
        Ok(s) => {
            out.push_str(&s);
            Ok(())
        }
        Err(e) => Err(IoError::Xml(e.to_string())),
    }
}

#[cfg(test)]
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
    scan(xml::to_utf8(xml).as_ref(), cb)
}

/// The general walker, kept as the reference the scanner is tested against.
#[cfg(test)]
pub fn parse_general(xml: &[u8], cb: &mut dyn FnMut(Item<'_>) -> Result<(), IoError>) -> Result<(), IoError> {
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

// ---------------------------------------------------------------- the scanner
//
// A worksheet's rows are regular enough to read without an event per tag: this walker looks for
// `<`, reads the name and attributes with `memchr`, and copies the text of `<v>`, `<f>` and the
// span of `<is>` — everything else in the region (a comment, a processing instruction, an element
// it does not know) is skipped. It answers exactly what the general walker above answers, item for
// item; the tests hold the two against each other on the shapes a sheet takes.

/// Reads the rows region with the scanner. Falls back to the general walker only for a region the
/// scanner cannot finish (an unterminated tag), where the two agree on failure anyway.
fn scan(xml: &[u8], cb: &mut dyn FnMut(Item<'_>) -> Result<(), IoError>) -> Result<(), IoError> {
    let mut b = Buffers::default();
    let mut at = 0usize;
    let mut in_row = false;
    // What the text between tags belongs to.
    let mut target = Target::None;
    while at < xml.len() {
        let Some(rel) = memchr::memchr(b'<', &xml[at..]) else { break };
        let lt = at + rel;
        if lt > at
            && let Some(raw) = xml.get(at..lt)
        {
            match target {
                Target::None => {}
                Target::Value => unescape_into(&mut b.v, raw),
                Target::Formula => unescape_into(&mut b.f_text, raw),
            }
        }
        at = lt;
        let Some(&next) = xml.get(at + 1) else { break };
        match next {
            // `</name>`
            b'/' => {
                let start = at + 2;
                let end = xml[start..].iter().position(|c| *c == b'>').map(|p| start + p).unwrap_or(xml.len());
                match local(&xml[start..end]) {
                    b"v" | b"f" => target = Target::None,
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
                at = end.saturating_add(1);
            }
            // `<!--…-->`, `<![CDATA[…]]>`, `<?…?>`, `<!DOCTYPE …>`.
            b'!' | b'?' => {
                let rest = xml.get(at..).unwrap_or(&[]);
                let (body_len, close): (usize, &[u8]) = if rest.starts_with(b"<!--") {
                    (4, b"-->")
                } else if rest.starts_with(b"<![CDATA[") {
                    (9, b"]]>")
                } else if rest.starts_with(b"<?") {
                    (2, b"?>")
                } else {
                    (2, b">")
                };
                let from = at + body_len;
                let Some(rel) = find(xml.get(from..).unwrap_or(&[]), close) else { break };
                let body_end = from + rel;
                // CDATA is text, unchanged; a comment or a declaration carries nothing we read.
                if close == b"]]>"
                    && let Some(raw) = xml.get(from..body_end)
                {
                    let text = String::from_utf8_lossy(raw).into_owned();
                    match target {
                        Target::None => {}
                        Target::Value => b.v.push_str(&text),
                        Target::Formula => b.f_text.push_str(&text),
                    }
                }
                at = body_end + close.len();
            }
            _ => {
                // `<name …>`, `<name …/>` or `<name>`.
                let start = at + 1;
                let mut i = start;
                while i < xml.len() && !matches!(xml[i], b'>' | b'/' | b' ' | b'\t' | b'\r' | b'\n') {
                    i += 1;
                }
                let (attrs_end, self_closing) = tag_end(xml, i);
                let raw = xml.get(i..attrs_end).unwrap_or(&[]);
                match local(&xml[start..i]) {
                    b"row" => {
                        read_row_attrs(&mut b, raw);
                        in_row = true;
                        cb(Item::RowStart(b.row_attrs()))?;
                        if self_closing {
                            in_row = false;
                            cb(Item::RowEnd)?;
                        }
                    }
                    b"c" => {
                        start_cell(&mut b, raw);
                        if self_closing {
                            cb(Item::Cell(b.cell_attrs()))?;
                            b.inline = None;
                        }
                    }
                    b"v" => {
                        // The cell has a value element even when it is empty or self-closing,
                        // which the reader tells apart from a cell with none.
                        b.cell_has_v = true;
                        if !self_closing {
                            target = Target::Value;
                        }
                    }
                    b"f" => {
                        start_formula(&mut b, raw);
                        if !self_closing {
                            target = Target::Formula;
                        }
                    }
                    b"is" => {
                        // `<is/>` is an empty string, which the walker also reports as an empty
                        // tree rather than as no tree at all.
                        if self_closing {
                            b.inline = Some(El::default());
                            at = attrs_end.saturating_add(1);
                            continue;
                        }
                        // The inline string's tree comes from the general parser, given its span: a
                        // string with runs or phonetics is rare and its tree is not something to
                        // re-derive here.
                        let from = start.saturating_sub(1);
                        let Some(rel) = find(xml.get(attrs_end..).unwrap_or(&[]), b"</is>") else {
                            return Err(IoError::Xml("an inline string is never closed".into()));
                        };
                        let end = attrs_end + rel + b"</is>".len();
                        b.inline = xml::element_fragment(xml.get(from..end).unwrap_or(&[]));
                        at = end;
                        continue;
                    }
                    _ => {}
                }
                at = attrs_end.saturating_add(1);
            }
        }
    }
    if in_row {
        cb(Item::RowEnd)?;
    }
    Ok(())
}

/// Where the text after a tag goes.
#[derive(Clone, Copy, PartialEq)]
enum Target {
    None,
    Value,
    Formula,
}

/// The first occurrence of `needle`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let last = hay.len() - needle.len();
    (0..=last).find(|&i| hay.get(i..i + needle.len()) == Some(needle))
}

/// Where a tag's attributes end: the index of the `>` (or `/`) that closes it, and whether the tag
/// closes itself. A quoted value may hold either byte.
fn tag_end(xml: &[u8], from: usize) -> (usize, bool) {
    let mut i = from;
    let mut quote = 0u8;
    while let Some(&c) = xml.get(i) {
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if c == b'>' {
            return (i, false);
        } else if c == b'/' && xml.get(i + 1) == Some(&b'>') {
            return (i, true);
        }
        i += 1;
    }
    (xml.len(), false)
}

/// The attributes of a tag, as `(name, raw value)`.
fn attrs(raw: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        while at < raw.len() && raw[at].is_ascii_whitespace() {
            at += 1;
        }
        if at >= raw.len() {
            return None;
        }
        let name_start = at;
        while at < raw.len() && !raw[at].is_ascii_whitespace() && raw[at] != b'=' {
            at += 1;
        }
        let name = raw.get(name_start..at).unwrap_or(&[]);
        while at < raw.len() && raw[at].is_ascii_whitespace() {
            at += 1;
        }
        if raw.get(at) != Some(&b'=') {
            return Some((name, &[][..]));
        }
        at += 1;
        while at < raw.len() && raw[at].is_ascii_whitespace() {
            at += 1;
        }
        let quote = match raw.get(at) {
            Some(&q) if q == b'"' || q == b'\'' => {
                at += 1;
                q
            }
            _ => 0,
        };
        let value_start = at;
        if quote == 0 {
            while at < raw.len() && !raw[at].is_ascii_whitespace() {
                at += 1;
            }
            return Some((name, raw.get(value_start..at).unwrap_or(&[])));
        }
        while at < raw.len() && raw[at] != quote {
            at += 1;
        }
        let value = raw.get(value_start..at).unwrap_or(&[]);
        at = at.saturating_add(1);
        Some((name, value))
    })
}

fn read_row_attrs(b: &mut Buffers, raw: &[u8]) {
    for s in &mut b.row {
        s.clear();
    }
    b.hidden = false;
    b.collapsed = false;
    b.custom_height = false;
    b.custom_format = false;
    for (name, value) in attrs(raw) {
        match name {
            b"r" => set(&mut b.row[0], value),
            b"ht" => set(&mut b.row[1], value),
            b"outlineLevel" => set(&mut b.row[2], value),
            b"s" => set(&mut b.row[3], value),
            b"hidden" => b.hidden = flag_value(value),
            b"collapsed" => b.collapsed = flag_value(value),
            b"customHeight" => b.custom_height = flag_value(value),
            b"customFormat" => b.custom_format = flag_value(value),
            _ => {}
        }
    }
}

fn start_cell(b: &mut Buffers, raw: &[u8]) {
    for s in &mut b.cell {
        s.clear();
    }
    b.cell_cm = false;
    b.cell_has_f = false;
    b.cell_has_v = false;
    b.inline = None;
    b.v.clear();
    b.f_text.clear();
    for (name, value) in attrs(raw) {
        match name {
            b"r" => set(&mut b.cell[0], value),
            b"t" => set(&mut b.cell[1], value),
            b"s" => set(&mut b.cell[2], value),
            b"vm" => set(&mut b.cell[3], value),
            b"cm" => b.cell_cm = true,
            _ => {}
        }
    }
}

fn start_formula(b: &mut Buffers, raw: &[u8]) {
    for s in &mut b.f {
        s.clear();
    }
    b.cell_has_f = true;
    b.f_text.clear();
    for (name, value) in attrs(raw) {
        match name {
            b"si" => set(&mut b.f[0], value),
            b"ref" => set(&mut b.f[1], value),
            b"t" => set(&mut b.f[2], value),
            _ => {}
        }
    }
}

/// `1`, `true`, `on` (and the capitalised spelling the old walker also took).
fn flag_value(value: &[u8]) -> bool {
    matches!(value, b"1" | b"true" | b"on" | b"TRUE")
}

/// An attribute value into its buffer, with entity and character references resolved.
fn set(out: &mut String, raw: &[u8]) {
    out.clear();
    unescape_into(out, raw);
}

/// Appends text with `&amp;`, `&#65;` and `&#x42;` resolved; a reference the XML spec does not name
/// stays as it was written, which is what the general walker's `append_ref` does.
fn unescape_into(out: &mut String, raw: &[u8]) {
    let mut at = 0usize;
    while at < raw.len() {
        let Some(rel) = memchr::memchr(b'&', &raw[at..]) else {
            push_bytes(out, raw.get(at..).unwrap_or(&[]));
            return;
        };
        let amp = at + rel;
        push_bytes(out, raw.get(at..amp).unwrap_or(&[]));
        let Some(semi) = memchr::memchr(b';', raw.get(amp..).unwrap_or(&[])).map(|p| amp + p) else {
            push_bytes(out, raw.get(amp..).unwrap_or(&[]));
            return;
        };
        let entity = raw.get(amp + 1..semi).unwrap_or(&[]);
        match entity.first() {
            Some(b'#') => {
                let digits = entity.get(1..).unwrap_or(&[]);
                let code = if let Some(hex) = digits.strip_prefix(b"x").or_else(|| digits.strip_prefix(b"X")) {
                    u32::from_str_radix(std::str::from_utf8(hex).unwrap_or(""), 16).ok()
                } else {
                    std::str::from_utf8(digits).ok().and_then(|d| d.parse::<u32>().ok())
                };
                match code.and_then(char::from_u32) {
                    Some(c) => out.push(c),
                    None => push_bytes(out, raw.get(amp..=semi).unwrap_or(&[])),
                }
            }
            Some(_) => match std::str::from_utf8(entity).ok().and_then(xml::named_entity) {
                Some(c) => out.push(c),
                None => push_bytes(out, raw.get(amp..=semi).unwrap_or(&[])),
            },
            None => push_bytes(out, raw.get(amp..=semi).unwrap_or(&[])),
        }
        at = semi + 1;
    }
}

/// Appends bytes as text, replacing anything that is not UTF-8 (a malformed file) rather than
/// dropping the cell.
fn push_bytes(out: &mut String, bytes: &[u8]) {
    match std::str::from_utf8(bytes) {
        Ok(s) => out.push_str(s),
        Err(_) => out.push_str(&String::from_utf8_lossy(bytes)),
    }
}
