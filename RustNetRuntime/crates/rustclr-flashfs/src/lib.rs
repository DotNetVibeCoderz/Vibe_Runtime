//! A filesystem for raw NOR flash: named blobs in a log, newest wins.
//!
//! ``rustclr-fs`` — the VFS, FAT-over-block-device and encrypted overlay the
//! virtual device uses — is `std`-bound, and `fatfs` does not come apart from
//! `std` without a `core_io` shim. This is the bare-metal alternative: a
//! purpose-built store for what a microcontroller application actually asks of
//! a filesystem, which is to name a blob, write it, read it back, and list what
//! is there.
//!
//! Depends on nothing but [`ExtMemory`], so it stays in the host workspace and
//! its test run while also building for a bare-metal target.
//!
//! ## The format
//!
//! ```text
//! magic u32 | flags u32 | name_len u32 | data_len u32 | name[] pad4 | data[] pad4
//! ```
//!
//! Writing appends. Deleting appends a tombstone. Reading scans for the newest
//! record with a matching name; a tombstone found first means "not there".
//! Since erased NOR reads as `0xFFFFFFFF`, a header whose magic does not match
//! ends the scan — so a scan costs what is *used*, not what is reserved.
//!
//! Directories are not objects, they are prefixes. [`create_directory`] writes
//! an empty marker so an empty directory can still be listed, and [`list`]
//! returns the immediate children of a prefix.
//!
//! ## Why append rather than overwrite
//!
//! NOR flash clears bits and cannot set them; the only way back is erasing a
//! whole sector. Rewriting in place would mean read-erase-write of the
//! surrounding sector on every save — slow, and a power-loss window over data
//! that was previously fine. Appending costs nothing until the window fills,
//! and then one compaction reclaims every superseded record at once.
//!
//! That same asymmetry is why [`blank`] checks the *whole* span a record will
//! occupy before writing it. A region that is erased at the front and written
//! further in passes a short probe, and the write then ANDs into live data with
//! no error reported anywhere. That is not hypothetical: it is what a stock
//! Sipeed Maix Go did to an application stored on it, and the corruption
//! surfaced only after the next power cycle.
//!
//! ## What it is not
//!
//! No handles, no seeking, no partial writes: a file is rewritten whole.
//! [`append_bytes`] reads, concatenates and writes a new record, which is right
//! for a log line and wrong for a megabyte. Nothing here is a journal — though
//! a reset midway through an append does leave the previous version intact,
//! which is the useful half of a transaction and comes for free.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use rustclr_hal::extmem::ExtMemory;

/// "RNF1" little-endian. Any other value ends the scan.
const MAGIC: u32 = 0x3146_4E52;
const HEADER: u32 = 16;

/// This record supersedes an earlier one with the same name and means the file
/// is gone. Tombstones are records like any other, so deleting costs a write
/// rather than an erase.
const FLAG_DELETED: u32 = 1 << 0;
/// A directory marker: exists so an empty directory can be listed.
const FLAG_DIRECTORY: u32 = 1 << 1;

/// Longest path stored. Bounded so a corrupt length cannot make the scanner
/// allocate wildly before the magic check catches up with it.
pub const MAX_NAME: u32 = 255;

/// Largest single file. Generous next to a multi-megabyte window, small enough
/// that a bad length is obvious.
pub const MAX_FILE: u32 = 1024 * 1024;

fn align4(n: u32) -> u32 {
    (n + 3) & !3
}

/// Normalise a path: leading slash optional, no trailing slash, no empty or
/// `.` segments, `\` accepted as a separator.
///
/// Done in one place because two spellings of one path would otherwise be two
/// files, and the bug would only show up when a [`list`] disagreed with a
/// [`read`].
pub fn normalise(path: &str) -> Result<String, String> {
    let mut out = String::new();
    for segment in path.split(['/', '\\']) {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return Err(String::from("'..' is not supported"));
        }
        out.push('/');
        out.push_str(segment);
    }
    if out.is_empty() {
        out.push('/');
    }
    if out.len() as u32 > MAX_NAME {
        return Err(format!("path longer than {MAX_NAME} bytes"));
    }
    Ok(out)
}

