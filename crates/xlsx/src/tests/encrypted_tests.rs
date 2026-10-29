//! Encrypted workbooks end to end: compound file → streams → plain package.

use crate::testing::{compound_file, encrypt_workbook, encrypt_workbook_standard};
use crate::{DecryptError, decrypt};

#[test]
fn the_test_compound_writer_reads_back() {
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 253) as u8).collect();
    let mid: Vec<u8> = (0..4095u32).map(|i| i as u8).collect();
    let file = compound_file(&[("Small", b"hello"), ("Big", &big), ("Empty", b""), ("Mid", &mid)]);
    let cf = crate::cfb::Compound::open(&file).unwrap();
    assert_eq!(cf.root_stream("Mid").unwrap().unwrap(), mid, "just under the cutoff: the mini stream");
    assert_eq!(cf.root_stream("Small").unwrap().unwrap(), b"hello");
    assert_eq!(cf.root_stream("big").unwrap().unwrap(), big);
    assert_eq!(cf.root_stream("Empty").unwrap().unwrap(), b"");
    assert_eq!(cf.root_stream("Missing").unwrap(), None);
}

#[test]
fn a_workbook_saved_with_a_password_reopens_with_the_same_cells() {
    use gridcraft_core::{CellRef, Value};

    let mut wb = gridcraft_model::Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    sh.set_value(CellRef::parse("A1").unwrap(), Value::from("Quarterly"));
    sh.set_value(CellRef::parse("A2").unwrap(), Value::Number(1200.5));
    sh.set_value(CellRef::parse("B2").unwrap(), Value::Bool(true));
    sh.set_value(CellRef::parse("C2").unwrap(), Value::from("=Été!"));
    for row in 0..200u32 {
        sh.set_value(CellRef::new(row + 5, 0), Value::Number(f64::from(row) * 1.5));
    }
    let xlsx = crate::write_xlsx(&wb).unwrap();
    let file = crate::encrypt_workbook(&xlsx, "Pa55word").unwrap();
    assert_eq!(crate::sniff(&file), crate::Format::Encrypted);
    assert_eq!(crate::check_encryption(&file), Ok(()));
    assert_eq!(decrypt(&file, "Pa55word").unwrap(), xlsx, "the package must come back byte for byte");
    assert_eq!(decrypt(&file, "pa55word"), Err(DecryptError::WrongPassword));
    assert_eq!(decrypt(&file, ""), Err(DecryptError::WrongPassword));
    // And the same package reads back as the same workbook.
    let (back, _) = crate::read_xlsx(&decrypt(&file, "Pa55word").unwrap()).unwrap();
    assert_eq!(back.sheets.len(), wb.sheets.len());
    for (a, b) in wb.sheets.iter().zip(&back.sheets) {
        assert_eq!(a.cells.iter().count(), b.cells.iter().count());
        for (at, cell) in b.cells.iter() {
            assert_eq!(cell.value, a.value(at), "{at:?}");
        }
    }
}

