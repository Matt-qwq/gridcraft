//! Line-ending-tolerant comparisons for the generated-file `--check` gates.
//!
//! Git checks text files out with CRLF on Windows (`core.autocrlf=true`, the default there)
//! while the generators emit LF. Comparing the raw text made every Windows checkout report its
//! generated files stale even when their content was identical, and made `cargo xtask ci` red
//! there with nothing to fix.

use std::borrow::Cow;
use std::path::Path;

/// `text` with CRLF folded to LF: a checkout's line endings are not content.
fn normalized(text: &str) -> Cow<'_, str> {
    if text.contains("\r\n") { Cow::Owned(text.replace("\r\n", "\n")) } else { Cow::Borrowed(text) }
}

/// Whether a generated file holds exactly what the generator emitted, whichever line endings
/// git checked it out with.
fn matches(on_disk: &str, generated: &str) -> bool {
    normalized(on_disk) == normalized(generated)
}

/// `--check` for one generated file: it is up to date when its content matches, ignoring the
/// checkout's line endings. `regenerate` is the command the error tells the user to run.
pub fn verify(path: &Path, generated: &str, regenerate: &str) -> Result<(), String> {
    let on_disk = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if !matches(&on_disk, generated) {
        return Err(format!("{} is out of date — run {regenerate} and commit the result", path.display()));
    }
    Ok(())
}

/// `content` written with the line endings `path` already uses, so regenerating a CRLF checkout
/// does not flip the file to LF. A new file gets LF, like the checked-in generated files.
pub fn for_write(path: &Path, content: &str) -> String {
    let crlf = std::fs::read_to_string(path).is_ok_and(|text| text.contains("\r\n"));
    let lf = normalized(content);
    if crlf { lf.replace('\n', "\r\n") } else { lf.into_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gridcraft-xtask-eol-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir.join(name)
    }

    #[test]
    fn check_accepts_a_crlf_and_an_lf_checkout_of_the_same_content() {
        let generated = "// generated\nfn a() {}\n";
        let lf = temp("lf.txt");
        let crlf = temp("crlf.txt");
        fs::write(&lf, generated).expect("write LF file");
        fs::write(&crlf, generated.replace('\n', "\r\n")).expect("write CRLF file");
        verify(&lf, generated, "`cargo xtask generator`").expect("an LF checkout is up to date");
        verify(&crlf, generated, "`cargo xtask generator`").expect("a CRLF checkout with the same content is up to date");
    }

    #[test]
    fn check_still_rejects_different_content() {
        let path = temp("stale.txt");
        fs::write(&path, "// generated\r\nfn a() {}\r\n").expect("write stale file");
        let err = verify(&path, "// generated\nfn b() {}\n", "`cargo xtask generator`").expect_err("different content is stale");
        assert!(err.contains("is out of date"), "{err}");
    }

    #[test]
    fn writing_keeps_the_line_endings_the_file_already_uses() {
        let crlf = temp("write-crlf.txt");
        fs::write(&crlf, "old\r\ntext\r\n").expect("write CRLF file");
        assert_eq!(for_write(&crlf, "new\ntext\n"), "new\r\ntext\r\n");
        let lf = temp("write-lf.txt");
        fs::write(&lf, "old\ntext\n").expect("write LF file");
        assert_eq!(for_write(&lf, "new\ntext\n"), "new\ntext\n");
        let new = temp("write-new.txt");
        let _ = fs::remove_file(&new);
        assert_eq!(for_write(&new, "new\ntext\n"), "new\ntext\n", "a new generated file gets the generator's LF");
    }
}
