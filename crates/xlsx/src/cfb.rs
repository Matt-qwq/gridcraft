//! Just enough of the Compound File Binary (CFB/OLE2) container to tell what a non-zip
//! spreadsheet file is: Excel wraps password-protected workbooks in a CFB holding an
//! `EncryptedPackage` stream, while Excel 97-2003 `.xls` (and other legacy Office files) are
//! CFB files without one. Every read is bounds-checked and every loop bounded.

/// The CFB signature at the start of the file.
pub(crate) const SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

const END_OF_CHAIN: u32 = 0xFFFF_FFFE;
/// Sector ids at or above this are markers (free, end of chain, FAT/DIFAT sector), not sectors.
const MAX_REG_SECT: u32 = 0xFFFF_FFFA;
/// Directory sectors followed at most (4096 entries with 512-byte sectors).
const MAX_DIR_SECTORS: usize = 1024;
/// DIFAT sectors followed at most.
const MAX_DIFAT_SECTORS: usize = 1024;

/// Stream names that mark an encrypted (password-protected) OOXML package.
const ENCRYPTED_STREAMS: [&str; 2] = ["EncryptedPackage", "EncryptionInfo"];

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([*s.first()?, *s.get(1)?]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([*s.first()?, *s.get(1)?, *s.get(2)?, *s.get(3)?]))
}

/// Whether the compound file holds an encryption stream. Walks the directory through the FAT;
/// when the structure can't be followed (truncated or malformed), falls back to scanning the
/// bytes for the UTF-16LE stream names.
pub(crate) fn is_encrypted(bytes: &[u8]) -> bool {
    match directory_names(bytes) {
        Some(names) => names.iter().any(|n| ENCRYPTED_STREAMS.iter().any(|e| n.eq_ignore_ascii_case(e))),
        None => ENCRYPTED_STREAMS.iter().any(|e| contains_utf16(bytes, e)),
    }
}

fn contains_utf16(hay: &[u8], needle: &str) -> bool {
    let n: Vec<u8> = needle.encode_utf16().flat_map(u16::to_le_bytes).collect();
    !n.is_empty() && hay.windows(n.len()).any(|w| w == n.as_slice())
}

/// Names of every directory entry, or `None` when the directory can't be read.
fn directory_names(b: &[u8]) -> Option<Vec<String>> {
    if !b.starts_with(&SIGNATURE) {
        return None;
    }
    let shift = u16_at(b, 30)?;
    if shift != 9 && shift != 12 {
        return None;
    }
    let size = 1usize << shift;
    let per = size / 4;
    let sector = |id: u32| -> Option<&[u8]> {
        let start = (id as usize).checked_add(1)?.checked_mul(size)?;
        b.get(start..start.checked_add(size)?)
    };
    // The DIFAT: the FAT's sector ids, 109 in the header then chained DIFAT sectors.
    let mut difat: Vec<u32> = (0..109).filter_map(|i| u32_at(b, 76 + i * 4)).filter(|&s| s < MAX_REG_SECT).collect();
    let mut next = u32_at(b, 68)?;
    for _ in 0..MAX_DIFAT_SECTORS {
        if next >= MAX_REG_SECT {
            break;
        }
        let s = sector(next)?;
        difat.extend((0..per - 1).filter_map(|i| u32_at(s, i * 4)).filter(|&x| x < MAX_REG_SECT));
        next = u32_at(s, (per - 1) * 4)?;
    }
    let fat_next = |id: u32| -> Option<u32> {
        let idx = id as usize;
        let fat_sector = *difat.get(idx / per)?;
        u32_at(sector(fat_sector)?, (idx % per) * 4)
    };
    let mut names = Vec::new();
    let mut id = u32_at(b, 48)?;
    for _ in 0..MAX_DIR_SECTORS {
        if id == END_OF_CHAIN {
            return Some(names);
        }
        if id >= MAX_REG_SECT {
            return None;
        }
        let s = sector(id)?;
        for e in s.as_chunks::<128>().0 {
            let len = usize::from(u16_at(e, 64)?);
            if !(2..=64).contains(&len) {
                continue;
            }
            let units: Vec<u16> = (0..(len - 2) / 2).filter_map(|i| u16_at(e, i * 2)).collect();
            names.push(String::from_utf16_lossy(&units));
        }
        id = fat_next(id)?;
    }
    // A directory longer than the bound (or a cyclic chain): report what was read.
    Some(names)
}

/// Directory entry type of a stream object ([MS-CFB] 2.6.1).
const STREAM_OBJECT: u8 = 2;
/// "No entry" in the directory tree (NOSTREAM).
const NO_STREAM: u32 = 0xFFFF_FFFF;
/// Streams larger than this are refused (the format allows more, a workbook never needs it).
const MAX_STREAM: u64 = 1 << 30;

/// One directory entry ([MS-CFB] 2.6).
struct DirEntry {
    name: String,
    kind: u8,
    left: u32,
    right: u32,
    child: u32,
    start: u32,
    size: u64,
}

/// A parsed compound file: the FAT, the mini FAT and the directory, all bounds-checked.
/// Implemented from Microsoft's public [MS-CFB] specification.
pub(crate) struct Compound<'a> {
    bytes: &'a [u8],
    sector_size: usize,
    mini_sector_size: usize,
    mini_cutoff: u64,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    entries: Vec<DirEntry>,
}

