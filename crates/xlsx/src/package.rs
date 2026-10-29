//! OPC package access: reading zip parts with decompression limits, relationships and part-name
//! resolution; writing a zip package deterministically.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{BufReader, Cursor, Read, Seek, Write};

use crate::IoError;
use crate::xml::{self, El};

/// Largest decompressed part accepted.
pub const MAX_PART: u64 = 512 * 1024 * 1024;
/// Largest total decompressed size read from one package.
pub const MAX_TOTAL: u64 = 1024 * 1024 * 1024;

/// The limits the reading in this process actually uses.
///
/// They are the defaults above unless `GRIDCRAFT_MAX_PART_MB` / `GRIDCRAFT_MAX_TOTAL_MB` say
/// otherwise: real reports and data exports do exceed 512 MB in one sheet, and someone opening
/// their own file is not the threat these caps are about. The defaults stay hostile-input safe;
/// an override is clamped so a typo cannot ask for terabytes.
pub fn max_part() -> u64 {
    limits().0
}

fn limits() -> (u64, u64) {
    const CEILING: u64 = 16 * 1024; // 16 GB, far above any real workbook
    static LIMITS: std::sync::OnceLock<(u64, u64)> = std::sync::OnceLock::new();
    *LIMITS.get_or_init(|| {
        let mb = |name: &str, default: u64| -> u64 {
            std::env::var(name)
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|v| *v > 0)
                .map(|v| v.min(CEILING))
                .map(|v| v * 1024 * 1024)
                .unwrap_or(default)
        };
        let part = mb("GRIDCRAFT_MAX_PART_MB", MAX_PART);
        let total = mb("GRIDCRAFT_MAX_TOTAL_MB", MAX_TOTAL).max(part);
        (part, total)
    })
}
/// Most entries accepted in the zip directory.
pub const MAX_ENTRIES: usize = 200_000;

/// Where a package's bytes come from: a slice already in memory, or the file itself, read a part at
/// a time. Boxed so the two share every code path — a zip archive is generic over its reader, and
/// threading that type parameter through the whole reader would cost a hundred signatures.
pub trait Source: Read + Seek + Send {}
impl<T: Read + Seek + Send> Source for T {}

pub struct Package<'a> {
    zip: zip::ZipArchive<Box<dyn Source + 'a>>,
    /// Lower-case normalized part name → zip index.
    names: HashMap<String, usize>,
    total: u64,
}

/// A part being decompressed as it is read. Stops at the package's remaining budget, so a part
/// that lies about its size still cannot expand past the limit `read` applies.
pub struct PartReader<'z, R: Read> {
    inner: zip::read::ZipFile<'z, R>,
    left: u64,
    eof: bool,
}

impl<R: Read> PartReader<'_, R> {
    /// True when the part stopped at the limit rather than at its own end.
    pub fn hit_limit(&self) -> bool {
        self.left == 0 && !self.eof
    }
}

impl<R: Read> Read for PartReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 || buf.is_empty() {
            return Ok(0);
        }
        let take = buf.len().min(self.left as usize);
        let n = self.inner.read(&mut buf[..take])?;
        if n == 0 {
            self.eof = true;
        }
        self.left -= n as u64;
        Ok(n)
    }
}

/// A relationship.
#[derive(Clone, Debug)]
pub struct Rel {
    pub id: String,
    /// Last path segment of the relationship type (`worksheet`, `styles`, …): the same for the
    /// transitional and strict namespaces.
    pub kind: String,
    /// Resolved part name (no leading `/`), or the raw target for external relationships.
    pub target: String,
    pub external: bool,
}

pub fn normalize(name: &str) -> String {
    name.replace('\\', "/").trim_start_matches('/').to_ascii_lowercase()
}

