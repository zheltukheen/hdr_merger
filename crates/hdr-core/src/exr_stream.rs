//! Streaming OpenEXR writer: one scanline in RAM at a time.
//!
//! A 60 MP master is handed to this writer row by row, so the full `f32`
//! buffer never has to exist. The file is a single-part, scanline,
//! uncompressed, 32-bit float RGB EXR in increasing-Y order, written
//! little-endian as the OpenEXR 2.x layout requires.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use crate::error::{HdrError, Result};

const MAGIC: [u8; 4] = [0x76, 0x2f, 0x31, 0x01];
const VERSION: u32 = 2;
const NO_COMPRESSION: u8 = 0;
const INCREASING_Y: u8 = 0;
const FLOAT_PIXEL_TYPE: i32 = 2;
const CHANNEL_PLANES: usize = 3;
const BYTES_PER_FLOAT: usize = 4;
const SCANLINE_HEADER_BYTES: u64 = 8;
const ZERO_CHUNK: usize = 8 * 1024;

/// Streaming writer for 32-bit float RGB scanline OpenEXR files.
pub struct ExrStreamWriter {
    writer: BufWriter<File>,
    width: u32,
    height: u32,
    row_len: usize,
    data_size: u32,
    rows_written: u32,
    overflowed: bool,
    offsets: Vec<u64>,
    table_offset: u64,
    position: u64,
    row_buf: Vec<u8>,
}

impl ExrStreamWriter {
    /// Create the file, write the header and reserve the line-offset table.
    /// 32-bit float RGB, no compression, scanline order, top to bottom.
    pub fn create(path: &Path, width: u32, height: u32) -> Result<Self> {
        let (data_size, row_len) = validate_dimensions(width, height)?;
        let mut writer = BufWriter::new(File::create(path)?);

        let header = header_bytes(width, height);
        writer.write_all(&header)?;
        let table_offset = header.len() as u64;

        let table_bytes = u64::from(height) * 8;
        write_zeroes(&mut writer, table_bytes)?;

        Ok(Self {
            writer,
            width,
            height,
            row_len,
            data_size,
            rows_written: 0,
            overflowed: false,
            offsets: Vec::with_capacity(height as usize),
            table_offset,
            position: table_offset + table_bytes,
            row_buf: vec![0; SCANLINE_HEADER_BYTES as usize + data_size as usize],
        })
    }

    /// Append the next scanline. `row.len()` must be `width * 3`.
    /// Rows must be supplied in order, exactly `height` times.
    ///
    /// A row past `height` is accepted but flagged; [`finish`](Self::finish)
    /// then refuses to produce a truncated file.
    pub fn write_row(&mut self, row: &[f32]) -> Result<()> {
        if row.len() != self.row_len {
            return Err(HdrError::Invalid(format!(
                "scanline has {} values, expected width*3 = {}",
                row.len(),
                self.row_len
            )));
        }
        if self.rows_written >= self.height {
            self.overflowed = true;
            return Ok(());
        }

        let width = self.width as usize;
        let plane_bytes = width * BYTES_PER_FLOAT;
        self.row_buf[..4].copy_from_slice(&(self.rows_written as i32).to_le_bytes());
        self.row_buf[4..8].copy_from_slice(&self.data_size.to_le_bytes());
        for (plane, channel) in [2usize, 1, 0].into_iter().enumerate() {
            let start = SCANLINE_HEADER_BYTES as usize + plane * plane_bytes;
            write_channel_plane(&mut self.row_buf[start..start + plane_bytes], row, channel);
        }

        self.offsets.push(self.position);
        self.writer.write_all(&self.row_buf)?;
        self.position += SCANLINE_HEADER_BYTES + u64::from(self.data_size);
        self.rows_written += 1;
        Ok(())
    }

    /// Patch the offset table, flush and return the number of bytes written.
    pub fn finish(mut self) -> Result<u64> {
        if self.overflowed || self.rows_written != self.height {
            return Err(HdrError::Invalid(format!(
                "expected exactly {} scanlines, wrote {}",
                self.height, self.rows_written
            )));
        }

        self.writer.seek(SeekFrom::Start(self.table_offset))?;
        for offset in &self.offsets {
            self.writer.write_all(&offset.to_le_bytes())?;
        }
        self.writer.flush()?;
        Ok(self.writer.get_ref().metadata()?.len())
    }
}

