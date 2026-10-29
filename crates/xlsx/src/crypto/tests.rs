//! Decryption tests. The expected keys and IVs below were computed independently with
//! msoffcrypto-tool 6.0.0 (MIT) for the synthetic password and salts used here; the encrypted
//! files are built in code by `crate::testing`. No Excel-produced file is used.

use super::*;
use crate::testing::{agile_info, encrypt_agile, encrypt_agile_with_integrity, encrypt_standard, with_agile_header};

const PASSWORD: &str = "Pass1234";

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn salt() -> Vec<u8> {
    (0u8..16).collect()
}

#[test]
fn aes_decrypts_the_fips_197_vectors() {
    let mut b: [u8; 16] = hex("69c4e0d86a7b0430d8cdb78070b4c55a").try_into().unwrap();
    Aes::new(&hex("000102030405060708090a0b0c0d0e0f")).unwrap().decrypt_block(&mut b);
    assert_eq!(b.to_vec(), hex("00112233445566778899aabbccddeeff"));
    let mut b: [u8; 16] = hex("8ea2b7ca516745bfeafc49904b496089").try_into().unwrap();
    Aes::new(&hex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f")).unwrap().decrypt_block(&mut b);
    assert_eq!(b.to_vec(), hex("00112233445566778899aabbccddeeff"));
}

#[test]
fn agile_keys_match_msoffcrypto() {
    let h = iterated_hash(Hash::Sha512, PASSWORD, &salt(), 1000);
    assert_eq!(derive(Hash::Sha512, &h, &BLOCK_VERIFIER_INPUT, 32), hex("c57def04938e1e83497271f6ec099e890bfd2bd9cf05ff208f8b2405df361ae6"));
    assert_eq!(derive(Hash::Sha512, &h, &BLOCK_KEY, 32), hex("4ab08cf2ed95199d58d04f19331049a8a07cc1ff508431a1dd0785c84e261d03"));
    let h = iterated_hash(Hash::Sha1, PASSWORD, &salt(), 1000);
    assert_eq!(derive(Hash::Sha1, &h, &BLOCK_VERIFIER_INPUT, 16), hex("88c7567d028eae33a9a55f65729dcd2b"));
    let key_data = Params { hash: Hash::Sha512, salt: (16u8..32).collect(), key_len: 32 };
    assert_eq!(key_data.iv(Some(&1u32.to_le_bytes())), hex("da22ccba062aff577d207b2dee4e165b"));
}

#[test]
fn agile_pads_short_hashes_with_0x36() {
    // SHA-1 gives 20 bytes; an AES-256 key needs 32, so 12 bytes of 0x36 follow.
    let k = derive(Hash::Sha1, b"a", b"b", 32);
    assert_eq!(k[..20], Hash::Sha1.digest(&[b"ab"])[..]);
    assert_eq!(k[20..], [0x36; 12]);
}

#[test]
fn standard_keys_match_msoffcrypto() {
    assert_eq!(standard_key(PASSWORD, &salt(), 16), hex("ae0b402aeb23a779471a130843e431d4"));
    assert_eq!(standard_key(PASSWORD, &salt(), 32), hex("ae0b402aeb23a779471a130843e431d47b416314b5146b491cb815283663d26f"));
}

#[test]
fn base64_decodes_and_rejects_junk() {
    assert_eq!(base64("AAECAwQFBgcICQoLDA0ODw==").unwrap(), salt());
    assert_eq!(base64(" AAEC\nAw== ").unwrap(), vec![0, 1, 2, 3]);
    assert!(matches!(base64("AA*C"), Err(DecryptError::Malformed(_))));
}

/// A payload that isn't a whole number of segments or blocks.
fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + i / 251) as u8).collect()
}

#[test]
fn agile_round_trips() {
    let plain = payload(10_000);
    let (info, package) = encrypt_agile(&plain, PASSWORD, 1000);
    assert_eq!(decrypt_streams(&info, &package, PASSWORD).unwrap(), plain);
}

#[test]
fn standard_round_trips() {
    let plain = payload(10_000);
    let (info, package) = encrypt_standard(&plain, PASSWORD);
    assert_eq!(decrypt_streams(&info, &package, PASSWORD).unwrap(), plain);
}

#[test]
fn non_ascii_passwords_round_trip() {
    // UTF-16LE, surrogate pairs included: "Pässwörd€" plus an emoji outside the BMP.
    let pw = "P\u{e4}ssw\u{f6}rd\u{20ac}\u{1F511}";
    let (info, package) = encrypt_agile(&payload(100), pw, 1000);
    assert_eq!(decrypt_streams(&info, &package, pw).unwrap(), payload(100));
    assert_eq!(decrypt_streams(&info, &package, "Passwrd"), Err(DecryptError::WrongPassword));
    let (info, package) = encrypt_standard(&payload(100), pw);
    assert_eq!(decrypt_streams(&info, &package, pw).unwrap(), payload(100));
}

#[test]
fn a_wrong_password_is_reported_as_such() {
    let (info, package) = encrypt_agile(&payload(100), PASSWORD, 1000);
    assert_eq!(decrypt_streams(&info, &package, "pass1234"), Err(DecryptError::WrongPassword));
    assert_eq!(decrypt_streams(&info, &package, ""), Err(DecryptError::WrongPassword));
    let (info, package) = encrypt_standard(&payload(100), PASSWORD);
    assert_eq!(decrypt_streams(&info, &package, "Pass12345"), Err(DecryptError::WrongPassword));
}

