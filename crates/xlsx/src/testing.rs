//! Test support: builds password-encrypted workbooks in code, so tests here and in the crates
//! above (engine, UI) never need an encrypted file on disk. Compiled for this crate's tests and,
//! elsewhere, only with the `test-support` feature (a dev-dependency feature, never shipped).
//!
//! Salts and keys are fixed so the output is deterministic. This is not an encryptor for real
//! use: a fixed key protects nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// Each helper is used by some crates' test suites and not others (the module is compiled into
// every test build through the `test-support` feature), so "never used" is normal here.
#![allow(dead_code)]

use aes::cipher::{BlockCipherEncrypt, KeyInit};

use crate::crypto::{
    BLOCK_KEY, BLOCK_VERIFIER_HASH, BLOCK_VERIFIER_INPUT, F_AES, F_CRYPTOAPI, Hash, PASSWORD_ENCRYPTOR, Params, SEGMENT, derive, iterated_hash,
    standard_key,
};

/// The spin count `encrypt_workbook` uses: Excel's default.
pub const SPIN_COUNT: u32 = 100_000;

const PASSWORD_SALT: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
const KEY_DATA_SALT: [u8; 16] = [16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31];
const SECRET_KEY: [u8; 32] =
    [64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95];
const VERIFIER: [u8; 16] = [32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47];

/// `package` (normally an `.xlsx` zip) as Excel stores it with a password: Agile encryption
/// (AES-256, SHA-512) inside a compound file.
pub fn encrypt_workbook(package: &[u8], password: &str) -> Vec<u8> {
    let (info, encrypted) = encrypt_agile(package, password, SPIN_COUNT);
    compound_file(&[("EncryptionInfo", &info), ("EncryptedPackage", &encrypted)])
}

/// The same with Standard encryption (AES-128, SHA-1), as Excel 2007 wrote it.
pub fn encrypt_workbook_standard(package: &[u8], password: &str) -> Vec<u8> {
    let (info, encrypted) = encrypt_standard(package, password);
    compound_file(&[("EncryptionInfo", &info), ("EncryptedPackage", &encrypted)])
}

/// `file` (Agile-encrypted, as [`crate::encrypt_workbook`] writes it) with one byte of the
/// `EncryptedPackage` ciphertext changed: the file's own data-integrity check has to refuse it.
/// `None` when `file` isn't an encrypted workbook with that stream.
pub fn corrupt_encrypted_package(file: &[u8]) -> Option<Vec<u8>> {
    let (info, mut package) = crate::crypto::streams(file).ok()?;
    // Byte 8 is the first ciphertext byte, after the 8-byte plaintext size: every real package
    // has at least one AES block there, and it decrypts into the first 16 bytes of every package.
    *package.get_mut(8)? ^= 0xFF;
    crate::cfb::write_compound(&[("EncryptionInfo", &info), ("EncryptedPackage", &package)]).ok()
}