/// Reject shapes the on-disk format cannot describe.
fn validate_dimensions(width: u32, height: u32) -> Result<(u32, usize)> {
    if width == 0 || height == 0 {
        return Err(HdrError::Invalid("EXR dimensions must be non-zero".into()));
    }
    if width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(HdrError::Invalid(
            "EXR dimensions must fit in a signed 32-bit int".into(),
        ));
    }
    let data_size = width
        .checked_mul((CHANNEL_PLANES * BYTES_PER_FLOAT) as u32)
        .ok_or_else(|| HdrError::Invalid("scanline byte size overflows u32".into()))?;
    let row_len = (width as usize)
        .checked_mul(CHANNEL_PLANES)
        .ok_or_else(|| HdrError::Invalid("row length overflows usize".into()))?;
    Ok((data_size, row_len))
}

/// Copy one interleaved RGB channel into its planar slot.
fn write_channel_plane(out: &mut [u8], row: &[f32], channel: usize) {
    for (slot, pixel) in out
        .chunks_exact_mut(BYTES_PER_FLOAT)
        .zip(row.chunks_exact(3))
    {
        slot.copy_from_slice(&pixel[channel].to_le_bytes());
    }
}

fn write_zeroes(writer: &mut impl Write, mut bytes: u64) -> Result<()> {
    let zeros = [0u8; ZERO_CHUNK];
    while bytes > 0 {
        let chunk = bytes.min(ZERO_CHUNK as u64) as usize;
        writer.write_all(&zeros[..chunk])?;
        bytes -= chunk as u64;
    }
    Ok(())
}

/// Complete header: magic, version, the standard attributes, terminator.
fn header_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut header = Vec::with_capacity(256);
    header.extend_from_slice(&MAGIC);
    push_u32(&mut header, VERSION);

    let mut channels = Vec::new();
    push_channel_list(&mut channels);
    push_attribute(&mut header, b"channels", b"chlist", &channels);
    push_attribute(
        &mut header,
        b"compression",
        b"compression",
        &[NO_COMPRESSION],
    );

    let mut window = Vec::with_capacity(16);
    push_window(&mut window, width, height);
    push_attribute(&mut header, b"dataWindow", b"box2i", &window);
    push_attribute(&mut header, b"displayWindow", b"box2i", &window);

    push_attribute(&mut header, b"lineOrder", b"lineOrder", &[INCREASING_Y]);
    push_attribute(
        &mut header,
        b"pixelAspectRatio",
        b"float",
        &1.0f32.to_le_bytes(),
    );

    let mut center = Vec::with_capacity(8);
    push_f32(&mut center, 0.0);
    push_f32(&mut center, 0.0);
    push_attribute(&mut header, b"screenWindowCenter", b"v2f", &center);
    push_attribute(
        &mut header,
        b"screenWindowWidth",
        b"float",
        &1.0f32.to_le_bytes(),
    );

    header.push(0);
    header
}

/// Channel list in file order: B, G, R, then the empty-name terminator.
fn push_channel_list(out: &mut Vec<u8>) {
    for name in [b"B", b"G", b"R"] {
        push_channel(out, name);
    }
    out.push(0);
}

fn push_channel(out: &mut Vec<u8>, name: &[u8]) {
    out.extend_from_slice(name);
    out.push(0);
    push_i32(out, FLOAT_PIXEL_TYPE);
    out.push(0); // pLinear
    out.extend_from_slice(&[0; 3]); // reserved
    push_i32(out, 1); // xSampling
    push_i32(out, 1); // ySampling
}

fn push_window(out: &mut Vec<u8>, width: u32, height: u32) {
    push_i32(out, 0);
    push_i32(out, 0);
    push_i32(out, width as i32 - 1);
    push_i32(out, height as i32 - 1);
}