impl<'a> Package<'a> {
    pub fn open(bytes: &'a [u8]) -> Result<Package<'a>, IoError> {
        Self::from_source(Box::new(Cursor::new(bytes)))
    }

    /// Reads the directory of a zip file and decompresses parts as they are asked for, so a
    /// workbook whose parts are mostly unread — a 1.3 GB report of which 49 MB is looked at —
    /// costs the parts it reads and not the size of the file.
    pub fn open_file(file: std::fs::File) -> Result<Package<'a>, IoError> {
        let source: Box<dyn Source + 'static> = Box::new(BufReader::with_capacity(64 * 1024, file));
        Self::from_source(source)
    }

    fn from_source(source: Box<dyn Source + 'a>) -> Result<Package<'a>, IoError> {
        let zip = zip::ZipArchive::new(source).map_err(|e| IoError::Zip(e.to_string()))?;
        if zip.len() > MAX_ENTRIES {
            return Err(IoError::TooLarge(format!("{} zip entries", zip.len())));
        }
        let mut names = HashMap::new();
        for i in 0..zip.len() {
            if let Some(n) = zip.name_for_index(i) {
                names.entry(normalize(n)).or_insert(i);
            }
        }
        Ok(Package { zip, names, total: 0 })
    }

    pub fn has(&self, part: &str) -> bool {
        self.names.contains_key(&normalize(part))
    }

    /// Reads a part's bytes; `Ok(None)` when it doesn't exist.
    pub fn read(&mut self, part: &str) -> Result<Option<Vec<u8>>, IoError> {
        self.read_limited(part, limits().0)
    }

    /// Everything `read` would check about a part, without reading it: its decompressed size as the
    /// zip directory declares it. `None` when the part is missing.
    pub fn declared_size(&mut self, part: &str) -> Option<u64> {
        let idx = *self.names.get(&normalize(part))?;
        self.zip.by_index(idx).ok().map(|f| f.size())
    }

    /// Opens a part for streaming: the decompressor itself, with the same limits `read` enforces.
    /// The caller reads the part in windows and never holds more than a window of it, which is how
    /// a sheet that unpacks to a gigabyte is read without a gigabyte of buffer.
    pub fn open_part(&mut self, part: &str) -> Result<Option<PartReader<'_, Box<dyn Source + 'a>>>, IoError> {
        let (max_part, max_total) = limits();
        let Some(&idx) = self.names.get(&normalize(part)) else {
            return Ok(None);
        };
        let used = self.total;
        let f = self.zip.by_index(idx).map_err(|e| IoError::Zip(format!("{part}: {e}")))?;
        let size = f.size();
        if size > max_part {
            return Err(IoError::TooLarge(format!("{part} expands to {size} bytes")));
        }
        let remaining = max_total.saturating_sub(used);
        if size > remaining {
            return Err(IoError::TooLarge(format!("package expands to more than {} MB", max_total / (1024 * 1024))));
        }
        self.total = used.saturating_add(size);
        // The directory's size is only a claim: a part that lies still cannot expand past the limit.
        Ok(Some(PartReader { inner: f, left: remaining, eof: false }))
    }

    /// Reads a small metadata part without allowing it to expand to the general part limit.
    pub fn read_limited(&mut self, part: &str, maximum: u64) -> Result<Option<Vec<u8>>, IoError> {
        let (max_part, max_total) = limits();
        let maximum = maximum.min(max_part);
        let Some(&idx) = self.names.get(&normalize(part)) else {
            return Ok(None);
        };
        let mut f = self.zip.by_index(idx).map_err(|e| IoError::Zip(format!("{part}: {e}")))?;
        if f.size() > maximum {
            return Err(IoError::TooLarge(format!("{part} expands to {} bytes", f.size())));
        }
        let remaining = max_total.saturating_sub(self.total);
        let limit = maximum.min(remaining);
        // Allocate what the directory says the part unpacks to. Growing a 600 MB part from a 16 MB
        // start doubles half a dozen times, and the last step holds the old and the new buffer at
        // once: 1.5 GB of peak for a 600 MB part. `f.size()` is checked against the limit above, so
        // this can never ask for more than the guard allows.
        let mut out = Vec::with_capacity(f.size().min(limit) as usize);
        (&mut f).take(limit + 1).read_to_end(&mut out).map_err(|e| IoError::Zip(format!("{part}: {e}")))?;
        if out.len() as u64 > limit {
            return Err(IoError::TooLarge(if remaining < maximum {
                format!("package expands to more than {} MB", max_total / (1024 * 1024))
            } else {
                format!("{part} expands to more than {maximum} bytes")
            }));
        }
        self.total += out.len() as u64;
        Ok(Some(out))
    }

    pub fn read_xml(&mut self, part: &str) -> Result<Option<El>, IoError> {
        match self.read(part)? {
            Some(b) => xml::parse(&b).map(Some).map_err(|e| prefix(part, e)),
            None => Ok(None),
        }
    }

    /// Relationships of a part (`xl/workbook.xml` → `xl/_rels/workbook.xml.rels`). Missing or
    /// broken rels parts give an empty list.
    pub fn rels(&mut self, part: &str) -> Result<Vec<Rel>, IoError> {
        let (dir, file) = split_dir(part);
        let rels_name = if dir.is_empty() { format!("_rels/{file}.rels") } else { format!("{dir}/_rels/{file}.rels") };
        let root = match self.read(&rels_name)? {
            Some(b) => match xml::parse(&b) {
                Ok(r) => r,
                Err(_) => return Ok(vec![]),
            },
            None => return Ok(vec![]),
        };
        Ok(root
            .kids("Relationship")
            .filter_map(|r| {
                let id = r.attr("Id")?.to_string();
                let ty = r.attr("Type").unwrap_or("");
                let kind = ty.rsplit('/').next().unwrap_or("").to_string();
                let target_raw = r.attr("Target")?;
                let external = r.attr("TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External"));
                let target = if external { target_raw.to_string() } else { resolve(dir, target_raw) };
                Some(Rel { id, kind, target, external })
            })
            .collect())
    }
}