/// A version 3 compound file (512-byte sectors) holding `streams` under the root, laid out as
/// Excel lays it out: streams under 4096 bytes in the mini stream, larger ones in regular
/// sectors. Up to 109 FAT sectors (about 7 MB of streams).
pub fn compound_file(streams: &[(&str, &[u8])]) -> Vec<u8> {
    const SECTOR: usize = 512;
    const MINI: usize = 64;
    const CUTOFF: usize = 4096;
    const FREE: u32 = 0xFFFF_FFFF;
    const END: u32 = 0xFFFF_FFFE;
    const FAT_SECTOR: u32 = 0xFFFF_FFFD;
    let is_mini = |d: &[u8]| d.len() < CUTOFF;

    // The mini stream: small streams in 64-byte mini sectors, chained by the mini FAT.
    let mut container = vec![];
    let mut mini_fat: Vec<u32> = vec![];
    let mut mini_starts = vec![];
    for (_, d) in streams {
        if !is_mini(d) || d.is_empty() {
            mini_starts.push(END);
            continue;
        }
        let first = mini_fat.len();
        let n = d.len().div_ceil(MINI);
        for k in 0..n {
            mini_fat.push(if k + 1 == n { END } else { (first + k + 1) as u32 });
        }
        mini_starts.push(first as u32);
        container.extend_from_slice(d);
        container.resize((first + n) * MINI, 0);
    }

    // Regular-sector blobs, in order: directory, mini FAT, mini stream, then each large stream.
    let dir_len = (streams.len() + 1).div_ceil(4) * SECTOR;
    let mut blobs: Vec<Vec<u8>> = vec![vec![0; dir_len], mini_fat.iter().flat_map(|v| v.to_le_bytes()).collect(), container.clone()];
    blobs.extend(streams.iter().map(|(_, d)| if is_mini(d) { vec![] } else { d.to_vec() }));
    let counts: Vec<usize> = blobs.iter().map(|b| b.len().div_ceil(SECTOR)).collect();
    let body: usize = counts.iter().sum();
    let mut fat_sectors = 1;
    while fat_sectors * 128 < fat_sectors + body {
        fat_sectors += 1;
    }
    assert!(fat_sectors <= 109, "the test writer has no DIFAT sectors");
    let mut fat = vec![FREE; fat_sectors * 128];
    fat[..fat_sectors].fill(FAT_SECTOR);
    let mut starts = vec![];
    let mut next = fat_sectors;
    for &n in &counts {
        if n == 0 {
            starts.push(END);
            continue;
        }
        starts.push(next as u32);
        for k in 0..n {
            fat[next + k] = if k + 1 == n { END } else { (next + k + 1) as u32 };
        }
        next += n;
    }

    let put16 = |b: &mut [u8], at: usize, v: u16| b[at..at + 2].copy_from_slice(&v.to_le_bytes());
    let put32 = |b: &mut [u8], at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    // Directory: the root (owning the mini stream), then the streams chained as right siblings.
    let dir = &mut blobs[0];
    for k in 0..dir_len / 128 {
        for at in [68, 72, 76] {
            put32(dir, k * 128 + at, FREE);
        }
    }
    let mut entry = |k: usize, name: &str, kind: u8, start: u32, size: usize| {
        let e = k * 128;
        let units: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        dir[e..e + units.len()].copy_from_slice(&units);
        put16(dir, e + 64, units.len() as u16 + 2);
        dir[e + 66] = kind;
        dir[e + 67] = 1; // black
        if kind == 5 && !streams.is_empty() {
            put32(dir, e + 76, 1);
        }
        if kind == 2 && k < streams.len() {
            put32(dir, e + 72, k as u32 + 1);
        }
        put32(dir, e + 116, start);
        put32(dir, e + 120, size as u32);
    };
    entry(0, "Root Entry", 5, starts[2], container.len());
    for (k, (name, data)) in streams.iter().enumerate() {
        let start = if is_mini(data) { mini_starts[k] } else { starts[3 + k] };
        entry(k + 1, name, 2, start, data.len());
    }

    let mut out = vec![0u8; SECTOR * (1 + fat_sectors + body)];
    out[..8].copy_from_slice(&crate::cfb::SIGNATURE);
    put16(&mut out, 24, 0x3E); // minor version
    put16(&mut out, 26, 3); // major version
    put16(&mut out, 28, 0xFFFE); // byte order
    put16(&mut out, 30, 9); // 512-byte sectors
    put16(&mut out, 32, 6); // 64-byte mini sectors
    put32(&mut out, 44, fat_sectors as u32);
    put32(&mut out, 48, starts[0]);
    put32(&mut out, 56, CUTOFF as u32);
    put32(&mut out, 60, starts[1]); // first mini FAT sector (END when there is none)
    put32(&mut out, 64, counts[1] as u32);
    put32(&mut out, 68, END); // no DIFAT sectors
    for i in 0..109 {
        put32(&mut out, 76 + i * 4, if i < fat_sectors { i as u32 } else { FREE });
    }
    for (i, v) in fat.iter().enumerate() {
        put32(&mut out, SECTOR + i * 4, *v);
    }
    for (blob, start) in blobs.iter().zip(&starts) {
        if !blob.is_empty() {
            let at = SECTOR * (1 + *start as usize);
            out[at..at + blob.len()].copy_from_slice(blob);
        }
    }
    out
}

fn encrypt_block(key: &[u8], block: &mut [u8; 16]) {
    let b = aes::Block::cast_from_core_mut(block);
    match key.len() {
        16 => aes::Aes128::new_from_slice(key).unwrap().encrypt_block(b),
        32 => aes::Aes256::new_from_slice(key).unwrap().encrypt_block(b),
        n => panic!("no {n}-byte AES in the test encryptor"),
    }
}

fn pad(data: &[u8]) -> Vec<u8> {
    let mut v = data.to_vec();
    v.resize(data.len().div_ceil(16) * 16, 0);
    v
}

fn encrypt_ecb(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut v = pad(data);
    v.as_chunks_mut::<16>().0.iter_mut().for_each(|b| encrypt_block(key, b));
    v
}