fn push_attribute(out: &mut Vec<u8>, name: &[u8], kind: &[u8], value: &[u8]) {
    out.extend_from_slice(name);
    out.push(0);
    out.extend_from_slice(kind);
    out.push(0);
    push_i32(out, value.len() as i32);
    out.extend_from_slice(value);
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{load, InputEncoding};
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("hdr-core-exr-stream-tests");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir.join(name)
    }

    fn known_pixel(x: u32, y: u32) -> [f32; 3] {
        [
            x as f32 * 0.25 + y as f32 * 0.5,
            0.5 + x as f32,
            2.0 + y as f32 * 0.125,
        ]
    }

    fn write_known_image(writer: &mut ExrStreamWriter, width: u32, height: u32) {
        for y in 0..height {
            let mut row = Vec::with_capacity(width as usize * 3);
            for x in 0..width {
                row.extend_from_slice(&known_pixel(x, y));
            }
            writer.write_row(&row).expect("write row");
        }
    }

    fn remove(path: &std::path::Path) {
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn writes_a_readable_exr() {
        let (width, height) = (8, 4);
        let path = temp_path("readable.exr");
        remove(&path);

        let mut writer = ExrStreamWriter::create(&path, width, height).expect("create");
        write_known_image(&mut writer, width, height);
        writer.finish().expect("finish");

        let loaded = load(&path, InputEncoding::Linear).expect("load");
        assert_eq!((loaded.width, loaded.height), (width, height));
        for (x, y) in [(0, 0), (3, 2), (7, 3)] {
            let expected = known_pixel(x, y);
            let actual = loaded.pixel(x, y);
            for channel in 0..3 {
                assert!(
                    (actual[channel] - expected[channel]).abs() < 1e-5,
                    "pixel ({x},{y}) channel {channel}: {actual:?} != {expected:?}"
                );
            }
        }
        remove(&path);
    }

    #[test]
    fn rejects_a_short_row() {
        let path = temp_path("short-row.exr");
        remove(&path);
        let mut writer = ExrStreamWriter::create(&path, 4, 2).expect("create");

        let error = writer.write_row(&[0.0; 11]).expect_err("short row");
        assert!(matches!(error, crate::HdrError::Invalid(_)), "{error:?}");
        remove(&path);
    }

    #[test]
    fn rejects_too_many_rows() {
        let path = temp_path("too-many-rows.exr");
        remove(&path);
        let mut writer = ExrStreamWriter::create(&path, 2, 2).expect("create");
        let row = [0.1f32; 6];
        for _ in 0..3 {
            writer.write_row(&row).expect("write row");
        }

        let error = writer.finish().expect_err("too many rows");
        assert!(matches!(error, crate::HdrError::Invalid(_)), "{error:?}");
        remove(&path);
    }

    #[test]
    fn reports_bytes_written() {
        let path = temp_path("bytes.exr");
        remove(&path);

        let mut writer = ExrStreamWriter::create(&path, 3, 2).expect("create");
        write_known_image(&mut writer, 3, 2);
        let reported = writer.finish().expect("finish");

        let on_disk = std::fs::metadata(&path).expect("metadata").len();
        assert_eq!(reported, on_disk);
        remove(&path);
    }

    #[test]
    fn exr_crate_reads_what_we_write() {
        let (width, height) = (3, 2);
        let path = temp_path("third-party.exr");
        remove(&path);

        let mut writer = ExrStreamWriter::create(&path, width, height).expect("create");
        write_known_image(&mut writer, width, height);
        writer.finish().expect("finish");

        let image = exr::prelude::read_first_rgba_layer_from_file(
            &path,
            |resolution, _| vec![vec![[0.0f32; 4]; resolution.width()]; resolution.height()],
            |pixels, position, (r, g, b, a): (f32, f32, f32, f32)| {
                pixels[position.y()][position.x()] = [r, g, b, a];
            },
        )
        .expect("read with exr crate");

        let layer = image.layer_data;
        assert_eq!(layer.size.width(), width as usize);
        assert_eq!(layer.size.height(), height as usize);
        let expected = known_pixel(2, 1);
        let actual = layer.channel_data.pixels[1][2];
        for channel in 0..3 {
            assert!(
                (actual[channel] - expected[channel]).abs() < 1e-5,
                "third-party pixel: {actual:?} != {expected:?}"
            );
        }
        remove(&path);
    }
}
