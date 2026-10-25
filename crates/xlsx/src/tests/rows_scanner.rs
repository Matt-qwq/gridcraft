//! The row scanner against the general XML walker: the same items, in the same order, for every
//! shape a worksheet's rows take.
//!
//! The scanner exists for speed — it looks for `<` with `memchr` instead of building an event per
//! tag — so the only thing that may differ is how fast it is. Each case below is read both ways and
//! the two lists of items are compared whole, which covers attribute order, quoting, entities,
//! inline strings, comments, CDATA and the tags the scanner does not know.

use crate::rows::{Item, parse, parse_general};

/// One line per item: the scanner and the walker are compared as text, which is both readable in
/// a failure and impossible to get wrong with lifetimes.
fn collected(body: &str, f: fn(&[u8], &mut dyn FnMut(Item<'_>) -> Result<(), crate::IoError>) -> Result<(), crate::IoError>) -> Vec<String> {
    let mut out = Vec::new();
    f(body.as_bytes(), &mut |item| {
        out.push(format!("{item:?}"));
        Ok(())
    })
    .expect("parses");
    out
}

fn both(body: &str) -> (Vec<String>, Vec<String>) {
    (collected(body, parse), collected(body, parse_general))
}

fn assert_same(name: &str, body: &str) {
    let (scanned, walked) = both(body);
    assert_eq!(scanned, walked, "{name}");
}

#[test]
fn numbers_and_empty_cells() {
    assert_same("plain rows", r#"<row r="1" spans="1:3"><c r="A1" s="0"><v>1</v></c><c r="B1"><v>2.5</v></c><c r="C1"/></row>"#);
    assert_same("empty value", r#"<row r="2"><c r="A2"><v/></c><c r="B2"><v></v></c></row>"#);
    assert_same("empty row", r#"<row r="3"/><row r="4"></row>"#);
    assert_same("no row or cell addresses", r#"<row><c><v>7</v></c><c><v>8</v></c></row>"#);
    assert_same("a trailing sheetData close", r#"<row r="5"><c r="A5"><v>1</v></c></row></sheetData>"#);
}

#[test]
fn attribute_shapes() {
    assert_same(
        "odd order, single quotes, spacing",
        "<row   s='3'  ht=\"22.5\"  r = '7' ><c   vm = '1' t='n'  r='A7'  s = \"2\" ><v> 42 </v></c></row>",
    );
    assert_same("row flags", r#"<row r="1" hidden="1" collapsed="true" customHeight="TRUE" customFormat="on" outlineLevel="2" s="4" ht="15"/>"#);
    assert_same("flags that are not set", r#"<row r="1" hidden="0" collapsed="false" customHeight="0"/>"#);
    assert_same("an unknown attribute", r#"<row r="1" thickBot="1" x14ac:dyDescent="0.3"><c r="A1" ph="1"><v>1</v></c></row>"#);
    assert_same("a self-closing cell with attributes", r#"<row r="2"><c r="B2" s="1" t="str" vm="0" cm="1"/></row>"#);
    assert_same("entity in an attribute", r#"<row r="1"><c r="A1" t="inlineStr" x="a&amp;b"><v>1</v></c></row>"#);
}

#[test]
fn formulas() {
    assert_same("a normal formula", r#"<row r="1"><c r="A1"><f>1+2</f><v>3</v></c></row>"#);
    assert_same("shared", r#"<row r="1"><c r="A1"><f t="shared" si="0" ref="A1:A9">A1*2</f><v>2</v></c></row>"#);
    assert_same("a shared follower", r#"<row r="2"><c r="A2"><f t="shared" si="0"/><v>4</v></c></row>"#);
    assert_same("array", r#"<row r="1"><c r="B1"><f t="array" ref="B1:B2">{1;2}</f><v>1</v></c></row>"#);
    assert_same("a formula with entities", r#"<row r="1"><c r="A1"><f>IF(A2&lt;3,&amp;A2,"&quot;x&quot;")</f><v>1</v></c></row>"#);
    assert_same("an empty formula", r#"<row r="1"><c r="A1"><f/><v>0</v></c></row>"#);
    assert_same("a formula before its value", r#"<row r="1"><c r="C1"><f>SUM(A1:B1)</f><v>9</v></c></row>"#);
}

#[test]
fn inline_strings() {
    assert_same("plain", r#"<row r="1"><c r="A1" t="inlineStr"><is><t>hello</t></is></c></row>"#);
    assert_same("preserve", r#"<row r="1"><c r="A1" t="inlineStr"><is><t xml:space="preserve">  spaced  </t></is></c></row>"#);
    assert_same("runs", r#"<row r="1"><c r="A1" t="inlineStr"><is><r><rPr><b/></rPr><t>bold</t></r><r><t>plain</t></r></is></c></row>"#);
    assert_same("entities in the text", r#"<row r="1"><c r="A1" t="inlineStr"><is><t>a &amp; b &lt;c&gt; &#65;</t></is></c></row>"#);
    assert_same("an empty inline string", r#"<row r="1"><c r="A1" t="inlineStr"><is/></c></row>"#);
    assert_same(
        "two inline cells in a row",
        r#"<row r="1"><c r="A1" t="inlineStr"><is><t>a</t></is></c><c r="B1" t="inlineStr"><is><t>b</t></is></c></row>"#,
    );
}

#[test]
fn the_things_between_rows() {
    assert_same("a comment", r#"<row r="1"><c r="A1"><v>1</v></c></row><!-- a note with <tags> inside --><row r="2"><c r="A2"><v>2</v></c></row>"#);
    assert_same("a processing instruction", r#"<?xml-stylesheet href="x"?><row r="1"><c r="A1"><v>1</v></c></row>"#);
    assert_same("a declaration", r#"<!DOCTYPE x [<!ENTITY a "b">]><row r="1"><c r="A1"><v>1</v></c></row>"#);
    assert_same("CDATA in a value", r#"<row r="1"><c r="A1"><v><![CDATA[12]]></v></c></row>"#);
    assert_same("an element we do not know", r#"<row r="1"><x:foo bar="1">text<y/></x:foo><c r="A1"><v>1</v></c></row>"#);
    assert_same("prefixed row and cell", r#"<x:row r="1"><x:c r="A1"><x:v>5</x:v></x:c></x:row>"#);
    assert_same("a row that never closes", r#"<row r="1"><c r="A1"><v>1</v></c>"#);
    assert_same("text outside any value", r#"<row r="1">ignored<c r="A1">also ignored<v>3</v></c></row>"#);
}

#[test]
fn a_whole_sheet_of_rows() {
    // The mixed fixture the other streaming tests use: heights, shared strings and formulas over
    // forty rows, which is where a boundary bug in a scanner would show.
    let body = super::streaming::streaming_body(40);
    assert_same("forty mixed rows", &body);
}
