//! Layered PSD/PSB export.
//!
//! Every frame becomes a layer carrying its exclusion mask as a Photoshop
//! user-supplied layer mask (`channel id -2`). Layer records are written in
//! PSD file order, top to bottom: `layers[0]` is the topmost layer in the
//! Layers panel.
//!
//! Channel data uses PackBits RLE. The merged preview is always written, as
//! the format requires; Photoshop shows it while layers are hidden.

use crate::error::{HdrError, Result};
use std::fs::File;
use std::io::{BufWriter, Write};

/// PSD (version 1) maximum width or height.
const PSD_MAX_DIMENSION: u32 = 30_000;
/// PSB (version 2) maximum width or height.
const PSB_MAX_DIMENSION: u32 = 300_000;
/// Conservative PSD file-size ceiling; above it a PSB is needed.
const PSD_SIZE_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
/// PackBits streams are stored in rows no longer than this.
const MAX_PACKBITS_RUN: usize = 128;

#[derive(Clone, Debug)]
pub struct PsdLayer {
    pub name: String,
    /// 8-bit RGB, width*height*3, row major.
    pub rgb: Vec<u8>,
    /// 8-bit mask, width*height. 255 = visible, 0 = hidden (the inverse of the
    /// engine's exclusion Mask). `None` writes no mask channel.
    pub mask: Option<Vec<u8>>,
    pub visible: bool,
    pub opacity: u8,
}

/// Writes an 8-bit RGB document. `merged` is the flattened preview that
/// Photoshop shows when layers are hidden; pass the composite if you have it,
/// otherwise the first layer's RGB is used.
pub fn save(
    path: &std::path::Path,
    width: u32,
    height: u32,
    layers: &[PsdLayer],
    merged: Option<&[u8]>,
    psb: bool,
) -> crate::Result<()> {
    validate_document(width, height, layers, merged, psb)?;

    let composite = match merged {
        Some(data) => data,
        None => layers
            .first()
            .map(|layer| layer.rgb.as_slice())
            .ok_or_else(|| {
                HdrError::Invalid("PSD export needs a layer or a merged image".into())
            })?,
    };

    let plans = plan_layers(layers, width, height, psb)?;
    let file = File::create(path)?;
    let mut out = BufWriter::new(file);

    write_header(&mut out, width, height, psb)?;
    write_u32(&mut out, 0)?; // colour mode data length
    write_u32(&mut out, 0)?; // image resources length
    write_layer_and_mask(&mut out, &plans, width, psb)?;
    write_composite(&mut out, width, height, composite, psb)?;
    out.flush()?;
    Ok(())
}

/// PSB is required above 30000 px on either side or when the document would
/// exceed the 2 GB PSD limit; this is the cheap heuristic the UI uses.
pub fn should_use_psb(width: u32, height: u32, layer_count: usize) -> bool {
    if width > PSD_MAX_DIMENSION || height > PSD_MAX_DIMENSION {
        return true;
    }
    let pixels = u64::from(width) * u64::from(height);
    // Worst case: RGB plus a mask for every layer, plus the composite.
    let bytes_per_pixel = (layer_count as u64).saturating_mul(4).saturating_add(3);
    pixels.saturating_mul(bytes_per_pixel) > PSD_SIZE_LIMIT
}

/// The inverse of the engine's exclusion mask: excluded -> 0, kept -> 255.
pub fn mask_channel_from_exclusion(exclusion: &crate::Mask) -> Vec<u8> {
    exclusion
        .data
        .iter()
        .map(|value| if *value > 127 { 0 } else { 255 })
        .collect()
}