/// The first part whose declared unpacked size is over [`MAX_PART`], with that size.
pub fn oversized_part(bytes: &[u8]) -> Option<(String, u64)> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    (0..zip.len().min(MAX_ENTRIES)).find_map(|i| {
        let f = zip.by_index_raw(i).ok()?;
        (f.size() > limits().0).then(|| (f.name().to_string(), f.size()))
    })
}

fn prefix(part: &str, e: IoError) -> IoError {
    match e {
        IoError::Xml(m) => IoError::Xml(format!("{part}: {m}")),
        o => o,
    }
}

/// `xl/worksheets/sheet1.xml` → (`xl/worksheets`, `sheet1.xml`).
pub fn split_dir(part: &str) -> (&str, &str) {
    let p = part.trim_start_matches('/');
    match p.rfind('/') {
        Some(i) => (p.get(..i).unwrap_or(""), p.get(i + 1..).unwrap_or("")),
        None => ("", p),
    }
}

/// Resolves a relationship target against the source part's directory.
pub fn resolve(dir: &str, target: &str) -> String {
    let target = target.replace('\\', "/");
    let target = target.split('#').next().unwrap_or("").to_string();
    let mut segs: Vec<&str> = if target.starts_with('/') { vec![] } else { dir.split('/').filter(|s| !s.is_empty()).collect() };
    for s in target.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            s => segs.push(s),
        }
    }
    segs.join("/")
}

// ---------------------------------------------------------------- writing

/// The package as a zip. The parts of a workbook compress independently, and deflate is 1.8 s of
/// a 4.2 s save, so they are compressed on several threads and then laid out — the container
/// itself is a handful of headers, written here rather than through `zip` for that reason.
///
/// Any archive that would need zip64 (a part at 4 GB, an offset past it) is handed to `zip`
/// instead; an xlsx that large is rare, and a second writer for it is cheaper than zip64 here.
pub struct ZipOut {
    entries: Vec<(String, Part)>,
}

impl ZipOut {
    pub fn new() -> ZipOut {
        ZipOut { entries: Vec::new() }
    }