fn encrypt_cbc(key: &[u8], iv: &[u8], data: &[u8]) -> Vec<u8> {
    let mut v = pad(data);
    let mut prev: [u8; 16] = iv.try_into().unwrap();
    for b in v.as_chunks_mut::<16>().0 {
        b.iter_mut().zip(prev).for_each(|(x, p)| *x ^= p);
        encrypt_block(key, b);
        prev = *b;
    }
    v
}

fn base64_encode(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            s.push(if i <= c.len() { T[(n >> (18 - 6 * i)) as usize & 63] as char } else { '=' });
        }
    }
    s
}

fn package(size: usize, body: Vec<u8>) -> Vec<u8> {
    let mut p = (size as u64).to_le_bytes().to_vec();
    p.extend(body);
    p
}

/// The Agile XML descriptor: AES-256, SHA-512, a password encryptor salted with 0..16.
pub(crate) fn agile_info(password: &str, spin: u32) -> String {
    agile_xml(password, spin, None)
}

/// [`agile_info`] with a `dataIntegrity` element for `plain`: the HMAC key and value encrypted
/// with the **package key** (not a password-derived key), each under its own block key's IV from
/// the keyData salt ([MS-OFFCRYPTO] 2.3.4.14). Written out here with this module's own AES and
/// XML rather than through `crypto::encrypt_workbook`, so a reader that confuses the two block
/// keys, or derives the fields from the password, fails a test instead of passing its own
/// round-trip.
pub(crate) fn agile_info_with_integrity(password: &str, spin: u32, plain: &[u8]) -> String {
    agile_xml(password, spin, Some(plain))
}

fn agile_xml(password: &str, spin: u32, integrity_for: Option<&[u8]>) -> String {
    let key = Params { hash: Hash::Sha512, salt: PASSWORD_SALT.to_vec(), key_len: 32 };
    let h = iterated_hash(Hash::Sha512, password, &key.salt, spin);
    let iv = key.iv(None);
    let field = |block_key: &[u8], data: &[u8]| base64_encode(&encrypt_cbc(&derive(Hash::Sha512, &h, block_key, 32), &iv, data));
    let input = field(&BLOCK_VERIFIER_INPUT, &VERIFIER);
    let value = field(&BLOCK_VERIFIER_HASH, &Hash::Sha512.digest(&[&VERIFIER]));
    let secret = field(&BLOCK_KEY, &SECRET_KEY);
    let params = |salt: &[u8]| {
        format!(
            "saltSize=\"16\" blockSize=\"16\" keyBits=\"256\" hashSize=\"64\" cipherAlgorithm=\"AES\" cipherChaining=\"ChainingModeCBC\" hashAlgorithm=\"SHA512\" saltValue=\"{}\"",
            base64_encode(salt)
        )
    };
    // A fixed HMAC key (64 bytes, SHA-512's digest size) and the two block keys of section 2.3.4.14.
    let hmac_key = [0x11u8; 64];
    let integrity = match integrity_for {
        Some(plain) => {
            let field_iv = |block_key: &[u8]| {
                let mut d = Hash::Sha512.digest(&[&KEY_DATA_SALT[..], block_key]);
                d.truncate(16);
                d
            };
            let key = encrypt_cbc(&SECRET_KEY, &field_iv(&crate::crypto::BLOCK_HMAC_KEY), &hmac_key);
            let value = encrypt_cbc(&SECRET_KEY, &field_iv(&crate::crypto::BLOCK_HMAC_VALUE), &crate::crypto::hmac_sha512(&hmac_key, plain));
            format!("<dataIntegrity encryptedHmacKey=\"{}\" encryptedHmacValue=\"{}\"/>", base64_encode(&key), base64_encode(&value))
        }
        None => String::new(),
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<encryption xmlns=\"http://schemas.microsoft.com/office/2006/encryption\" xmlns:p=\"http://schemas.microsoft.com/office/2006/keyEncryptor/password\"><keyData {}/>{integrity}<keyEncryptors><keyEncryptor uri=\"{PASSWORD_ENCRYPTOR}\"><p:encryptedKey spinCount=\"{spin}\" {} encryptedVerifierHashInput=\"{input}\" encryptedVerifierHashValue=\"{value}\" encryptedKeyValue=\"{secret}\"/></keyEncryptor></keyEncryptors></encryption>",
        params(&KEY_DATA_SALT),
        params(&PASSWORD_SALT),
    )
}

/// `EncryptionInfo` for an Agile descriptor: version 4.4, reserved 0x40, then the XML.
pub(crate) fn with_agile_header(xml: &str) -> Vec<u8> {
    let mut info = vec![4, 0, 4, 0, 0x40, 0, 0, 0];
    info.extend_from_slice(xml.as_bytes());
    info
}

