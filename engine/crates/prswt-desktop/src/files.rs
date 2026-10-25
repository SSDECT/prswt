//! DSK-02/03: the desktop shell's disk access: capped reads, atomic saves and
//! the recent-files list. The engine and UI crates never touch the file
//! system (ADR-0009); everything that does lives here and is tested here.

use std::io::Read;
use std::path::{Path, PathBuf};

/// Entries kept under File ▸ Open recent.
pub const MAX_RECENT: usize = 10;

/// Reads at most `max` bytes of `path` as text. Imports may come from tools
/// that write Windows-1252, so invalid UTF-8 is replaced, not rejected; the
/// importing command then reports what it cannot parse.
pub fn read_text(path: &Path, max: usize) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let len = file.metadata().map_or(0, |m| m.len());
    if usize::try_from(len).map_or(true, |l| l > max) {
        return Err(format!("{} is larger than {max} bytes", path.display()));
    }
    let mut bytes = Vec::new();
    // `take` also guards against a file that grows between the size check and the read.
    file.take(u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() > max {
        return Err(format!("{} is larger than {max} bytes", path.display()));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Writes `text` to `path` so that a crash or full disk never leaves a
/// half-written file: write a sibling temp file, flush it to disk, then
/// rename it over the target (atomic on Windows, Linux and macOS).
pub fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    // Unique per call: the window and the cloud worker may write the same
    // file at once, and must not share (and truncate) one temp file.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} is not a file path", path.display()))?;
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".{}.{n}.tmp", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    result.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

/// `path` with `ext` appended when it has no extension (dialogs on Linux
/// do not always add the filter's extension).
pub fn with_extension(path: PathBuf, ext: &str) -> PathBuf {
    if path.extension().is_some() {
        path
    } else {
        path.with_extension(ext)
    }
}

/// Moves `path` to the front of `recent`, dropping duplicates and the
/// oldest entries beyond [`MAX_RECENT`].
pub fn push_recent(recent: &mut Vec<String>, path: &Path) {
    let p = path.display().to_string();
    recent.retain(|r| *r != p);
    recent.insert(0, p);
    recent.truncate(MAX_RECENT);
}

/// Recent-files list from its stored JSON; anything unreadable gives an
/// empty list rather than an error (settings are a convenience).
pub fn parse_recent(stored: Option<&str>) -> Vec<String> {
    let mut list: Vec<String> = stored
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    list.retain(|p| !p.is_empty() && p.len() < 4096);
    list.truncate(MAX_RECENT);
    list
}

/// An uncompressed 32-bit BMP of an RGBA image (`--screenshot`; BMP
/// needs no encoder dependency and every OS opens it). `None` when the
/// pixel count does not match or the image is too large for the format.
pub fn bmp(width: usize, height: usize, rgba: &[u8]) -> Option<Vec<u8>> {
    let pixels = width.checked_mul(height)?;
    if rgba.len() != pixels.checked_mul(4)? {
        return None;
    }
    let size = u32::try_from(pixels.checked_mul(4)?.checked_add(54)?).ok()?;
    let (w, h) = (i32::try_from(width).ok()?, i32::try_from(height).ok()?);
    let mut out = Vec::with_capacity(usize::try_from(size).ok()?);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    // A negative height stores the rows top-down, as the image has them.
    out.extend_from_slice(&h.checked_neg()?.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    for &[r, g, b, a] in rgba.as_chunks::<4>().0 {
        out.extend_from_slice(&[b, g, r, a]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dsk_07_screenshot_bmp_has_header_and_bgra_pixels() {
        let img = bmp(2, 1, &[255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
        assert_eq!(&img[..2], b"BM");
        assert_eq!(img.len(), 54 + 8);
        assert_eq!(&img[54..62], &[0, 0, 255, 255, 255, 0, 0, 255]);
        assert!(bmp(2, 2, &[0; 8]).is_none());
        assert!(bmp(usize::MAX, 2, &[]).is_none());
    }

    #[test]
    fn dsk_02_atomic_write_replaces_and_leaves_no_temp_file() {
        let dir = scratch("atomic");
        let path = dir.join("plan.prswt");
        write_atomic(&path, "one").unwrap();
        write_atomic(&path, "two").unwrap();
        assert_eq!(read_text(&path, 100).unwrap(), "two");
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        assert!(write_atomic(&dir.join("missing").join("x.prswt"), "x").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dsk_02_reads_are_capped_and_lossy() {
        let dir = scratch("read");
        let path = dir.join("p.xer");
        std::fs::write(&path, b"ERMHDR\t\xe9t\xe9").unwrap();
        assert_eq!(read_text(&path, 100).unwrap(), "ERMHDR\t\u{fffd}t\u{fffd}");
        assert!(read_text(&path, 4).unwrap_err().contains("larger"));
        assert!(read_text(&dir.join("none"), 100).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn dsk_03_recent_list_is_bounded_and_tolerates_garbage() {
        let mut r = Vec::new();
        for i in 0..15 {
            push_recent(&mut r, Path::new(&format!("p{i}.prswt")));
        }
        push_recent(&mut r, Path::new("p10.prswt"));
        assert_eq!(r.len(), MAX_RECENT);
        assert_eq!(r[0], "p10.prswt");
        assert_eq!(r.iter().filter(|p| *p == "p10.prswt").count(), 1);
        assert!(parse_recent(Some("{not json")).is_empty());
        assert!(parse_recent(None).is_empty());
        assert_eq!(parse_recent(Some(r#"["a",""]"#)), ["a"]);
        assert_eq!(
            with_extension(PathBuf::from("x"), "prswt"),
            PathBuf::from("x.prswt")
        );
        assert_eq!(
            with_extension(PathBuf::from("x.xml"), "prswt"),
            PathBuf::from("x.xml")
        );
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("prswt-files-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