    /// The level is 3, not the usual 6: measured on a 599 MB worksheet, level 6 packs it in 4.29 s
    /// to 65 MB, level 3 in 1.25 s to 74 MB, level 1 in 0.53 s to 107 MB. A report that took 9 s to
    /// save takes 4.3 at level 3 and comes out 13% larger — still smaller than the file WPS wrote for
    /// it. Saving is the slowest thing a spreadsheet does; that trade is worth making.
    ///
    /// The part is taken, not copied: the save holds each part's bytes once.
    pub fn add(&mut self, name: &str, data: Vec<u8>) -> Result<(), IoError> {
        self.add_part(name, Part::Raw(data));
        Ok(())
    }

    /// Adds a part that may already be deflated (a worksheet, packed while it was written).
    pub(crate) fn add_part(&mut self, name: &str, part: Part) {
        self.entries.push((name.to_string(), part));
    }

    pub fn finish(self) -> Result<Vec<u8>, IoError> {
        write_zip(self.entries)
    }
}

/// Deflate level for a part, as [`ZipOut::add`] documents.
const DEFLATE_LEVEL: u32 = 3;

/// A part that would need zip64 (its own size, or where it lands in the archive).
const ZIP64_PART: usize = u32::MAX as usize - 1;

/// Deflates one part. `zlib-rs` is the backend `zip`'s `deflate` feature already uses, so the bytes
/// are what the previous writer produced.
fn deflate(data: &[u8]) -> Result<Vec<u8>, IoError> {
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::with_capacity(data.len() / 6 + 64), flate2::Compression::new(DEFLATE_LEVEL));
    encoder.write_all(data).map_err(|e| IoError::Zip(e.to_string()))?;
    encoder.finish().map_err(|e| IoError::Zip(e.to_string()))
}

fn push_u16(out: &mut Vec<u8>, n: u16) {
    out.extend_from_slice(&n.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_le_bytes());
}

/// The most deflate can make a part: zlib's documented bound, which the layout check below uses.
fn deflate_bound(len: usize) -> usize {
    len.saturating_add(len / 4096).saturating_add(len / 16384).saturating_add(len / 33_554_432).saturating_add(13)
}

/// The part's size before deflate. A pre-deflated part records the size it was packed from, so the
/// decision below is the same whether a part is raw or packed.
fn unpacked_size(part: &Part) -> usize {
    match part {
        Part::Raw(data) => data.len(),
        Part::Packed(packed) => packed.size,
    }
}

/// The largest the archive can come out as: both headers of every part, its worst-case deflated
/// size, and the end record.
fn zip64_bound(entries: &[(String, Part)]) -> usize {
    let mut bound: usize = 22;
    for (name, part) in entries {
        // A packed body is already smaller than this bound, but the uncompressed size is used so
        // pre-compressing a part cannot change which archive writer a package takes.
        bound = bound.saturating_add(30 + 46 + 2 * name.len()).saturating_add(deflate_bound(unpacked_size(part)));
    }
    bound
}

/// Whether the archive needs zip64 (a part, or an offset, past 4 GB), which this writer does not
/// do. Deflating takes each part's bytes, so the other writer can no longer be handed them
/// afterwards: the decision is taken here, from a bound that covers the whole layout, and an
/// archive that passes it places no size and no offset past 4 GB. That is why the layout is no
/// longer re-checked while the parts are written.
fn needs_zip64(entries: &[(String, Part)]) -> bool {
    // The estimate the writer has always used is checked first, so the same packages take the same
    // writer as before: it credits deflate with packing every part to a third, and therefore sends
    // a package of a few GB to the other writer well before its offsets could reach 4 GB.
    let estimated: usize = entries
        .iter()
        .map(|(_, part)| {
            let size = unpacked_size(part);
            size / 3 + size
        })
        .sum();
    estimated >= ZIP64_PART || zip64_bound(entries) >= ZIP64_PART || entries.iter().any(|(_, part)| unpacked_size(part) >= ZIP64_PART)
}

/// One part, deflated, with what its headers need.
pub(crate) struct Packed {
    name: String,
    body: Vec<u8>,
    crc: u32,
    size: usize,
}