/// `EncryptionInfo` and `EncryptedPackage` for `plain`, Agile (AES-256, SHA-512).
pub(crate) fn encrypt_agile(plain: &[u8], password: &str, spin: u32) -> (Vec<u8>, Vec<u8>) {
    encrypt_agile_as(plain, password, spin, false)
}

/// [`encrypt_agile`] with a `dataIntegrity` element ([MS-OFFCRYPTO] 2.3.4.14) over `plain`.
pub(crate) fn encrypt_agile_with_integrity(plain: &[u8], password: &str, spin: u32) -> (Vec<u8>, Vec<u8>) {
    encrypt_agile_as(plain, password, spin, true)
}

fn encrypt_agile_as(plain: &[u8], password: &str, spin: u32, integrity: bool) -> (Vec<u8>, Vec<u8>) {
    let key_data = Params { hash: Hash::Sha512, salt: KEY_DATA_SALT.to_vec(), key_len: 32 };
    let mut body = vec![];
    for (i, segment) in plain.chunks(SEGMENT).enumerate() {
        body.extend(encrypt_cbc(&SECRET_KEY, &key_data.iv(Some(&(i as u32).to_le_bytes())), segment));
    }
    let info = if integrity { agile_info_with_integrity(password, spin, plain) } else { agile_info(password, spin) };
    (with_agile_header(&info), package(plain.len(), body))
}

/// `EncryptionInfo` and `EncryptedPackage` for `plain`, Standard (AES-128, SHA-1).
pub(crate) fn encrypt_standard(plain: &[u8], password: &str) -> (Vec<u8>, Vec<u8>) {
    let key = standard_key(password, &PASSWORD_SALT, 16);
    let mut header = vec![];
    for v in [F_CRYPTOAPI | F_AES, 0, 0x660E, 0x8004, 128, 0x18, 0, 0] {
        header.extend_from_slice(&u32::to_le_bytes(v));
    }
    header.extend("\0".encode_utf16().flat_map(u16::to_le_bytes));
    let mut info = vec![4, 0, 2, 0];
    info.extend_from_slice(&(F_CRYPTOAPI | F_AES).to_le_bytes());
    info.extend_from_slice(&(header.len() as u32).to_le_bytes());
    info.extend(header);
    info.extend_from_slice(&16u32.to_le_bytes());
    info.extend(PASSWORD_SALT);
    info.extend(encrypt_ecb(&key, &VERIFIER));
    info.extend_from_slice(&20u32.to_le_bytes());
    info.extend(encrypt_ecb(&key, &Hash::Sha1.digest(&[&VERIFIER])));
    (info, package(plain.len(), encrypt_ecb(&key, plain)))
}

// ---------------------------------------------------------------- package validator

/// A strict check of an `.xlsx` package, for tests: every relationship target exists, every part
/// has a content type, no part is orphaned, and `[Content_Types].xml` has no override for a part
/// the package doesn't have. These are the four ways a package makes Excel show a repair prompt.
///
/// The writer runs it on every package the test suite saves ([`crate::write::checked`]), so a
/// future change that writes a part without a relationship (or a relationship to a part it forgot
/// to write) fails every test that saves, not one.
pub fn check_package(bytes: &[u8]) -> Result<(), String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("not a zip: {e}"))?;
    let mut parts: std::collections::HashMap<String, Vec<u8>> = std::collections::HashMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| format!("entry {i}: {e}"))?;
        if f.is_dir() {
            continue;
        }
        let mut v = vec![];
        std::io::Read::read_to_end(&mut f, &mut v).map_err(|e| format!("{}: {e}", f.name()))?;
        parts.insert(f.name().to_string(), v);
    }
    check_unpacked(&parts)
}

