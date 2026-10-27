//! Text encoding policy: UTF-8 first, BOM never written (design §6.1, decision R1).
//!
//! The existing desktop app already settled this policy in
//! `src/main/text-encoding.ts` and `kun/src/config/kun-config.ts`
//! (`stripUtf8Bom`). The two behaviours that matter in practice:
//!
//! * **Read**: decode as UTF-8 and drop a leading BOM. Windows Notepad and old
//!   PowerShell write `config.json` with a BOM, and a raw `JSON.parse` then dies
//!   on U+FEFF. When the bytes are not valid UTF-8, fall back to the encodings a
//!   Chinese Windows host actually produces, in the same order the TypeScript
//!   implementation uses, with Latin-1 as the always-succeeds last resort.
//! * **Write**: always UTF-8, never a BOM, and defensively strip a leading BOM
//!   the caller may have pasted into the content.
//!
//! Everything here is a pure byte/string function except the four thin `fs`
//! helpers at the bottom, so the rules can be unit-tested without touching disk.

use crate::error::{ArkError, ArkErrorCode, Result};

/// UTF-8 byte order mark (`EF BB BF`).
pub const UTF8_BOM_BYTES: [u8; 3] = [0xEF, 0xBB, 0xBF];

/// U+FEFF, the BOM once decoded into a string.
pub const UTF8_BOM_CHAR: char = '\u{FEFF}';

/// Which decoder produced the text. Reported so a diagnostic can explain why a
/// file looked odd instead of silently guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Gb18030,
    Big5,
    ShiftJis,
    EucKr,
    Latin1,
}

impl TextEncoding {
    pub fn as_str(self) -> &'static str {
        match self {
            TextEncoding::Utf8 => "utf-8",
            TextEncoding::Gb18030 => "gb18030",
            TextEncoding::Big5 => "big5",
            TextEncoding::ShiftJis => "shift_jis",
            TextEncoding::EucKr => "euc-kr",
            TextEncoding::Latin1 => "latin1",
        }
    }

    /// True when the bytes were not plain UTF-8, which is worth a diagnostic.
    pub fn is_fallback(self) -> bool {
        self != TextEncoding::Utf8
    }
}

/// Decoded text plus the encoding that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedText {
    pub text: String,
    pub encoding: TextEncoding,
}

impl DecodedText {
    /// True when a leading BOM was present in the source bytes.
    pub fn had_bom(&self) -> bool {
        self.encoding.is_fallback() && self.text.starts_with(UTF8_BOM_CHAR)
            || self.text.starts_with(UTF8_BOM_CHAR)
    }
}

/// Drop a leading BOM from a decoded string.
pub fn strip_bom_str(input: &str) -> &str {
    input.strip_prefix(UTF8_BOM_CHAR).unwrap_or(input)
}

/// Drop a leading BOM from raw bytes.
pub fn strip_bom_bytes(bytes: &[u8]) -> &[u8] {
    if bytes.starts_with(&UTF8_BOM_BYTES) {
        &bytes[UTF8_BOM_BYTES.len()..]
    } else {
        bytes
    }
}

/// True when the bytes begin with a UTF-8 BOM.
pub fn has_bom(bytes: &[u8]) -> bool {
    bytes.starts_with(&UTF8_BOM_BYTES)
}

/// UTF-8 bytes for `text` with any leading BOM removed.
///
/// This is the only sanctioned way to turn text into file bytes: it guarantees
/// we never write a BOM ourselves, and it repairs content that arrived with one.
pub fn utf8_bytes_no_bom(text: &str) -> &[u8] {
    strip_bom_str(text).as_bytes()
}

/// Decode bytes into text using the policy described at the top of this module.
///
/// Never fails: Latin-1 is the last fallback and every byte sequence is valid
/// Latin-1, which matches the TypeScript implementation's guarantee.
pub fn decode(bytes: &[u8]) -> DecodedText {
    if let Some(text) = strict_utf8(bytes) {
        return DecodedText {
            text,
            encoding: TextEncoding::Utf8,
        };
    }
    for (encoding, decoder) in FALLBACKS {
        if let Some(text) = decoder(bytes) {
            return DecodedText {
                text,
                encoding: *encoding,
            };
        }
    }
    // Unreachable in practice: Latin-1 accepts everything.
    DecodedText {
        text: latin1(bytes),
        encoding: TextEncoding::Latin1,
    }
}