impl<'a> Compound<'a> {
    /// Parses the header, FAT, mini FAT and directory. Errors on anything that can't be followed.
    pub(crate) fn open(b: &'a [u8]) -> Result<Compound<'a>, String> {
        let bad = |m: &str| format!("damaged compound file: {m}");
        if !b.starts_with(&SIGNATURE) {
            return Err(bad("missing signature"));
        }
        let major = u16_at(b, 26).ok_or_else(|| bad("truncated header"))?;
        let shift = u16_at(b, 30).ok_or_else(|| bad("truncated header"))?;
        if shift != 9 && shift != 12 {
            return Err(bad("unsupported sector size"));
        }
        let mini_shift = u16_at(b, 32).ok_or_else(|| bad("truncated header"))?;
        if mini_shift != 6 {
            return Err(bad("unsupported mini sector size"));
        }
        let sector_size = 1usize << shift;
        let per = sector_size / 4;
        // Sectors that physically exist after the header sector: every chain is bounded by it.
        let sectors = (b.len() / sector_size).saturating_sub(1);
        let sector = |id: u32| -> Option<&[u8]> {
            let start = (id as usize).checked_add(1)?.checked_mul(sector_size)?;
            b.get(start..start.checked_add(sector_size)?)
        };
        // DIFAT: 109 FAT sector ids in the header, then chained DIFAT sectors.
        let mut fat_sectors: Vec<u32> = (0..109).filter_map(|i| u32_at(b, 76 + i * 4)).filter(|&s| s < MAX_REG_SECT).collect();
        let mut next = u32_at(b, 68).ok_or_else(|| bad("truncated header"))?;
        let mut hops = 0usize;
        while next < MAX_REG_SECT {
            hops += 1;
            if hops > sectors.min(MAX_DIFAT_SECTORS * 64) {
                return Err(bad("DIFAT chain loops"));
            }
            let s = sector(next).ok_or_else(|| bad("DIFAT sector out of range"))?;
            fat_sectors.extend((0..per - 1).filter_map(|i| u32_at(s, i * 4)).filter(|&x| x < MAX_REG_SECT));
            next = u32_at(s, (per - 1) * 4).ok_or_else(|| bad("truncated DIFAT sector"))?;
        }
        if fat_sectors.len() > sectors {
            return Err(bad("more FAT sectors than the file holds"));
        }
        let mut fat = Vec::with_capacity(fat_sectors.len().saturating_mul(per));
        for id in fat_sectors {
            let s = sector(id).ok_or_else(|| bad("FAT sector out of range"))?;
            fat.extend((0..per).filter_map(|i| u32_at(s, i * 4)));
        }
        let mut cf = Compound { bytes: b, sector_size, mini_sector_size: 64, mini_cutoff: 4096, fat, mini_fat: vec![], entries: vec![] };
        if let Some(c) = u32_at(b, 56) {
            cf.mini_cutoff = u64::from(c);
        }
        // Directory.
        let dir_start = u32_at(b, 48).ok_or_else(|| bad("truncated header"))?;
        let dir = cf.read_chain(dir_start, u64::MAX)?;
        for e in dir.as_chunks::<128>().0 {
            let len = usize::from(u16_at(e, 64).unwrap_or(0)).min(64);
            let units: Vec<u16> = (0..len.saturating_sub(2) / 2).filter_map(|i| u16_at(e, i * 2)).collect();
            let mut size = u32_at(e, 120).map(u64::from).unwrap_or(0);
            if major >= 4 {
                size |= u64::from(u32_at(e, 124).unwrap_or(0)) << 32;
            }
            cf.entries.push(DirEntry {
                name: String::from_utf16_lossy(&units),
                kind: e.get(66).copied().unwrap_or(0),
                left: u32_at(e, 68).unwrap_or(NO_STREAM),
                right: u32_at(e, 72).unwrap_or(NO_STREAM),
                child: u32_at(e, 76).unwrap_or(NO_STREAM),
                start: u32_at(e, 116).unwrap_or(END_OF_CHAIN),
                size,
            });
        }
        if cf.entries.first().is_none_or(|r| r.kind != 5) {
            return Err(bad("missing root entry"));
        }
        // Mini FAT.
        let mini_start = u32_at(b, 60).ok_or_else(|| bad("truncated header"))?;
        if mini_start < MAX_REG_SECT {
            let raw = cf.read_chain(mini_start, u64::MAX)?;
            cf.mini_fat = (0..raw.len() / 4).filter_map(|i| u32_at(&raw, i * 4)).collect();
        }
        Ok(cf)
    }

    /// Follows a FAT chain from `start`, reading at most `limit` bytes. A chain longer than the
    /// FAT (so a cycle) or pointing outside the file is an error. The file may end inside the
    /// chain's last sector (some writers don't pad it, and Excel opens those files): that sector
    /// reads short, and the caller checks the stream's declared size against what was read.
    fn read_chain(&self, start: u32, limit: u64) -> Result<Vec<u8>, String> {
        let limit = usize::try_from(limit.min(MAX_STREAM)).unwrap_or(usize::MAX);
        let mut out = Vec::new();
        let mut id = start;
        let mut seen = vec![false; self.fat.len()];
        let mut cut = false;
        while id != END_OF_CHAIN && out.len() < limit {
            if cut {
                return Err("damaged compound file: sector out of range (the file may be truncated)".into());
            }
            if id >= MAX_REG_SECT {
                return Err("damaged compound file: broken sector chain".into());
            }
            match seen.get_mut(id as usize) {
                Some(v) if !*v => *v = true,
                Some(_) => return Err("damaged compound file: sector chain loops".into()),
                None => return Err("damaged compound file: sector missing from the FAT".into()),
            }
            let start = (id as usize).checked_add(1).and_then(|i| i.checked_mul(self.sector_size));
            let s = start
                .and_then(|s| self.bytes.get(s..s.checked_add(self.sector_size)?.min(self.bytes.len())))
                .ok_or("damaged compound file: sector out of range (the file may be truncated)")?;
            cut = s.len() < self.sector_size;
            out.extend_from_slice(s);
            id = *self.fat.get(id as usize).ok_or("damaged compound file: sector missing from the FAT")?;
        }
        Ok(out)
    }

    /// Follows a mini FAT chain inside the mini stream container. Like `read_chain`, the
    /// container may end inside the chain's last mini sector.
    fn read_mini_chain(&self, container: &[u8], start: u32, limit: usize) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        let mut id = start;
        let mut seen = vec![false; self.mini_fat.len()];
        let mut cut = false;
        while id != END_OF_CHAIN && out.len() < limit {
            if cut {
                return Err("damaged compound file: mini sector out of range".into());
            }
            if id >= MAX_REG_SECT {
                return Err("damaged compound file: broken mini sector chain".into());
            }
            match seen.get_mut(id as usize) {
                Some(v) if !*v => *v = true,
                Some(_) => return Err("damaged compound file: mini sector chain loops".into()),
                None => return Err("damaged compound file: mini sector missing from the mini FAT".into()),
            }
            let at = (id as usize).checked_mul(self.mini_sector_size);
            let s = at
                .and_then(|a| container.get(a..a.checked_add(self.mini_sector_size)?.min(container.len())))
                .ok_or("damaged compound file: mini sector out of range")?;
            cut = s.len() < self.mini_sector_size;
            out.extend_from_slice(s);
            id = *self.mini_fat.get(id as usize).ok_or("damaged compound file: mini sector missing from the mini FAT")?;
        }
        Ok(out)
    }

