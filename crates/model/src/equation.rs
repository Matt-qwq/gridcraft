//! Excel's equation objects (Insert ▸ Equation).
//!
//! In a file an equation is a floating drawing shape whose text body carries an Office Math
//! Markup paragraph as the `a14` extension of DrawingML:
//! `<xdr:sp><xdr:txBody><a:p><a14:m><m:oMathPara><m:oMath>…` (ECMA-376 Part 1 §22.1 for the
//! DrawingML shape, Part 1 §22.9 for the OMML inside it). A shape whose text body holds no math is
//! a plain shape, not an equation, so the distinction is the markup, not the shape kind.
//!
//! **GridCraft does not lay mathematics out in two dimensions.** What it paints inside the
//! object's box is the equation's `source`, its linear text; what it keeps and writes is the real
//! math: `omml` holds the `a14:m` element's content exactly as the file had it, so fractions,
//! radicals, sums, delimiters and matrices in an existing file survive a save byte for byte. So a
//! file's equation is never invisible and never rewritten just because it was opened — and what
//! the reader sees is the linear form, not the 2-D layout.
//!
//! Editing the text of an equation replaces that markup with plain runs (`m:oMathPara` /
//! `m:oMath` / `m:r` / `m:t`), which is exactly what an equation made of plain text is in
//! Excel. The alternative was refusing the edit, which would have made a hand-typed equation
//! impossible to correct.

use serde::{Deserialize, Serialize};

use crate::features::Anchor;
use crate::style::Color;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Equation {
    pub id: u32,
    /// Where the equation sits on the sheet, and how big its box is.
    pub anchor: Anchor,
    /// Box fill; `Color::Auto` for none, which is Excel's default for an equation.
    pub fill: Color,
    /// Box outline; `Color::Auto` for none, which is Excel's default for an equation.
    pub line: Color,
    /// The equation as linear text: what GridCraft paints in the box, and what an edit types.
    /// Reading flattens the markup's structure into its runs in document order — the text of a
    /// fraction is its numerator's and denominator's runs side by side, with no bar — because the
    /// layout belongs to the file, not to this model.
    pub source: String,
    /// The file's own math: the content of its `<a14:m>` element, kept verbatim so a save writes
    /// it back unchanged. `None` when the markup is generated from `source` instead (a new
    /// equation, or one whose text was edited).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omml: Option<String>,
    /// Alternative text a screen reader reads for the equation (`xdr:cNvPr/@descr`), like any
    /// other drawing object's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
}
