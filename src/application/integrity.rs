//! Post-scrape integrity checks for downloaded media.
//!
//! Detects files that are present but likely incomplete:
//! - orphaned `.part` temp files (gallery-dl was interrupted mid-download)
//! - zero-byte files
//! - MP4-family files missing the `moov` atom (truncated/unplayable video —
//!   exactly what happens when a CDN connection dies near the end)

use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

/// What kind of integrity problem a file has (if any).
///
/// The variants deliberately separate "this download is broken, get rid of it"
/// from "this download is fine and the toolchain is the limitation". The
/// report used to print one list and one remedy for all of them, which told
/// users to delete complete HEVC videos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityIssue {
    /// `*.part` leftover — download was interrupted.
    PartialFile,
    /// File exists but is empty.
    Empty,
    /// MP4/MOV/M4V without a `moov` atom — truncated or unplayable.
    MissingMoov,
    /// Video track uses HEVC/H.265 — complete file, but many players
    /// (and stripped ffmpeg builds) cannot decode it.
    HevcCodec,
    /// The file was reported as downloaded but could not be read: permissions,
    /// a filesystem error, or an EIO mid-read. Nothing is known about whether
    /// the download itself is intact, so it must not be reported as truncated.
    Unreadable,
}

/// What the user should do about an [`IntegrityIssue`].
///
/// The remedy is the part that was wrong before: `HevcCodec` used to inherit
/// "delete the file and re-run" from the same bucket as `MissingMoov`, which
/// means deleting a complete download and re-fetching identical bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remedy {
    /// Incomplete or corrupt — deleting and re-running is the fix.
    Delete,
    /// Complete, but this tool will not play it. Leave it alone.
    LeaveIt,
    /// Cannot tell — the file could not be inspected at all.
    Unknown,
}

impl IntegrityIssue {
    /// The action that actually helps for this issue.
    pub fn remedy(&self) -> Remedy {
        match self {
            IntegrityIssue::PartialFile | IntegrityIssue::Empty | IntegrityIssue::MissingMoov => {
                Remedy::Delete
            }
            IntegrityIssue::HevcCodec => Remedy::LeaveIt,
            IntegrityIssue::Unreadable => Remedy::Unknown,
        }
    }
}

impl IntegrityIssue {
    pub fn description(&self) -> &'static str {
        match self {
            IntegrityIssue::PartialFile => "incomplete download (.part)",
            IntegrityIssue::Empty => "empty file (0 bytes)",
            IntegrityIssue::MissingMoov => "video truncated (no moov atom — not playable)",
            IntegrityIssue::HevcCodec => {
                "video is H.265/HEVC — complete but your player may not support it"
            }
            IntegrityIssue::Unreadable => "could not be read — completeness unknown",
        }
    }
}

/// Codec + resolution read from an MP4 container (pure byte parsing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaInfo {
    /// Sample-entry fourcc of the first video track: `avc1` (H.264),
    /// `hvc1`/`hev1` (H.265), etc.
    pub codec: String,
    pub width: u16,
    pub height: u16,
}

/// Media extensions checked for the MP4 `moov` atom.
const MOOV_EXTENSIONS: &[&str] = &["mp4", "mov", "m4v"];

/// Sample-entry fourccs that identify a video track.
const VIDEO_FOURCCS: &[&str] = &[
    "avc1", "avc3", // H.264
    "hvc1", "hev1", // H.265
    "av01", // AV1
    "vp09", // VP9
    "mp4v", // MPEG-4 part 2
];

/// One parsed ISO-BMFF box: fourcc, absolute offset, total size.
#[derive(Debug, Clone, Copy)]
struct BoxRef {
    kind: [u8; 4],
    offset: u64,
    size: u64,
}

impl BoxRef {
    fn kind(&self) -> &str {
        std::str::from_utf8(&self.kind).unwrap_or("????")
    }
}

