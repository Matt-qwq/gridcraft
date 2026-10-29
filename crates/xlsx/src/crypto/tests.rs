//! Decryption tests. The expected keys and IVs below were computed independently with
//! msoffcrypto-tool 6.0.0 (MIT) for the synthetic password and salts used here; the encrypted
//! files are built in code by `crate::testing`. No Excel-produced file is used.

use super::*;
use crate::testing::{agile_info, encrypt_agile, encrypt_standard, with_agile_header};

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
    let h = iterated_hash(d.key.hash, PASSWORD, &d.key.salt, d.spin);
    let mut iv = d.key.salt.clone();
    iv.resize(16, 0x36);
    let field = |data: &[u8]| {
        let key = derive(d.key.hash, &h, &BLOCK_HMAC, 32);
        let mut data = data.to_vec();
        Aes::new(&key).unwrap().decrypt_cbc(&iv, &mut data).unwrap();
        data
    };
    // The descriptor's XML names the two HMAC values; read them the way Excel's writer put them.
    let xml = std::str::from_utf8(info.get(8..).unwrap()).unwrap();
    let attr = |name: &str| {
        let at = xml.find(&format!("{name}=\"")).unwrap() + name.len() + 2;
        let end = xml.get(at..).unwrap().find('"').unwrap() + at;
        base64(xml.get(at..end).unwrap()).unwrap()
    };
    let hmac_key = field(&attr("encryptedHmacKey"));
    let hmac_value = field(&attr("encryptedHmacValue"));
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
    let (info, package) = streams(&good).unwrap();
    let mut stream = package.clone();
    stream[8 + 100] ^= 0xFF; // inside the first encrypted segment
    let flipped = crate::cfb::write_compound(&[("EncryptionInfo", &info), ("EncryptedPackage", &stream)]).unwrap();
    if let Ok(out) = decrypt(&flipped, PASSWORD) {
        assert_ne!(out, plain, "a flipped ciphertext byte came back as the package");
    }
}