#[test]
fn unsupported_schemes_are_named() {
    let unsupported = |info: &[u8]| match decrypt_streams(info, &[0; 8], PASSWORD) {
        Err(DecryptError::Unsupported(s)) => s,
        other => panic!("expected Unsupported, got {other:?}"),
    };
    let (mut rc4, _) = encrypt_standard(&payload(10), PASSWORD);
    rc4[4..8].copy_from_slice(&F_CRYPTOAPI.to_le_bytes()); // fCryptoAPI without fAES
    assert_eq!(unsupported(&rc4), "RC4");
    assert_eq!(unsupported(&[3, 0, 3, 0, 0, 0, 0, 0]), "Extensible");
    assert_eq!(unsupported(&[1, 0, 1, 0, 0, 0, 0, 0]), "version 1.1");
    let des = agile_info(PASSWORD, 1000).replace("cipherAlgorithm=\"AES\"", "cipherAlgorithm=\"3DES\"");
    assert_eq!(unsupported(&with_agile_header(&des)), "3DES");
    let cert = agile_info(PASSWORD, 1000).replace("keyEncryptor/password", "keyEncryptor/certificate");
    assert_eq!(unsupported(&with_agile_header(&cert)), "certificate");
}

#[test]
fn damaged_files_are_malformed_not_panics() {
    let is_malformed = |r: Result<Vec<u8>, DecryptError>| matches!(r, Err(DecryptError::Malformed(_)));
    let plain = payload(5000);
    for (info, package) in [encrypt_agile(&plain, PASSWORD, 1000), encrypt_standard(&plain, PASSWORD)] {
        // Every truncation of either stream.
        for cut in (0..info.len()).step_by(7) {
            let _ = decrypt_streams(&info[..cut], &package, PASSWORD);
        }
        for cut in (0..package.len()).step_by(97) {
            let _ = decrypt_streams(&info, &package[..cut], PASSWORD);
        }
        // A stated size past the ciphertext never drives an allocation.
        let mut huge = package.clone();
        huge[..8].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(is_malformed(decrypt_streams(&info, &huge, PASSWORD)));
        // Ciphertext that isn't whole AES blocks.
        let mut ragged = package.clone();
        ragged.truncate(ragged.len() - 3);
        let len = (ragged.len() - 8) as u64;
        ragged[..8].copy_from_slice(&len.to_le_bytes());
        assert!(is_malformed(decrypt_streams(&info, &ragged, PASSWORD)));
    }
    let xml = agile_info(PASSWORD, 1000);
    let bad = |s: String| is_malformed(decrypt_streams(&with_agile_header(&s), &[0; 8], PASSWORD));
    assert!(bad(xml.replace("spinCount=\"1000\"", "spinCount=\"4000000000\"")));
    assert!(bad(xml.replace("saltValue=\"", "saltValue=\"*")));
    assert!(bad(xml.replace("keyBits=\"256\"", "keyBits=\"100\"")));
    assert!(bad(xml.replace("blockSize=\"16\"", "blockSize=\"8\"")));
    assert!(bad(xml.replace("<keyData", "<other")));
    assert!(bad("<encryption".into()));
}

#[test]
fn non_ascii_passwords_hash_as_utf16le_like_msoffcrypto() {
    // "Pässwörd€" plus an emoji outside the BMP, a surrogate pair in UTF-16.
    let h = iterated_hash(Hash::Sha512, "P\u{e4}ssw\u{f6}rd\u{20ac}\u{1F511}", &salt(), 1000);
    assert_eq!(
        h,
        hex("2bca96ca0a7e8d88d9314a2eb1819a1ece94ff9ce34266ff82f891a88c4240c4c9f39c729b7c5c410c27fc8e2b7ad90d8c005eb68a603377b8a4ab858041c76a")
    );
}

// ---- writing -------------------------------------------------------------------------------

/// Faster than Excel's 100,000 rounds; the algorithm is the same.
fn encrypt(package: &[u8], password: &str) -> Vec<u8> {
    encrypt_workbook_with_spin(package, password, 1000).unwrap()
}

#[test]
fn hmac_sha512_matches_rfc_4231() {
    // RFC 4231 test cases 1, 2 and 6 (case 2's key is the 4-byte "Jefe").
    assert_eq!(
        hmac_sha512(&[0x0b; 20], b"Hi There"),
        hex("87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cdedaa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854")
    );
    assert_eq!(
        hmac_sha512(b"Jefe", b"what do ya want for nothing?"),
        hex("164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea2505549758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737")
    );
    let long_key = [0xaa; 131];
    assert_eq!(
        hmac_sha512(&long_key, b"Test Using Larger Than Block-Size Key - Hash Key First"),
        hex("80b24263c7c1a3ebb71493c1dd7be8b49b46d1f41b4aeec1121b013783f8f3526b56d037e05f2598bd0fd2215d6a1e5295e64f73f63f0aec8b915a985d786598")
    );
}