    /// The contents of the stream called `name` (case-insensitive) directly under the root
    /// storage, or `None` when there is no such stream.
    /// The directory entry of the stream called `name` (case-insensitive) directly under the
    /// root storage.
    fn root_entry(&self, name: &str) -> Result<Option<&DirEntry>, String> {
        let Some(root) = self.entries.first() else { return Ok(None) };
        // Walk the root's children (a red-black tree through left/right siblings), bounded.
        let mut stack = vec![root.child];
        let mut seen = 0usize;
        while let Some(i) = stack.pop() {
            if i == NO_STREAM {
                continue;
            }
            seen += 1;
            if seen > self.entries.len() {
                return Err("damaged compound file: directory tree loops".into());
            }
            let Some(e) = self.entries.get(i as usize) else { continue };
            if e.kind == STREAM_OBJECT && e.name.eq_ignore_ascii_case(name) {
                return Ok(Some(e));
            }
            stack.push(e.left);
            stack.push(e.right);
        }
        Ok(None)
    }

    /// Whether the root storage holds a stream called `name`, without reading it.
    pub(crate) fn has_root_stream(&self, name: &str) -> bool {
        matches!(self.root_entry(name), Ok(Some(_)))
    }

    /// The contents of the stream called `name` (case-insensitive) directly under the root
    /// storage, or `None` when there is no such stream.
    pub(crate) fn root_stream(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let Some(root) = self.entries.first() else { return Ok(None) };
        let found = self.root_entry(name)?;
        let Some(e) = found else { return Ok(None) };
        if e.size > MAX_STREAM {
            return Err("the workbook stream is larger than 1 GiB".into());
        }
        let size = usize::try_from(e.size).map_err(|_| "stream too large".to_string())?;
        let mut data = if e.size < self.mini_cutoff {
            let container = self.read_chain(root.start, root.size)?;
            self.read_mini_chain(&container, e.start, size)?
        } else {
            self.read_chain(e.start, e.size)?
        };
        if data.len() < size {
            return Err("damaged compound file: a stream is shorter than its declared size (the file may be truncated)".into());
        }
        data.truncate(size);
        Ok(Some(data))
    }
}

// ---- writing -------------------------------------------------------------------------------

/// The compound-file sector and mini-sector sizes. Version 3 files (the only kind Excel writes)
/// use 512-byte sectors; the mini stream exists for streams under the 4096-byte cutoff.
const SECTOR: usize = 512;
const MINI: usize = 64;
const MINI_CUTOFF: usize = 4096;
/// Markers in the FAT for a free sector, the end of a chain, a FAT sector and a DIFAT sector.
const FREE: u32 = 0xFFFF_FFFF;
const FAT_SECTOR: u32 = 0xFFFF_FFFD;
const DIFAT_SECTOR: u32 = 0xFFFF_FFFC;
/// Streams one compound file written here holds (an encrypted workbook has two).
const MAX_STREAMS: usize = 64;
/// The output is refused above this (the reader's own stream cap, plus one sector of slack).
const MAX_BYTES: usize = (1 << 30) + (1 << 20);

