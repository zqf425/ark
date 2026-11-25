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

/// Stream a text file line by line, strictly decoding UTF-8.
///
/// `visit` receives each line without its terminator; a leading BOM is
/// dropped exactly like [`read_text`]. Returns `Ok(true)` when every line
/// was visited, so peak memory is the longest line, not the file. Returns
/// `Ok(false)` when the bytes are not valid UTF-8: in that case an
/// unspecified subset of lines may already have been visited and the caller
/// must redo the whole file with [`read_text`], whose fallback chain
/// (GB18030, Big5, ...) owns the legacy-codepage policy.
pub fn try_stream_utf8_lines(path: &Path, visit: &mut dyn FnMut(&str)) -> Result<bool> {
    match stream_utf8_lines(path, visit)? {
        Utf8Stream::Complete => Ok(true),
        Utf8Stream::NotUtf8 => Ok(false),
    }
}

/// Outcome of the strict UTF-8 streaming pass.
enum Utf8Stream {
    /// Every line was visited.
    Complete,
    /// The bytes are not valid UTF-8; some lines may already be visited.
    NotUtf8,
}

/// Single-pass strict UTF-8 line streamer.
///
/// Validity is checked per newline-terminated segment, which equals
/// whole-file validity because a multi-byte sequence never contains a
/// newline byte — no cross-segment state is needed.
fn stream_utf8_lines(path: &Path, visit: &mut dyn FnMut(&str)) -> Result<Utf8Stream> {
    use std::io::Read as _;

    let file = std::fs::File::open(path).map_err(|err| io_error("read", path, err))?;
    let mut reader = std::io::BufReader::new(file);
    let mut tail: Vec<u8> = Vec::new();
    let mut chunk: Vec<u8> = vec![0_u8; 64 * 1024];
    let mut first_segment = true;
    loop {
        let read = reader
            .read(&mut chunk)
            .map_err(|err| io_error("read", path, err))?;
        if read == 0 {
            break;
        }
        tail.extend_from_slice(&chunk[..read]);
        // Bind the scan first: a match on a call borrowing `tail` would keep
        // that borrow alive for the whole match and block `tail.drain` below.
        let scan = visit_complete_lines(&tail, visit, first_segment);
        match scan {
            SegmentScan::Consumed(consumed) => {
                tail.drain(..consumed);
                // The BOM only ever heads the first visited segment.
                first_segment = false;
            }
            // No newline yet: the line may complete in a later chunk.
            SegmentScan::Incomplete => {}
            SegmentScan::Invalid => return Ok(Utf8Stream::NotUtf8),
        }
    }
    // No trailing newline: the tail is the final line and must still be
    // visited, or the line count would not match `read_text(..).lines()`.
    if !tail.is_empty() {
        let text = match decode_segment(&tail, first_segment) {
            Some(text) => text,
            None => return Ok(Utf8Stream::NotUtf8),
        };
        for line in text.lines() {
            visit(line);
        }
    }
    Ok(Utf8Stream::Complete)
}

/// Outcome of scanning the bytes buffered after one read.
enum SegmentScan {
    /// No newline yet: the first buffered line may complete in a later chunk.
    Incomplete,
    /// The leading newline-terminated part was visited; that many bytes
    /// were consumed.
    Consumed(usize),
    /// The buffered part is not valid UTF-8.
    Invalid,
}

/// Visit every line inside the leading newline-terminated part of `bytes`
/// and report how far the scan advanced.
///
/// When no newline is present nothing is visited and the line may still be
/// completed by a later chunk; that is a normal mid-file state, distinct
/// from invalid UTF-8.
fn visit_complete_lines(
    bytes: &[u8],
    visit: &mut dyn FnMut(&str),
    first_segment: bool,
) -> SegmentScan {
    let Some(last) = bytes.iter().rposition(|byte| *byte == b'\n') else {
        return SegmentScan::Incomplete;
    };
    let complete = &bytes[..=last];
    let Some(text) = decode_segment(complete, first_segment) else {
        return SegmentScan::Invalid;
    };
    for line in text.lines() {
        visit(line);
    }
    SegmentScan::Consumed(complete.len())
}