#[test]
fn agile_integrity_ivs_and_block_keys_match_the_spec() {
    // [MS-OFFCRYPTO] 2.3.4.14: the data-integrity fields have two block keys of their own, and
    // each field's IV is H(keyData salt ‖ block key) cut to one AES block. The reference values
    // come from Python's hashlib (an independent SHA-512), reproducing msoffcrypto-tool 6.0.0's
    // `verify_integrity`, which cites the same section: a wrong block key or a shared IV shows up
    // here as a wrong 16 bytes, not as a file Excel silently refuses.
    let key_data = Params { hash: Hash::Sha512, salt: (16u8..32).collect(), key_len: 32 };
    assert_eq!(key_data.iv(Some(&BLOCK_HMAC_KEY)), hex("7d786688805c5934859055665c8e1a80"));
    assert_eq!(key_data.iv(Some(&BLOCK_HMAC_VALUE)), hex("21c5bb2dfaab23710918879eafcd6b11"));
    assert_eq!(BLOCK_HMAC_KEY, [0x5F, 0xB2, 0xAD, 0x01, 0x0C, 0xB9, 0xE1, 0xF6]);
    assert_eq!(BLOCK_HMAC_VALUE, [0xA0, 0x67, 0x7F, 0x02, 0xB2, 0x2C, 0x84, 0x33]);
}

#[test]
fn hmac_covers_the_shorter_hashes_a_foreign_file_may_name() {
    // RFC 4231 test case 1 for SHA-256 and RFC 2202's for SHA-1: a file written by another tool
    // can name either for its data integrity, and the check must still be the real HMAC.
    assert_eq!(hmac(Hash::Sha256, &[0x0b; 20], b"Hi There"), hex("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"));
    assert_eq!(hmac(Hash::Sha1, &[0x0b; 20], b"Hi There"), hex("b617318655057264e28bc0b6fb378c8ef146be00"));
    assert_eq!(hmac(Hash::Sha384, &[0x0b; 20], b"Hi There").len(), 48);
}

#[test]
fn agile_encryption_round_trips_at_every_size() {
    for len in [0usize, 1, 15, 16, 17, 4095, 4096, 4097, 8192, 10_000] {
        let plain = payload(len);
        let file = encrypt(&plain, PASSWORD);
        assert!(sniff_encrypted(&file), "a {len}-byte package");
        assert_eq!(decrypt(&file, PASSWORD).unwrap(), plain, "a {len}-byte package");
        assert_eq!(decrypt(&file, "wrong"), Err(DecryptError::WrongPassword));
        assert_eq!(check_encryption(&file), Ok(()));
    }
}

/// Whether the compound file names the encryption streams (the reader's own test).
fn sniff_encrypted(bytes: &[u8]) -> bool {
    crate::cfb::is_encrypted(bytes)
}

#[test]
fn every_save_gets_fresh_salts_and_a_fresh_key() {
    let plain = payload(100);
    let a = encrypt(&plain, PASSWORD);
    let b = encrypt(&plain, PASSWORD);
    assert_ne!(a, b, "two saves of one package are identical");
    assert_eq!(decrypt(&a, PASSWORD).unwrap(), plain);
    assert_eq!(decrypt(&b, PASSWORD).unwrap(), plain);
    // The two `EncryptionInfo` descriptors differ in every salt and every encrypted value.
    let (info_a, _) = streams(&a).unwrap();
    let (info_b, _) = streams(&b).unwrap();
    assert_ne!(info_a, info_b);
}

#[test]
fn a_saved_workbook_carries_a_data_integrity_hmac_that_matches() {
    let plain = payload(9000);
    let file = encrypt(&plain, PASSWORD);
    let (info, package) = streams(&file).unwrap();
    let d = agile_descriptor(info.get(8..).unwrap()).unwrap();
    // The password key encryptor's own fields: a key derived from the password, the salt as IV.
    let h = iterated_hash(d.key.hash, PASSWORD, &d.key.salt, d.spin);
    let mut iv = d.key.salt.clone();
    iv.resize(16, 0x36);
    let password_field = |data: &[u8], block_key: &[u8]| {
        let mut data = data.to_vec();
        Aes::new(&derive(d.key.hash, &h, block_key, 32)).unwrap().decrypt_cbc(&iv, &mut data).unwrap();
        data
    };
    let secret = password_field(&d.key_value, &BLOCK_KEY);
    // The data-integrity fields: encrypted with the package key, each under its own block key's
    // IV, derived from the keyData salt ([MS-OFFCRYPTO] 2.3.4.14).
    let aes = Aes::new(secret.get(..32).unwrap()).unwrap();
    let integrity_field = |data: &[u8], block_key: &[u8]| {
        let mut data = data.to_vec();
        aes.decrypt_cbc(&d.key_data.iv(Some(block_key)), &mut data).unwrap();
        data
    };
    let (encrypted_key, encrypted_value) = d.integrity.clone().unwrap();
    let hmac_key = integrity_field(&encrypted_key, &BLOCK_HMAC_KEY);
    let hmac_value = integrity_field(&encrypted_value, &BLOCK_HMAC_VALUE);
    assert_eq!(hmac_key.len(), 64);
    assert_eq!(&hmac_value[..64], &hmac_sha512(&hmac_key, &plain)[..]);
    // ...and the package stream is the plaintext's length followed by whole AES blocks: every
    // 4096-byte segment is a multiple of 16 already, so only the last is padded.
    let last = plain.len() % 4096;
    let body = plain.len() - last + last.div_ceil(16) * 16;
    assert_eq!(package.len(), 8 + body);
    assert_eq!(u64::from_le_bytes(package[..8].try_into().unwrap()), plain.len() as u64);
}