fn validate_document(
    width: u32,
    height: u32,
    layers: &[PsdLayer],
    merged: Option<&[u8]>,
    psb: bool,
) -> Result<()> {
    let max_dimension = if psb {
        PSB_MAX_DIMENSION
    } else {
        PSD_MAX_DIMENSION
    };
    if width == 0 || height == 0 || width > max_dimension || height > max_dimension {
        return Err(HdrError::Invalid(format!(
            "invalid PSD dimensions {width}x{height} (max {max_dimension})"
        )));
    }
    if layers.len() > i16::MAX as usize {
        return Err(HdrError::Invalid(format!(
            "too many layers for a PSD: {}",
            layers.len()
        )));
    }

    let pixels = usize::try_from(u64::from(width) * u64::from(height))
        .map_err(|_| HdrError::Invalid("document is too large".into()))?;
    let rgb_len = pixels
        .checked_mul(3)
        .ok_or_else(|| HdrError::Invalid("document is too large".into()))?;

    for (index, layer) in layers.iter().enumerate() {
        if layer.rgb.len() != rgb_len {
            return Err(HdrError::Invalid(format!(
                "layer {index} has {} RGB bytes, expected {rgb_len}",
                layer.rgb.len()
            )));
        }
        if let Some(mask) = &layer.mask {
            if mask.len() != pixels {
                return Err(HdrError::Invalid(format!(
                    "layer {index} has {} mask bytes, expected {pixels}",
                    mask.len()
                )));
            }
        }
        if layer.name.len() > u8::MAX as usize {
            return Err(HdrError::Invalid(format!(
                "layer {index} name is longer than 255 bytes"
            )));
        }
    }

    if let Some(data) = merged {
        if data.len() != rgb_len {
            return Err(HdrError::Invalid(format!(
                "merged image has {} RGB bytes, expected {rgb_len}",
                data.len()
            )));
        }
    }
    Ok(())
}

struct ChannelPlan<'a> {
    id: i16,
    data: &'a [u8],
    stride: usize,
    offset: usize,
    row_lengths: Vec<u32>,
}

struct LayerPlan<'a> {
    record: Vec<u8>,
    channels: Vec<ChannelPlan<'a>>,
}

fn plan_layers<'a>(
    layers: &'a [PsdLayer],
    width: u32,
    height: u32,
    wide: bool,
) -> Result<Vec<LayerPlan<'a>>> {
    let mut plans = Vec::with_capacity(layers.len());

    for layer in layers {
        let mut channels = Vec::with_capacity(if layer.mask.is_some() { 4 } else { 3 });
        for id in [0i16, 1, 2] {
            let offset = id as usize;
            channels.push(ChannelPlan {
                id,
                data: &layer.rgb,
                stride: 3,
                offset,
                row_lengths: rle_row_lengths(
                    width as usize,
                    height as usize,
                    &layer.rgb,
                    3,
                    offset,
                ),
            });
        }
        if let Some(mask) = &layer.mask {
            channels.push(ChannelPlan {
                id: -2,
                data: mask,
                stride: 1,
                offset: 0,
                row_lengths: rle_row_lengths(width as usize, height as usize, mask, 1, 0),
            });
        }

        let record = layer_record(layer, &channels, width, height, wide)?;
        plans.push(LayerPlan { record, channels });
    }
    Ok(plans)
}

fn layer_record(
    layer: &PsdLayer,
    channels: &[ChannelPlan<'_>],
    width: u32,
    height: u32,
    wide: bool,
) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(128 + layer.name.len());
    push_i32(&mut out, 0); // top
    push_i32(&mut out, 0); // left
    push_i32(&mut out, height as i32); // bottom
    push_i32(&mut out, width as i32); // right
    push_u16(&mut out, channels.len() as u16);

    for channel in channels {
        push_i16(&mut out, channel.id);
        let length = channel_length(channel, wide);
        if wide {
            push_u64(&mut out, length);
        } else {
            let length = u32::try_from(length).map_err(|_| {
                HdrError::Invalid("a layer channel exceeds the PSD size limit".into())
            })?;
            push_u32(&mut out, length);
        }
    }

    out.extend_from_slice(b"8BIM");
    out.extend_from_slice(b"norm");
    out.push(layer.opacity);
    out.push(0); // clipping: base
    out.push(if layer.visible { 0x02 } else { 0 }); // bit 1 = visible
    out.push(0); // filler

    let extra = layer_extra(layer, width, height);
    push_u32(&mut out, extra.len() as u32);
    out.extend_from_slice(&extra);
    Ok(out)
}