struct Record {
    flags: u32,
    name: String,
    data_at: u32,
    data_len: u32,
}

/// Walk the log in write order. A caller that keeps the last match per name
/// gets "newest wins" for free. Returns the offset just past the last valid
/// record — where the next append goes.
fn scan<F: FnMut(&Record)>(region: &mut dyn ExtMemory, mut visit: F) -> Result<u32, String> {
    let capacity = region.size();
    let mut at = 0u32;

    while at + HEADER <= capacity {
        let mut header = [0u8; HEADER as usize];
        region.read(at, &mut header).map_err(|e| format!("{e}"))?;
        let word =
            |i: usize| u32::from_le_bytes([header[i], header[i + 1], header[i + 2], header[i + 3]]);
        if word(0) != MAGIC {
            break;
        }
        let flags = word(4);
        let name_len = word(8);
        let data_len = word(12);
        if name_len > MAX_NAME || data_len > MAX_FILE {
            break;
        }
        let name_at = at + HEADER;
        let data_at = name_at + align4(name_len);
        let next = data_at + align4(data_len);
        if next > capacity {
            // A truncated tail — a reset mid-write — ends the log rather than
            // being trusted.
            break;
        }

        let mut name = vec![0u8; name_len as usize];
        region.read(name_at, &mut name).map_err(|e| format!("{e}"))?;
        if let Ok(name) = String::from_utf8(name) {
            visit(&Record { flags, name, data_at, data_len });
        }
        at = next;
    }
    Ok(at)
}

/// The newest record for `path`, live or tombstoned.
fn newest(region: &mut dyn ExtMemory, path: &str) -> Result<Option<(u32, u32, u32)>, String> {
    let mut found: Option<(u32, u32, u32)> = None;
    scan(region, |r| {
        if r.name == path {
            found = Some((r.flags, r.data_at, r.data_len));
        }
    })?;
    Ok(found)
}

fn newest_live(region: &mut dyn ExtMemory, path: &str) -> Result<Option<(u32, u32)>, String> {
    Ok(newest(region, path)?
        .filter(|(flags, _, _)| flags & FLAG_DELETED == 0)
        .map(|(_, at, len)| (at, len)))
}

pub fn exists(region: &mut dyn ExtMemory, path: &str) -> Result<bool, String> {
    let path = normalise(path)?;
    Ok(newest_live(region, &path)?.is_some())
}

pub fn read(region: &mut dyn ExtMemory, path: &str) -> Result<Vec<u8>, String> {
    let path = normalise(path)?;
    let (at, len) = newest_live(region, &path)?.ok_or_else(|| format!("no such file: {path}"))?;
    let mut data = vec![0u8; len as usize];
    region.read(at, &mut data).map_err(|e| format!("{e}"))?;
    Ok(data)
}

pub fn write(region: &mut dyn ExtMemory, path: &str, data: &[u8]) -> Result<(), String> {
    let path = normalise(path)?;
    append(region, &path, data, 0)
}

pub fn append_bytes(region: &mut dyn ExtMemory, path: &str, extra: &[u8]) -> Result<(), String> {
    let path = normalise(path)?;
    let mut data = match newest_live(region, &path)? {
        Some((at, len)) => {
            let mut existing = vec![0u8; len as usize];
            region.read(at, &mut existing).map_err(|e| format!("{e}"))?;
            existing
        }
        None => Vec::new(),
    };
    data.extend_from_slice(extra);
    append(region, &path, &data, 0)
}

pub fn delete(region: &mut dyn ExtMemory, path: &str) -> Result<(), String> {
    let path = normalise(path)?;
    if newest_live(region, &path)?.is_none() {
        // Deleting what is not there is not an error, and a tombstone for it
        // would burn flash for nothing.
        return Ok(());
    }
    append(region, &path, &[], FLAG_DELETED)
}

pub fn create_directory(region: &mut dyn ExtMemory, path: &str) -> Result<(), String> {
    let path = normalise(path)?;
    if newest_live(region, &path)?.is_some() {
        return Ok(());
    }
    append(region, &path, &[], FLAG_DIRECTORY)
}

