//! Diagnostics log export and Simulator loopback control-port helpers for iOS.
//! Kept platform-neutral so host unit tests exercise the exact truncation, export, and env parsing logic.

use std::borrow::Cow;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Exported diagnostics filename placed in the Files-visible notebooks folder.
pub const DIAGNOSTICS_FILENAME: &str = "nocturne-diagnostics.log";
/// Maximum size in bytes of the exported diagnostics log tail (512 KiB).
pub const MAX_EXPORT_BYTES: usize = 512 * 1024;
/// Environment variable configuring the Simulator loopback control port.
pub const CONTROL_PORT_ENV: &str = "NOCTURNE_IOS_CONTROL_PORT";

const EMPTY_LOG_NOTICE: &str = "Nocturne iOS diagnostics log initialized (no log entries written yet).\n";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Exports `active_log` to `notebooks_dir/nocturne-diagnostics.log` atomically.
///
/// Creates `notebooks_dir` if missing, writes a one-line notice when `active_log`
/// does not exist, and trims oversized or non-UTF-8 logs to a valid UTF-8 line tail.
pub fn export_diagnostics_log(active_log: &Path, notebooks_dir: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(notebooks_dir).map_err(|err| format!("cannot create {}: {err}", notebooks_dir.display()))?;
    let raw_bytes = match fs::read(active_log) {
        Ok(bytes) => Some(bytes),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(format!("cannot read {}: {err}", active_log.display())),
    };
    let export_text = raw_bytes.as_deref().map_or(Cow::Borrowed(EMPTY_LOG_NOTICE), |b| trim_to_tail_utf8_line(b, MAX_EXPORT_BYTES));

    let target = notebooks_dir.join(DIAGNOSTICS_FILENAME);
    let temp = write_unique_temp(notebooks_dir, export_text.as_bytes())?;
    if let Err(err) = fs::rename(&temp, &target) {
        let _ = fs::remove_file(&temp);
        return Err(format!("cannot replace {}: {err}", target.display()));
    }
    Ok(target)
}

/// Trims `bytes` to a trailing slice of at most `max_bytes`, aligned to a valid UTF-8 line start.
pub fn trim_to_tail_utf8_line(bytes: &[u8], max_bytes: usize) -> Cow<'_, str> {
    if max_bytes == 0 || bytes.is_empty() {
        return Cow::Borrowed("");
    }
    let slice = if bytes.len() <= max_bytes { bytes } else { slice_tail_at_line(bytes, max_bytes) };
    match String::from_utf8_lossy(slice) {
        Cow::Borrowed(text) => Cow::Borrowed(text),
        Cow::Owned(text) if text.len() <= max_bytes => Cow::Owned(text),
        Cow::Owned(text) => Cow::Owned(String::from_utf8_lossy(slice_tail_at_line(text.as_bytes(), max_bytes)).into_owned()),
    }
}

/// Parses an optional loopback control port string (`"0"` allows an ephemeral OS port).
pub fn parse_control_port(raw: Option<&str>) -> Option<u16> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() { None } else { trimmed.parse::<u16>().ok() }
}

/// Reads the requested Simulator loopback control port from `NOCTURNE_IOS_CONTROL_PORT`.
pub fn requested_control_port_from_env() -> Option<u16> {
    parse_control_port(std::env::var(CONTROL_PORT_ENV).ok().as_deref())
}

fn slice_tail_at_line(bytes: &[u8], max_bytes: usize) -> &[u8] {
    let start = bytes.len().saturating_sub(max_bytes);
    let at_line_start = start.checked_sub(1).and_then(|i| bytes.get(i)) == Some(&b'\n');
    let tail = bytes.get(start..).unwrap_or_default();
    // Skip any UTF-8 continuation bytes (10xxxxxx) split across the cut boundary.
    let utf8_start = tail.iter().position(|&b| (b & 0b1100_0000) != 0b1000_0000).unwrap_or(tail.len());
    let aligned = tail.get(utf8_start..).unwrap_or_default();
    if at_line_start && utf8_start == 0 {
        aligned
    } else if let Some(nl) = aligned.iter().position(|&b| b == b'\n')
        && nl + 1 < aligned.len()
    {
        aligned.get(nl + 1..).unwrap_or_default()
    } else {
        aligned
    }
}

