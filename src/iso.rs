// Copyright © 2026 James 'akses' Burger
//
// This program is free software: you can redistribute it and/or modify it under the terms of the
// GNU General Public License as published by the Free Software Foundation, either version 3 of
// the License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY;
// without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
//
// See the GNU General Public License for more details. You should have received a copy of
// the GNU General Public License along with this program. If not, see <https://www.gnu.org/licenses/>.
// --------------------------------------------------------- //
// Cellar - Cross-platform GUI for ISO 9660 image creation.  //
// Joliet support for long filenames.                        //
// --------------------------------------------------------- //
// iso.rs - Structs and functions for working with ISO files.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// A file to be included in the ISO image.
#[allow(dead_code)]
pub struct IsoFile {
    /// The name as it should appear inside the ISO (collision-resolved).
    pub name: String,
    /// Where to get the file data from.
    pub source: IsoFileSource,
    /// Size in bytes (must match the actual data).
    pub size: u64,
    /// File modification time for directory records.
    pub mtime: Option<IsoDateTime>,
}

/// Source of data for an ISO file.
pub enum IsoFileSource {
    /// Read from a path on disk.
    Path(PathBuf),
    /// Use in-memory bytes (e.g. for manifest files).
    Bytes(Vec<u8>),
}

/// ISO 9660 7-byte recording date/time (used in directory records).
#[derive(Clone, Copy, Debug)]
pub struct IsoDateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub timezone_offset: i8, // 15-minute intervals from GMT
}

impl IsoDateTime {
    pub fn from_system_time(st: std::time::SystemTime) -> Option<Self> {
        let dur = st.duration_since(std::time::UNIX_EPOCH).ok()?;
        let secs = dur.as_secs() as i64;
        let days = secs / 86400;
        let rem = secs % 86400;
        let hour = (rem / 3600) as u8;
        let rem = rem % 3600;
        let minute = (rem / 60) as u8;
        let second = (rem % 60) as u8;
        let mut year = 1970u16;
        let mut days_left = days;
        loop {
            let leap = is_leap_year(year);
            let year_days = if leap { 366 } else { 365 };
            if days_left < year_days {
                break;
            }
            days_left -= year_days;
            year += 1;
        }
        let (month, day) = day_of_year_to_month_day(year, days_left as u16 + 1);
        Some(Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
            timezone_offset: 0,
        })
    }

    fn to_7byte(self) -> [u8; 7] {
        [
            (self.year.saturating_sub(1900)) as u8,
            self.month,
            self.day,
            self.hour,
            self.minute,
            self.second,
            self.timezone_offset as u8,
        ]
    }

    fn to_17byte(self) -> [u8; 17] {
        let mut buf = [b'0'; 17];
        let y = format!("{:04}", self.year);
        let m = format!("{:02}", self.month);
        let d = format!("{:02}", self.day);
        let h = format!("{:02}", self.hour);
        let min = format!("{:02}", self.minute);
        let s = format!("{:02}", self.second);
        let hs = "00";
        buf[0..4].copy_from_slice(y.as_bytes());
        buf[4..6].copy_from_slice(m.as_bytes());
        buf[6..8].copy_from_slice(d.as_bytes());
        buf[8..10].copy_from_slice(h.as_bytes());
        buf[10..12].copy_from_slice(min.as_bytes());
        buf[12..14].copy_from_slice(s.as_bytes());
        buf[14..16].copy_from_slice(hs.as_bytes());
        buf[16] = self.timezone_offset as u8;
        buf
    }
}

fn is_leap_year(y: u16) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