fn bad_write(m: &str) -> String {
    format!("can't write the compound file: {m}")
}

fn put16(b: &mut [u8], at: usize, v: u16) -> Result<(), String> {
    let s = b.get_mut(at..at.checked_add(2).ok_or_else(|| bad_write("offset overflow"))?).ok_or_else(|| bad_write("offset past the end"))?;
    s.copy_from_slice(&v.to_le_bytes());
    Ok(())
}

fn put32(b: &mut [u8], at: usize, v: u32) -> Result<(), String> {
    let s = b.get_mut(at..at.checked_add(4).ok_or_else(|| bad_write("offset overflow"))?).ok_or_else(|| bad_write("offset past the end"))?;
    s.copy_from_slice(&v.to_le_bytes());
    Ok(())
}

/// A directory entry ([MS-CFB] 2.6.1): 128 bytes, name in UTF-16, no nested storage.
struct Entry<'a> {
    name: &'a str,
    kind: u8,
    left: u32,
    right: u32,
    child: u32,
    start: u32,
    size: u64,
}

fn write_entry(out: &mut [u8], at: usize, e: &Entry<'_>) -> Result<(), String> {
    let units: Vec<u16> = e.name.encode_utf16().collect();
    if units.is_empty() || units.len() > 31 {
        return Err(bad_write("a stream name must be 1 to 31 characters"));
    }
    if units.iter().any(|u| *u == 0 || *u == u16::from(b'/') || *u == u16::from(b'\\')) {
        return Err(bad_write("a stream name holds a control character or a path separator"));
    }
    let name_len = (units.len() as u16).checked_add(1).ok_or_else(|| bad_write("name too long"))?;
    for (i, u) in units.iter().enumerate() {
        let at = at.checked_add(i.checked_mul(2).ok_or_else(|| bad_write("offset overflow"))?).ok_or_else(|| bad_write("offset overflow"))?;
        put16(out, at, *u)?;
    }
    put16(out, at.checked_add(64).ok_or_else(|| bad_write("offset overflow"))?, name_len.checked_mul(2).ok_or_else(|| bad_write("name too long"))?)?;
    *out.get_mut(at + 66).ok_or_else(|| bad_write("offset past the end"))? = e.kind;
    // Every node black: the tree is a sorted right chain, so lookups still follow the name order.
    *out.get_mut(at + 67).ok_or_else(|| bad_write("offset past the end"))? = 1;
    put32(out, at + 68, e.left)?;
    put32(out, at + 72, e.right)?;
    put32(out, at + 76, e.child)?;
    put32(out, at + 116, e.start)?;
    // Version 3: the size is 32 bits; the high 32 bits are zero.
    put32(out, at + 120, u32::try_from(e.size).map_err(|_| bad_write("a stream is larger than 4 GiB"))?)?;
    put32(out, at + 124, 0)
}

/// The order the CFB directory tree sorts names in: by length, then case-insensitively.
fn name_key(name: &str) -> (usize, String) {
    (name.encode_utf16().count(), name.to_uppercase())
}