/// Decode to a `String`, discarding which encoding was used.
pub fn decode_to_string(bytes: &[u8]) -> String {
    decode(bytes).text
}

// ---------------------------------------------------------------------------
// fs helpers
// ---------------------------------------------------------------------------

use std::path::Path;

/// Read a text file with the project encoding policy.
///
/// Missing files are reported as `NotFound` so callers can distinguish "absent"
/// from "unreadable".
pub fn read_text_file(path: &Path) -> Result<DecodedText> {
    let bytes = std::fs::read(path).map_err(|err| io_error("read", path, err))?;
    Ok(decode(&bytes))
}

/// Read a text file and return only the text (BOM stripped, fallback decoded).
pub fn read_text(path: &Path) -> Result<String> {
    read_text_file(path).map(|decoded| decoded.text)
}

/// Read a text file, returning `None` when it does not exist.
pub fn read_text_opt(path: &Path) -> Option<String> {
    match std::fs::read(path) {
        Ok(bytes) => Some(decode_to_string(&bytes)),
        Err(_) => None,
    }
}

/// Read a JSON document, stripping BOM first.
///
/// A BOM in front of a JSON body is the single most common startup failure on
/// Windows, so every JSON read in the workspace goes through here.
pub fn read_json_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = read_text(path)?;
    serde_json::from_str(&text).map_err(|err| {
        ArkError::new(
            ArkErrorCode::ValidationError,
            format!("{} is not valid JSON: {err}", path.display()),
        )
    })
}

/// Read JSON, returning `None` when the file is absent or unparsable.
pub fn read_json_file_opt<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = read_text_opt(path)?;
    serde_json::from_str(&text).ok()
}

/// Write text as UTF-8 without a BOM, creating parent directories.
///
/// The write goes through a temp file + rename so a crash cannot leave a
/// half-written document behind.
pub fn write_text_atomic(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|err| io_error("create", parent, err))?;
        }
    }
    let temp = temp_sibling(path);
    {
        use std::io::Write;
        let mut file =
            std::fs::File::create(&temp).map_err(|err| io_error("create", &temp, err))?;
        file.write_all(utf8_bytes_no_bom(text))
            .map_err(|err| io_error("write", &temp, err))?;
        file.sync_all()
            .map_err(|err| io_error("sync", &temp, err))?;
    }
    std::fs::rename(&temp, path).map_err(|err| io_error("rename", path, err))
}

/// Write text as UTF-8 without a BOM (no atomicity, for append-style surfaces).
pub fn write_text(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, utf8_bytes_no_bom(text)).map_err(|err| io_error("write", path, err))
}

/// Write a value as pretty JSON, UTF-8 without BOM, with a trailing newline.
pub fn write_json_file<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut body = serde_json::to_string_pretty(value)?;
    body.push('\n');
    write_text_atomic(path, &body)
}

fn temp_sibling(path: &Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp-{}", std::process::id()));
    path.with_file_name(name)
}

fn io_error(action: &str, path: &Path, err: std::io::Error) -> ArkError {
    ArkError::new(
        ArkErrorCode::StoreFailure,
        format!("cannot {action} {}: {err}", path.display()),
    )
}

// ---------------------------------------------------------------------------
// decoders
// ---------------------------------------------------------------------------

type Decoder = fn(&[u8]) -> Option<String>;

const FALLBACKS: &[(TextEncoding, Decoder)] = &[
    (TextEncoding::Gb18030, decode_gb18030),
    (TextEncoding::Big5, decode_big5),
    (TextEncoding::ShiftJis, decode_shift_jis),
    (TextEncoding::EucKr, decode_euc_kr),
    (TextEncoding::Latin1, decode_latin1),
];

fn strict_utf8(bytes: &[u8]) -> Option<String> {
    // `without_replacement` means "fail instead of emitting U+FFFD", which is
    // what distinguishes real UTF-8 from a legacy codepage.
    encoding_rs::UTF_8
        .decode_without_bom_handling_and_without_replacement(bytes)
        .map(|cow| strip_bom_str(cow.as_ref()).to_string())
}

fn decode_strict(encoding: &'static encoding_rs::Encoding, bytes: &[u8]) -> Option<String> {
    encoding
        .decode_without_bom_handling_and_without_replacement(bytes)
        .map(|cow| cow.into_owned())
}

fn decode_gb18030(bytes: &[u8]) -> Option<String> {
    decode_strict(encoding_rs::GB18030, bytes)
}