fn write_unique_temp(dir: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = dir.join(format!("{DIAGNOSTICS_FILENAME}.{}.{seq}.tmp", std::process::id()));
    let write_res = OpenOptions::new().write(true).create_new(true).open(&temp).and_then(|mut file| file.write_all(bytes));
    if let Err(err) = write_res {
        let _ = fs::remove_file(&temp);
        return Err(format!("cannot write {}: {err}", temp.display()));
    }
    Ok(temp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_store::testing::TempDir;

    fn test_paths(label: &str) -> (TempDir, PathBuf, PathBuf) {
        let temp = TempDir::new(label);
        let (log, notebooks) = (temp.path().join("nocturne.log"), temp.path().join("Notebooks"));
        (temp, log, notebooks)
    }

    fn assert_no_temp_files(dir: &Path) {
        let has_tmp = fs::read_dir(dir).expect("read dir").filter_map(Result::ok).any(|e| e.file_name().to_string_lossy().ends_with(".tmp"));
        assert!(!has_tmp, "unexpected .tmp file in {}", dir.display());
    }

    #[test]
    fn missing_active_log_writes_placeholder_notice() {
        let (_temp, active_log, notebooks) = test_paths("ios-diag-missing");
        let exported = export_diagnostics_log(&active_log, &notebooks).expect("export placeholder");
        assert_eq!(exported, notebooks.join(DIAGNOSTICS_FILENAME));
        assert_eq!(fs::read_to_string(&exported).expect("read exported"), EMPTY_LOG_NOTICE);
        assert_no_temp_files(&notebooks);
    }

    #[test]
    fn small_active_log_is_copied_verbatim_without_tmp_leftovers() {
        let (_temp, active_log, notebooks) = test_paths("ios-diag-small");
        let body = "2026-11-16 14:00:00 INFO start\n2026-11-16 14:00:01 INFO window ready\n";
        fs::write(&active_log, body).expect("write active log");
        let exported = export_diagnostics_log(&active_log, &notebooks).expect("export log");
        assert_eq!(fs::read_to_string(&exported).expect("read exported"), body);
        let names: Vec<_> = fs::read_dir(&notebooks).expect("read dir").filter_map(Result::ok).map(|e| e.file_name()).collect();
        assert_eq!(names, [DIAGNOSTICS_FILENAME]);
    }

    #[test]
    fn over_limit_log_with_multibyte_utf8_aligns_to_whole_line() {
        let sample = "2026-11-16 INFO header\n2026-11-16 INFO 😀 é مرحبا 日本語\n2026-11-16 INFO tail-line\n";
        for max_bytes in 26..=sample.len() {
            let t = trim_to_tail_utf8_line(sample.as_bytes(), max_bytes);
            assert!(t.len() <= max_bytes && t.starts_with("2026-11-16 INFO ") && t.ends_with("tail-line\n") && !t.contains('\u{FFFD}'));
        }
        let single = "abc😀é日本語";
        for max_bytes in 1..=single.len() {
            let trimmed = trim_to_tail_utf8_line(single.as_bytes(), max_bytes);
            assert!(trimmed.len() <= max_bytes && single.ends_with(trimmed.as_ref()) && !trimmed.contains('\u{FFFD}'));
        }

        let (_temp, active_log, notebooks) = test_paths("ios-diag-large");
        let line = "2026-11-16 INFO multichar 😀 é مرحبا 日本語 entry\n";
        let final_line = "2026-11-16 INFO final-marker-line\n";
        let big = format!("{}{final_line}", line.repeat((MAX_EXPORT_BYTES / line.len()) + 8));
        fs::write(&active_log, &big).expect("write oversized log");
        let text = fs::read_to_string(export_diagnostics_log(&active_log, &notebooks).expect("export oversized")).expect("utf8");
        assert!(text.len() <= MAX_EXPORT_BYTES && text.starts_with("2026-11-16 INFO ") && text.ends_with(final_line) && !text.contains('\u{FFFD}'));
    }

    #[test]
    fn non_utf8_bytes_never_panic_and_stay_within_limit() {
        let (_temp, active_log, notebooks) = test_paths("ios-diag-non-utf8");
        let raw = b"\xff\xfe\x80\x80corrupt-prefix\n2026-11-16 INFO recovered \xff\x90 line\n";
        fs::write(&active_log, raw).expect("write binary bytes");
        let text = fs::read_to_string(export_diagnostics_log(&active_log, &notebooks).expect("export lossy")).expect("utf8");
        assert!(text.len() <= MAX_EXPORT_BYTES && text.contains("recovered"));
        assert!(trim_to_tail_utf8_line(&[0xFF; 32], 10).len() <= 10);
    }

    #[test]
    fn overwriting_existing_export_replaces_previous_contents() {
        let (_temp, active_log, notebooks) = test_paths("ios-diag-overwrite");
        fs::write(&active_log, "first longer diagnostics payload\n").expect("write first");
        let exported = export_diagnostics_log(&active_log, &notebooks).expect("first export");
        fs::write(&active_log, "second\n").expect("write second");
        let second = export_diagnostics_log(&active_log, &notebooks).expect("second export");
        assert_eq!(second, exported);
        assert_eq!(fs::read_to_string(&second).expect("read second"), "second\n");
        assert_no_temp_files(&notebooks);
    }

    #[test]
    fn rename_failure_cleans_up_temp_file() {
        let (_temp, active_log, notebooks) = test_paths("ios-diag-cleanup");
        fs::write(&active_log, "entry\n").expect("write log");
        fs::create_dir_all(notebooks.join(DIAGNOSTICS_FILENAME)).expect("blocking dir");
        let err = export_diagnostics_log(&active_log, &notebooks).expect_err("rename over dir must fail");
        assert!(err.contains("cannot replace"));
        assert_no_temp_files(&notebooks);
    }

    #[test]
    fn parse_control_port_handles_valid_empty_and_invalid_inputs() {
        for raw in [None, Some(""), Some("   "), Some("65536"), Some("-1"), Some("abc")] {
            assert_eq!(parse_control_port(raw), None, "input={raw:?}");
        }
        for (raw, port) in [("0", 0), (" 45123 ", 45123), ("65535", 65535)] {
            assert_eq!(parse_control_port(Some(raw)), Some(port));
        }
        let _ = requested_control_port_from_env();
    }
}