#[test]
fn passwords_of_every_shape_round_trip() {
    let plain = payload(200);
    // An empty password and a very long one (a 100 kB password must not slow a save down: it is
    // hashed once, then the spin count walks the hash).
    for pw in ["", "a", &"ü".repeat(50_000)] {
        let file = encrypt(&plain, pw);
        assert_eq!(decrypt(&file, pw).unwrap(), plain, "a {}-byte password", pw.len());
        assert_eq!(decrypt(&file, "x"), Err(DecryptError::WrongPassword));
        assert_eq!(decrypt(&file, ""), if pw.is_empty() { Ok(plain.clone()) } else { Err(DecryptError::WrongPassword) });
    }
}

#[test]
fn a_truncated_or_mangled_encrypted_workbook_is_an_error() {
    let plain = payload(8000);
    let good = encrypt(&plain, PASSWORD);
    for cut in (0..good.len()).step_by(37) {
        // A cut inside the final sector's padding still reads: the streams are complete. Any
        // other cut is an error, never a panic and never a short package passed off as whole.
        if let Ok(out) = decrypt(&good[..cut], PASSWORD) {
            assert_eq!(out, plain, "{cut} of {}", good.len());
        }
        let _ = check_encryption(&good[..cut]);
    }
    // Byte mutations anywhere: never a panic, never a hang. Bytes outside the streams (header
    // padding, a sector's slack after the last stream byte) can still decrypt to the package,
    // which is correct; a change inside the ciphertext must not.
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    for _ in 0..400 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let at = (seed as usize) % good.len();
        let mut b = good.clone();
        b[at] ^= 0xFF;
        let _ = decrypt(&b, PASSWORD);
    }
}

/// `file` with its `EncryptionInfo` XML edited, keeping the same package stream.
fn with_edited_info(file: &[u8], edit: impl Fn(&str) -> String) -> Vec<u8> {
    let (info, package) = streams(file).unwrap();
    let xml = std::str::from_utf8(info.get(8..).unwrap()).unwrap();
    let edited = with_agile_header(&edit(xml));
    crate::cfb::write_compound(&[("EncryptionInfo", &edited), ("EncryptedPackage", &package)]).unwrap()
}

/// `xml` with the first base64 character of `name` replaced by another one: still valid base64,
/// so the file parses and decrypts to different bytes.
fn alter_attr(xml: &str, name: &str) -> String {
    let at = xml.find(&format!("{name}=\"")).unwrap() + name.len() + 2;
    let mut b = xml.as_bytes().to_vec();
    b[at] = if b[at] == b'A' { b'B' } else { b'A' };
    String::from_utf8(b).unwrap()
}

#[test]
fn a_flipped_ciphertext_byte_is_corrupted_not_a_silently_decrypted_file() {
    let plain = payload(9000);
    let file = encrypt(&plain, PASSWORD);
    let (info, package) = streams(&file).unwrap();
    // The first segment, a later segment and the final block all cover real package bytes.
    for at in [8usize, 8 + 100, 8 + SEGMENT + 5, package.len() - 1] {
        let mut stream = package.clone();
        stream[at] ^= 0x01;
        let flipped = crate::cfb::write_compound(&[("EncryptionInfo", &info), ("EncryptedPackage", &stream)]).unwrap();
        assert_eq!(decrypt(&flipped, PASSWORD), Err(DecryptError::Corrupted), "a flipped byte at {at}");
    }
    // The size prefix: one byte less promises a shorter package, whose HMAC cannot match either.
    let mut stream = package.clone();
    stream[0] = stream[0].wrapping_sub(1);
    let flipped = crate::cfb::write_compound(&[("EncryptionInfo", &info), ("EncryptedPackage", &stream)]).unwrap();
    assert_eq!(decrypt(&flipped, PASSWORD), Err(DecryptError::Corrupted));
}

#[test]
fn a_tampered_integrity_or_key_field_is_corrupted() {
    let plain = payload(9000);
    let file = encrypt(&plain, PASSWORD);
    // The integrity key and value are the check itself: changing either must fail it.
    for name in ["encryptedHmacValue", "encryptedHmacKey"] {
        let tampered = with_edited_info(&file, |xml| alter_attr(xml, name));
        assert_eq!(decrypt(&tampered, PASSWORD), Err(DecryptError::Corrupted), "{name}");
    }
    // The encrypted package key is the rest of the chain: a different key decrypts the package to
    // nonsense, and the HMAC refuses it rather than handing the nonsense back as the workbook.
    let tampered = with_edited_info(&file, |xml| alter_attr(xml, "encryptedKeyValue"));
    assert_eq!(decrypt(&tampered, PASSWORD), Err(DecryptError::Corrupted));
    // The verifier can only say "wrong password" when its own fields are changed — which is why
    // the HMAC, not the verifier, is what proves the data.
    let tampered = with_edited_info(&file, |xml| alter_attr(xml, "encryptedVerifierHashValue"));
    assert_eq!(decrypt(&tampered, PASSWORD), Err(DecryptError::WrongPassword));
}