/// Walk sibling boxes between `start..end`. Stops safely on malformed sizes.
fn walk_boxes(f: &mut std::fs::File, start: u64, end: u64) -> std::io::Result<Vec<BoxRef>> {
    let mut out = Vec::new();
    let mut off = start;
    while off + 8 <= end {
        f.seek(std::io::SeekFrom::Start(off))?;
        let mut hdr = [0u8; 8];
        if f.read_exact(&mut hdr).is_err() {
            break;
        }
        let mut size = u32::from_be_bytes(hdr[..4].try_into().unwrap_or([0; 4])) as u64;
        let header_len = 8u64;
        if size == 1 {
            let mut ext = [0u8; 8];
            if f.read_exact(&mut ext).is_err() {
                break;
            }
            size = u64::from_be_bytes(ext);
        } else if size == 0 {
            size = end - off; // box extends to container end
        }
        if size < header_len || off + size > end {
            break; // malformed — stop walking
        }
        out.push(BoxRef {
            kind: hdr[4..8].try_into().unwrap_or([b'?'; 4]),
            offset: off,
            size,
        });
        off += size;
    }
    Ok(out)
}

fn read_box_body(f: &mut std::fs::File, b: &BoxRef) -> std::io::Result<Vec<u8>> {
    let body = (b.size as usize).saturating_sub(8).min(1 << 20); // cap 1 MiB
    f.seek(std::io::SeekFrom::Start(b.offset + 8))?;
    let mut buf = vec![0u8; body];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

/// Extract codec + resolution from an MP4 file. `None` when the container
/// cannot be parsed or has no recognizable video track.
pub fn probe_mp4(path: &Path) -> Option<MediaInfo> {
    let mut f = std::fs::File::open(path).ok()?;
    let end = f.metadata().ok()?.len();

    let top = walk_boxes(&mut f, 0, end).ok()?;
    let moov = top.iter().find(|b| b.kind() == "moov")?.to_owned();
    let moov_end = moov.offset + moov.size;

    for trak in walk_boxes(&mut f, moov.offset + 8, moov_end).ok()? {
        if trak.kind() != "trak" {
            continue;
        }
        let trak_end = trak.offset + trak.size;
        // Resolution lives in tkhd: width/height are the last 8 bytes
        // (two 16.16 fixed-point values), same layout for box versions 0/1.
        let mut width = 0u16;
        let mut height = 0u16;
        for child in walk_boxes(&mut f, trak.offset + 8, trak_end).ok()? {
            if child.kind() != "tkhd" {
                continue;
            }
            let body = read_box_body(&mut f, &child).ok()?;
            if body.len() >= 8 {
                let w = u32::from_be_bytes(body[body.len() - 8..body.len() - 4].try_into().ok()?);
                let h = u32::from_be_bytes(body[body.len() - 4..].try_into().ok()?);
                width = (w >> 16) as u16;
                height = (h >> 16) as u16;
            }
        }
        // Codec lives in mdia→minf→stbl→stsd first sample entry.
        for mdia in walk_boxes(&mut f, trak.offset + 8, trak_end).ok()? {
            if mdia.kind() != "mdia" {
                continue;
            }
            let mdia_end = mdia.offset + mdia.size;
            for minf in walk_boxes(&mut f, mdia.offset + 8, mdia_end).ok()? {
                if minf.kind() != "minf" {
                    continue;
                }
                let minf_end = minf.offset + minf.size;
                for stbl in walk_boxes(&mut f, minf.offset + 8, minf_end).ok()? {
                    if stbl.kind() != "stbl" {
                        continue;
                    }
                    let stbl_end = stbl.offset + stbl.size;
                    for stsd in walk_boxes(&mut f, stbl.offset + 8, stbl_end).ok()? {
                        if stsd.kind() != "stsd" {
                            continue;
                        }
                        let body = read_box_body(&mut f, &stsd).ok()?;
                        // body: version+flags(4) entry_count(4) entry{size(4) fourcc(4)...}
                        if body.len() >= 16 {
                            let fourcc = String::from_utf8_lossy(&body[12..16]).into_owned();
                            if VIDEO_FOURCCS.contains(&fourcc.as_str()) {
                                return Some(MediaInfo {
                                    codec: fourcc,
                                    width,
                                    height,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

fn extension_is(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| exts.contains(&e.to_ascii_lowercase().as_str()))
}

/// Check one downloaded media file for signs of incompleteness.
///
/// A read failure is reported as [`IntegrityIssue::Unreadable`] rather than
/// folded into `MissingMoov`: a file whose bytes cannot be inspected tells us
/// nothing about whether it is truncated, and the remedy for the two is not the
/// same — one is "re-download", the other is "check permissions".
pub fn check_file(path: &Path) -> Option<IntegrityIssue> {
    if path.to_string_lossy().ends_with(".part") {
        return Some(IntegrityIssue::PartialFile);
    }
    let md = match std::fs::metadata(path) {
        Ok(md) => md,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Not necessarily a missing download: the lines this is fed come
            // from gallery-dl's stdout, so some are not paths at all. Keep the
            // old behaviour of staying quiet rather than reporting a phantom.
            return None;
        }
        Err(_) => {
            // The path exists but cannot be inspected — permissions, a failing
            // filesystem, EIO. We learn nothing about whether the download is
            // intact, and the remedies for "truncated" and "cannot read" are
            // different.
            return Some(IntegrityIssue::Unreadable);
        }
    };
    if md.len() == 0 {
        return Some(IntegrityIssue::Empty);
    }
    if md.is_dir() {
        return None;
    }
    if !extension_is(path, MOOV_EXTENSIONS) {
        return None;
    }
    // MP4 family: playable files contain a `moov` box. Search by stream in
    // chunks with a 4-byte overlap so an atom name split across chunk
    // boundaries is still found.
    match contains_moov(path) {
        MoovScan::Found => {}
        MoovScan::Absent => return Some(IntegrityIssue::MissingMoov),
        MoovScan::Unreadable => return Some(IntegrityIssue::Unreadable),
    }
    // Structurally complete — flag HEVC video (complete but often undecodable).
    if let Some(info) = probe_mp4(path)
        && (info.codec == "hvc1" || info.codec == "hev1")
    {
        return Some(IntegrityIssue::HevcCodec);
    }
    None
}

/// Aggregate `codec resolution ×count` for the media files reported by a run.
/// Returns `None` when nothing could be probed (e.g. all images, or CI mode
/// where no paths were captured).
pub fn quality_summary(reported_paths: &[String]) -> Option<String> {
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for line in reported_paths {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let path = Path::new(line);
        if !extension_is(path, MOOV_EXTENSIONS) {
            continue;
        }
        if let Some(info) = probe_mp4(path) {
            *counts
                .entry(format!("{}x{} {}", info.width, info.height, info.codec))
                .or_default() += 1;
        }
    }
    if counts.is_empty() {
        return None;
    }
    let parts: Vec<String> = counts
        .into_iter()
        .map(|(k, n)| format!("{k} ×{n}"))
        .collect();
    Some(format!("quality: {}", parts.join(", ")))
}

/// Outcome of scanning a file for the `moov` atom.
enum MoovScan {
    Found,
    /// Scanned the whole file and it is not there.
    Absent,
    /// Could not scan — open or read failed. Distinct from `Absent`, because
    /// the two call for opposite advice.
    Unreadable,
}

/// Streamed search for the byte pattern `moov` inside a file — zero-alloc.
fn contains_moov(path: &Path) -> MoovScan {
    use std::sync::OnceLock;
    static FINDER: OnceLock<memchr::memmem::Finder> = OnceLock::new();
    let finder = FINDER.get_or_init(|| memchr::memmem::Finder::new(b"moov"));
    const CHUNK: usize = 64 * 1024;
    let Ok(mut f) = std::fs::File::open(path) else {
        return MoovScan::Unreadable;
    };
    let mut buf = [0u8; CHUNK];
    let mut tail = [0u8; 3];
    let mut tail_len: usize = 0;
    loop {
        let n = match f.read(&mut buf) {
            Ok(0) => return MoovScan::Absent,
            Ok(n) => n,
            Err(_) => return MoovScan::Unreadable,
        };
        // Check boundary crossing (tail + buf prefix) without large alloc
        if tail_len > 0 {
            let mut boundary = [0u8; 6];
            boundary[..tail_len].copy_from_slice(&tail[..tail_len]);
            let take = n.min(3);
            boundary[tail_len..tail_len + take].copy_from_slice(&buf[..take]);
            if finder.find(&boundary[..tail_len + take]).is_some() {
                return MoovScan::Found;
            }
        }
        if finder.find(&buf[..n]).is_some() {
            return MoovScan::Found;
        }
        // Keep last 3 bytes for next iteration
        if n >= 3 {
            tail.copy_from_slice(&buf[n - 3..n]);
            tail_len = 3;
        } else {
            // n < 3: need to shift tail and append
            let mut new_tail = [0u8; 3];
            let keep = tail_len.min(3 - n);
            if keep > 0 {
                new_tail[..keep].copy_from_slice(&tail[tail_len - keep..tail_len]);
            }
            new_tail[keep..keep + n].copy_from_slice(&buf[..n]);
            tail = new_tail;
            tail_len = (tail_len + n).min(3);
        }
    }
}

/// Recursively find `*.part` leftovers under `dir` (interrupted downloads).
/// Cheap walk used in addition to the per-file checks above; catches files
/// gallery-dl never finished even if they were not reported this run.
pub fn scan_partials(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e == "part") {
                found.push(p);
            }
        }
    }
    found.sort();
    found
}

/// Full post-scrape report: checks every path gallery-dl reported as
/// downloaded this run, plus an orphaned `.part` sweep of the output dir.
///
/// Lines starting with `#` (gallery-dl's "skipped/already have" marker) are
/// ignored so pre-existing files are never re-read needlessly.
///
/// Returns `(checked, issues)`. The issue is returned rather than its
/// description string so callers can group by [`IntegrityIssue::remedy`] and
/// give each group the advice that actually applies.
pub fn verify_run(
    reported_paths: &[String],
    output_dir: Option<&Path>,
) -> (usize, Vec<(PathBuf, IntegrityIssue)>) {
    let mut issues: Vec<(PathBuf, IntegrityIssue)> = Vec::new();
    let mut checked = 0usize;

    for line in reported_paths {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let path = Path::new(line);
        checked += 1;
        if let Some(issue) = check_file(path) {
            issues.push((path.to_path_buf(), issue));
        }
    }

    if let Some(dir) = output_dir {
        for part in scan_partials(dir) {
            if !issues.iter().any(|(p, _)| *p == part) {
                issues.push((part, IntegrityIssue::PartialFile));
            }
        }
    }
    // By path only: the issue is diagnostic data, not part of the ordering.
    issues.sort_by(|a, b| a.0.cmp(&b.0));
    (checked, issues)
}

/// One file flagged during the post-run check, with the reason.
pub type IssueEntry = (PathBuf, IntegrityIssue);

/// Split a run's issues into the three advice buckets.
///
/// Returned as `(delete, leave, unreadable)` so a caller can print one list
/// per remedy instead of one list and one blanket instruction.
pub fn partition_by_remedy(
    issues: &[IssueEntry],
) -> (Vec<IssueEntry>, Vec<IssueEntry>, Vec<IssueEntry>) {
    let mut delete = Vec::new();
    let mut leave = Vec::new();
    let mut unknown = Vec::new();
    for (path, issue) in issues {
        match issue.remedy() {
            Remedy::Delete => delete.push((path.clone(), *issue)),
            Remedy::LeaveIt => leave.push((path.clone(), *issue)),
            Remedy::Unknown => unknown.push((path.clone(), *issue)),
        }
    }
    (delete, leave, unknown)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{
        IntegrityIssue, Remedy, check_file, partition_by_remedy, scan_partials, verify_run,
    };
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn partial_file_flagged() {
        let dir = TempDir::new().expect("tempdir");
        let p = dir.path().join("123_2026-01-01.mp4.part");
        std::fs::write(&p, b"half written data").expect("write");
        assert_eq!(check_file(&p), Some(IntegrityIssue::PartialFile));
    }

    #[test]
    fn empty_file_flagged() {
        let dir = TempDir::new().expect("tempdir");
        let p = dir.path().join("123_2026-01-01.mp4");
        std::fs::File::create(&p).expect("create empty");
        assert_eq!(check_file(&p), Some(IntegrityIssue::Empty));
    }

    #[test]
    fn mp4_with_moov_ok() {
        let dir = TempDir::new().expect("tempdir");
        let p = dir.path().join("ok.mp4");
        // Minimal fake: moov split across the 1MB chunk boundary must be found too
        let mut f = std::fs::File::create(&p).expect("create");
        f.write_all(&vec![0u8; 1024 * 1024 - 2]).expect("pad");
        f.write_all(b"mo").expect("half marker");
        f.write_all(b"ov").expect("other half marker");
        f.write_all(&[0xde, 0xad]).expect("tail");
        assert_eq!(check_file(&p), None);
    }

    #[test]
    fn truncated_mp4_without_moov_flagged() {
        let dir = TempDir::new().expect("tempdir");
        let p = dir.path().join("cut.mp4");
        std::fs::write(&p, vec![0xff; 4096]).expect("write garbage without moov");
        assert_eq!(check_file(&p), Some(IntegrityIssue::MissingMoov));
    }

    #[test]
    fn non_mp4_media_skips_moov_check() {
        let dir = TempDir::new().expect("tempdir");
        let p = dir.path().join("photo.jpg");
        std::fs::write(&p, vec![0xff; 512]).expect("write");
        assert_eq!(check_file(&p), None);
    }

    #[test]
    fn missing_file_returns_none_not_error() {
        let dir = TempDir::new().expect("tempdir");
        assert_eq!(check_file(&dir.path().join("ghost.mp4")), None);
    }

    /// A file that exists but cannot be inspected is neither "truncated" nor
    /// "fine" — the old code reported it as `MissingMoov`, which told the user
    /// to delete a file that might be perfectly intact.
    #[cfg(unix)]
    #[test]
    fn unreadable_file_is_not_reported_as_truncated() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().expect("tempdir");
        let p = dir.path().join("locked.mp4");
        // A well-formed MP4 payload: the only thing wrong is readability, so
        // any "truncated" verdict here would be a false alarm.
        let mut f = std::fs::File::create(&p).expect("create");
        f.write_all(b"\x00\x00\x00\x18ftypmp42moov").expect("seed");
        drop(f);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).expect("lock");

        let got = check_file(&p);
        // Restore before asserting so the TempDir can clean itself up.
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).expect("unlock");

        if got.is_none() {
            // Running as root, or a filesystem that ignores the mode bits:
            // there is no unreadable file to observe, so nothing to prove.
            eprintln!("skipping: this process can read a 0o000 file");
            return;
        }
        assert_eq!(got, Some(IntegrityIssue::Unreadable));
    }

    /// The grouping the report is built on: only genuinely broken downloads
    /// get told to delete and re-run.
    #[test]
    fn remedy_separates_deletable_from_complete() {
        assert_eq!(IntegrityIssue::PartialFile.remedy(), Remedy::Delete);
        assert_eq!(IntegrityIssue::Empty.remedy(), Remedy::Delete);
        assert_eq!(IntegrityIssue::MissingMoov.remedy(), Remedy::Delete);

        // The regression: a complete HEVC video must never inherit
        // "delete the file and re-run".
        assert_eq!(IntegrityIssue::HevcCodec.remedy(), Remedy::LeaveIt);
        assert_eq!(IntegrityIssue::Unreadable.remedy(), Remedy::Unknown);
    }

    #[test]
    fn partition_by_remedy_splits_the_three_groups() {
        let issues = vec![
            (PathBuf::from("a.part"), IntegrityIssue::PartialFile),
            (PathBuf::from("b.mp4"), IntegrityIssue::MissingMoov),
            (PathBuf::from("c.mp4"), IntegrityIssue::HevcCodec),
            (PathBuf::from("d.mp4"), IntegrityIssue::Unreadable),
        ];
        let (delete, leave, unknown) = partition_by_remedy(&issues);

        let names = |v: &Vec<(PathBuf, IntegrityIssue)>| -> Vec<String> {
            v.iter().map(|(p, _)| p.display().to_string()).collect()
        };
        assert_eq!(names(&delete), vec!["a.part", "b.mp4"]);
        assert_eq!(names(&leave), vec!["c.mp4"]);
        assert_eq!(names(&unknown), vec!["d.mp4"]);
    }

    #[test]
    fn scan_partials_finds_nested() {
        let dir = TempDir::new().expect("tempdir");
        let nested = dir.path().join("root/tiktok/user/videos");
        std::fs::create_dir_all(&nested).expect("mkdirs");
        std::fs::write(nested.join("a.mp4.part"), b"x").expect("w1");
        std::fs::write(dir.path().join("b.part"), b"y").expect("w2");
        std::fs::write(nested.join("good.mp4"), b"data").expect("w3");

        let found = scan_partials(dir.path());
        assert_eq!(found.len(), 2, "found: {found:?}");
        assert!(found[0].to_string_lossy().contains("b.part"));
    }

    #[test]
    fn scan_partials_empty_when_clean() {
        let dir = TempDir::new().expect("tempdir");
        assert!(scan_partials(dir.path()).is_empty());
    }

    #[test]
    fn verify_run_skips_gallerydl_skip_marker_lines() {
        let dir = TempDir::new().expect("tempdir");
        let good = dir.path().join("good.mp4");
        std::fs::write(&good, b"datamoovdata").expect("write");

        // "# " prefix means gallery-dl skipped it — must not be checked
        // (otherwise every pre-existing big video would be re-read each run)
        let (checked, issues) = verify_run(&[format!("# {}", good.display())], Some(dir.path()));
        assert_eq!(checked, 0);
        assert!(issues.is_empty());
    }

    #[test]
    fn verify_run_reports_truncated_and_partials() {
        let dir = TempDir::new().expect("tempdir");
        let cut = dir.path().join("cut.mp4");
        std::fs::write(&cut, vec![0xaa; 1024]).expect("write truncated mp4");
        let part = dir.path().join("sub.part");
        std::fs::create_dir(dir.path().join("sub")).expect("mkdir");
        std::fs::write(&part, b"half").expect("write part");
        let good = dir.path().join("good.mp4");
        std::fs::write(&good, b"xmoovx").expect("write good");

        let (checked, issues) = verify_run(
            &[
                cut.display().to_string(),
                good.display().to_string(),
                "# skipped.mp4".to_string(),
            ],
            Some(dir.path()),
        );
        assert_eq!(checked, 2); // cut + good; "# skipped" ignored
        assert_eq!(issues.len(), 2, "issues: {issues:?}"); // cut.mp4 + sub.part
        assert!(
            issues
                .iter()
                .any(|(p, i)| p == &cut && *i == IntegrityIssue::MissingMoov)
        );
        assert!(
            issues
                .iter()
                .any(|(p, i)| *p == part && *i == IntegrityIssue::PartialFile)
        );
    }

    // --- MP4 box parser (probe_mp4 / quality_summary / HevcCodec) ---

    /// Assemble an ISO-BMFF box: [size u32][fourcc][body].
    fn box_of(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + body.len());
        out.extend_from_slice(&((8 + body.len()) as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out
    }

    /// Minimal but structurally valid MP4 with one video track.
    fn fake_mp4(fourcc: &[u8; 4], width: u16, height: u16) -> Vec<u8> {
        // tkhd: version+flags + payload; width/height are the LAST 8 bytes
        // as two 16.16 fixed-point values.
        let mut tkhd_body = vec![0u8; 4 + 76];
        let w = (width as u32) << 16;
        let h = (height as u32) << 16;
        let n = tkhd_body.len();
        tkhd_body[n - 8..n - 4].copy_from_slice(&w.to_be_bytes());
        tkhd_body[n - 4..].copy_from_slice(&h.to_be_bytes());
        let tkhd = box_of(b"tkhd", &tkhd_body);

        // stsd: version+flags(4) entry_count(4) entry{size(4) fourcc(4)...}
        let mut entry = Vec::new();
        entry.extend_from_slice(&64u32.to_be_bytes());
        entry.extend_from_slice(fourcc);
        entry.extend_from_slice(&[0u8; 56]);
        let mut stsd_body = vec![0u8; 8];
        stsd_body.extend_from_slice(&entry);
        let stsd = box_of(b"stsd", &stsd_body);

        let stbl = box_of(b"stbl", &stsd);
        let minf = box_of(b"minf", &stbl);
        let mdia = box_of(b"mdia", &minf);
        let trak = box_of(b"trak", &[tkhd, mdia].concat());
        let moov = box_of(b"moov", &trak);

        let ftyp = box_of(b"ftyp", b"isom");
        [ftyp, moov, box_of(b"mdat", &[0u8; 128])].concat()
    }

    #[test]
    fn probe_mp4_reads_codec_and_resolution() {
        let dir = TempDir::new().expect("tempdir");
        let p = dir.path().join("v.mp4");
        std::fs::write(&p, fake_mp4(b"avc1", 1080, 1920)).expect("write");
        let info = super::probe_mp4(&p).expect("probe");
        assert_eq!(info.codec, "avc1");
        assert_eq!(info.width, 1080);
        assert_eq!(info.height, 1920);
    }

    #[test]
    fn probe_mp4_detects_hevc() {
        let dir = TempDir::new().expect("tempdir");
        let p = dir.path().join("hevc.mp4");
        std::fs::write(&p, fake_mp4(b"hvc1", 720, 1280)).expect("write");
        let info = super::probe_mp4(&p).expect("probe");
        assert_eq!(info.codec, "hvc1");
    }

    #[test]
    fn hevc_file_flagged_as_issue_but_not_corrupt() {
        let dir = TempDir::new().expect("tempdir");
        let p = dir.path().join("hevc.mp4");
        std::fs::write(&p, fake_mp4(b"hvc1", 720, 1280)).expect("write");
        assert_eq!(
            check_file(&p),
            Some(IntegrityIssue::HevcCodec),
            "complete HEVC file must be flagged as codec issue, not corruption"
        );
    }

    #[test]
    fn h264_file_passes_all_checks() {
        let dir = TempDir::new().expect("tempdir");
        let p = dir.path().join("ok.mp4");
        let mut data = fake_mp4(b"avc1", 1080, 1920);
        data.extend_from_slice(b"moov-marker-elsewhere"); // moov present anyway
        std::fs::write(&p, data).expect("write");
        assert_eq!(check_file(&p), None);
    }

    #[test]
    fn quality_summary_aggregates_counts() {
        let dir = TempDir::new().expect("tempdir");
        let a = dir.path().join("a.mp4");
        let b = dir.path().join("b.mp4");
        std::fs::write(&a, fake_mp4(b"avc1", 1080, 1920)).expect("w a");
        std::fs::write(&b, fake_mp4(b"hvc1", 720, 1280)).expect("w b");

        let summary = super::quality_summary(&[
            a.display().to_string(),
            b.display().to_string(),
            "# skipped".to_string(),
            dir.path().join("photo.jpg").display().to_string(),
        ])
        .expect("summary");
        assert!(summary.contains("1080x1920 avc1 ×1"), "{summary}");
        assert!(summary.contains("720x1280 hvc1 ×1"), "{summary}");
        assert!(!summary.contains("jpg"));
    }

    #[test]
    fn quality_summary_none_without_media() {
        let dir = TempDir::new().expect("tempdir");
        assert_eq!(super::quality_summary(&[]), None);
        let jpg = dir.path().join("x.jpg");
        std::fs::write(&jpg, b"data").expect("w");
        assert_eq!(super::quality_summary(&[jpg.display().to_string()]), None);
    }
}
