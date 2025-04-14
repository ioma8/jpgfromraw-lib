use anyhow::{ensure, Result};
use byteorder::{BigEndian, ByteOrder, LittleEndian};
use memchr::memmem;
use memmap2::Mmap;
use std::path::Path;

#[cfg(unix)]
mod unix;

#[cfg(windows)]
mod windows;

#[cfg(unix)]
use unix as platform;

use std::time::Instant;
#[cfg(windows)]
use windows as platform;

/// An embedded JPEG in a RAW file.
#[derive(Default, Eq, PartialEq)]
pub struct EmbeddedJpegInfo {
    offset: usize,
    length: usize,
    orientation: Option<u16>,
}

pub enum FindJpegType {
    Largest,
    Smallest,
}

const TIFF_HEADER: &[u8; 4] = b"II*\0";
const EXIF_HEADER: &[u8; 6] = b"Exif\0\0";
const EXIF_HEADER_SIZE: usize = 6;

fn find_tiff_header_offset(raw_buf: &[u8]) -> Result<usize> {
    if &raw_buf[0..4] == TIFF_HEADER {
        return Ok(0);
    }

    let found = memmem::find_iter(raw_buf, EXIF_HEADER).next();
    if let Some(pos) = found {
        return Ok(pos + EXIF_HEADER_SIZE);
    }
    Err(anyhow::anyhow!(
        "No Exif APP1 segment with TIFF header found"
    ))
}
/// Find the largest embedded JPEG data in a memory-mapped RAW buffer.
///
/// This function parses the IFDs in the TIFF structure of the RAW file to find the largest JPEG
/// thumbnail embedded in the file.
///
/// We hand roll the IFD parsing because libraries do not fit requirements. For example:
///
/// - kamadak-exif: Reads into a big `Vec<u8>`, which is huge for our big RAW.
/// - quickexif: Cannot iterate over IFDs.
fn find_largest_embedded_jpeg(
    raw_buf: &[u8],
    tiff_offset: usize,
    find_type: FindJpegType,
) -> Result<EmbeddedJpegInfo> {
    const IFD_ENTRY_SIZE: usize = 12;
    const TIFF_MAGIC_LE: &[u8] = b"II*\0";
    const TIFF_MAGIC_BE: &[u8] = b"MM\0*";
    const JPEG_TAG: u16 = 0x201;
    const JPEG_LENGTH_TAG: u16 = 0x202;
    const ORIENTATION_TAG: u16 = 0x112;

    let raw_buf = &raw_buf[tiff_offset..];

    ensure!(raw_buf.len() >= 8, "Not enough data for TIFF header");

    let is_le = &raw_buf[0..4] == TIFF_MAGIC_LE;
    ensure!(
        is_le || &raw_buf[0..4] == TIFF_MAGIC_BE,
        "Not a valid TIFF file"
    );

    let read_u16 = if is_le {
        LittleEndian::read_u16
    } else {
        BigEndian::read_u16
    };

    let read_u32 = if is_le {
        LittleEndian::read_u32
    } else {
        BigEndian::read_u32
    };

    let mut next_ifd_offset = read_u32(&raw_buf[4..8]).try_into()?;
    let mut largest_jpeg = EmbeddedJpegInfo::default();

    while next_ifd_offset != 0 {
        ensure!(next_ifd_offset + 2 <= raw_buf.len(), "Invalid IFD offset");

        let cursor = &raw_buf[next_ifd_offset..];
        let num_entries = read_u16(&cursor[..2]).into();
        let entries_cursor = &cursor[2..];

        let entries_len = num_entries * IFD_ENTRY_SIZE;
        ensure!(
            entries_cursor.len() >= entries_len,
            "Invalid number of IFD entries"
        );

        let mut cur_offset = None;
        let mut cur_length = None;
        let mut cur_orientation = None;

        for entry in entries_cursor
            .chunks_exact(IFD_ENTRY_SIZE)
            .take(num_entries)
        {
            let tag = read_u16(&entry[..2]);

            match tag {
                JPEG_TAG => cur_offset = Some(read_u32(&entry[8..12]).try_into()?),
                JPEG_LENGTH_TAG => cur_length = Some(read_u32(&entry[8..12]).try_into()?),
                ORIENTATION_TAG => cur_orientation = Some(read_u16(&entry[8..10])),
                _ => {}
            }

            if let (Some(offset), Some(length)) = (cur_offset, cur_length) {
                match find_type {
                    FindJpegType::Smallest => {
                        if length < largest_jpeg.length || largest_jpeg.length == 0 {
                            largest_jpeg = EmbeddedJpegInfo {
                                offset,
                                length,
                                orientation: cur_orientation,
                            };
                        }
                    }
                    FindJpegType::Largest => {
                        if length > largest_jpeg.length {
                            largest_jpeg = EmbeddedJpegInfo {
                                offset,
                                length,
                                orientation: cur_orientation,
                            };
                        }
                    }
                }
                break;
            }
        }

        let next_ifd_offset_offset = 2 + entries_len;
        ensure!(
            cursor.len() >= next_ifd_offset_offset + 4,
            "Invalid next IFD offset"
        );
        next_ifd_offset = read_u32(&cursor[next_ifd_offset_offset..][..4]).try_into()?;
    }

    ensure!(
        largest_jpeg != EmbeddedJpegInfo::default(),
        "No JPEG data found"
    );
    ensure!(
        largest_jpeg.offset + largest_jpeg.length <= raw_buf.len(),
        "JPEG data exceeds file size"
    );

    let jpeg_offset = largest_jpeg.offset + tiff_offset;

    Ok(EmbeddedJpegInfo {
        offset: jpeg_offset,
        length: largest_jpeg.length,
        orientation: largest_jpeg.orientation,
    })
}