#[test]
fn a_package_shorter_than_its_stated_size_is_damage() {
    let plain = payload(9000);
    let file = encrypt(&plain, PASSWORD);
    let (info, package) = streams(&file).unwrap();
    // Two ciphertext blocks short of the 9000 bytes the stream promises: the loop cannot fill the
    // package, which is damage, not a 8968-byte workbook.
    let mut stream = package.clone();
    stream.truncate(package.len() - 32);
    let short = crate::cfb::write_compound(&[("EncryptionInfo", &info), ("EncryptedPackage", &stream)]).unwrap();
    assert!(matches!(decrypt(&short, PASSWORD), Err(DecryptError::Malformed(_))));
}

#[test]
fn a_descriptor_without_data_integrity_still_decrypts() {
    // Not every writer writes `dataIntegrity`; those files have no check to make, and refusing
    // them would lock readers out of their own workbooks.
    let plain = payload(5000);
    let (info, package) = encrypt_agile(&plain, PASSWORD, 1000);
    assert!(!std::str::from_utf8(info.get(8..).unwrap()).unwrap().contains("dataIntegrity"));
    assert_eq!(decrypt_streams(&info, &package, PASSWORD).unwrap(), plain);
}

#[test]
fn a_data_integrity_file_written_by_another_implementation_reads() {
    // The descriptor and the package come from the test support module: fixed keys and salts,
    // its own AES and its own XML, the HMAC fields encrypted with the package key under each
    // field's own block key. A reader that swaps the two block keys, derives those fields from
    // the password, or skips the check cannot pass this.
    let plain = payload(9000);
    let (info, package) = encrypt_agile_with_integrity(&plain, PASSWORD, 1000);
    assert!(std::str::from_utf8(&info).unwrap().contains("dataIntegrity"));
    assert_eq!(decrypt_streams(&info, &package, PASSWORD).unwrap(), plain);
    // One wrong byte in the package, and one in the HMAC value: both refused as corruption.
    let mut stream = package.clone();
    stream[8 + 500] ^= 0x01;
    assert_eq!(decrypt_streams(&info, &stream, PASSWORD), Err(DecryptError::Corrupted));
    let xml = String::from_utf8(info[8..].to_vec()).unwrap();
    let tampered = with_agile_header(&alter_attr(&xml, "encryptedHmacValue"));
    assert_eq!(decrypt_streams(&tampered, &package, PASSWORD), Err(DecryptError::Corrupted));
    // And a wrong password is still its own failure, not corruption.
    assert_eq!(decrypt_streams(&info, &package, "wrong"), Err(DecryptError::WrongPassword));
}

// ---- the container, through a third-party reader --------------------------------------------

/// The two streams as an independent CFB reader sees them, with the container's version. The
/// `cfb` crate (MIT, mdsteele/rust-cfb) shares no code with this crate: it parses the header,
/// the FAT, the DIFAT, the mini stream and the directory tree itself, and rejects a directory
/// whose name order is wrong while doing it.
///
/// Its `permissive` mode is the fair oracle, not `strict`: strict additionally demands that the
/// mini FAT hold no more entries than the mini stream has 64-byte sectors, but a mini FAT lives
/// in whole 512-byte sectors (128 entries) so its last sector is padding whenever the mini stream
/// is smaller — the usual case.
fn independently_read(bytes: &[u8]) -> (cfb::Version, Vec<(String, Vec<u8>)>) {
    use std::io::Read as _;
    let mut cf = cfb::CompoundFile::open(std::io::Cursor::new(bytes)).expect("the independent CFB reader rejects our container");
    let version = cf.version();
    let mut streams = vec![];
    for name in ["EncryptionInfo", "EncryptedPackage"] {
        let stated = cf.entry(name).unwrap_or_else(|e| panic!("the independent reader has no {name}: {e}")).len();
        let mut stream = cf.open_stream(name).unwrap_or_else(|e| panic!("the independent reader can't open {name}: {e}"));
        let mut data = vec![];
        stream.read_to_end(&mut data).unwrap_or_else(|e| panic!("the independent reader can't read {name}: {e}"));
        assert_eq!(data.len() as u64, stated, "{name}: the directory states {stated} bytes");
        streams.push((name.to_string(), data));
    }
    (version, streams)
}

#[test]
fn an_independent_reader_sees_a_valid_encrypted_container() {
    let plain = payload(9000);
    let file = encrypt(&plain, PASSWORD);
    // Header: the compound-file signature, version 3 and the 512-byte sector shift Excel writes.
    assert_eq!(file.get(..8), Some(&crate::cfb::SIGNATURE[..]));
    assert_eq!(u16::from_le_bytes([file[26], file[27]]), 3);
    assert_eq!(u16::from_le_bytes([file[28], file[29]]), 0xFFFE);
    assert_eq!(u16::from_le_bytes([file[30], file[31]]), 9);
    let (version, outside) = independently_read(&file);
    assert_eq!(version, cfb::Version::V3);
    // Exactly the two streams, holding exactly what our own reader reads from them: one in the
    // mini stream, one in regular sectors.
    let (info, package) = streams(&file).unwrap();
    assert_eq!(outside, vec![("EncryptionInfo".to_string(), info), ("EncryptedPackage".to_string(), package)]);
    assert!(outside[0].1.len() < 4096, "EncryptionInfo should live in the mini stream");
    assert!(outside[1].1.len() > 4096, "EncryptedPackage should live in regular sectors");
}