/// Immediate children of `path`, sorted, directories included.
pub fn list(region: &mut dyn ExtMemory, path: &str) -> Result<Vec<String>, String> {
    let prefix = normalise(path)?;
    // Root is already its own separator; anything else needs one appended so
    // that "/log" does not match "/logbook".
    let prefix = if prefix == "/" { prefix } else { format!("{prefix}/") };

    let mut live: Vec<(String, bool)> = Vec::new();
    scan(region, |r| {
        let deleted = r.flags & FLAG_DELETED != 0;
        match live.iter_mut().find(|(name, _)| *name == r.name) {
            Some(entry) => entry.1 = deleted,
            None => live.push((r.name.clone(), deleted)),
        }
    })?;

    let mut names: Vec<String> = Vec::new();
    for (name, deleted) in live {
        if deleted || !name.starts_with(prefix.as_str()) {
            continue;
        }
        let rest = &name[prefix.len()..];
        if rest.is_empty() {
            continue;
        }
        // Only the immediate child: everything below a subdirectory collapses
        // to that subdirectory's own name.
        let child = match rest.find('/') {
            Some(cut) => &rest[..cut],
            None => rest,
        }
        .to_string();
        if !names.contains(&child) {
            names.push(child);
        }
    }
    names.sort();
    Ok(names)
}

/// Bytes of the window currently in use.
pub fn used(region: &mut dyn ExtMemory) -> u32 {
    scan(region, |_| {}).unwrap_or(0)
}

fn record_bytes(name: &str, data: &[u8], flags: u32) -> Vec<u8> {
    let mut record =
        Vec::with_capacity((HEADER + align4(name.len() as u32) + align4(data.len() as u32)) as usize);
    record.extend_from_slice(&MAGIC.to_le_bytes());
    record.extend_from_slice(&flags.to_le_bytes());
    record.extend_from_slice(&(name.len() as u32).to_le_bytes());
    record.extend_from_slice(&(data.len() as u32).to_le_bytes());
    record.extend_from_slice(name.as_bytes());
    while record.len() % 4 != 0 {
        record.push(0xFF);
    }
    record.extend_from_slice(data);
    while record.len() % 4 != 0 {
        record.push(0xFF);
    }
    record
}

fn append(region: &mut dyn ExtMemory, path: &str, data: &[u8], flags: u32) -> Result<(), String> {
    if data.len() as u32 > MAX_FILE {
        return Err(format!("file larger than {MAX_FILE} bytes"));
    }
    let record = record_bytes(path, data, flags);
    let capacity = region.size();

    let mut end = scan(region, |_| {})?;
    if end + record.len() as u32 > capacity || !blank(region, end, record.len() as u32)? {
        compact(region, record.len() as u32)?;
        end = scan(region, |_| {})?;
        if end + record.len() as u32 > capacity {
            return Err(String::from("filesystem full even after compaction"));
        }
    }
    put(region, end, &record)
}

/// Is the whole span the record will occupy erased?
///
/// The whole span, not its first bytes — see the crate note on why a short
/// probe is not enough.
pub fn blank(region: &mut dyn ExtMemory, at: u32, len: u32) -> Result<bool, String> {
    if at.saturating_add(len) > region.size() {
        return Ok(false);
    }
    let mut probe = [0u8; 256];
    let mut done = 0u32;
    while done < len {
        let step = (len - done).min(probe.len() as u32) as usize;
        region.read(at + done, &mut probe[..step]).map_err(|e| format!("{e}"))?;
        if probe[..step].iter().any(|b| *b != 0xFF) {
            return Ok(false);
        }
        done += step as u32;
    }
    Ok(true)
}

/// Write a record and read it back, because NOR reports nothing when a program
/// lands badly.
fn put(region: &mut dyn ExtMemory, at: u32, record: &[u8]) -> Result<(), String> {
    region.write(at, record).map_err(|e| format!("{e}"))?;

    let mut check = vec![0u8; record.len()];
    region.read(at, &mut check).map_err(|e| format!("{e}"))?;
    if let Some(bad) = record.iter().zip(check.iter()).position(|(a, b)| a != b) {
        return Err(format!(
            "flash verify failed at offset {}: wrote {:#04x}, read {:#04x}",
            at + bad as u32,
            record[bad],
            check[bad]
        ));
    }
    Ok(())
}