#[test]
fn every_cut_of_a_saved_encrypted_workbook_is_an_error_not_a_panic() {
    let mut wb = gridcraft_model::Workbook::new();
    wb.sheet_mut(0).unwrap().set_value(gridcraft_core::CellRef::parse("A1").unwrap(), gridcraft_core::Value::from("hi"));
    let file = crate::encrypt_workbook(&crate::write_xlsx(&wb).unwrap(), "pw").unwrap();
    for cut in (0..file.len()).step_by(97) {
        if let Ok(plain) = decrypt(&file[..cut], "pw") {
            // A cut inside the last sector's slack still reads whole; it must then be the package.
            assert!(plain.starts_with(b"PK"), "{cut} of {}", file.len());
        }
    }
    // A file cut inside the EncryptedPackage stream is damage, never a short package.
    let (info, _) = crate::crypto::streams(&file).unwrap();
    for size in [0usize, 4, 7] {
        let short = crate::cfb::write_compound(&[("EncryptionInfo", &info), ("EncryptedPackage", &vec![0u8; size])]).unwrap();
        assert!(decrypt(&short, "pw").is_err(), "a {size}-byte package stream");
    }
    // A stream that states more bytes than it holds is refused before any allocation.
    let lying = 100u64.to_le_bytes().to_vec();
    let short = crate::cfb::write_compound(&[("EncryptionInfo", &info), ("EncryptedPackage", &lying)]).unwrap();
    assert!(decrypt(&short, "pw").is_err());
    // An existing but empty package: with the file's own integrity data it cannot be the package
    // the file was saved with, so it is corruption; a writer that wrote no integrity data leaves
    // nothing to check, and an empty package stays a clean empty result.
    let empty = crate::cfb::write_compound(&[("EncryptionInfo", &info), ("EncryptedPackage", &0u64.to_le_bytes())]).unwrap();
    assert_eq!(decrypt(&empty, "pw"), Err(DecryptError::Corrupted));
    let unchecked = crate::testing::encrypt_workbook(b"", "pw");
    assert_eq!(decrypt(&unchecked, "pw").unwrap(), Vec::<u8>::new());
}

#[test]
fn encrypted_files_decrypt_to_the_package() {
    let package: Vec<u8> = (0..3_000_000u32).map(|i| (i * 31 % 251) as u8).collect();
    let agile = encrypt_workbook(&package, "Pass1234");
    assert!(crate::cfb::is_encrypted(&agile));
    assert_eq!(decrypt(&agile, "Pass1234").unwrap(), package);
    assert_eq!(decrypt(&agile, "wrong"), Err(DecryptError::WrongPassword));
    let standard = encrypt_workbook_standard(&package, "Pass1234");
    assert_eq!(decrypt(&standard, "Pass1234").unwrap(), package);
}

#[test]
fn a_compound_file_without_the_streams_is_malformed() {
    let file = compound_file(&[("EncryptionInfo", &[4, 0, 4, 0])]);
    assert!(matches!(decrypt(&file, "x"), Err(DecryptError::Malformed(_))));
    assert!(matches!(decrypt(b"not a compound file", "x"), Err(DecryptError::Malformed(_))));
}

#[test]
fn a_gridcraft_workbook_survives_encryption() {
    let b2 = gridcraft_core::CellRef::parse("B2").unwrap();
    let mut wb = gridcraft_model::Workbook::new();
    wb.sheet_mut(0).unwrap().set_value(b2, gridcraft_core::Value::Number(42.0));
    let xlsx = crate::write_xlsx(&wb).unwrap();
    let file = encrypt_workbook(&xlsx, "Pass1234");
    assert_eq!(crate::sniff(&file), crate::Format::Encrypted);
    let (back, _) = crate::read_xlsx(&decrypt(&file, "Pass1234").unwrap()).unwrap();
    assert_eq!(back.active().unwrap().value(b2), gridcraft_core::Value::Number(42.0));
}

#[test]
fn schemes_no_password_opens_are_named_before_asking_for_one() {
    use crate::check_encryption;
    use crate::testing::{agile_info, with_agile_header};
    let unsupported = |s: &str| Err(DecryptError::Unsupported(s.into()));
    assert_eq!(check_encryption(&encrypt_workbook(b"package", "pw")), Ok(()));
    assert_eq!(check_encryption(&encrypt_workbook_standard(b"package", "pw")), Ok(()));
    let file = |info: &[u8]| compound_file(&[("EncryptionInfo", info), ("EncryptedPackage", &[0; 8])]);
    assert_eq!(check_encryption(&file(&[3, 0, 2, 0, 4, 0, 0, 0])), unsupported("RC4"));
    assert_eq!(check_encryption(&file(&[4, 0, 3, 0, 0, 0, 0, 0])), unsupported("Extensible"));
    let cert = agile_info("pw", 1000).replace("keyEncryptor/password", "keyEncryptor/certificate");
    assert_eq!(check_encryption(&file(&with_agile_header(&cert))), unsupported("certificate"));
    assert!(matches!(check_encryption(&file(&[4, 0])), Err(DecryptError::Malformed(_))));
    // Rights management (IRM) keeps an EncryptedPackage but no EncryptionInfo.
    let irm = compound_file(&[("EncryptedPackage", &[0; 8])]);
    assert_eq!(check_encryption(&irm), unsupported("rights-management"));
    assert_eq!(decrypt(&irm, "pw"), Err(DecryptError::Unsupported("rights-management".into())));
}