#[test]
fn an_independent_reader_sees_a_valid_standard_container() {
    // The same container, holding Standard streams: version 3, both streams, a small
    // `EncryptionInfo` in the mini stream and the package in regular sectors.
    let plain = payload(9000);
    let file = encrypt_standard_workbook(&plain, PASSWORD);
    let (version, outside) = independently_read(&file);
    assert_eq!(version, cfb::Version::V3);
    let (info, package) = streams(&file).unwrap();
    assert_eq!(outside, vec![("EncryptionInfo".to_string(), info.clone()), ("EncryptedPackage".to_string(), package.clone())]);
    assert!(info.len() < 4096, "EncryptionInfo should live in the mini stream");
    // And what the outside reader hands back still decrypts, and is the package.
    assert_eq!(decrypt_streams(&outside[0].1, &outside[1].1, PASSWORD).unwrap(), plain);
}

#[test]
fn an_independent_reader_follows_the_difat_of_a_large_encrypted_package() {
    // Past the 109 FAT sector ids the header holds, the FAT's own directory moves into DIFAT
    // sectors: a package of 8 MB needs them, and only an outside reader proves they are right.
    let plain = payload(8_000_000);
    let file = encrypt_workbook_with_spin(&plain, PASSWORD, 1000).unwrap();
    let fat_sectors = u32::from_le_bytes([file[44], file[45], file[46], file[47]]);
    let difat_sectors = u32::from_le_bytes([file[72], file[73], file[74], file[75]]);
    assert!(fat_sectors > 109, "{fat_sectors} FAT sectors");
    assert!(difat_sectors > 0, "the header names no DIFAT sector");
    let (version, outside) = independently_read(&file);
    assert_eq!(version, cfb::Version::V3);
    let (_, package) = streams(&file).unwrap();
    assert_eq!(outside[1].1, package);
    assert_eq!(decrypt(&file, PASSWORD).unwrap(), plain);
}

#[test]
fn a_saved_encrypted_workbook_holds_a_package_we_would_accept() {
    use gridcraft_core::{CellRef, Value};
    let mut wb = gridcraft_model::Workbook::new();
    wb.sheet_mut(0).unwrap().set_value(CellRef::parse("A1").unwrap(), Value::Number(42.0));
    let xlsx = crate::write_xlsx(&wb).unwrap();
    let file = encrypt_workbook_with_spin(&xlsx, PASSWORD, 1000).unwrap();
    // An outside reader unbundles the package, and decrypting what it read gives the zip back.
    let (_, outside) = independently_read(&file);
    let plain = decrypt_streams(&outside[0].1, &outside[1].1, PASSWORD).unwrap();
    assert_eq!(plain, xlsx);
    // The plaintext we encrypt is a package this crate accepts: the content-types part is there
    // and the strict validator passes on every part and relationship.
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&plain)).unwrap();
    let names: Vec<String> = (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_string()).collect();
    assert!(names.iter().any(|n| n == "[Content_Types].xml"), "{names:?}");
    crate::testing::check_package(&plain).unwrap();
}

// ---- writing: Standard encryption ---------------------------------------------------------

/// A Standard-encrypted workbook as the real writer produces it (fresh salt and verifier, not the
/// fixed-key `testing::encrypt_standard` helper the reader's tests use).
fn encrypt_standard_workbook(package: &[u8], password: &str) -> Vec<u8> {
    encrypt_workbook_as(package, password, EncryptionScheme::Standard).unwrap()
}

#[test]
fn the_scheme_names_are_the_two_we_write() {
    assert_eq!(EncryptionScheme::NAMES, ["agile", "standard"]);
    assert_eq!(EncryptionScheme::default(), EncryptionScheme::Agile);
    for scheme in [EncryptionScheme::Agile, EncryptionScheme::Standard] {
        assert_eq!(EncryptionScheme::from_name(scheme.name()), Some(scheme));
    }
    assert_eq!(EncryptionScheme::from_name("AGILE"), Some(EncryptionScheme::Agile));
    assert_eq!(EncryptionScheme::from_name("Standard"), Some(EncryptionScheme::Standard));
    // No trimming, no invented aliases: a name that isn't one of the two matches nothing.
    for unknown in ["", " agile", "rc4", "AES", "aes-256", "agile2"] {
        assert_eq!(EncryptionScheme::from_name(unknown), None, "{unknown:?}");
    }
}

#[test]
fn standard_encryption_round_trips_at_every_size() {
    // 0 to 100 kB, including the AES block boundaries and the 4096-byte ones a package hits.
    // Standard encrypts the whole stream in one ECB pass, so a segment boundary is nothing
    // special — the sizes are here because an off-by-one in the padding would show at each.
    for len in [0usize, 1, 15, 16, 4095, 4096, 4097, 100 * 1024] {
        let plain = payload(len);
        let file = encrypt_standard_workbook(&plain, PASSWORD);
        assert!(sniff_encrypted(&file), "a {len}-byte package");
        assert_eq!(encryption_scheme(&file), Some(EncryptionScheme::Standard), "a {len}-byte package");
        assert_eq!(check_encryption(&file), Ok(()), "a {len}-byte package");
        assert_eq!(decrypt(&file, PASSWORD).unwrap(), plain, "a {len}-byte package");
        assert_eq!(decrypt(&file, "wrong"), Err(DecryptError::WrongPassword), "a {len}-byte package");
    }
}