fn layer_extra(layer: &PsdLayer, width: u32, height: u32) -> Vec<u8> {
    let mut extra = Vec::with_capacity(32 + layer.name.len());

    if layer.mask.is_some() {
        push_u32(&mut extra, 20); // mask parameters block length
        push_i32(&mut extra, 0); // top
        push_i32(&mut extra, 0); // left
        push_i32(&mut extra, height as i32); // bottom
        push_i32(&mut extra, width as i32); // right
        extra.push(0); // default colour
        extra.push(0); // flags
        extra.extend_from_slice(&[0, 0]); // pad the block to 20 bytes
    } else {
        push_u32(&mut extra, 0); // no layer mask
    }

    push_u32(&mut extra, 0); // layer blending ranges length

    let name = layer.name.as_bytes();
    extra.push(name.len() as u8);
    extra.extend_from_slice(name);
    let padding = (4 - (1 + name.len()) % 4) % 4;
    extra.extend(std::iter::repeat(0u8).take(padding));

    extra
}

fn channel_length(channel: &ChannelPlan<'_>, wide: bool) -> u64 {
    let table_bytes = channel.row_lengths.len() as u64 * if wide { 4 } else { 2 };
    let data_bytes: u64 = channel.row_lengths.iter().map(|len| u64::from(*len)).sum();
    2 + table_bytes + data_bytes // 2 bytes = RLE compression marker
}