#[test]
fn a_workbook_written_with_standard_encryption_reopens_with_the_same_cells() {
    // `crate::encrypt_workbook_as(.., Standard)` is the writer `file.save` uses — fresh salt and
    // verifier, AES-128-ECB over the package — not the fixed-key `testing` helper.
    use gridcraft_core::{CellRef, Value};
    let mut wb = gridcraft_model::Workbook::new();
    let sh = wb.sheet_mut(0).unwrap();
    sh.set_value(CellRef::parse("A1").unwrap(), Value::from("Legacy"));
    sh.set_value(CellRef::parse("B2").unwrap(), Value::Number(-12.5));
    let xlsx = crate::write_xlsx(&wb).unwrap();
    let file = crate::encrypt_workbook_as(&xlsx, "Pa55word", crate::EncryptionScheme::Standard).unwrap();
    assert_eq!(crate::sniff(&file), crate::Format::Encrypted);
    assert_eq!(crate::encryption_scheme(&file), Some(crate::EncryptionScheme::Standard));
    assert_eq!(crate::check_encryption(&file), Ok(()));
    assert_eq!(decrypt(&file, "Pa55word").unwrap(), xlsx, "the package must come back byte for byte");
    assert_eq!(decrypt(&file, "pa55word"), Err(DecryptError::WrongPassword));
    let (back, _) = crate::read_xlsx(&decrypt(&file, "Pa55word").unwrap()).unwrap();
    assert_eq!(back.active().unwrap().value(CellRef::parse("B2").unwrap()), Value::Number(-12.5));
}

#[test]
fn standard_encryption_leaves_a_flipped_byte_to_the_zip_to_refuse() {
    // Standard has no data-integrity field ([MS-OFFCRYPTO] 2.3.2): the verifier is the scheme's
    // only check, so a flipped ciphertext byte decrypts to a changed package and the zip's own
    // structure is what refuses the workbook. Pinning the honest limit of the scheme, and why
    // Agile is the default.
    let mut wb = gridcraft_model::Workbook::new();
    wb.sheet_mut(0).unwrap().set_value(gridcraft_core::CellRef::parse("A1").unwrap(), gridcraft_core::Value::from("hi"));
    let xlsx = crate::write_xlsx(&wb).unwrap();
    let file = crate::encrypt_workbook_as(&xlsx, "pw", crate::EncryptionScheme::Standard).unwrap();
    let (info, package) = crate::crypto::streams(&file).unwrap();
    // A byte inside the end-of-central-directory record (the zip's own index) is the clearest
    // case: the package decrypts, and the reader cannot find the archive's index any more.
    let eocd = xlsx.windows(4).rposition(|w| w.starts_with(&[0x50, 0x4B, 0x05, 0x06])).expect("the package has an EOCD");
    let mut stream = package.clone();
    *stream.get_mut(8 + eocd).unwrap() ^= 0x01;
    let flipped = crate::cfb::write_compound(&[("EncryptionInfo", &info), ("EncryptedPackage", &stream)]).unwrap();
    let plain = decrypt(&flipped, "pw").expect("Standard has no check over the package");
    assert_ne!(plain, xlsx);
    assert!(crate::read_xlsx(&plain).is_err(), "the zip's own index check has to catch it");
}