/// [`check_package`] for a package that is already unpacked (part name → bytes).
pub fn check_unpacked(parts: &std::collections::HashMap<String, Vec<u8>>) -> Result<(), String> {
    use std::collections::{HashMap, HashSet, VecDeque};

    use crate::package::{normalize, resolve, split_dir};
    use crate::xml;

    let ct = parts.get("[Content_Types].xml").ok_or("[Content_Types].xml is missing")?;
    let types = xml::parse(ct).map_err(|e| format!("[Content_Types].xml is not XML: {e}"))?;
    let mut defaults: HashMap<String, String> = HashMap::new();
    for d in types.kids("Default") {
        let (Some(ext), Some(_)) = (d.attr("Extension"), d.attr("ContentType")) else {
            return Err("[Content_Types].xml has a <Default> without Extension or ContentType".into());
        };
        if defaults.insert(ext.to_ascii_lowercase(), d.attr("ContentType").unwrap_or("").to_string()).is_some() {
            return Err(format!("[Content_Types].xml declares the extension .{ext} twice"));
        }
    }
    let mut overrides: HashSet<String> = HashSet::new();
    for o in types.kids("Override") {
        let (Some(p), Some(_)) = (o.attr("PartName"), o.attr("ContentType")) else {
            return Err("[Content_Types].xml has an <Override> without PartName or ContentType".into());
        };
        let key = normalize(p);
        if !overrides.insert(key.clone()) {
            return Err(format!("[Content_Types].xml has two overrides for /{key}"));
        }
        if !parts.keys().any(|n| normalize(n) == key) {
            return Err(format!("[Content_Types].xml has a stale override for /{key}: the package has no such part"));
        }
    }

    // Content types, part-name uniqueness and the relationship graph.
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    let mut lower: HashMap<String, String> = HashMap::new();
    for name in parts.keys() {
        if let Some(other) = lower.insert(name.to_ascii_lowercase(), name.clone()) {
            return Err(format!("part names differ only in case: {other} and {name}"));
        }
        if name != "[Content_Types].xml" {
            let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
            let covered = overrides.contains(&normalize(name)) || ext.is_some_and(|e| defaults.contains_key(&e));
            if !covered {
                return Err(format!("{name} has no content type"));
            }
        }
        if !name.ends_with(".rels") {
            continue;
        }
        let source = rels_source(name)?;
        if let Some(src) = &source
            && !parts.keys().any(|n| normalize(n) == normalize(src))
        {
            return Err(format!("{name} is the relationships of {src}, which the package doesn't have"));
        }
        let root = xml::parse(parts.get(name).map(Vec::as_slice).unwrap_or_default()).map_err(|e| format!("{name}: {e}"))?;
        let dir = source.as_deref().map(|s| split_dir(s).0).unwrap_or("");
        let mut ids: HashSet<&str> = HashSet::new();
        let mut targets = vec![];
        for r in root.kids("Relationship") {
            let (Some(id), Some(target)) = (r.attr("Id"), r.attr("Target")) else {
                return Err(format!("{name}: a <Relationship> has no Id or no Target"));
            };
            if !ids.insert(id) {
                return Err(format!("{name}: the relationship id {id} is used twice"));
            }
            if r.attr("TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External")) {
                continue;
            }
            let t = resolve(dir, target);
            if !parts.keys().any(|n| normalize(n) == normalize(&t)) {
                return Err(format!("{name}: {id} points at {target} (={t}), which the package doesn't have"));
            }
            targets.push(t);
        }
        edges.insert(source.clone().unwrap_or_default(), targets);
    }

    // No part is orphaned: every one is reachable from the package root through relationships.
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = edges.get("").cloned().unwrap_or_default().into();
    while let Some(n) = queue.pop_front() {
        if !seen.insert(n.clone()) {
            continue;
        }
        if let Some(next) = edges.get(&n) {
            queue.extend(next.iter().cloned());
        }
    }
    for name in parts.keys() {
        if name == "[Content_Types].xml" {
            continue;
        }
        if !name.ends_with(".rels") {
            if !seen.iter().any(|n| normalize(n) == normalize(name)) {
                return Err(format!("{name} is orphaned: no relationship points at it"));
            }
            continue;
        }
        match rels_source(name)? {
            Some(src) if !seen.iter().any(|n| normalize(n) == normalize(&src)) => {
                return Err(format!("{name} is orphaned: its part {src} is not reachable from the package root"));
            }
            _ => {}
        }
    }
    Ok(())
}

/// The part a `.rels` part describes: `xl/_rels/sheet1.xml.rels` → `xl/worksheets/sheet1.xml`… seen
/// relative to its own folder; `_rels/.rels` → the package root (`None`).
fn rels_source(name: &str) -> Result<Option<String>, String> {
    use crate::package::split_dir;
    let (dir, file) = split_dir(name);
    let stem = file.strip_suffix(".rels").ok_or_else(|| format!("{name} is not a .rels part"))?;
    if stem.is_empty() {
        return Ok(None);
    }
    let Some(parent) = dir.strip_suffix("_rels").map(|p| p.strip_suffix('/').unwrap_or(p)) else {
        return Err(format!("{name} is not in a _rels folder"));
    };
    Ok(Some(if parent.is_empty() { stem.to_string() } else { format!("{parent}/{stem}") }))
}