#[test]
fn every_standard_save_gets_a_fresh_salt_and_verifier() {
    let plain = payload(100);
    let a = encrypt_standard_workbook(&plain, PASSWORD);
    let b = encrypt_standard_workbook(&plain, PASSWORD);
    assert_ne!(a, b, "two saves of one package are identical");
    let (info_a, _) = streams(&a).unwrap();
    let (info_b, _) = streams(&b).unwrap();
    assert_eq!(info_a.len(), info_b.len(), "the header is a fixed size");
    assert_ne!(info_a.get(12..).map(<[u8]>::to_vec), info_b.get(12..).map(<[u8]>::to_vec), "two saves share their salt or verifier");
    assert_eq!(decrypt(&a, PASSWORD).unwrap(), plain);
    assert_eq!(decrypt(&b, PASSWORD).unwrap(), plain);
}

#[test]
fn a_saved_standard_header_is_the_one_the_spec_describes() {
    // [MS-OFFCRYPTO] 2.3.2: version 4.2, fCryptoAPI|fAES, the header, the verifier, and nothing
    // else in `EncryptionInfo`.
    let file = encrypt_standard_workbook(&payload(40), PASSWORD);
    let (info, package) = streams(&file).unwrap();
    assert_eq!(info.get(..4), Some(&[4u8, 0, 2, 0][..]), "version 4.2");
    assert_eq!(u32_at(&info, 4), Some(F_CRYPTOAPI | F_AES), "fCryptoAPI|fAES");
    let header_size = u32_at(&info, 8).unwrap() as usize;
    let header = info.get(12..12 + header_size).unwrap();
    assert_eq!(u32_at(header, 0), Some(F_CRYPTOAPI | F_AES));
    assert_eq!(u32_at(header, 4), Some(0), "SizeExtra");
    assert_eq!(u32_at(header, 8), Some(CALG_AES_128), "AlgID");
    assert_eq!(u32_at(header, 12), Some(CALG_SHA1), "AlgIDHash");
    assert_eq!(u32_at(header, 16), Some(128), "KeySize");
    assert_eq!(u32_at(header, 20), Some(PROV_RSA_AES), "ProviderType");
    assert_eq!(u32_at(header, 24), Some(0), "Reserved1");
    assert_eq!(u32_at(header, 28), Some(0), "Reserved2");
    let name = header.get(32..).unwrap();
    let utf16: Vec<u16> = name.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    assert_eq!(utf16.last(), Some(&0), "CSPName is NUL-terminated");
    assert_eq!(String::from_utf16(&utf16[..utf16.len() - 1]).as_deref().unwrap_or(""), STANDARD_CSP_NAME);
    // The verifier (2.3.2.4): 16-byte salt, 16-byte encrypted verifier, hash size 20, 32-byte
    // encrypted hash — and nothing after it.
    let v = info.get(12 + header_size..).unwrap();
    assert_eq!(v.len(), 72);
    assert_eq!(u32_at(v, 0), Some(16), "SaltSize");
    let salt = v.get(4..20).unwrap();
    assert_eq!(u32_at(v, 36), Some(20), "VerifierHashSize");
    // The package (2.3.2.5): the plaintext size, then one AES block per 16 plaintext bytes.
    assert_eq!(u64::from_le_bytes(package.get(..8).and_then(|b| b.try_into().ok()).unwrap()), 40);
    assert_eq!(package.len(), 8 + 48);
    // Two saves never share the salt.
    let (again, _) = streams(&encrypt_standard_workbook(&payload(40), PASSWORD)).unwrap();
    assert_ne!(salt, again.get(12 + header_size + 4..12 + header_size + 20).unwrap());
}

#[test]
fn standard_encryption_has_no_integrity_field_so_only_the_verifier_notices_damage() {
    // ECMA-376 Part 1 §4.1 / [MS-OFFCRYPTO] 2.3.2 defines no data-integrity value: unlike Agile's
    // `dataIntegrity` HMAC (2.3.4.14), the password verifier is the scheme's only check. This test
    // pins both halves of that: what the verifier catches, and what nothing in the scheme can.
    let plain = payload(4096 + 100);
    let file = encrypt_standard_workbook(&plain, PASSWORD);
    let (info, package) = streams(&file).unwrap();
    let header_size = u32_at(&info, 8).unwrap() as usize;
    let verifier_at = 12 + header_size;
    let rebuild = |info: &[u8], package: &[u8]| crate::cfb::write_compound(&[("EncryptionInfo", info), ("EncryptedPackage", package)]).unwrap();
    // A changed byte in the salt, the encrypted verifier or the encrypted verifier hash makes the
    // verifier fail what the file says, so the file reads as a wrong password — the only verdict
    // the scheme can give.
    for at in [verifier_at + 4, verifier_at + 20, verifier_at + 40] {
        let mut edited = info.clone();
        edited[at] ^= 0x01;
        assert_eq!(decrypt(&rebuild(&edited, &package), PASSWORD), Err(DecryptError::WrongPassword), "a changed byte at {at}");
    }
    // A flipped ciphertext byte is *not* detectable here: the package decrypts, with exactly one
    // 16-byte block changed (ECB, so no other block moves). The damage is found by the package's
    // own zip CRCs after decryption, never by the encryption layer.
    let at = 8 + 32;
    let mut stream = package.clone();
    stream[at] ^= 0xFF;
    let flipped = rebuild(&info, &stream);
    let out = decrypt(&flipped, PASSWORD).expect("Standard has no check over the package");
    let block = at - 8;
    assert_eq!(out.len(), plain.len());
    assert_eq!(out.get(..block), plain.get(..block));
    assert_ne!(out.get(block..block + 16), plain.get(block..block + 16));
    assert_eq!(out.get(block + 16..), plain.get(block + 16..), "ECB leaves every other block alone");
    // Which is why the engine hands it to the zip reader, and the zip reader refuses it.
    assert!(crate::read_xlsx(&out).is_err());
}