/// Decode one segment strictly, dropping the BOM in the first one.
fn decode_segment(bytes: &[u8], first_segment: bool) -> Option<&str> {
    let text = std::str::from_utf8(bytes).ok()?;
    if first_segment {
        // Same BOM rule as `read_text`: a leading U+FEFF is dropped.
        return Some(strip_bom_str(text));
    }
    Some(text)
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
    std::fs::rename(&temp, path).map_err(|err| io_error("rename", path, err))?;
    sync_parent_dir(path)
}

/// Persist the directory entry changed by an atomic rename.
fn sync_parent_dir(path: &Path) -> Result<()> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    #[cfg(test)]
    if let Some(error) = injected_parent_sync_failure(path) {
        return Err(error);
    }
    sync_dir(parent).map_err(|err| io_error("sync parent directory", parent, err))
}

#[cfg(unix)]
fn sync_dir(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_dir(path: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .sync_all()
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
    name.push(format!(".{}.tmp", uuid::Uuid::new_v4()));
    path.with_file_name(name)
}

#[cfg(test)]
static PARENT_SYNC_FAILURES: std::sync::OnceLock<std::sync::Mutex<Option<std::path::PathBuf>>> =
    std::sync::OnceLock::new();

#[cfg(test)]
fn fail_next_parent_sync(path: &Path) {
    *PARENT_SYNC_FAILURES
        .get_or_init(Default::default)
        .lock()
        .unwrap() = Some(path.to_path_buf());
}

#[cfg(test)]
fn injected_parent_sync_failure(path: &Path) -> Option<ArkError> {
    let mut failures = PARENT_SYNC_FAILURES
        .get_or_init(Default::default)
        .lock()
        .unwrap();
    if failures.as_deref() == Some(path) {
        failures.take();
        Some(io_error(
            "sync parent directory",
            path,
            std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "injected parent directory fsync failure",
            ),
        ))
    } else {
        None
    }
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

/// Bytes at the end of a chunk that may still become one UTF-8 or GB18030 codepoint.
pub fn incomplete_encoded_suffix(bytes: &[u8]) -> usize {
    let utf8 = match std::str::from_utf8(bytes) {
        Ok(_) => return 0,
        Err(error) if error.error_len().is_none() => bytes.len() - error.valid_up_to(),
        Err(_) => 0,
    };
    utf8.max(incomplete_gb18030_suffix(bytes))
}

fn incomplete_gb18030_suffix(bytes: &[u8]) -> usize {
    fn lead(byte: u8) -> bool {
        (0x81..=0xfe).contains(&byte)
    }

    fn four_byte_digit(byte: u8) -> bool {
        (0x30..=0x39).contains(&byte)
    }

    fn two_byte_trail(byte: u8) -> bool {
        (0x40..=0x7e).contains(&byte) || (0x80..=0xfe).contains(&byte)
    }

    let mut index = 0;
    while index < bytes.len() {
        let first = bytes[index];
        if first <= 0x80 {
            index += 1;
            continue;
        }
        if !lead(first) {
            return 0;
        }
        let Some(&second) = bytes.get(index + 1) else {
            return 1;
        };
        if four_byte_digit(second) {
            let Some(&third) = bytes.get(index + 2) else {
                return 2;
            };
            if !lead(third) {
                return 0;
            }
            let Some(&fourth) = bytes.get(index + 3) else {
                return 3;
            };
            if !four_byte_digit(fourth) {
                return 0;
            }
            index += 4;
        } else if two_byte_trail(second) {
            index += 2;
        } else {
            return 0;
        }
    }
    0
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
    fn incomplete_encoded_suffix_keeps_gb18030_codepoint_boundaries() {
        let character = [0x95_u8, 0x32, 0x82, 0x36];
        for split in 1..character.len() {
            assert_eq!(incomplete_encoded_suffix(&character[..split]), split);
        }
        assert_eq!(incomplete_encoded_suffix(&character), 0);

        let mut prefixed = b"ok".to_vec();
        prefixed.extend_from_slice(&character[..2]);
        assert_eq!(incomplete_encoded_suffix(&prefixed), 2);
    }

    #[test]
    fn incomplete_encoded_suffix_keeps_utf8_boundaries_but_not_malformed_input() {
        let character = "中".as_bytes();
        assert_eq!(incomplete_encoded_suffix(&character[..2]), 2);
        assert_eq!(incomplete_encoded_suffix(character), 0);
        assert_eq!(incomplete_encoded_suffix(&[0xff, 0x81]), 0);
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
    fn atomic_write_propagates_parent_directory_sync_failure() {
        let dir = std::env::temp_dir().join(format!("ark-enc-sync-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        fail_next_parent_sync(&path);
        let result = write_text_atomic(&path, "content");
        assert!(result.is_err(), "rename must be followed by parent fsync");
        assert_eq!(std::fs::read(&path).unwrap(), b"content");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_temp_siblings_are_unique_within_one_process() {
        let path = std::path::Path::new("nested/config.json");
        assert_ne!(temp_sibling(path), temp_sibling(path));
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

    /// Scratch directory for the line-streaming tests; `suffix` keeps the
    /// parallel tests from sharing one directory.
    fn lines_dir(suffix: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ark-enc-lines-{suffix}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn streamed_lines_match_whole_file_read() {
        let dir = lines_dir("match");
        let path = dir.join("many.txt");
        // Larger than the 64 KiB read chunk so several reads are needed.
        let body: String = (0..9_000).map(|index| format!("line {index}\n")).collect();
        std::fs::write(&path, body.as_bytes()).unwrap();

        let mut streamed: Vec<String> = Vec::new();
        let complete =
            try_stream_utf8_lines(&path, &mut |line| streamed.push(line.to_string())).unwrap();
        assert!(complete);
        let whole = read_text(&path).unwrap();
        let expected: Vec<&str> = whole.lines().collect();
        assert_eq!(streamed, expected);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn streamed_lines_keep_codepoints_across_read_boundaries() {
        // The file is larger than one read and a 3-byte codepoint sits right
        // at the chunk boundary, so a naive per-read decode would corrupt it.
        // Splitting on newlines is always a char boundary in UTF-8, so the
        // streamed lines must be byte-identical to a whole-file read.
        let dir = lines_dir("multibyte");
        let path = dir.join("multibyte.txt");
        let head = "a".repeat(64 * 1024 - 2);
        let body = format!("{head}中\ntail\n");
        std::fs::write(&path, body.as_bytes()).unwrap();

        let mut streamed: Vec<String> = Vec::new();
        let complete =
            try_stream_utf8_lines(&path, &mut |line| streamed.push(line.to_string())).unwrap();
        assert!(complete);
        assert_eq!(streamed.len(), 2);
        assert_eq!(streamed[0], format!("{head}中"));
        assert_eq!(streamed[1], "tail");
        assert!(!streamed[0].contains('\u{FFFD}'));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn streamed_bom_is_stripped_like_whole_file_read() {
        let dir = lines_dir("bom");
        let path = dir.join("bom.txt");
        let mut bytes = UTF8_BOM_BYTES.to_vec();
        bytes.extend_from_slice(b"a\nb\n");
        std::fs::write(&path, &bytes).unwrap();

        let mut streamed: Vec<String> = Vec::new();
        let complete =
            try_stream_utf8_lines(&path, &mut |line| streamed.push(line.to_string())).unwrap();
        assert!(complete);
        assert_eq!(streamed, vec!["a".to_string(), "b".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_utf8_file_is_reported_for_whole_file_fallback() {
        let dir = lines_dir("gbk");
        let path = dir.join("gbk.txt");
        // "中文" in GBK is not valid UTF-8; the caller must redo the file
        // with `read_text`, whose fallback chain decodes it.
        std::fs::write(&path, [0xD6_u8, 0xD0, 0xCE, 0xC4, b'\n']).unwrap();

        let mut streamed: Vec<String> = Vec::new();
        let complete =
            try_stream_utf8_lines(&path, &mut |line| streamed.push(line.to_string())).unwrap();
        assert!(!complete);
        assert_eq!(read_text(&path).unwrap(), "中文\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_without_trailing_newline_keeps_its_last_line() {
        let dir = lines_dir("tail");
        let path = dir.join("tail.txt");
        std::fs::write(&path, b"a\nb").unwrap();

        let mut streamed: Vec<String> = Vec::new();
        let complete =
            try_stream_utf8_lines(&path, &mut |line| streamed.push(line.to_string())).unwrap();
        assert!(complete);
        assert_eq!(streamed, vec!["a".to_string(), "b".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_a_streaming_error() {
        let dir = lines_dir("missing");
        let path = dir.join("nope.txt");
        let mut streamed: Vec<String> = Vec::new();
        assert!(try_stream_utf8_lines(&path, &mut |line| streamed.push(line.to_string())).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