/// Extract the JPEG bytes from the memory-mapped RAW buffer.
fn extract_jpeg<'raw>(raw_buf: &'raw Mmap, jpeg: &'raw EmbeddedJpegInfo) -> Result<&'raw [u8]> {
    platform::prefetch_jpeg(raw_buf, jpeg)?;
    Ok(&raw_buf[jpeg.offset..jpeg.offset + jpeg.length])
}

/// The embedded JPEG comes with no EXIF data. While most of it is outside of the scope of this
/// application, it's pretty vexing to have the wrong orientation, so copy that over.
#[rustfmt::skip]
const fn get_header_bytes(orientation: u16) -> [u8; 34] {
    let orientation_bytes = orientation.to_le_bytes();
    [
        0xff, 0xd8, // SOI
        0xff, 0xe1, // APP1
        0x00, 0x1e, // 30 bytes including this length
        0x45, 0x78, 0x69, 0x66, 0x00, 0x00, // Exif\0\0
        0x49, 0x49, 0x2A, 0x00, // TIFF LE
        0x08, 0x00, 0x00, 0x00, // Offset to IFD
        0x01, 0x00, // One entry in IFD
        0x12, 0x01, // Tag for orientation
        0x03, 0x00, // Type: SHORT
        0x01, 0x00, 0x00, 0x00, // Count: 1
        orientation_bytes[0], orientation_bytes[1], // Orientation
        0x00, 0x00, // Next IFD
    ]
}

async fn get_jpeg_data(jpeg_buf: &[u8], jpeg_info: &EmbeddedJpegInfo) -> Result<Vec<u8>> {
    let mut jpeg_data = Vec::with_capacity(jpeg_buf.len() + 34);
    jpeg_data.extend_from_slice(&get_header_bytes(jpeg_info.orientation.unwrap_or(1)));
    jpeg_data.extend_from_slice(&jpeg_buf[2..]);
    Ok(jpeg_data)
}

/// Process a single RAW file to extract the embedded JPEG, and then write the extracted JPEG to
/// the output directory.
pub async fn process_file(
    entry_path: &Path,
    out_dir: &Path,
    relative_path: &Path,
    find_type: FindJpegType,
) -> Result<()> {
    let jpeg_data = process_file_bytes(entry_path, find_type).await?;
    let mut output_file = out_dir.join(relative_path);
    output_file.set_extension("jpg");
    if let Some(parent) = output_file.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&output_file, &jpeg_data).await?;
    Ok(())
}

// Process a single RAW file to extract the embedded JPEG and return the JPEG bytes.
pub async fn process_file_bytes(entry_path: &Path, find_type: FindJpegType) -> Result<Vec<u8>> {
    let start = Instant::now();
    let in_file = platform::open_raw(entry_path).await?;
    println!("Time to open_raw: {:?}", start.elapsed());

    let start = Instant::now();
    let raw_buf = platform::mmap_raw(in_file)?;
    println!("Time to mmap_raw: {:?}", start.elapsed());

    let start = Instant::now();
    let tiff_offset = if let Ok(offset) = find_tiff_header_offset(&raw_buf) {
        offset
    } else {
        0
    };
    println!("Offset found at: {}", tiff_offset);
    println!("Time to find_tiff_header_offset: {:?}", start.elapsed());

    let start = Instant::now();
    let jpeg_info = find_largest_embedded_jpeg(&raw_buf, tiff_offset, find_type)?;
    println!("Time to find_largest_embedded_jpeg: {:?}", start.elapsed());

    let start = Instant::now();
    let jpeg_buf = extract_jpeg(&raw_buf, &jpeg_info)?;
    println!("Time to extract_jpeg: {:?}", start.elapsed());

    let start = Instant::now();
    let jpeg_data = get_jpeg_data(jpeg_buf, &jpeg_info).await?;
    println!("Time to get_jpeg_data: {:?}", start.elapsed());

    Ok(jpeg_data)
}