#[test]
fn the_standard_derivation_agrees_with_a_second_spelling_of_the_spec() {
    // [MS-OFFCRYPTO] 2.3.4.7 / ECMA-376 Part 1 §4.1, spelled out again here from the salted hash:
    // H0 = SHA1(salt ‖ UTF-16LE(password)), Hn = SHA1(LE32(n-1) ‖ Hn-1) 50,000 times,
    // Hblock = SHA1(H50000 ‖ LE32(0)), then SHA1((Hblock ⊕ 0x36…) in 64 bytes) and the 0x5C one,
    // concatenated and cut to the key size. A change in `iterated_hash` or `standard_key` has to be
    // wrong in two places to pass.
    let salt = salt();
    let password: Vec<u8> = PASSWORD.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut h = Hash::Sha1.digest(&[&salt, &password]);
    for i in 0..50_000u32 {
        h = Hash::Sha1.digest(&[&i.to_le_bytes(), &h]);
    }
    let block = Hash::Sha1.digest(&[&h, &0u32.to_le_bytes()]);
    let expand = |pad: u8| {
        let mut buf = [pad; 64];
        for (b, x) in buf.iter_mut().zip(&block) {
            *b ^= x;
        }
        Hash::Sha1.digest(&[&buf])
    };
    let mut key = expand(0x36);
    key.extend(expand(0x5C));
    key.truncate(STANDARD_KEY_LEN);
    assert_eq!(standard_key(PASSWORD, &salt, STANDARD_KEY_LEN), key);
    // And the same bytes the reader's msoffcrypto-derived vector already pins.
    assert_eq!(key, hex("ae0b402aeb23a779471a130843e431d4"));
}

#[test]
fn the_scheme_of_a_written_file_can_be_read_back() {
    let plain = payload(64);
    let agile = encrypt_workbook_as(&plain, PASSWORD, EncryptionScheme::Agile).unwrap();
    assert_eq!(encryption_scheme(&agile), Some(EncryptionScheme::Agile));
    let standard = encrypt_standard_workbook(&plain, PASSWORD);
    assert_eq!(encryption_scheme(&standard), Some(EncryptionScheme::Standard));
    assert_eq!(encryption_scheme(b"not a compound file"), None);
    assert_eq!(encryption_scheme(b""), None);
    // An RC4 Standard file is not a scheme we can write: it has no name here.
    let (mut rc4, _) = crate::testing::encrypt_standard(&payload(10), PASSWORD);
    rc4.get_mut(4..8).unwrap().copy_from_slice(&F_CRYPTOAPI.to_le_bytes());
    let rc4_file = crate::cfb::write_compound(&[("EncryptionInfo", &rc4), ("EncryptedPackage", &[0u8; 8])]).unwrap();
    assert_eq!(encryption_scheme(&rc4_file), None);
}

#[test]
fn damaged_standard_files_are_errors_not_panics() {
    let plain = payload(5000);
    let file = encrypt_standard_workbook(&plain, PASSWORD);
    let (info, package) = streams(&file).unwrap();
    for cut in (0..file.len()).step_by(53) {
        let _ = decrypt(&file[..cut], PASSWORD);
        let _ = encryption_scheme(&file[..cut]);
    }
    for cut in (0..info.len()).step_by(3) {
        let _ = decrypt_streams(&info[..cut], &package, PASSWORD);
    }
    for cut in (0..package.len()).step_by(29) {
        let _ = decrypt_streams(&info, &package[..cut], PASSWORD);
    }
    // A stated size past the ciphertext, and a ragged ciphertext: both damage, never allocation.
    let mut huge = package.clone();
    huge.get_mut(..8).unwrap().copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(matches!(decrypt_streams(&info, &huge, PASSWORD), Err(DecryptError::Malformed(_))));
    let mut ragged = package.clone();
    ragged.truncate(ragged.len() - 3);
    assert!(matches!(decrypt_streams(&info, &ragged, PASSWORD), Err(DecryptError::Malformed(_))));
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn a_build_without_a_csprng_refuses_both_schemes() {
    // The web build has no secure random source, so neither scheme may write a file there. The
    // seam makes the native source fail exactly as the web build's does; the refusal and its
    // message are the ones the wasm branch returns, shared by both schemes.
    force_no_entropy(true);
    for scheme in [EncryptionScheme::Agile, EncryptionScheme::Standard] {
        match encrypt_workbook_as(&payload(10), PASSWORD, scheme) {
            Err(IoError::NoEntropy(message)) => assert_eq!(message, NO_ENTROPY, "{scheme:?}"),
            other => panic!("{scheme:?} wrote {} bytes without a CSPRNG", other.map(|b| b.len()).unwrap_or(0)),
        }
    }
    force_no_entropy(false);
    // With the source back, a save works again: the seam is a switch, not damage.
    assert!(encrypt_workbook_as(&payload(10), PASSWORD, EncryptionScheme::Standard).is_ok());
    assert!(encrypt_workbook_as(&payload(10), PASSWORD, EncryptionScheme::Agile).is_ok());
}