fn write_layer_and_mask(
    out: &mut impl Write,
    plans: &[LayerPlan<'_>],
    width: u32,
    wide: bool,
) -> std::io::Result<()> {
    let mut body_len: u64 = 2; // layer count
    for plan in plans {
        body_len += plan.record.len() as u64;
        for channel in &plan.channels {
            body_len += channel_length(channel, wide);
        }
    }
    let padded_len = body_len + body_len % 2;
    let section_len = if wide { 8 } else { 4 } + padded_len + 4;

    if wide {
        write_u64(out, section_len)?;
        write_u64(out, padded_len)?;
    } else {
        let section_len = u32::try_from(section_len)
            .map_err(|_| std::io::Error::other("PSD layer section exceeds 4 GB"))?;
        let padded_len = u32::try_from(padded_len)
            .map_err(|_| std::io::Error::other("PSD layer info exceeds 4 GB"))?;
        write_u32(out, section_len)?;
        write_u32(out, padded_len)?;
    }

    write_u16(out, plans.len() as u16)?;
    for plan in plans {
        out.write_all(&plan.record)?;
    }
    for plan in plans {
        for channel in &plan.channels {
            write_channel(out, width, channel, wide)?;
        }
    }
    if body_len % 2 == 1 {
        out.write_all(&[0])?;
    }

    write_u32(out, 0)?; // global layer mask info length
    Ok(())
}

fn write_header(out: &mut impl Write, width: u32, height: u32, wide: bool) -> std::io::Result<()> {
    out.write_all(b"8BPS")?;
    write_u16(out, if wide { 2 } else { 1 })?;
    out.write_all(&[0; 6])?;
    write_u16(out, 3)?; // channels: R, G, B
    write_u32(out, height)?;
    write_u32(out, width)?;
    write_u16(out, 8)?; // depth
    write_u16(out, 3)?; // RGB colour mode
    Ok(())
}

fn write_channel(
    out: &mut impl Write,
    width: u32,
    channel: &ChannelPlan<'_>,
    wide: bool,
) -> std::io::Result<()> {
    write_u16(out, 1)?; // RLE
    for length in &channel.row_lengths {
        write_row_length(out, *length, wide)?;
    }
    let mut row = vec![0u8; width as usize];
    for scanline in 0..channel.row_lengths.len() {
        fill_plane_row(
            channel.data,
            scanline * width as usize * channel.stride,
            channel.stride,
            channel.offset,
            &mut row,
        );
        out.write_all(&packbits_row(&row))?;
    }
    Ok(())
}

fn write_composite(
    out: &mut impl Write,
    width: u32,
    height: u32,
    merged: &[u8],
    wide: bool,
) -> std::io::Result<()> {
    let mut row_lengths = Vec::with_capacity(3 * height as usize);
    for channel in 0..3 {
        row_lengths.extend(rle_row_lengths(
            width as usize,
            height as usize,
            merged,
            3,
            channel,
        ));
    }

    write_u16(out, 1)?; // RLE
    for length in &row_lengths {
        write_row_length(out, *length, wide)?;
    }
    let mut row = vec![0u8; width as usize];
    for channel in 0..3 {
        for scanline in 0..height as usize {
            fill_plane_row(merged, scanline * width as usize * 3, 3, channel, &mut row);
            out.write_all(&packbits_row(&row))?;
        }
    }
    Ok(())
}

fn write_row_length(out: &mut impl Write, length: u32, wide: bool) -> std::io::Result<()> {
    if wide {
        return write_u32(out, length);
    }
    let length = u16::try_from(length)
        .map_err(|_| std::io::Error::other("RLE scanline exceeds the PSD row limit"))?;
    write_u16(out, length)
}

fn rle_row_lengths(
    width: usize,
    height: usize,
    data: &[u8],
    stride: usize,
    offset: usize,
) -> Vec<u32> {
    let mut row = vec![0u8; width];
    let mut lengths = Vec::with_capacity(height);
    for scanline in 0..height {
        fill_plane_row(data, scanline * width * stride, stride, offset, &mut row);
        lengths.push(packbits_row(&row).len() as u32);
    }
    lengths
}

/// Copy one channel row out of an interleaved plane (`stride` bytes per pixel).
fn fill_plane_row(plane: &[u8], row_start: usize, stride: usize, offset: usize, row: &mut [u8]) {
    for (index, byte) in row.iter_mut().enumerate() {
        *byte = plane[row_start + index * stride + offset];
    }
}

/// PackBits-encode one scanline.
fn packbits_row(row: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(row.len() + row.len() / MAX_PACKBITS_RUN + 1);
    let mut index = 0;

    while index < row.len() {
        let run = run_length(&row[index..]);
        if run >= 3 {
            let count = run.min(MAX_PACKBITS_RUN);
            out.push((257 - count) as u8);
            out.push(row[index]);
            index += count;
        } else {
            let start = index;
            index += run;
            while index < row.len() && index - start < MAX_PACKBITS_RUN {
                let run = run_length(&row[index..]);
                if run >= 3 {
                    break;
                }
                index += run.min(MAX_PACKBITS_RUN - (index - start));
            }
            let count = index - start;
            out.push((count - 1) as u8);
            out.extend_from_slice(&row[start..index]);
        }
    }
    out
}

fn run_length(data: &[u8]) -> usize {
    let first = data[0];
    data.iter()
        .take(MAX_PACKBITS_RUN)
        .take_while(|byte| **byte == first)
        .count()
}

fn write_u16(out: &mut impl Write, value: u16) -> std::io::Result<()> {
    out.write_all(&value.to_be_bytes())
}

fn write_u32(out: &mut impl Write, value: u32) -> std::io::Result<()> {
    out.write_all(&value.to_be_bytes())
}

fn write_u64(out: &mut impl Write, value: u64) -> std::io::Result<()> {
    out.write_all(&value.to_be_bytes())
}

fn push_i16(out: &mut Vec<u8>, value: i16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::psd::{Psd, PsdChannelKind};
    use std::path::PathBuf;

    use crate::Mask;

    const WIDTH: u32 = 32;
    const HEIGHT: u32 = 24;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("hdr-core-psd-tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn sample_layer(name: &str, seed: u8, with_mask: bool) -> PsdLayer {
        let mut rgb = vec![0u8; (WIDTH * HEIGHT * 3) as usize];
        for (index, pixel) in rgb.chunks_exact_mut(3).enumerate() {
            pixel[0] = (index % 251) as u8;
            pixel[1] = seed;
            pixel[2] = (index / 3 % 251) as u8;
        }
        let mask = with_mask.then(|| {
            (0..WIDTH * HEIGHT)
                .map(|index| if index % 7 == 0 { 0 } else { 255 })
                .collect()
        });
        PsdLayer {
            name: name.to_string(),
            rgb,
            mask,
            visible: true,
            opacity: 255,
        }
    }

    fn sample_layers() -> Vec<PsdLayer> {
        let mut layers = vec![
            sample_layer("top", 10, false),
            sample_layer("middle", 20, true),
            sample_layer("bottom", 30, false),
        ];
        layers[0].visible = false;
        layers[1].opacity = 200;
        layers
    }

    #[test]
    fn save_writes_layers_names_and_mask() {
        let layers = sample_layers();
        let merged = vec![128u8; (WIDTH * HEIGHT * 3) as usize];
        let path = temp_path("layers.psd");
        save(&path, WIDTH, HEIGHT, &layers, Some(&merged), false).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let document = Psd::from_bytes(&bytes).unwrap();
        assert_eq!((document.width(), document.height()), (WIDTH, HEIGHT));

        let names: Vec<&str> = document.layers().iter().map(|layer| layer.name()).collect();
        assert_eq!(names, vec!["bottom", "middle", "top"]);

        let masked = document.layer_by_name("middle").unwrap();
        assert_eq!(masked.opacity(), 200);
        assert!(masked.visible());
        assert!(masked
            .compression(PsdChannelKind::UserSuppliedLayerMask)
            .is_ok());
        let unmasked = document.layer_by_name("top").unwrap();
        assert!(!unmasked.visible());
        assert!(unmasked
            .compression(PsdChannelKind::UserSuppliedLayerMask)
            .is_err());

        // The psd crate's own PackBits decoder reads back the RGB channel data.
        let rgba = document.layer_by_name("bottom").unwrap().rgba();
        let (x, y) = (5usize, 7usize);
        let source = (y * WIDTH as usize + x) * 3;
        let destination = (y * WIDTH as usize + x) * 4;
        assert_eq!(
            &rgba[destination..destination + 3],
            &layers[2].rgb[source..source + 3]
        );

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn psd_header_uses_version_one() {
        let path = temp_path("version.psd");
        save(&path, WIDTH, HEIGHT, &sample_layers(), None, false).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"8BPS");
        assert_eq!(u16::from_be_bytes([bytes[4], bytes[5]]), 1);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn psb_header_and_layer_count_are_readable() {
        let path = temp_path("version.psb");
        save(&path, WIDTH, HEIGHT, &sample_layers(), None, true).unwrap();
        let bytes = std::fs::read(&path).unwrap();

        assert_eq!(&bytes[0..4], b"8BPS");
        assert_eq!(u16::from_be_bytes([bytes[4], bytes[5]]), 2);
        assert_eq!(bytes[6..12], [0; 6]);

        // 26 header + 4 colour mode + 4 resources + 8 layer/mask length
        // + 8 layer info length = 50.
        let layer_count = u16::from_be_bytes([bytes[50], bytes[51]]);
        assert_eq!(layer_count, 3);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn should_use_psb_switches_above_30000_px() {
        assert!(should_use_psb(31_000, 100, 1));
        assert!(should_use_psb(100, 31_000, 1));
        assert!(!should_use_psb(3_000, 2_000, 1));
        assert!(!should_use_psb(30_000, 1_000, 1));
    }

    #[test]
    fn mask_channel_inverts_exclusion() {
        let mut exclusion = Mask::new(4, 1);
        exclusion.data = vec![0, 127, 128, 255];
        assert_eq!(
            mask_channel_from_exclusion(&exclusion),
            vec![255, 255, 0, 0]
        );
    }

    #[test]
    fn packbits_round_trips_rows() {
        let rows = vec![
            Vec::new(),
            vec![7],
            vec![1, 2, 3],
            vec![9; 128],
            vec![9; 129],
            vec![9, 9],
            (0..300).map(|value| (value % 256) as u8).collect(),
            (0..300).map(|value| (value / 7 % 256) as u8).collect(),
        ];
        for row in rows {
            let packed = packbits_row(&row);
            assert_eq!(unpackbits(&packed), row);
        }
    }

    fn unpackbits(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut index = 0;
        while index < data.len() {
            let header = data[index] as i8 as i16;
            index += 1;
            if header == -128 {
                continue;
            }
            if header >= 0 {
                let count = header as usize + 1;
                out.extend_from_slice(&data[index..index + count]);
                index += count;
            } else {
                let count = (1 - header) as usize;
                out.extend(std::iter::repeat_n(data[index], count));
                index += 1;
            }
        }
        out
    }
}