impl Packed {
    /// The part before deflate. The zip64 writer compresses each part itself and cannot take an
    /// already-deflated body, so the rare mixed archive pays an inflate rather than dropping the
    /// part.
    fn inflate(&self) -> Result<Vec<u8>, IoError> {
        let mut raw = Vec::with_capacity(self.size);
        flate2::read::DeflateDecoder::new(self.body.as_slice()).read_to_end(&mut raw).map_err(|e| IoError::Zip(format!("{}: {e}", self.name)))?;
        if raw.len() != self.size || crc32fast::hash(&raw) != self.crc {
            return Err(IoError::Zip(format!("{}: a packed part does not match its recorded size and CRC", self.name)));
        }
        Ok(raw)
    }
}

/// A part handed to the archive: bytes still to deflate, or a part that was deflated as it was
/// produced.
pub(crate) enum Part {
    Raw(Vec<u8>),
    Packed(Packed),
}

/// Deflates one part and reports what its headers need. The part arrives by value, so it is dropped
/// when this returns instead of being held until the whole archive is assembled.
pub(crate) fn pack(name: String, data: Vec<u8>) -> Result<Packed, IoError> {
    let crc = crc32fast::hash(&data);
    let size = data.len();
    let body = deflate(&data)?;
    Ok(Packed { name, body, crc, size })
}