/// Writes a compound file (version 3, 512-byte sectors) holding `streams` directly under the
/// root storage, laid out the way Excel lays a workbook out: streams under 4096 bytes in the mini
/// stream, larger ones in regular sectors, DIFAT sectors past the 109 FAT sectors the header
/// names. Clean-room from [MS-CFB] 2.2-2.6.
pub(crate) fn write_compound(streams: &[(&str, &[u8])]) -> Result<Vec<u8>, String> {
    if streams.len() > MAX_STREAMS {
        return Err(bad_write("too many streams"));
    }
    let total_bytes = streams.iter().try_fold(0usize, |n, (_, d)| n.checked_add(d.len())).ok_or_else(|| bad_write("too large"))?;
    if total_bytes > MAX_BYTES {
        return Err(bad_write("more than 1 GiB of streams"));
    }
    // Streams kept in the mini stream, chained in 64-byte mini sectors.
    let mut mini_fat: Vec<u32> = vec![];
    let mut container: Vec<u8> = vec![];
    let mut mini_start: Vec<u32> = vec![END_OF_CHAIN; streams.len()];
    for (i, (_, data)) in streams.iter().enumerate() {
        if data.is_empty() || data.len() >= MINI_CUTOFF {
            continue;
        }
        let n = data.len().div_ceil(MINI);
        let first = mini_fat.len();
        for k in 0..n {
            let next = if k + 1 == n { END_OF_CHAIN } else { u32::try_from(first + k + 1).map_err(|_| bad_write("too many mini sectors"))? };
            mini_fat.push(next);
        }
        mini_start[i] = u32::try_from(first).map_err(|_| bad_write("too many mini sectors"))?;
        container.extend_from_slice(data);
        container.resize((first + n) * MINI, 0);
    }

    // Blobs written into whole sectors, in order: the directory, the mini FAT, the mini stream,
    // then each stream too large for the mini stream.
    let entries = streams.len().checked_add(1).ok_or_else(|| bad_write("too many streams"))?;
    let dir_len = entries.checked_mul(128).ok_or_else(|| bad_write("too large"))?.div_ceil(SECTOR) * SECTOR;
    // The mini FAT is a stream in regular sectors like any other, so its last sector is padded.
    // Pad with FREESECT, not zero: a zero entry names mini sector 0, and a reader that followed
    // it past the mini stream's own length would loop into the live part of the chain.
    let mut mini_fat_blob: Vec<u8> = mini_fat.iter().flat_map(|v| v.to_le_bytes()).collect();
    mini_fat_blob.resize(mini_fat_blob.len().div_ceil(SECTOR) * SECTOR, 0xFF);
    let mut blobs: Vec<Vec<u8>> = vec![vec![0u8; dir_len], mini_fat_blob, container.clone()];
    // In the caller's order, so a stream's own entry finds its data.
    let mut large_at: Vec<Option<usize>> = vec![None; streams.len()];
    for (i, (_, data)) in streams.iter().enumerate() {
        if data.is_empty() || data.len() < MINI_CUTOFF {
            continue;
        }
        let mut body = data.to_vec();
        body.resize(body.len().div_ceil(SECTOR) * SECTOR, 0);
        large_at[i] = Some(blobs.len());
        blobs.push(body);
    }
    let counts: Vec<usize> = blobs.iter().map(|b| b.len().div_ceil(SECTOR)).collect();
    let body: usize = counts.iter().try_fold(0usize, |n, c| n.checked_add(*c)).ok_or_else(|| bad_write("too large"))?;

    // The FAT's own sectors are described by the FAT, so its count is a fixed point; past 109
    // FAT sectors the header's DIFAT overflows into DIFAT sectors, which need their own sectors.
    let mut fat_sectors = 1usize;
    let (difat_sectors, total) = loop {
        let difat = fat_sectors.saturating_sub(109).div_ceil(127);
        let total = fat_sectors.checked_add(difat).and_then(|n| n.checked_add(body)).ok_or_else(|| bad_write("too large"))?;
        let need = total.div_ceil(128);
        if need <= fat_sectors {
            break (difat, total);
        }
        fat_sectors = need;
    };
    let len = total
        .checked_add(1)
        .and_then(|n| n.checked_mul(SECTOR))
        .filter(|n| *n <= MAX_BYTES)
        .ok_or_else(|| bad_write("more than 1 GiB of streams"))?;

    let mut fat = vec![FREE; fat_sectors.checked_mul(128).ok_or_else(|| bad_write("too large"))?];
    for v in fat.iter_mut().take(fat_sectors) {
        *v = FAT_SECTOR;
    }
    for v in fat.iter_mut().skip(fat_sectors).take(difat_sectors) {
        *v = DIFAT_SECTOR;
    }
    let mut starts: Vec<u32> = Vec::with_capacity(counts.len());
    let mut next = fat_sectors + difat_sectors;
    for &n in &counts {
        if n == 0 {
            starts.push(END_OF_CHAIN);
            continue;
        }
        starts.push(u32::try_from(next).map_err(|_| bad_write("too many sectors"))?);
        for k in 0..n {
            let slot = fat.get_mut(next + k).ok_or_else(|| bad_write("too many sectors"))?;
            *slot = if k + 1 == n { END_OF_CHAIN } else { u32::try_from(next + k + 1).map_err(|_| bad_write("too many sectors"))? };
        }
        next += n;
    }

    let mut out = vec![0u8; len];
    out.get_mut(..SIGNATURE.len()).ok_or_else(|| bad_write("truncated"))?.copy_from_slice(&SIGNATURE);
    put16(&mut out, 24, 0x3E)?; // minor version
    put16(&mut out, 26, 3)?; // major version: 512-byte sectors
    put16(&mut out, 28, 0xFFFE)?; // little-endian byte order
    put16(&mut out, 30, 9)?; // sector shift: 2^9 = 512
    put16(&mut out, 32, 6)?; // mini sector shift: 2^6 = 64
    put32(&mut out, 44, u32::try_from(fat_sectors).map_err(|_| bad_write("too many FAT sectors"))?)?;
    put32(&mut out, 48, starts.first().copied().unwrap_or(END_OF_CHAIN))?; // the directory
    put32(&mut out, 56, MINI_CUTOFF as u32)?;
    put32(&mut out, 60, starts.get(1).copied().unwrap_or(END_OF_CHAIN))?; // the mini FAT
    put32(&mut out, 64, u32::try_from(counts.get(1).copied().unwrap_or(0)).map_err(|_| bad_write("too large"))?)?;
    let first_difat = if difat_sectors > 0 { fat_sectors } else { END_OF_CHAIN as usize };
    put32(&mut out, 68, u32::try_from(first_difat).map_err(|_| bad_write("too many sectors"))?)?;
    put32(&mut out, 72, u32::try_from(difat_sectors).map_err(|_| bad_write("too many sectors"))?)?;
    for i in 0..109 {
        let v = if i < fat_sectors { i as u32 } else { FREE };
        put32(&mut out, 76 + i * 4, v)?;
    }
    for (i, v) in fat.iter().enumerate() {
        let at = i.checked_mul(4).and_then(|n| n.checked_add(SECTOR)).ok_or_else(|| bad_write("offset overflow"))?;
        put32(&mut out, at, *v)?;
    }
    // DIFAT sectors: 127 FAT sector ids each, then the next DIFAT sector.
    for k in 0..difat_sectors {
        let sector = fat_sectors + k;
        let at = sector.checked_add(1).and_then(|n| n.checked_mul(SECTOR)).ok_or_else(|| bad_write("offset overflow"))?;
        for j in 0..127 {
            let id = 109 + k * 127 + j;
            put32(&mut out, at + j * 4, if id < fat_sectors { id as u32 } else { FREE })?;
        }
        let next = if k + 1 == difat_sectors { END_OF_CHAIN } else { u32::try_from(sector + 1).map_err(|_| bad_write("too many sectors"))? };
        put32(&mut out, at + 127 * 4, next)?;
    }
    for (blob, start) in blobs.iter().zip(&starts) {
        if blob.is_empty() {
            continue;
        }
        let at = (*start as usize).checked_add(1).and_then(|n| n.checked_mul(SECTOR)).ok_or_else(|| bad_write("offset overflow"))?;
        let to = at.checked_add(blob.len()).ok_or_else(|| bad_write("offset overflow"))?;
        out.get_mut(at..to).ok_or_else(|| bad_write("offset past the end"))?.copy_from_slice(blob);
    }

    // The directory: the root first, then the streams in the tree's name order as right siblings.
    // It was copied into its sectors above, so the entries go at its own sector here.
    let dir_at = (*starts.first().ok_or_else(|| bad_write("missing directory"))? as usize)
        .checked_add(1)
        .and_then(|n| n.checked_mul(SECTOR))
        .ok_or_else(|| bad_write("offset overflow"))?;
    let mut order: Vec<usize> = (0..streams.len()).collect();
    order.sort_by(|a, b| name_key(streams[*a].0).cmp(&name_key(streams[*b].0)));
    let root = Entry {
        name: "Root Entry",
        kind: 5,
        left: NO_STREAM,
        right: NO_STREAM,
        child: if streams.is_empty() { NO_STREAM } else { 1 },
        start: starts.get(2).copied().unwrap_or(END_OF_CHAIN),
        size: container.len() as u64,
    };
    write_entry(&mut out, dir_at, &root)?;
    for (position, index) in order.iter().enumerate() {
        let (name, data) = streams.get(*index).copied().ok_or_else(|| bad_write("missing stream"))?;
        let right = if position + 1 < order.len() { u32::try_from(position + 2).map_err(|_| bad_write("too many streams"))? } else { NO_STREAM };
        let start = match large_at[*index] {
            Some(b) => starts.get(b).copied().unwrap_or(END_OF_CHAIN),
            None => mini_start.get(*index).copied().unwrap_or(END_OF_CHAIN),
        };
        let e = Entry { name, kind: 2, left: NO_STREAM, right, child: NO_STREAM, start, size: data.len() as u64 };
        write_entry(&mut out, dir_at + (position + 1) * 128, &e)?;
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal valid compound file (512-byte sectors: FAT at sector 0, directory at sector 1)
    /// whose directory holds a root entry plus the given stream names.
    pub(crate) fn build(streams: &[&str]) -> Vec<u8> {
        let mut b = vec![0u8; 512 * 3];
        b[..8].copy_from_slice(&SIGNATURE);
        b[24..26].copy_from_slice(&0x3Eu16.to_le_bytes()); // minor version
        b[26..28].copy_from_slice(&3u16.to_le_bytes()); // major version
        b[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes()); // byte order
        b[30..32].copy_from_slice(&9u16.to_le_bytes()); // sector shift
        b[32..34].copy_from_slice(&6u16.to_le_bytes()); // mini sector shift
        b[44..48].copy_from_slice(&1u32.to_le_bytes()); // FAT sectors
        b[48..52].copy_from_slice(&1u32.to_le_bytes()); // first directory sector
        b[60..64].copy_from_slice(&END_OF_CHAIN.to_le_bytes()); // no mini FAT
        b[68..72].copy_from_slice(&END_OF_CHAIN.to_le_bytes()); // no DIFAT sectors
        for i in 0..109 {
            b[76 + i * 4..80 + i * 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        }
        b[76..80].copy_from_slice(&0u32.to_le_bytes()); // FAT lives in sector 0
        // FAT (sector 0, at byte 512): sector 0 is a FAT sector, sector 1 ends its chain.
        for i in 0..128 {
            b[512 + i * 4..516 + i * 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        }
        b[512..516].copy_from_slice(&0xFFFF_FFFDu32.to_le_bytes());
        b[516..520].copy_from_slice(&END_OF_CHAIN.to_le_bytes());
        // Directory (sector 1, at byte 1024).
        for (k, name) in std::iter::once("Root Entry").chain(streams.iter().copied()).take(4).enumerate() {
            let e = 1024 + k * 128;
            let units: Vec<u16> = name.encode_utf16().take(31).collect();
            for (i, u) in units.iter().enumerate() {
                b[e + i * 2..e + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
            }
            b[e + 64..e + 66].copy_from_slice(&((units.len() as u16 + 1) * 2).to_le_bytes());
            b[e + 66] = if k == 0 { 5 } else { 2 };
        }
        b
    }

    #[test]
    fn reads_directory_names() {
        let b = build(&["EncryptionInfo", "EncryptedPackage"]);
        assert_eq!(directory_names(&b), Some(vec!["Root Entry".into(), "EncryptionInfo".into(), "EncryptedPackage".into()]));
        assert!(is_encrypted(&b));
        assert!(!is_encrypted(&build(&["Workbook", "\u{5}SummaryInformation"])));
    }

    #[test]
    fn hostile_headers_never_panic() {
        let mut b = build(&["Workbook"]);
        for at in [30usize, 48, 68, 76, 512, 516, 1024 + 64] {
            for v in [0u32, 1, 7, 0xFFFF_FFFA, 0xFFFF_FFFF, 0x7FFF_FFFF] {
                let mut c = b.clone();
                c[at..at + 4].copy_from_slice(&v.to_le_bytes());
                let _ = is_encrypted(&c);
                for cut in [0usize, 8, 100, 600, 1100] {
                    let _ = is_encrypted(&c[..cut.min(c.len())]);
                }
            }
        }
        // A directory chain that loops back on itself stops at the bound.
        b[516..520].copy_from_slice(&1u32.to_le_bytes());
        assert!(!is_encrypted(&b));
    }

    /// `compound_file` with one regular-sector stream of `len` bytes (FAT in sector 0, directory
    /// in sector 1, the stream from sector 2), cut right after the stream's last byte, the way
    /// some writers leave the final sector unpadded.
    fn unpadded(len: usize) -> (Vec<u8>, Vec<u8>) {
        let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        let mut file = crate::testing::compound_file(&[("Workbook", &data)]);
        file.truncate(512 * 3 + len);
        (file, data)
    }

    #[test]
    fn an_unpadded_last_sector_still_reads() {
        let (file, data) = unpadded(5000);
        assert_ne!(file.len() % 512, 0);
        let cf = Compound::open(&file).unwrap();
        assert_eq!(cf.root_stream("Workbook").unwrap().unwrap(), data);
        // Bytes the stream needs that the file doesn't hold are still damage.
        let cf = Compound::open(&file[..file.len() - 1]).unwrap();
        assert!(cf.root_stream("Workbook").is_err());
    }

    #[test]
    fn an_unpadded_last_mini_sector_still_reads() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let mut file = crate::testing::compound_file(&[("Workbook", &data)]);
        // Header, FAT, directory, mini FAT, then the mini stream holding the stream's bytes.
        file.truncate(512 * 4 + data.len());
        let cf = Compound::open(&file).unwrap();
        assert_eq!(cf.root_stream("Workbook").unwrap().unwrap(), data);
        let cf = Compound::open(&file[..file.len() - 1]).unwrap();
        assert!(cf.root_stream("Workbook").is_err());
    }

    #[test]
    fn a_cut_sector_inside_a_chain_is_damage() {
        let (mut file, _) = unpadded(5000);
        // The cut sector (11) points on to the directory sector and the stream claims those
        // bytes too: the partial sector would splice 392 bytes into the middle of the stream.
        file[512 + 11 * 4..512 + 12 * 4].copy_from_slice(&1u32.to_le_bytes());
        file[1024 + 128 + 120..1024 + 128 + 124].copy_from_slice(&5512u32.to_le_bytes());
        let cf = Compound::open(&file).unwrap();
        assert!(cf.root_stream("Workbook").is_err());
    }

    #[test]
    fn a_cut_mini_sector_inside_a_chain_is_damage() {
        let small = [7u8; 100];
        let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let mut file = crate::testing::compound_file(&[("Small", &small), ("Workbook", &data)]);
        // "Small" holds mini sectors 0-1, "Workbook" 2-17; the mini stream starts at sector 3.
        file.truncate(512 * 4 + 128 + data.len());
        assert_eq!(Compound::open(&file).unwrap().root_stream("Workbook").unwrap().unwrap(), data);
        // The cut mini sector (17) points on to sector 0 and the stream claims those bytes too.
        file[1536 + 17 * 4..1536 + 18 * 4].copy_from_slice(&0u32.to_le_bytes());
        file[1024 + 2 * 128 + 120..1024 + 2 * 128 + 124].copy_from_slice(&1064u32.to_le_bytes());
        let cf = Compound::open(&file).unwrap();
        assert!(cf.root_stream("Workbook").is_err());
    }

    /// The writer's layout, at every size class: an empty stream, a mini-stream one, the cutoff
    /// either side, a large one, and one big enough to need DIFAT sectors (over 109 FAT sectors).
    #[test]
    fn written_compound_files_read_back() {
        let data = |n: usize| -> Vec<u8> { (0..n).map(|i| (i % 251) as u8).collect() };
        let big = data(11_000_000);
        for len in [0usize, 1, 63, 64, 65, 1000, 4095, 4096, 4097, 10_000] {
            let small = data(len);
            let file = write_compound(&[("EncryptionInfo", &small), ("EncryptedPackage", &big)]).unwrap();
            let cf = Compound::open(&file).unwrap();
            assert_eq!(cf.root_stream("EncryptionInfo").unwrap().unwrap(), small, "a {len}-byte stream");
            assert_eq!(cf.root_stream("EncryptedPackage").unwrap().unwrap(), big);
        }
        // The 11 MB package must need DIFAT sectors: the header only names 109 FAT sectors.
        let file = write_compound(&[("EncryptionInfo", b"info"), ("EncryptedPackage", &big)]).unwrap();
        let fat_sectors = u32::from_le_bytes([file[44], file[45], file[46], file[47]]);
        assert!(fat_sectors > 109, "{fat_sectors} FAT sectors");
        assert!(u32::from_le_bytes([file[72], file[73], file[74], file[75]]) > 0, "no DIFAT sectors");
        assert_eq!(Compound::open(&file).unwrap().root_stream("EncryptedPackage").unwrap().unwrap(), big);
        assert!(is_encrypted(&write_compound(&[("EncryptionInfo", b"info"), ("EncryptedPackage", b"pk")]).unwrap()));
    }

    /// The writer's layout at every size class, read by a third-party CFB reader: `cfb` (MIT,
    /// mdsteele/rust-cfb) is an independent implementation that shares no code with this crate.
    /// It parses the header, FAT, DIFAT, mini stream and directory tree itself, checks that each
    /// directory name sorts correctly for its parent, and reports each stream's declared size.
    ///
    /// `permissive` mode is the fair oracle, not `strict`: strict additionally demands that the
    /// mini FAT hold no more entries than the mini stream has 64-byte sectors, but a mini FAT is
    /// stored in whole 512-byte sectors (128 entries), so its last sector is padding whenever the
    /// mini stream is smaller — the usual case.
    #[test]
    fn an_independent_reader_accepts_every_layout_the_writer_makes() {
        use std::io::Read as _;
        let data = |n: usize| -> Vec<u8> { (0..n).map(|i| (i % 251) as u8).collect() };
        // (the size the directory states, the bytes the reader hands back)
        let read = |file: &[u8], name: &str| -> (u64, Vec<u8>) {
            let mut cf = cfb::CompoundFile::open(std::io::Cursor::new(file)).expect("the independent reader rejects the container");
            assert_eq!(cf.version(), cfb::Version::V3);
            let stated = cf.entry(name).expect("no such stream").len();
            let mut got = vec![];
            cf.open_stream(name).expect("can't open the stream").read_to_end(&mut got).unwrap();
            (stated, got)
        };
        // Every size class: nothing (no mini sector at all), the mini stream either side of a
        // mini sector, the 4096-byte cutoff either side, and a stream worth many sectors.
        for len in [0usize, 1, 63, 64, 65, 1000, 4095, 4096, 4097, 100_000] {
            let small = data(len);
            let big = data(5000);
            let file = write_compound(&[("EncryptionInfo", &small), ("EncryptedPackage", &big)]).unwrap();
            assert_eq!(read(&file, "EncryptionInfo"), (len as u64, small), "a {len}-byte stream");
            assert_eq!(read(&file, "EncryptedPackage"), (5000, big));
        }
        // No mini stream at all: both streams in regular sectors, the root's mini stream empty.
        let a = data(4096);
        let b = data(4096);
        let file = write_compound(&[("EncryptionInfo", &a), ("EncryptedPackage", &b)]).unwrap();
        assert_eq!(read(&file, "EncryptionInfo"), (4096, a));
        assert_eq!(read(&file, "EncryptedPackage"), (4096, b));
        // Past the 109 FAT sectors the header names, the FAT's directory lives in DIFAT sectors:
        // the outside reader follows that chain and returns the whole 7.2 MB stream.
        let big = data(7_200_000);
        let file = write_compound(&[("EncryptionInfo", b"info"), ("EncryptedPackage", &big)]).unwrap();
        assert!(u32::from_le_bytes([file[44], file[45], file[46], file[47]]) > 109, "no DIFAT sectors needed");
        assert!(u32::from_le_bytes([file[72], file[73], file[74], file[75]]) > 0, "the header names no DIFAT sector");
        assert_eq!(read(&file, "EncryptedPackage"), (7_200_000, big));
    }

    #[test]
    fn the_writer_refuses_what_it_cannot_lay_out() {
        assert!(write_compound(&[("", b"x")]).is_err());
        assert!(write_compound(&[("a/b", b"x")]).is_err());
        let long = "n".repeat(32);
        assert!(write_compound(&[(&long, b"x")]).is_err());
        let names: Vec<String> = (0..MAX_STREAMS + 1).map(|i| format!("s{i}")).collect();
        let many: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"x"[..])).collect();
        assert!(write_compound(&many).is_err());
        // A stream name that is too long is the only hostile input this writer takes: every
        // other size it is given it computes itself.
        let ok = write_compound(&[("Workbook", &[0u8; 600])]).unwrap();
        assert_eq!(ok.len() % SECTOR, 0);
    }

    /// The output is a fixed layout: the same streams always make the same bytes, so a writer
    /// change that moves a sector is a deliberate one.
    #[test]
    fn the_written_layout_is_stable() {
        let file = write_compound(&[("EncryptionInfo", &[1, 2, 3, 4]), ("EncryptedPackage", &[5; 5000])]).unwrap();
        assert_eq!(file.len(), 512 * (1 + 3 + 10 + 1));
        // Header: version 3, 512-byte sectors, the directory in sector 1 (sector 0 is the FAT),
        // the mini FAT in sector 2 and the mini stream in sector 3.
        assert_eq!((u16::from_le_bytes([file[26], file[27]]), u16::from_le_bytes([file[30], file[31]])), (3, 9));
        assert_eq!(u32::from_le_bytes([file[48], file[49], file[50], file[51]]), 1);
        assert_eq!(u32::from_le_bytes([file[60], file[61], file[62], file[63]]), 2);
        assert_eq!(u32::from_le_bytes([file[1024 + 116], file[1024 + 117], file[1024 + 118], file[1024 + 119]]), 3);
        let names = directory_names(&file).unwrap();
        assert_eq!(names, ["Root Entry", "EncryptionInfo", "EncryptedPackage"]);
    }
}