fn day_of_year_to_month_day(year: u16, day: u16) -> (u8, u8) {
    let days_in_month = [
        31,
        if is_leap_year(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut day = day;
    for (i, dim) in days_in_month.iter().enumerate() {
        if day <= *dim {
            return ((i + 1) as u8, day as u8);
        }
        day -= *dim;
    }
    (12, day as u8)
}

const SECTOR_SIZE: u64 = 2048;

// ISO 9660 reserves the first 16 sectors as the "system area". The volume
// descriptor set starts immediately after it.
const SYSTEM_AREA_SECTORS: u64 = 16;
const PVD_SECTOR: u32 = 16;
const SVD_SECTOR: u32 = 17;
const VDST_SECTOR: u32 = 18;

// This writer emits exactly one root-directory path table entry. That entry is
// 10 bytes in both little-endian and big-endian path tables.
const SINGLE_ROOT_PATH_TABLE_SIZE: u32 = 10;

const PVD_PATH_TABLE_L_SECTOR: u32 = 19;
const PVD_PATH_TABLE_M_SECTOR: u32 = 20;
const JOLIET_PATH_TABLE_L_SECTOR: u32 = 21;
const JOLIET_PATH_TABLE_M_SECTOR: u32 = 22;
const PVD_ROOT_DIR_SECTOR: u32 = 23;

// Directory records have a 1-byte total length field. The fixed portion is 33
// bytes, followed by the file identifier and an optional pad byte for alignment.
const DIR_RECORD_HEADER_LEN: usize = 33;
const DIR_RECORD_SELF_OR_PARENT_LEN: u32 = 34;
const MAX_DIR_RECORD_LEN: usize = u8::MAX as usize;

// ISO 9660 stores file sizes as "both-endian" u32 values: 4 little-endian bytes
// followed by 4 big-endian bytes. Larger files require multi-extent records,
// which this minimal writer intentionally does not implement.
const MAX_ISO9660_FILE_SIZE: u64 = u32::MAX as u64;

// Joliet file identifiers are UCS-2. Keeping them to 64 Unicode scalar values
// stays inside Joliet's practical filename limit and comfortably below the
// directory-record length ceiling after UTF-16 encoding.
const MAX_JOLIET_CHARS: usize = 64;

// ------------------------------------------------------------------
// Layout
// ------------------------------------------------------------------

#[allow(dead_code)]
#[derive(Debug)]
struct Layout {
    total_sectors: u32,
    pvd_sector: u32,
    svd_sector: u32,
    vdst_sector: u32,
    pvd_path_table_l_sector: u32,
    pvd_path_table_m_sector: u32,
    joliet_path_table_l_sector: u32,
    joliet_path_table_m_sector: u32,
    pvd_root_dir_sector: u32,
    pvd_root_dir_size: u32,
    joliet_root_dir_sector: u32,
    joliet_root_dir_size: u32,
    data_start_sector: u32,
    file_sectors: Vec<u32>,
    file_sizes: Vec<u64>,
    file_mtimes: Vec<Option<IsoDateTime>>,
    pvd_names: Vec<String>,
    joliet_names: Vec<Vec<u8>>,
    volume_label: String,
}

#[derive(Clone, Copy)]
enum VolumeTree {
    PrimaryIso9660,
    Joliet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JolietLabelMode {
    #[default]
    Strict,
    Legacy,
}

#[derive(Clone, Copy)]
enum PathTableEndian {
    TypeL,
    TypeM,
}

// ------------------------------------------------------------------
// Public API
// ------------------------------------------------------------------

/// Write a valid ISO 9660 image with Joliet support.
///
/// `label` is the volume label (sanitized internally).
/// `emit` is called with progress messages for UI display.
#[allow(dead_code)]
pub fn write_iso(
    output: &Path,
    label: &str,
    files: &[IsoFile],
    emit: &dyn Fn(&str),
) -> Result<(), String> {
    write_iso_with_options(output, label, files, JolietLabelMode::Strict, emit)
}

pub fn write_iso_with_options(
    output: &Path,
    label: &str,
    files: &[IsoFile],
    joliet_label_mode: JolietLabelMode,
    emit: &dyn Fn(&str),
) -> Result<(), String> {
    emit("Calculating ISO layout...");
    let layout = calculate_layout(label, files)?;

    let mut out = File::create(output).map_err(|e| format!("Could not create ISO file: {e}"))?;

    emit("Writing system area...");
    write_zeros(&mut out, SYSTEM_AREA_SECTORS * SECTOR_SIZE)?;

    // Write volume descriptors
    emit("Writing volume descriptors...");
    write_pvd(&mut out, &layout)?;
    write_svd(&mut out, &layout, joliet_label_mode)?;
    write_vdst(&mut out)?;

    // Write path tables
    emit("Writing path tables...");
    write_path_table(
        &mut out,
        &layout,
        VolumeTree::PrimaryIso9660,
        PathTableEndian::TypeL,
    )?;
    write_path_table(
        &mut out,
        &layout,
        VolumeTree::PrimaryIso9660,
        PathTableEndian::TypeM,
    )?;
    write_path_table(
        &mut out,
        &layout,
        VolumeTree::Joliet,
        PathTableEndian::TypeL,
    )?;
    write_path_table(
        &mut out,
        &layout,
        VolumeTree::Joliet,
        PathTableEndian::TypeM,
    )?;

    // Write root directories
    emit("Writing directory records...");
    write_root_dir(&mut out, &layout, VolumeTree::PrimaryIso9660)?;
    write_root_dir(&mut out, &layout, VolumeTree::Joliet)?;

    // Write file data
    for (i, file) in files.iter().enumerate() {
        emit(&format!("Writing file {}/{}...", i + 1, files.len()));
        let pos_before = stream_position(&mut out)?;
        write_file_data(&mut out, file)?;
        let pos_after = stream_position(&mut out)?;
        emit(&format!("  Wrote {} bytes", pos_after - pos_before));
    }

    // Pad final file to sector boundary
    let current_pos = stream_position(&mut out)?;
    let pad = (SECTOR_SIZE - (current_pos % SECTOR_SIZE)) % SECTOR_SIZE;
    if pad > 0 {
        write_zeros(&mut out, pad)?;
    }

    Ok(())
}

// ------------------------------------------------------------------
// Pass 1: Layout calculation
// ------------------------------------------------------------------

fn calculate_layout(label: &str, files: &[IsoFile]) -> Result<Layout, String> {
    let volume_label = sanitize_label(label);

    let mut pvd_names = Vec::with_capacity(files.len());
    let mut joliet_names = Vec::with_capacity(files.len());
    let mut file_sizes = Vec::with_capacity(files.len());
    let mut file_mtimes = Vec::with_capacity(files.len());
    for f in files {
        if f.size >= MAX_ISO9660_FILE_SIZE {
            return Err(format!(
                "File '{}' is {} bytes, which exceeds the ISO 9660 4 GiB limit",
                f.name, f.size
            ));
        }
        let pvd = to_pvd_name(&f.name);
        let joliet = to_joliet_name(&f.name);
        if dir_record_len(pvd.as_bytes()) > MAX_DIR_RECORD_LEN {
            return Err(format!("PVD name too long for directory record: '{}'", pvd));
        }
        if dir_record_len(&joliet) > MAX_DIR_RECORD_LEN {
            return Err(format!(
                "Joliet name too long for directory record ({} bytes after encoding)",
                joliet.len()
            ));
        }
        pvd_names.push(pvd);
        joliet_names.push(joliet);
        file_sizes.push(f.size);
        file_mtimes.push(f.mtime);
    }

    let pvd_root_size = compute_root_dir_size(&pvd_names, true);
    let joliet_root_size = compute_root_dir_size(&joliet_names, false);

    let pvd_sector = PVD_SECTOR;
    let svd_sector = SVD_SECTOR;
    let vdst_sector = VDST_SECTOR;
    let pvd_path_table_l_sector = PVD_PATH_TABLE_L_SECTOR;
    let pvd_path_table_m_sector = PVD_PATH_TABLE_M_SECTOR;
    let joliet_path_table_l_sector = JOLIET_PATH_TABLE_L_SECTOR;
    let joliet_path_table_m_sector = JOLIET_PATH_TABLE_M_SECTOR;
    let pvd_root_dir_sector = PVD_ROOT_DIR_SECTOR;
    let joliet_root_dir_sector = pvd_root_dir_sector + sectors_for_u32(pvd_root_size);
    let data_start_sector = joliet_root_dir_sector + sectors_for_u32(joliet_root_size);

    let mut file_sectors = Vec::with_capacity(files.len());
    let mut next_sector = data_start_sector;
    for size in &file_sizes {
        file_sectors.push(next_sector);
        next_sector += sectors_for_u64(*size);
    }

    let total_sectors = next_sector;

    Ok(Layout {
        total_sectors,
        pvd_sector,
        svd_sector,
        vdst_sector,
        pvd_path_table_l_sector,
        pvd_path_table_m_sector,
        joliet_path_table_l_sector,
        joliet_path_table_m_sector,
        pvd_root_dir_sector,
        pvd_root_dir_size: pvd_root_size,
        joliet_root_dir_sector,
        joliet_root_dir_size: joliet_root_size,
        data_start_sector,
        file_sectors,
        file_sizes,
        file_mtimes,
        pvd_names,
        joliet_names,
        volume_label,
    })
}

fn sectors_for_u32(bytes: u32) -> u32 {
    (bytes as u64).div_ceil(SECTOR_SIZE) as u32
}

fn sectors_for_u64(bytes: u64) -> u32 {
    bytes.div_ceil(SECTOR_SIZE) as u32
}

fn compute_root_dir_size(names: &[impl AsRef<[u8]>], _is_pvd: bool) -> u32 {
    let mut size = DIR_RECORD_SELF_OR_PARENT_LEN * 2; // "." and ".."
    for name in names {
        size += dir_record_len(name.as_ref()) as u32;
    }
    size
}

fn dir_record_len(name: &[u8]) -> usize {
    let unpadded = DIR_RECORD_HEADER_LEN + name.len();
    if unpadded % 2 == 1 {
        unpadded + 1
    } else {
        unpadded
    }
}

fn to_pvd_name(name: &str) -> String {
    // Primary volume descriptors use ISO 9660 Level 1 filenames: uppercase
    // ASCII, 8.3 format, and an explicit version suffix.
    let (stem, ext) = if let Some(i) = name.rfind('.') {
        (&name[..i], &name[i + 1..])
    } else {
        (name, "")
    };
    let stem: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .take(8)
        .collect();
    let ext: String = ext
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .take(3)
        .collect();
    if ext.is_empty() {
        format!("{};1", stem)
    } else {
        format!("{}.{};1", stem, ext)
    }
}

fn to_joliet_name(name: &str) -> Vec<u8> {
    let truncated: String = name.chars().take(MAX_JOLIET_CHARS).collect();
    truncated
        .encode_utf16()
        .flat_map(|u| u.to_be_bytes())
        .collect()
}

fn sanitize_label(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('_').to_string();
    if cleaned.is_empty() {
        "CELLAR".to_string()
    } else {
        cleaned.chars().take(32).collect()
    }
}

// ------------------------------------------------------------------
// Pass 2: Writing
// ------------------------------------------------------------------

fn stream_position<W: Write + std::io::Seek>(w: &mut W) -> Result<u64, String> {
    w.stream_position().map_err(|e| e.to_string())
}

fn write_zeros<W: Write>(w: &mut W, count: u64) -> Result<(), String> {
    const CHUNK: usize = 64 * 1024;
    let buf = vec![0u8; CHUNK];
    let mut remaining = count;
    while remaining > 0 {
        let n = std::cmp::min(remaining as usize, CHUNK);
        w.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        remaining -= n as u64;
    }
    Ok(())
}

fn write_pvd<W: Write>(w: &mut W, layout: &Layout) -> Result<(), String> {
    let mut buf = [0u8; SECTOR_SIZE as usize];

    // Standard volume descriptor header: type, magic identifier, version.
    buf[0] = 0x01;
    buf[1..6].copy_from_slice(b"CD001");
    buf[6] = 0x01;

    pad_ascii(&mut buf[8..40], "CELLAR_ISO");
    pad_ascii(&mut buf[40..72], &layout.volume_label);

    // ISO 9660 stores some numeric fields in both byte orders for portability.
    write_both32(&mut buf[80..88], layout.total_sectors);
    write_both16(&mut buf[120..124], 1);
    write_both16(&mut buf[124..128], 1);
    write_both16(&mut buf[128..132], SECTOR_SIZE as u16);
    write_both32(&mut buf[132..140], SINGLE_ROOT_PATH_TABLE_SIZE);

    // Path table locations: Type L is little-endian, Type M is big-endian.
    // Optional duplicate tables are not emitted, so their locations stay zero.
    write_le32(&mut buf[140..144], layout.pvd_path_table_l_sector);
    write_le32(&mut buf[144..148], 0); // optional L path table
    write_be32(&mut buf[148..152], layout.pvd_path_table_m_sector);
    write_be32(&mut buf[152..156], 0); // optional M path table

    // The embedded root directory record begins at byte 156. Earlier versions
    // of this writer placed it at 148; readers reject that because bytes
    // 148..156 are reserved for Type M path table fields.
    write_root_dir_record(
        &mut buf[156..190],
        layout.pvd_root_dir_sector,
        layout.pvd_root_dir_size,
    );

    pad_ascii(&mut buf[190..318], "");
    pad_ascii(&mut buf[318..446], "cellar");
    pad_ascii(&mut buf[446..574], "cellar");
    pad_ascii(&mut buf[574..702], "cellar");
    pad_ascii(&mut buf[702..739], "");
    pad_ascii(&mut buf[739..775], "");
    pad_ascii(&mut buf[775..812], "");

    let now = IsoDateTime::from_system_time(std::time::SystemTime::now()).unwrap_or(IsoDateTime {
        year: 2024,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
        timezone_offset: 0,
    });
    buf[813..830].copy_from_slice(&now.to_17byte());
    buf[830..847].copy_from_slice(&now.to_17byte());
    buf[847..864].copy_from_slice(&[b'0'; 17]);
    buf[864..881].copy_from_slice(&[b'0'; 17]);
    buf[881] = 0x01;

    w.write_all(&buf).map_err(|e| e.to_string())?;
    Ok(())
}

fn write_svd<W: Write>(
    w: &mut W,
    layout: &Layout,
    joliet_label_mode: JolietLabelMode,
) -> Result<(), String> {
    let mut buf = [0u8; SECTOR_SIZE as usize];

    // Supplementary volume descriptor. Same structure as the PVD, but marked
    // with Joliet escape sequences and UCS-2 filenames in its directory tree.
    buf[0] = 0x02;
    buf[1..6].copy_from_slice(b"CD001");
    buf[6] = 0x01;
    buf[7] = 0x00;

    // Joliet Level 3 escape sequences at offset 0x58 (32 bytes)
    buf[0x58] = 0x25;
    buf[0x59] = 0x2F;
    buf[0x5A] = 0x45;

    pad_ascii(&mut buf[8..40], "CELLAR_ISO");
    match joliet_label_mode {
        JolietLabelMode::Strict => pad_ucs2(&mut buf[40..72], &layout.volume_label),
        JolietLabelMode::Legacy => pad_ucs2(&mut buf[40..72], ""),
    }

    write_both32(&mut buf[80..88], layout.total_sectors);
    write_both16(&mut buf[120..124], 1);
    write_both16(&mut buf[124..128], 1);
    write_both16(&mut buf[128..132], SECTOR_SIZE as u16);
    write_both32(&mut buf[132..140], SINGLE_ROOT_PATH_TABLE_SIZE);

    write_le32(&mut buf[140..144], layout.joliet_path_table_l_sector);
    write_le32(&mut buf[144..148], 0); // optional L path table
    write_be32(&mut buf[148..152], layout.joliet_path_table_m_sector);
    write_be32(&mut buf[152..156], 0); // optional M path table

    write_root_dir_record(
        &mut buf[156..190],
        layout.joliet_root_dir_sector,
        layout.joliet_root_dir_size,
    );

    pad_ucs2(&mut buf[190..318], "");
    pad_ucs2(&mut buf[318..446], "cellar");
    pad_ucs2(&mut buf[446..574], "cellar");
    pad_ucs2(&mut buf[574..702], "cellar");
    pad_ucs2(&mut buf[702..739], "");
    pad_ucs2(&mut buf[739..775], "");
    pad_ucs2(&mut buf[775..812], "");

    let now = IsoDateTime::from_system_time(std::time::SystemTime::now()).unwrap_or(IsoDateTime {
        year: 2024,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
        timezone_offset: 0,
    });
    buf[813..830].copy_from_slice(&now.to_17byte());
    buf[830..847].copy_from_slice(&now.to_17byte());
    buf[847..864].copy_from_slice(&[b'0'; 17]);
    buf[864..881].copy_from_slice(&[b'0'; 17]);
    buf[881] = 0x01;

    w.write_all(&buf).map_err(|e| e.to_string())?;
    Ok(())
}

fn write_vdst<W: Write>(w: &mut W) -> Result<(), String> {
    let mut buf = [0u8; SECTOR_SIZE as usize];
    buf[0] = 0xFF;
    buf[1..6].copy_from_slice(b"CD001");
    buf[6] = 0x01;
    w.write_all(&buf).map_err(|e| e.to_string())?;
    Ok(())
}

fn write_path_table<W: Write>(
    w: &mut W,
    layout: &Layout,
    tree: VolumeTree,
    endian: PathTableEndian,
) -> Result<(), String> {
    let mut buf = [0u8; SECTOR_SIZE as usize];
    let extent = match tree {
        VolumeTree::PrimaryIso9660 => layout.pvd_root_dir_sector,
        VolumeTree::Joliet => layout.joliet_root_dir_sector,
    };

    // Path table root entry layout:
    //   0: identifier length (1 byte for root id 0)
    //   1: extended attribute record length
    //   2..6: root directory extent sector
    //   6..8: parent directory number (root points to itself)
    //   8: root identifier byte (0)
    //   9: pad byte to even length
    buf[0] = 0x01;
    buf[1] = 0x00;
    match endian {
        PathTableEndian::TypeL => {
            write_le32(&mut buf[2..6], extent);
            write_le16(&mut buf[6..8], 1);
        }
        PathTableEndian::TypeM => {
            write_be32(&mut buf[2..6], extent);
            write_be16(&mut buf[6..8], 1);
        }
    }
    buf[8] = 0x00;
    buf[9] = 0x00;

    w.write_all(&buf).map_err(|e| e.to_string())?;
    Ok(())
}

fn write_root_dir<W: Write>(w: &mut W, layout: &Layout, tree: VolumeTree) -> Result<(), String> {
    let sector = match tree {
        VolumeTree::PrimaryIso9660 => layout.pvd_root_dir_sector,
        VolumeTree::Joliet => layout.joliet_root_dir_sector,
    };
    let size = match tree {
        VolumeTree::PrimaryIso9660 => layout.pvd_root_dir_size,
        VolumeTree::Joliet => layout.joliet_root_dir_size,
    };

    let mut buf = vec![0u8; size as usize];
    let mut offset = 0usize;

    let dt = IsoDateTime::from_system_time(std::time::SystemTime::now()).unwrap_or(IsoDateTime {
        year: 2024,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
        timezone_offset: 0,
    });

    // The root directory contains synthetic self/parent records first, followed
    // by one flat file entry per user-provided file.
    offset += write_dir_record(
        &mut buf[offset..],
        sector,
        size,
        &dt.to_7byte(),
        0x02, // directory flag
        &[0x00],
    );

    offset += write_dir_record(
        &mut buf[offset..],
        sector,
        size,
        &dt.to_7byte(),
        0x02,
        &[0x01],
    );

    for i in 0..layout.file_sizes.len() {
        let file_sector = layout.file_sectors[i];
        let file_size: u32 = layout.file_sizes[i]
            .try_into()
            .map_err(|_| format!("File '{}' exceeds 4 GiB limit", layout.pvd_names[i]))?;
        let file_dt = layout.file_mtimes[i].unwrap_or(dt);
        let name_bytes: &[u8] = match tree {
            VolumeTree::PrimaryIso9660 => layout.pvd_names[i].as_bytes(),
            VolumeTree::Joliet => &layout.joliet_names[i],
        };
        offset += write_dir_record(
            &mut buf[offset..],
            file_sector,
            file_size,
            &file_dt.to_7byte(),
            0x00, // regular file
            name_bytes,
        );
    }

    w.write_all(&buf).map_err(|e| e.to_string())?;

    let pad = (SECTOR_SIZE - (size as u64 % SECTOR_SIZE)) % SECTOR_SIZE;
    if pad > 0 {
        write_zeros(w, pad)?;
    }

    Ok(())
}

fn write_dir_record(
    buf: &mut [u8],
    extent: u32,
    size: u32,
    date: &[u8; 7],
    flags: u8,
    name: &[u8],
) -> usize {
    let name_len = name.len();
    let rec_len = dir_record_len(name);
    debug_assert!(rec_len <= MAX_DIR_RECORD_LEN);
    debug_assert!(name_len <= u8::MAX as usize);

    // Directory record fields are byte-for-byte ISO 9660. Extent and data
    // length use both-endian u32 encoding; the filename bytes follow directly
    // after the fixed 33-byte header.
    buf[0] = rec_len as u8;
    buf[1] = 0;
    write_both32(&mut buf[2..10], extent);
    write_both32(&mut buf[10..18], size);
    buf[18..25].copy_from_slice(date);
    buf[25] = flags;
    buf[26] = 0;
    buf[27] = 0;
    write_both16(&mut buf[28..32], 1);
    buf[32] = name_len as u8;
    buf[33..33 + name_len].copy_from_slice(name);

    rec_len
}

fn write_file_data<W: Write>(w: &mut W, file: &IsoFile) -> Result<(), String> {
    const CHUNK: usize = 64 * 1024;
    let mut written = 0u64;

    match &file.source {
        IsoFileSource::Path(path) => {
            let mut f =
                File::open(path).map_err(|e| format!("Could not open {}: {e}", path.display()))?;
            let mut buf = [0u8; CHUNK];
            loop {
                let n = f.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                w.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                written += n as u64;
            }
        }
        IsoFileSource::Bytes(bytes) => {
            w.write_all(bytes).map_err(|e| e.to_string())?;
            written = bytes.len() as u64;
        }
    }

    if written != file.size {
        return Err(format!(
            "Size mismatch for '{}': expected {} bytes, wrote {} bytes. The file may have changed after staging.",
            file.name, file.size, written
        ));
    }

    let pad = (SECTOR_SIZE - (written % SECTOR_SIZE)) % SECTOR_SIZE;
    if pad > 0 {
        write_zeros(w, pad)?;
    }

    Ok(())
}

// ------------------------------------------------------------------
// Encoding helpers
// ------------------------------------------------------------------

fn write_both16(buf: &mut [u8], val: u16) {
    write_le16(buf, val);
    write_be16(&mut buf[2..], val);
}

fn write_both32(buf: &mut [u8], val: u32) {
    write_le32(buf, val);
    write_be32(&mut buf[4..], val);
}

fn write_le16(buf: &mut [u8], val: u16) {
    buf[0] = (val & 0xFF) as u8;
    buf[1] = ((val >> 8) & 0xFF) as u8;
}

fn write_be16(buf: &mut [u8], val: u16) {
    buf[0] = ((val >> 8) & 0xFF) as u8;
    buf[1] = (val & 0xFF) as u8;
}

fn write_le32(buf: &mut [u8], val: u32) {
    buf[0] = (val & 0xFF) as u8;
    buf[1] = ((val >> 8) & 0xFF) as u8;
    buf[2] = ((val >> 16) & 0xFF) as u8;
    buf[3] = ((val >> 24) & 0xFF) as u8;
}

fn write_be32(buf: &mut [u8], val: u32) {
    buf[0] = ((val >> 24) & 0xFF) as u8;
    buf[1] = ((val >> 16) & 0xFF) as u8;
    buf[2] = ((val >> 8) & 0xFF) as u8;
    buf[3] = (val & 0xFF) as u8;
}

fn pad_ascii(buf: &mut [u8], s: &str) {
    let bytes = s.as_bytes();
    let len = std::cmp::min(bytes.len(), buf.len());
    buf[..len].copy_from_slice(&bytes[..len]);
    for b in &mut buf[len..] {
        *b = b' ';
    }
}

/// Encode a string as UCS-2 Big Endian, space-padded to exactly `buf.len()` bytes.
/// Joliet requires UCS-2 BE for volume identifier, publisher, preparer, and
/// application fields in the Supplementary Volume Descriptor.
fn pad_ucs2(buf: &mut [u8], s: &str) {
    let max_chars = buf.len() / 2;
    let chars: Vec<u16> = s.chars().take(max_chars).map(|c| c as u16).collect();
    for (i, ch) in chars.iter().enumerate() {
        let off = i * 2;
        buf[off] = (ch >> 8) as u8;
        buf[off + 1] = (*ch & 0xFF) as u8;
    }
    let filled = chars.len() * 2;
    for b in &mut buf[filled..] {
        *b = 0x00;
    }
    for off in (filled..buf.len()).step_by(2) {
        buf[off] = 0x00;
        if off + 1 < buf.len() {
            buf[off + 1] = 0x20;
        }
    }
}

fn write_root_dir_record(buf: &mut [u8], extent: u32, size: u32) {
    let dt = IsoDateTime::from_system_time(std::time::SystemTime::now()).unwrap_or(IsoDateTime {
        year: 2024,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
        timezone_offset: 0,
    });
    write_dir_record(buf, extent, size, &dt.to_7byte(), 0x02, &[0x00]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_resolve_names_basic_and_collision() {
        use crate::backend::{resolve_names, StagedFile};
        use crate::manifest::FileMetadata;

        let staged = |name: &str| StagedFile {
            path: PathBuf::from(name),
            sha256: format!("sha256-{name}"),
            size: 100,
            metadata: FileMetadata::default(),
        };

        let files = vec![staged("a.txt"), staged("a.txt"), staged("b.txt")];
        let resolved = resolve_names(&files);
        let names: Vec<&str> = resolved.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(names, vec!["a.txt", "a_1.txt", "b.txt"]);
    }

    #[test]
    fn test_resolve_names_no_collision() {
        use crate::backend::{resolve_names, StagedFile};
        use crate::manifest::FileMetadata;

        let staged = |name: &str| StagedFile {
            path: PathBuf::from(name),
            sha256: String::new(),
            size: 0,
            metadata: FileMetadata::default(),
        };

        let files = vec![staged("x.txt"), staged("y.txt"), staged("z.txt")];
        let resolved = resolve_names(&files);
        let names: Vec<&str> = resolved.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(names, vec!["x.txt", "y.txt", "z.txt"]);
    }

    #[test]
    fn test_resolve_names_without_extension() {
        use crate::backend::{resolve_names, StagedFile};
        use crate::manifest::FileMetadata;

        let staged = |name: &str| StagedFile {
            path: PathBuf::from(name),
            sha256: String::new(),
            size: 0,
            metadata: FileMetadata::default(),
        };

        let files = vec![staged("README"), staged("README")];
        let resolved = resolve_names(&files);
        let names: Vec<&str> = resolved.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(names, vec!["README", "README_1"]);
    }

    #[allow(dead_code)]
    fn staged(name: &str, size: u64) -> crate::backend::StagedFile {
        crate::backend::StagedFile {
            path: PathBuf::from(name),
            sha256: String::new(),
            size,
            metadata: crate::manifest::FileMetadata::default(),
        }
    }

    #[test]
    fn test_pvd_byte_layout() {
        let tmp = std::env::temp_dir().join("cellar-pvd-layout-test");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("f.txt"), "hi").unwrap();

        let files = vec![IsoFile {
            name: "f.txt".to_string(),
            source: IsoFileSource::Path(tmp.join("f.txt")),
            size: 2,
            mtime: None,
        }];
        let iso_path = tmp.join("layout.iso");
        write_iso(&iso_path, "testlabel", &files, &|_| {}).unwrap();

        let data = fs::read(&iso_path).unwrap();
        let system_area_end = (SYSTEM_AREA_SECTORS * SECTOR_SIZE) as usize;
        let pvd = &data[system_area_end..system_area_end + SECTOR_SIZE as usize];

        assert_eq!(pvd[0], 0x01, "PVD type code must be 1");
        assert_eq!(
            &pvd[1..6],
            b"CD001",
            "PVD standard identifier must be CD001"
        );
        assert_eq!(pvd[6], 0x01, "PVD version must be 1");

        let vol_label = std::str::from_utf8(&pvd[40..72]).unwrap();
        assert!(
            vol_label.starts_with("TESTLABEL"),
            "volume label should start with TESTLABEL, got: {:?}",
            vol_label
        );

        let le_sectors = u32::from_le_bytes(pvd[80..84].try_into().unwrap());
        let be_sectors = u32::from_be_bytes(pvd[84..88].try_into().unwrap());
        assert_eq!(
            le_sectors, be_sectors,
            "PVD total sectors both-endian mismatch"
        );

        let le_pt_size = u32::from_le_bytes(pvd[132..136].try_into().unwrap());
        assert_eq!(le_pt_size, 10, "path table size must be 10");

        assert_eq!(
            pvd[156], 34,
            "root dir record length should be 34 (self entry)"
        );
        assert_ne!(
            pvd[148], 34,
            "offset 148 is M-path table location, not root dir"
        );

        assert_eq!(pvd[881], 0x01, "file structure version must be 1");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_svd_joliet_escape_sequence() {
        let tmp = std::env::temp_dir().join("cellar-svd-test");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("g.txt"), "data").unwrap();

        let files = vec![IsoFile {
            name: "g.txt".to_string(),
            source: IsoFileSource::Path(tmp.join("g.txt")),
            size: 4,
            mtime: None,
        }];
        let iso_path = tmp.join("svd.iso");
        write_iso(&iso_path, "jtest", &files, &|_| {}).unwrap();

        let data = fs::read(&iso_path).unwrap();
        let system_area_end = (SYSTEM_AREA_SECTORS * SECTOR_SIZE) as usize;
        let svd = &data
            [system_area_end + SECTOR_SIZE as usize..system_area_end + 2 * SECTOR_SIZE as usize];

        assert_eq!(svd[0], 0x02, "SVD type code must be 2");
        assert_eq!(&svd[1..6], b"CD001", "SVD standard identifier");
        assert_eq!(svd[6], 0x01, "SVD version must be 1");
        assert_eq!(
            &svd[0x58..=0x5A],
            &[0x25, 0x2F, 0x45],
            "Joliet escape sequence must be %/E"
        );

        assert_eq!(svd[156], 34, "SVD root dir record should be 34 bytes");

        // SVD volume label must be UCS-2 BE, not ASCII. "JTEST" encoded as
        // UCS-2 BE means every other byte should be 0x00 (the high byte of each
        // ASCII char). If pad_ascii were used instead, adjacent ASCII pairs
        // would decode as CJK characters.
        let svd_label = &svd[40..72];
        assert_eq!(
            svd_label[0], 0x00,
            "SVD volume label high byte must be 0 for ASCII-range characters"
        );
        assert_eq!(
            svd_label[1], b'J',
            "SVD volume label first character must be 'J' in UCS-2 BE"
        );
        assert_eq!(
            svd_label[2], 0x00,
            "SVD volume label second char high byte must be 0"
        );
        assert_eq!(
            svd_label[3], b'T',
            "SVD volume label second character must be 'T' in UCS-2 BE"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_svd_legacy_blank_label() {
        let tmp = std::env::temp_dir().join("cellar-svd-legacy-test");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("g.txt"), "data").unwrap();

        let files = vec![IsoFile {
            name: "g.txt".to_string(),
            source: IsoFileSource::Path(tmp.join("g.txt")),
            size: 4,
            mtime: None,
        }];
        let iso_path = tmp.join("svd-legacy.iso");
        write_iso_with_options(
            &iso_path,
            "jtest",
            &files,
            JolietLabelMode::Legacy,
            &|_| {},
        )
        .unwrap();

        let data = fs::read(&iso_path).unwrap();
        let system_area_end = (SYSTEM_AREA_SECTORS * SECTOR_SIZE) as usize;
        let svd = &data
            [system_area_end + SECTOR_SIZE as usize..system_area_end + 2 * SECTOR_SIZE as usize];
        let svd_label = &svd[40..72];

        assert!(
            svd_label.chunks_exact(2).all(|pair| pair == [0x00, 0x20]),
            "legacy mode should write a blank Joliet volume label"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_both_endian_encoding() {
        let mut buf16 = [0u8; 4];
        write_both16(&mut buf16, 0x1234);
        assert_eq!(u16::from_le_bytes([buf16[0], buf16[1]]), 0x1234);
        assert_eq!(u16::from_be_bytes([buf16[2], buf16[3]]), 0x1234);

        let mut buf32 = [0u8; 8];
        write_both32(&mut buf32, 0xDEADBEEF);
        assert_eq!(
            u32::from_le_bytes(buf32[0..4].try_into().unwrap()),
            0xDEADBEEF
        );
        assert_eq!(
            u32::from_be_bytes(buf32[4..8].try_into().unwrap()),
            0xDEADBEEF
        );
    }

    #[test]
    fn test_path_table_endian_selection() {
        let tmp = std::env::temp_dir().join("cellar-pt-endian-test");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("h.txt"), "hello").unwrap();

        let files = vec![IsoFile {
            name: "h.txt".to_string(),
            source: IsoFileSource::Path(tmp.join("h.txt")),
            size: 5,
            mtime: None,
        }];
        let iso_path = tmp.join("pt_endian.iso");
        write_iso(&iso_path, "pttest", &files, &|_| {}).unwrap();

        let data = fs::read(&iso_path).unwrap();

        // PVD Type L path table at sector 19
        let lpt_offset = 19 * 2048;
        let lpt = &data[lpt_offset..lpt_offset + 10];
        assert_eq!(lpt[0], 0x01, "L path table: identifier length must be 1");
        let lpt_extent = u32::from_le_bytes(lpt[2..6].try_into().unwrap());
        assert!(
            lpt_extent >= 23,
            "L path table extent should point to root dir at or after sector 23"
        );

        // PVD Type M path table at sector 20
        let mpt_offset = 20 * 2048;
        let mpt = &data[mpt_offset..mpt_offset + 10];
        assert_eq!(mpt[0], 0x01, "M path table: identifier length must be 1");
        let mpt_extent = u32::from_be_bytes(mpt[2..6].try_into().unwrap());
        assert_eq!(
            mpt_extent, lpt_extent,
            "M and L path tables must agree on root dir extent"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_to_pvd_name_format() {
        assert_eq!(to_pvd_name("hello.txt"), "HELLO.TXT;1");
        assert_eq!(to_pvd_name("readme"), "README;1");
        assert_eq!(to_pvd_name("archive.tar.gz"), "ARCHIVE_.GZ;1");
        assert_eq!(to_pvd_name("café.txt"), "CAF_.TXT;1");
        assert_eq!(to_pvd_name("malicious_sample.dll"), "MALICIOU.DLL;1");
        assert_eq!(to_pvd_name(".htaccess"), ".HTA;1");
        assert_eq!(
            to_pvd_name("a_very_long_filename_for_testing.txt"),
            "A_VERY_L.TXT;1"
        );
    }

    #[test]
    fn test_to_joliet_name_encoding() {
        let bytes = to_joliet_name("hi");
        assert_eq!(
            bytes,
            vec![0x00, 0x68, 0x00, 0x69],
            "ASCII Joliet = UCS-2 BE"
        );

        let cafe = to_joliet_name("café");
        assert_eq!(cafe[0..4], [0x00, 0x63, 0x00, 0x61], "'ca' in UCS-2 BE");

        let long_name: String = "x".repeat(100);
        let truncated = to_joliet_name(&long_name);
        let decoded_char_count = truncated.len() / 2;
        assert_eq!(
            decoded_char_count, 64,
            "Joliet name must truncate to 64 chars"
        );

        let empty = to_joliet_name("");
        assert!(empty.is_empty(), "empty name should produce empty bytes");
    }

    #[test]
    fn test_sanitize_label() {
        assert_eq!(sanitize_label("my label"), "MY_LABEL");
        assert_eq!(sanitize_label(""), "CELLAR");
        assert_eq!(sanitize_label("___"), "CELLAR");
        assert_eq!(
            sanitize_label("a_very_long_label_that_exceeds_32_characters"),
            "A_VERY_LONG_LABEL_THAT_EXCEEDS_3"
        );
        assert_eq!(sanitize_label("123"), "123");
    }

    #[test]
    fn test_dir_record_len_and_layout_overflow() {
        assert_eq!(
            dir_record_len(b"hi"),
            36,
            "odd-length name 'hi': 33+2=35, padded to 36"
        );
        assert_eq!(
            dir_record_len(b"abc"),
            36,
            "even-length name 'abc': 33+3=36"
        );
        assert_eq!(
            dir_record_len(b"test"),
            38,
            "even-length name 'test': 33+4=38"
        );

        let files = vec![IsoFile {
            name: "f.txt".to_string(),
            source: IsoFileSource::Bytes(vec![b'x']),
            size: 1,
            mtime: None,
        }];
        let layout = calculate_layout("test", &files).unwrap();
        assert!(layout.total_sectors >= 24);

        let big = IsoFile {
            name: "big.bin".to_string(),
            source: IsoFileSource::Bytes(vec![0u8; 1024]),
            size: u32::MAX as u64,
            mtime: None,
        };
        let err = calculate_layout("overflow", &[big]).unwrap_err();
        assert!(
            err.contains("4 GiB"),
            "4 GiB rejection msg should mention limit"
        );
    }

    #[test]
    fn test_pad_ucs2() {
        let mut buf = [0u8; 16];
        pad_ucs2(&mut buf, "AB");

        // 'A' = 0x0041 in UCS-2 BE -> [0x00, 0x41]
        // 'B' = 0x0042 in UCS-2 BE -> [0x00, 0x42]
        assert_eq!(buf[0], 0x00);
        assert_eq!(buf[1], 0x41);
        assert_eq!(buf[2], 0x00);
        assert_eq!(buf[3], 0x42);

        // Remaining 12 bytes should be UCS-2 spaces (0x00 0x20)
        assert_eq!(buf[4], 0x00);
        assert_eq!(buf[5], 0x20);
        assert_eq!(buf[14], 0x00);
        assert_eq!(buf[15], 0x20);

        // Empty string fills entirely with UCS-2 spaces
        let mut empty = [0xFFu8; 8];
        pad_ucs2(&mut empty, "");
        assert_eq!(empty, [0x00, 0x20, 0x00, 0x20, 0x00, 0x20, 0x00, 0x20]);
    }

    #[test]
    fn test_manifest_text_and_json() {
        use crate::manifest::{FileMetadata, Manifest, ManifestFields};

        let fields = ManifestFields {
            source: "https://example.com".to_string(),
            package_name: "acme".to_string(),
            package_version: "1.0".to_string(),
            severity: "high".to_string(),
            references: "https://cve.mitre.org/cgi-bin/cvename.cgi?name=CVE-2024-0001\nhttps://nvd.nist.gov/vuln/detail/CVE-2024-0001".to_string(),
            notes: "test notes".to_string(),
        };
        let meta = FileMetadata {
            mtime: Some("2024-01-01T00:00:00Z".to_string()),
            atime: None,
            ctime: None,
            permissions: Some(0o644),
            uid: Some(1000),
            gid: Some(1000),
        };
        let manifest = Manifest::build(
            "testlabel",
            &fields,
            vec![("readme.txt", "abc123", 42, &meta)],
        );

        let text = manifest.to_text();
        assert!(text.contains("cellar ISO manifest"), "text header");
        assert!(
            text.contains("readme.txt"),
            "text file listing uses resolved name"
        );
        assert!(text.contains("abc123"), "text contains hash");
        assert!(text.contains("Package: acme 1.0"), "text package line");
        assert!(
            text.contains("Source:  https://example.com"),
            "text source line"
        );
        assert!(text.contains("Severity: high"), "text severity");
        assert!(text.contains("References:"), "text references");
        assert!(text.contains("Notes:"), "text notes");

        let json = manifest.to_json();
        assert!(
            json.contains("\"name\": \"readme.txt\""),
            "json uses resolved name"
        );
        assert!(json.contains("\"sha256\": \"abc123\""), "json hash");
        assert!(json.contains("\"size_bytes\": 42"), "json size");
        assert!(json.contains("\"mtime\""), "json metadata present");
    }

    #[test]
    fn test_manifest_conditional_sections() {
        use crate::manifest::{FileMetadata, Manifest, ManifestFields};

        let fields = ManifestFields {
            source: String::new(),
            package_name: String::new(),
            package_version: String::new(),
            severity: String::new(),
            references: String::new(),
            notes: String::new(),
        };
        let meta = FileMetadata::default();
        let manifest = Manifest::build("minimal", &fields, vec![("a.txt", "deadbeef", 10, &meta)]);
        let text = manifest.to_text();
        assert!(
            !text.contains("Package:"),
            "empty package should be omitted"
        );
        assert!(!text.contains("Source:"), "empty source should be omitted");
        assert!(
            !text.contains("Severity:"),
            "empty severity should be omitted"
        );
        assert!(
            !text.contains("References:"),
            "empty references should be omitted"
        );
        assert!(!text.contains("Notes:"), "empty notes should be omitted");
    }

    #[test]
    fn test_file_size_mismatch_error() {
        let tmp = std::env::temp_dir().join("cellar-mismatch-test");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("liar.txt");
        fs::write(&path, "real content").unwrap();

        let real_len = fs::metadata(&path).unwrap().len();
        let wrong_size = real_len + 100;

        let file = IsoFile {
            name: "liar.txt".to_string(),
            source: IsoFileSource::Path(path.clone()),
            size: wrong_size,
            mtime: None,
        };
        let iso_path = tmp.join("mismatch.iso");
        let result = write_iso(&iso_path, "mismatch", &[file], &|_| {});
        assert!(result.is_err(), "should fail on size mismatch");
        assert!(
            result.unwrap_err().contains("Size mismatch"),
            "error should mention size mismatch"
        );

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_iso_creation_and_extraction() {
        let test_dir = std::env::temp_dir().join("cellar-iso-test");
        let _ = fs::remove_dir_all(&test_dir);
        fs::create_dir_all(&test_dir).unwrap();

        // Create test files
        fs::write(test_dir.join("hello.txt"), "Hello, World!").unwrap();
        fs::write(test_dir.join("foo.txt"), "foo content").unwrap();
        fs::write(test_dir.join("foo_1.txt"), "foo_1 content").unwrap();
        fs::write(test_dir.join("malicious_sample.dll"), "binary data here").unwrap();
        fs::write(test_dir.join("unicode.txt"), "unicode content").unwrap();

        let files = vec![
            IsoFile {
                name: "hello.txt".to_string(),
                source: IsoFileSource::Path(test_dir.join("hello.txt")),
                size: 13,
                mtime: None,
            },
            IsoFile {
                name: "foo.txt".to_string(),
                source: IsoFileSource::Path(test_dir.join("foo.txt")),
                size: 11,
                mtime: None,
            },
            IsoFile {
                name: "foo_1.txt".to_string(),
                source: IsoFileSource::Path(test_dir.join("foo_1.txt")),
                size: 13,
                mtime: None,
            },
            IsoFile {
                name: "malicious_sample.dll".to_string(),
                source: IsoFileSource::Path(test_dir.join("malicious_sample.dll")),
                size: 16,
                mtime: None,
            },
            IsoFile {
                name: "unicode.txt".to_string(),
                source: IsoFileSource::Path(test_dir.join("unicode.txt")),
                size: 15,
                mtime: None,
            },
        ];

        let iso_path = test_dir.join("test.iso");
        write_iso(&iso_path, "test-label", &files, &|_| {}).unwrap();

        // Verify ISO exists and has reasonable size
        let meta = fs::metadata(&iso_path).unwrap();
        assert!(meta.len() > 16 * 2048); // At least system area + descriptors
        assert!(meta.len().is_multiple_of(2048));

        // Extract with 7z and verify
        let extract_dir = test_dir.join("extracted");
        fs::create_dir_all(&extract_dir).unwrap();

        let output = std::process::Command::new("7z")
            .arg("x")
            .arg(&iso_path)
            .arg(format!("-o{}", extract_dir.display()))
            .output()
            .expect("7z must be installed for tests");

        assert!(
            output.status.success(),
            "7z extraction failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // Verify file contents
        let hello = fs::read_to_string(extract_dir.join("hello.txt")).unwrap();
        assert_eq!(hello, "Hello, World!");

        let foo = fs::read_to_string(extract_dir.join("foo.txt")).unwrap();
        assert_eq!(foo, "foo content");

        let foo1 = fs::read_to_string(extract_dir.join("foo_1.txt")).unwrap();
        assert_eq!(foo1, "foo_1 content");

        let dll = fs::read_to_string(extract_dir.join("malicious_sample.dll")).unwrap();
        assert_eq!(dll, "binary data here");

        let unicode = fs::read_to_string(extract_dir.join("unicode.txt")).unwrap();
        assert_eq!(unicode, "unicode content");

        // Cleanup
        let _ = fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn test_collision_naming() {
        let test_dir = std::env::temp_dir().join("cellar-collision-test");
        let _ = fs::remove_dir_all(&test_dir);
        fs::create_dir_all(&test_dir).unwrap();

        fs::write(test_dir.join("a.txt"), "first").unwrap();
        fs::write(test_dir.join("b.txt"), "second").unwrap();
        fs::write(test_dir.join("c.txt"), "third").unwrap();

        let files = vec![
            IsoFile {
                name: "a.txt".to_string(),
                source: IsoFileSource::Path(test_dir.join("a.txt")),
                size: 5,
                mtime: None,
            },
            IsoFile {
                name: "a_1.txt".to_string(),
                source: IsoFileSource::Path(test_dir.join("b.txt")),
                size: 6,
                mtime: None,
            },
            IsoFile {
                name: "a_2.txt".to_string(),
                source: IsoFileSource::Path(test_dir.join("c.txt")),
                size: 5,
                mtime: None,
            },
        ];

        let iso_path = test_dir.join("collision.iso");
        write_iso(&iso_path, "collision", &files, &|_| {}).unwrap();

        let extract_dir = test_dir.join("extracted");
        fs::create_dir_all(&extract_dir).unwrap();

        let output = std::process::Command::new("7z")
            .arg("x")
            .arg(&iso_path)
            .arg(format!("-o{}", extract_dir.display()))
            .output()
            .expect("7z must be installed");

        assert!(output.status.success());

        assert_eq!(
            fs::read_to_string(extract_dir.join("a.txt")).unwrap(),
            "first"
        );
        assert_eq!(
            fs::read_to_string(extract_dir.join("a_1.txt")).unwrap(),
            "second"
        );
        assert_eq!(
            fs::read_to_string(extract_dir.join("a_2.txt")).unwrap(),
            "third"
        );

        let _ = fs::remove_dir_all(&test_dir);
    }
}