/// The archive: local headers and data in order, then the central directory.
fn write_zip(entries: Vec<(String, Part)>) -> Result<Vec<u8>, IoError> {
    // A part or an offset past 4 GB needs zip64, which this writer does not do.
    if needs_zip64(&entries) {
        return write_zip_general(entries);
    }
    // The parts compress on the threads the machine has, and each worker takes its part with it
    // and drops it once deflated: the peak is the parts not yet compressed plus the packed ones,
    // never both full sets plus the archive. A part that was already packed (a sheet) costs its
    // worker nothing. The order they are written in is the caller's, so the directory is
    // deterministic.
    #[cfg(not(target_arch = "wasm32"))]
    let packed: Vec<Packed> = std::thread::scope(|scope| {
        let handles: Vec<_> = entries
            .into_iter()
            .map(|(name, part)| {
                scope.spawn(move || match part {
                    Part::Raw(data) => pack(name, data),
                    Part::Packed(packed) => Ok(packed),
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| match handle.join() {
                Ok(part) => part,
                // A worker that panicked took its part down with it, and an empty body would write
                // a corrupt archive: the save fails instead.
                Err(_) => Err(IoError::Zip("a compression thread panicked".into())),
            })
            .collect::<Result<Vec<_>, _>>()
    })?;
    // Wasm has no OS threads: deflate each part here instead. A packed sheet passes through, and
    // the same deflate makes the same bytes.
    #[cfg(target_arch = "wasm32")]
    let packed: Vec<Packed> = entries
        .into_iter()
        .map(|(name, part)| match part {
            Part::Raw(data) => pack(name, data),
            Part::Packed(packed) => Ok(packed),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let count = packed.len();
    let mut out: Vec<u8> = Vec::with_capacity(packed.iter().map(|p| p.body.len()).sum::<usize>() + count * 128 + 22);
    let mut directory: Vec<u8> = Vec::with_capacity(count * 64);
    // Each body is copied into the archive and dropped there: the packed set and the archive are
    // never both whole in memory.
    for Packed { name, body, crc, size } in packed {
        // `needs_zip64` bounded the whole layout below 4 GB, so every offset and size fits a u32.
        let offset = out.len() as u32;
        // Local file header. The DOS date is 1980-01-01, which is what `zip` wrote.
        push_u32(&mut out, 0x0403_4b50);
        push_u16(&mut out, 20);
        push_u16(&mut out, 0);
        push_u16(&mut out, 8);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0x0021);
        push_u32(&mut out, crc);
        push_u32(&mut out, body.len() as u32);
        push_u32(&mut out, size as u32);
        push_u16(&mut out, name.len() as u16);
        push_u16(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&body);
        // Central directory entry.
        push_u32(&mut directory, 0x0201_4b50);
        push_u16(&mut directory, 20);
        push_u16(&mut directory, 20);
        push_u16(&mut directory, 0);
        push_u16(&mut directory, 8);
        push_u16(&mut directory, 0);
        push_u16(&mut directory, 0x0021);
        push_u32(&mut directory, crc);
        push_u32(&mut directory, body.len() as u32);
        push_u32(&mut directory, size as u32);
        push_u16(&mut directory, name.len() as u16);
        push_u16(&mut directory, 0);
        push_u16(&mut directory, 0);
        push_u16(&mut directory, 0);
        push_u16(&mut directory, 0);
        push_u32(&mut directory, 0);
        push_u32(&mut directory, offset);
        directory.extend_from_slice(name.as_bytes());
    }
    let directory_offset = out.len() as u32;
    let directory_size = directory.len() as u32;
    out.extend_from_slice(&directory);
    push_u32(&mut out, 0x0605_4b50);
    push_u16(&mut out, 0);
    push_u16(&mut out, 0);
    push_u16(&mut out, count as u16);
    push_u16(&mut out, count as u16);
    push_u32(&mut out, directory_size);
    push_u32(&mut out, directory_offset);
    push_u16(&mut out, 0);
    Ok(out)
}

/// The `zip` crate's writer, for an archive this one does not handle (zip64). Each entry is taken
/// by value so its bytes are dropped once written: the archive's own buffer is the one copy the
/// caller's parts must not double.
fn write_zip_general(entries: Vec<(String, Part)>) -> Result<Vec<u8>, IoError> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, part) in entries {
        // This writer deflates each part itself, so a part packed while it was written is inflated
        // back here. Only the rare zip64 path does that; the fast path writes a packed part as is.
        let data: Cow<'_, [u8]> = match &part {
            Part::Raw(data) => Cow::Borrowed(data.as_slice()),
            Part::Packed(packed) => Cow::Owned(packed.inflate()?),
        };
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(i64::from(DEFLATE_LEVEL)))
            .last_modified_time(zip::DateTime::default())
            .large_file(data.len() as u64 >= u32::MAX as u64);
        zip.start_file(name.as_str(), opts).map_err(|e| IoError::Zip(e.to_string()))?;
        zip.write_all(data.as_ref()).map_err(|e| IoError::Zip(e.to_string()))?;
    }
    zip.finish().map(|c| c.into_inner()).map_err(|e| IoError::Zip(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolving() {
        assert_eq!(resolve("xl", "worksheets/sheet1.xml"), "xl/worksheets/sheet1.xml");
        assert_eq!(resolve("xl/worksheets", "../drawings/d1.xml"), "xl/drawings/d1.xml");
        assert_eq!(resolve("xl/worksheets", "/xl/tables/t.xml"), "xl/tables/t.xml");
        assert_eq!(resolve("", "xl/workbook.xml"), "xl/workbook.xml");
        assert_eq!(resolve("a", "../../../x"), "x");
        assert_eq!(split_dir("xl/workbook.xml"), ("xl", "workbook.xml"));
        assert_eq!(split_dir("x.xml"), ("", "x.xml"));
    }

    /// `add` takes a part's bytes instead of copying them (this only compiles for an owned
    /// vector), and the package still reads back what it was given, in the order it was given.
    #[test]
    fn zip_out_takes_each_part_by_value() {
        let pattern = |n: usize| -> Vec<u8> { (0..n).map(|i| (i % 251) as u8).collect() };
        let mut zip = ZipOut::new();
        zip.add("xl/worksheets/sheet1.xml", pattern(70_000)).unwrap();
        zip.add("[Content_Types].xml", Vec::from("<Types/>")).unwrap();
        // An empty part is still a part: deflate has bytes for it and the headers say so.
        zip.add("xl/empty.bin", Vec::new()).unwrap();
        let bytes = zip.finish().unwrap();

        let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
        assert_eq!(archive.len(), 3);
        let mut read = vec![];
        for i in 0..archive.len() {
            let mut f = archive.by_index(i).unwrap();
            let name = f.name().to_string();
            let declared = f.size() as usize;
            let mut body = Vec::new();
            f.read_to_end(&mut body).unwrap();
            assert_eq!(declared, body.len(), "{name}: the header's size is the part's");
            read.push((name, body));
        }
        let names: Vec<&str> = read.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["xl/worksheets/sheet1.xml", "[Content_Types].xml", "xl/empty.bin"]);
        assert_eq!(read[0].1, pattern(70_000));
        assert_eq!(read[1].1, b"<Types/>");
        assert!(read[2].1.is_empty());
    }

    /// The zip64 decision is taken before the parts are deflated (their bytes are gone after), so
    /// its bound must cover the archive the writer then lays out: no offset is re-checked.
    #[test]
    fn the_zip64_bound_covers_the_archive_it_decides_about() {
        let entries: Vec<(String, Part)> = (0..12usize)
            .map(|i| {
                let name = format!("xl/worksheets/sheet{}.xml", i + 1);
                (name, Part::Raw((0..(i + 1) * 5_000).map(|b| (b % 253) as u8).collect()))
            })
            .collect();
        assert!(!needs_zip64(&entries), "a package this size does not need zip64");
        let unpacked: usize = entries.iter().map(|(_, part)| unpacked_size(part)).sum();
        let bound = zip64_bound(&entries);
        assert!(bound > unpacked, "the bound allows for the headers and for deflate expanding: {bound} <= {unpacked}");
        let written = write_zip(entries).unwrap();
        assert!(written.len() <= bound, "the archive outgrew the bound it was checked against: {} > {bound}", written.len());
    }

    fn read_zip(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut parts = Vec::with_capacity(archive.len());
        for i in 0..archive.len() {
            let mut f = archive.by_index(i).unwrap();
            let name = f.name().to_string();
            let mut body = Vec::new();
            f.read_to_end(&mut body).unwrap();
            parts.push((name, body));
        }
        parts
    }

    /// A sheet deflated while it is written lands in the archive exactly as the same bytes deflated
    /// by the zip would: same order, same names, same headers, same body. The mixed list also
    /// survives the zip64 writer, which has to inflate a packed part before handing it to `zip`.
    #[test]
    fn packed_sheets_write_the_same_archive_as_raw_parts() {
        let part_bytes = |n: usize, salt: u8| -> Vec<u8> { (0..n).map(|i| (i as u8) ^ salt ^ ((i / 251) as u8)).collect() };
        let content: Vec<(String, Vec<u8>)> = vec![
            ("[Content_Types].xml".to_string(), Vec::from("<Types/>")),
            ("xl/worksheets/sheet1.xml".to_string(), part_bytes(220_000, 1)),
            ("xl/worksheets/sheet2.xml".to_string(), part_bytes(37_000, 7)),
            ("docProps/core.xml".to_string(), Vec::from("<core/>")),
        ];
        let build = |precompress: bool| -> Vec<(String, Part)> {
            content
                .iter()
                .map(|(name, data)| {
                    let part = if precompress && name.starts_with("xl/worksheets/") {
                        Part::Packed(pack(name.clone(), data.clone()).unwrap())
                    } else {
                        Part::Raw(data.clone())
                    };
                    (name.clone(), part)
                })
                .collect()
        };

        let raw = write_zip(build(false)).unwrap();
        let mixed = write_zip(build(true)).unwrap();
        assert_eq!(mixed, raw, "pre-deflating a sheet changed the archive");
        assert_eq!(read_zip(&raw), content, "the raw archive lost or reordered a part");
        assert_eq!(read_zip(&mixed), content, "the mixed archive lost or reordered a part");
        assert_eq!(needs_zip64(&build(false)), needs_zip64(&build(true)), "pre-deflating changed the zip64 decision");

        // The zip64 path recompresses each part itself. Inflating a packed part must give the same
        // archive as the raw list, and no part may go missing.
        let raw_general = write_zip_general(build(false)).unwrap();
        let mixed_general = write_zip_general(build(true)).unwrap();
        assert_eq!(mixed_general, raw_general, "the general writer changed a packed part");
        assert_eq!(read_zip(&mixed_general), content, "the general writer lost or reordered a part");
    }
}