/// Erase the used part of the window and write back only the live records.
///
/// Reads the live set into RAM first, which bounds compaction by the heap
/// rather than by the window — comfortable on a chip with megabytes of it.
///
/// **Only the used part.** Erasing the whole region is the obvious spelling and
/// it is badly wrong at this scale: a sector erase costs tens to hundreds of
/// milliseconds, so wiping a 15 MB window means thousands of them — minutes of
/// the device not answering, which presents to the tools as a board that has
/// died mid-upload. What has to be erased is what is written, plus room for the
/// record that triggered this, rounded out to whole sectors.
fn compact(region: &mut dyn ExtMemory, incoming: u32) -> Result<(), String> {
    let used = scan(region, |_| {})?;
    let mut live: Vec<(String, u32, u32, u32)> = Vec::new();
    scan(region, |r| {
        let entry = (r.name.clone(), r.flags, r.data_at, r.data_len);
        match live.iter_mut().find(|(name, ..)| *name == r.name) {
            Some(slot) => *slot = entry,
            None => live.push(entry),
        }
    })?;
    live.retain(|(_, flags, _, _)| flags & FLAG_DELETED == 0);

    let mut carried: Vec<(String, u32, Vec<u8>)> = Vec::with_capacity(live.len());
    for (name, flags, at, len) in live {
        let mut data = vec![0u8; len as usize];
        region.read(at, &mut data).map_err(|e| format!("{e}"))?;
        carried.push((name, flags, data));
    }

    let sector = region.sector_size().max(1);
    let wanted = used.saturating_add(incoming);
    let span = wanted.div_ceil(sector).saturating_mul(sector).min(region.size());
    region.erase(0, span).map_err(|e| format!("{e}"))?;

    let mut at = 0u32;
    for (name, flags, data) in carried {
        let record = record_bytes(&name, &data, flags);
        put(region, at, &record)?;
        at += record.len() as u32;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustclr_hal::extmem::ExtMemKind;
    use rustclr_hal::{HalError, HalResult};

    const SECTOR: u32 = 4096;

    /// A NOR part, modelled honestly: **programming only clears bits**, and the
    /// only way to set one again is erasing its whole sector. A `Vec<u8>` that
    /// simply stored what you wrote would pass every test here while hiding the
    /// exact failure this crate exists to prevent.
    struct FakeNor {
        cells: Vec<u8>,
        /// Sectors erased so far. Erasing is the slow operation on real NOR —
        /// tens to hundreds of milliseconds each — so counting them is the only
        /// way a host test can notice a compaction that would stall a device.
        erased_sectors: u32,
    }

    impl FakeNor {
        fn new(size: u32) -> Self {
            Self { cells: vec![0xFF; size as usize], erased_sectors: 0 }
        }
    }

    impl ExtMemory for FakeNor {
        fn kind(&self) -> ExtMemKind {
            ExtMemKind::QspiFlash
        }
        fn size(&self) -> u32 {
            self.cells.len() as u32
        }
        fn read(&mut self, addr: u32, buf: &mut [u8]) -> HalResult<()> {
            let end = addr as usize + buf.len();
            if end > self.cells.len() {
                return Err(HalError::InvalidArgument("read past the end"));
            }
            buf.copy_from_slice(&self.cells[addr as usize..end]);
            Ok(())
        }
        fn write(&mut self, addr: u32, data: &[u8]) -> HalResult<()> {
            let end = addr as usize + data.len();
            if end > self.cells.len() {
                return Err(HalError::InvalidArgument("write past the end"));
            }
            for (cell, byte) in self.cells[addr as usize..end].iter_mut().zip(data) {
                *cell &= *byte;
            }
            Ok(())
        }
        fn erase(&mut self, addr: u32, len: u32) -> HalResult<()> {
            let start = (addr - addr % SECTOR) as usize;
            let end = (((addr + len).div_ceil(SECTOR)) * SECTOR).min(self.size()) as usize;
            self.erased_sectors += ((end - start) as u32).div_ceil(SECTOR);
            self.cells[start..end].fill(0xFF);
            Ok(())
        }
        fn sector_size(&self) -> u32 {
            SECTOR
        }
    }

    #[test]
    fn paths_normalise_to_one_spelling() {
        assert_eq!(normalise("data/log.txt").unwrap(), "/data/log.txt");
        assert_eq!(normalise("/data/log.txt").unwrap(), "/data/log.txt");
        assert_eq!(normalise("//data//log.txt//").unwrap(), "/data/log.txt");
        assert_eq!(normalise("\\data\\log.txt").unwrap(), "/data/log.txt");
        assert_eq!(normalise("").unwrap(), "/");
        assert_eq!(normalise("/").unwrap(), "/");
        assert!(normalise("../secrets").is_err());
    }

    /// The header is fixed-width and both variable parts are padded to four
    /// bytes, so the scanner steps from record to record with arithmetic alone.
    #[test]
    fn a_record_is_header_then_padded_name_then_padded_data() {
        let record = record_bytes("/a", b"hello", 0);
        assert_eq!(record.len() as u32, HEADER + 4 + 8);
        assert_eq!(&record[0..4], &MAGIC.to_le_bytes());
        assert_eq!(&record[8..12], &2u32.to_le_bytes(), "name length");
        assert_eq!(&record[12..16], &5u32.to_le_bytes(), "data length");
        assert_eq!(&record[16..18], b"/a");
        assert_eq!(&record[18..20], &[0xFF, 0xFF], "name padded to 4");
        assert_eq!(&record[20..25], b"hello");
    }

    #[test]
    fn write_read_roundtrip() {
        let mut nor = FakeNor::new(64 * 1024);
        write(&mut nor, "/notes.txt", b"hello k210").unwrap();
        assert_eq!(read(&mut nor, "/notes.txt").unwrap(), b"hello k210");
        assert!(exists(&mut nor, "notes.txt").unwrap(), "an unnormalised path finds it too");
        assert!(read(&mut nor, "/absent").is_err());
    }

    #[test]
    fn the_newest_write_wins_and_delete_hides_it() {
        let mut nor = FakeNor::new(64 * 1024);
        write(&mut nor, "/a", b"one").unwrap();
        write(&mut nor, "/a", b"two").unwrap();
        assert_eq!(read(&mut nor, "/a").unwrap(), b"two");

        delete(&mut nor, "/a").unwrap();
        assert!(!exists(&mut nor, "/a").unwrap());
        assert!(read(&mut nor, "/a").is_err());

        // ...and a file can come back after being deleted.
        write(&mut nor, "/a", b"three").unwrap();
        assert_eq!(read(&mut nor, "/a").unwrap(), b"three");
    }

    #[test]
    fn append_concatenates() {
        let mut nor = FakeNor::new(64 * 1024);
        append_bytes(&mut nor, "/log", b"first\n").unwrap();
        append_bytes(&mut nor, "/log", b"second\n").unwrap();
        assert_eq!(read(&mut nor, "/log").unwrap(), b"first\nsecond\n");
    }

    #[test]
    fn list_returns_immediate_children_only() {
        let mut nor = FakeNor::new(64 * 1024);
        write(&mut nor, "/data/a.txt", b"a").unwrap();
        write(&mut nor, "/data/b.txt", b"b").unwrap();
        write(&mut nor, "/data/deep/c.txt", b"c").unwrap();
        write(&mut nor, "/other.txt", b"o").unwrap();
        create_directory(&mut nor, "/data/empty").unwrap();

        assert_eq!(list(&mut nor, "/data").unwrap(), ["a.txt", "b.txt", "deep", "empty"]);
        assert_eq!(list(&mut nor, "/").unwrap(), ["data", "other.txt"]);

        delete(&mut nor, "/data/a.txt").unwrap();
        assert_eq!(list(&mut nor, "/data").unwrap(), ["b.txt", "deep", "empty"]);
    }

    /// A prefix must not match a longer sibling name.
    #[test]
    fn listing_a_directory_does_not_catch_its_namesake() {
        let mut nor = FakeNor::new(64 * 1024);
        write(&mut nor, "/log/today", b"x").unwrap();
        write(&mut nor, "/logbook", b"y").unwrap();
        assert_eq!(list(&mut nor, "/log").unwrap(), ["today"]);
    }

    /// Filling the window must reclaim superseded records rather than fail.
    #[test]
    fn compaction_reclaims_superseded_records() {
        // Small enough that rewriting one file repeatedly fills it.
        let mut nor = FakeNor::new(8 * 1024);
        let body = [b'x'; 512];
        for _ in 0..40 {
            write(&mut nor, "/rewritten", &body).unwrap();
        }
        write(&mut nor, "/keep", b"kept").unwrap();
        for _ in 0..40 {
            write(&mut nor, "/rewritten", &body).unwrap();
        }

        assert_eq!(read(&mut nor, "/rewritten").unwrap(), body);
        assert_eq!(read(&mut nor, "/keep").unwrap(), b"kept");
        assert!(used(&mut nor) < nor.size(), "compaction should have reclaimed space");
    }

    /// The bug this crate was written after. A window that is erased at the
    /// front and dirty further in must not be appended into: the write would
    /// AND into live data and report success.
    #[test]
    fn a_partly_dirty_window_is_erased_before_use() {
        let mut nor = FakeNor::new(64 * 1024);
        // Leftovers from whatever ran on the board before us: past where the
        // first record's header sits, inside where its body would land.
        nor.cells[600..700].fill(0x5A);

        let body = [b'q'; 2048];
        write(&mut nor, "/first", &body).unwrap();
        assert_eq!(read(&mut nor, "/first").unwrap(), body, "must not be silently ANDed");
    }

    /// And the short-probe version, spelled out: four blank bytes at the append
    /// site say nothing about the bytes after them.
    #[test]
    fn blankness_is_judged_over_the_whole_span() {
        let mut nor = FakeNor::new(8 * 1024);
        nor.cells[1000] = 0x00;
        assert!(blank(&mut nor, 0, 4).unwrap(), "the first four bytes really are erased");
        assert!(!blank(&mut nor, 0, 2048).unwrap(), "but the span it would occupy is not");
    }

    /// Compaction must cost what is used, not what is reserved.
    ///
    /// This is the bug that stalled a `rustnet flash` on hardware: erasing the
    /// whole window meant thousands of sector erases on a 15 MB region, the
    /// device stopped answering for minutes, and the tools reported a timeout
    /// with no idea the board was busy rather than broken.
    #[test]
    fn compaction_erases_the_used_part_not_the_whole_window() {
        // A big window with a tiny amount of data in it — the shape of a
        // filesystem on a 16 MB part.
        let mut nor = FakeNor::new(4 * 1024 * 1024);
        let body = [b'z'; 1024];
        // Rewriting the same file leaves superseded copies behind but keeps the
        // live set at one small record.
        for _ in 0..6 {
            write(&mut nor, "/one", &body).unwrap();
        }
        // Dirty the byte just past the log so the next append cannot simply
        // continue and has to compact.
        let end = used(&mut nor);
        nor.cells[end as usize + 8] = 0x00;
        nor.erased_sectors = 0;

        write(&mut nor, "/one", &body).unwrap();

        assert_eq!(read(&mut nor, "/one").unwrap(), body);
        // The whole window is 1024 sectors. Touching more than a handful means
        // the erase is sized by the region rather than by its contents.
        assert!(
            nor.erased_sectors <= 8,
            "compaction erased {} sectors; it should only cover the used span",
            nor.erased_sectors
        );
    }

    #[test]
    fn a_file_larger_than_the_limit_is_refused() {
        let mut nor = FakeNor::new(8 * 1024);
        let huge = vec![0u8; MAX_FILE as usize + 1];
        assert!(write(&mut nor, "/huge", &huge).is_err());
    }

    #[test]
    fn a_directory_marker_lists_but_reads_empty() {
        let mut nor = FakeNor::new(8 * 1024);
        create_directory(&mut nor, "/cfg").unwrap();
        assert!(exists(&mut nor, "/cfg").unwrap());
        assert_eq!(read(&mut nor, "/cfg").unwrap(), b"");
        assert_eq!(FLAG_DIRECTORY, 2);
    }
}