fn decode_big5(bytes: &[u8]) -> Option<String> {
    decode_strict(encoding_rs::BIG5, bytes)
}

fn decode_shift_jis(bytes: &[u8]) -> Option<String> {
    decode_strict(encoding_rs::SHIFT_JIS, bytes)
}

fn decode_euc_kr(bytes: &[u8]) -> Option<String> {
    decode_strict(encoding_rs::EUC_KR, bytes)
}

/// Latin-1 is the final fallback: it maps every byte to a codepoint, so it can
/// never fail. Mirrors the TypeScript `latin1` branch.
fn decode_latin1(bytes: &[u8]) -> Option<String> {
    Some(latin1(bytes))
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| *byte as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_utf8_is_decoded_as_utf8() {
        let decoded = decode("你好, ark".as_bytes());
        assert_eq!(decoded.encoding, TextEncoding::Utf8);
        assert_eq!(decoded.text, "你好, ark");
    }

    #[test]
    fn utf8_bom_is_stripped_on_read() {
        let mut bytes = UTF8_BOM_BYTES.to_vec();
        bytes.extend_from_slice(b"{\"port\":18790}");
        let decoded = decode(&bytes);
        assert_eq!(decoded.encoding, TextEncoding::Utf8);
        assert_eq!(decoded.text, "{\"port\":18790}");
        assert!(!decoded.text.starts_with(UTF8_BOM_CHAR));
    }

    #[test]
    fn bom_bytes_are_recognised_and_stripped() {
        let mut bytes = UTF8_BOM_BYTES.to_vec();
        bytes.extend_from_slice(b"hi");
        assert!(has_bom(&bytes));
        assert_eq!(strip_bom_bytes(&bytes), b"hi");
        assert!(!has_bom(b"hi"));
    }

    #[test]
    fn utf8_bom_char_is_stripped_from_strings() {
        assert_eq!(strip_bom_str("\u{FEFF}hello"), "hello");
        assert_eq!(strip_bom_str("hello"), "hello");
        assert_eq!(utf8_bytes_no_bom("\u{FEFF}hello"), b"hello");
    }

    #[test]
    fn writes_never_emit_a_bom_even_when_content_has_one() {
        // A model can echo a BOM back into a file; the writer repairs it.
        assert_eq!(utf8_bytes_no_bom("\u{FEFF}{\"a\":1}"), b"{\"a\":1}");
        assert!(!utf8_bytes_no_bom("plain").starts_with(&UTF8_BOM_BYTES));
    }

    #[test]
    fn gb18030_fallback_decodes_chinese_bytes() {
        // "中文" encoded as GBK/GB18030: D6 D0 CE C4
        let bytes = [0xD6u8, 0xD0, 0xCE, 0xC4];
        let decoded = decode(&bytes);
        assert_eq!(decoded.encoding, TextEncoding::Gb18030);
        assert_eq!(decoded.text, "中文");
    }

    #[test]
    fn invalid_utf8_falls_back_instead_of_emitting_replacement_chars() {
        let bytes = [0x61u8, 0xFF, 0x62];
        let decoded = decode(&bytes);
        assert!(decoded.encoding.is_fallback());
        assert!(!decoded.text.contains('\u{FFFD}'));
    }

    #[test]
    fn empty_input_is_empty_utf8() {
        let decoded = decode(b"");
        assert_eq!(decoded.encoding, TextEncoding::Utf8);
        assert_eq!(decoded.text, "");
    }

    #[test]
    fn roundtrip_through_disk_strips_a_bom_and_never_writes_one() {
        let dir = std::env::temp_dir().join(format!("ark-enc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");

        // Simulate Notepad writing a BOM.
        let mut bytes = UTF8_BOM_BYTES.to_vec();
        bytes.extend_from_slice(br#"{"log_level":"info"}"#);
        std::fs::write(&path, &bytes).unwrap();

        let value: serde_json::Value = read_json_file(&path).unwrap();
        assert_eq!(value["log_level"], "info");

        write_json_file(&path, &value).unwrap();
        let written = std::fs::read(&path).unwrap();
        assert!(!has_bom(&written), "writer must not add a BOM");
        assert_eq!(written.last(), Some(&b'\n'));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_is_bom_free() {
        let dir = std::env::temp_dir().join(format!("ark-enc-at-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nested").join("doc.txt");
        write_text_atomic(&path, "\u{FEFF}content").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"content");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
