//! Reproducible run recipes: inputs, stack settings, tone settings, export.
//!
//! Masks are stored run-length encoded and packed as one hex string: each run
//! is a varint length followed by one value byte, all bytes hex encoded. A
//! 60 MP mask is 60 MB of raw data; as a JSON number array that becomes
//! hundreds of megabytes, and pretty-printing an array of runs makes even a
//! 1 MP mask ~1 MB. Painted masks are piecewise constant, so a single hex blob
//! keeps a set in the low megabytes and the round trip stays bit-exact.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{HdrError, Result};
use crate::mask::Mask;
use crate::pipeline::MergeOptions;
use crate::tonemap::ToneMapOptions;

/// Version of the on-disk recipe format.
pub const RECIPE_SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    pub schema: u32,
    pub frames: Vec<RecipeFrame>,
    pub stack: MergeOptions,
    pub tone: ToneMapOptions,
    #[serde(default)]
    pub export: ExportSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecipeFrame {
    pub path: PathBuf,
    pub included: bool,
    /// Stored compactly, see the module docs.
    #[serde(default, with = "compact_mask")]
    pub mask: Option<Mask>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ExportSettings {
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub max_edge: Option<u32>,
}

impl Recipe {
    pub fn new(frames: Vec<RecipeFrame>, stack: MergeOptions, tone: ToneMapOptions) -> Self {
        Self {
            schema: RECIPE_SCHEMA,
            frames,
            stack,
            tone,
            export: ExportSettings::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| HdrError::Engine(format!("encode recipe: {e}")))?;
        std::fs::write(path, text)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        let recipe: Recipe = serde_json::from_slice(&bytes)
            .map_err(|e| HdrError::Engine(format!("decode recipe {}: {e}", path.display())))?;
        if recipe.schema != RECIPE_SCHEMA {
            return Err(HdrError::Engine(format!(
                "recipe schema {} is not supported, expected schema {RECIPE_SCHEMA}",
                recipe.schema
            )));
        }
        Ok(recipe)
    }
}

/// On-disk mask shape: dimensions plus a hex-packed run list.
#[derive(Serialize, Deserialize)]
struct CompactMask {
    width: u32,
    height: u32,
    /// Hex of `(varint run length, value byte)` pairs, in pixel order.
    runs: String,
}

impl CompactMask {
    fn from_mask(mask: &Mask) -> Self {
        let mut bytes: Vec<u8> = Vec::new();
        let mut index = 0;
        while index < mask.data.len() {
            let value = mask.data[index];
            let mut count = 1u32;
            index += 1;
            while index < mask.data.len() && mask.data[index] == value {
                count += 1;
                index += 1;
            }
            push_varint(&mut bytes, count);
            bytes.push(value);
        }
        Self {
            width: mask.width,
            height: mask.height,
            runs: hex_encode(&bytes),
        }
    }

    fn into_mask(self) -> std::result::Result<Mask, String> {
        let expected = (self.width as usize)
            .checked_mul(self.height as usize)
            .ok_or("mask dimensions overflow")?;
        let bytes = hex_decode(&self.runs)?;
        let mut data: Vec<u8> = Vec::new();
        let mut cursor = 0usize;
        while cursor < bytes.len() {
            let count = read_varint(&bytes, &mut cursor)?;
            let value = *bytes
                .get(cursor)
                .ok_or("mask run is missing its value byte")?;
            cursor += 1;
            if count == 0 {
                return Err("mask run has zero length".into());
            }
            let new_len = data
                .len()
                .checked_add(count as usize)
                .ok_or("mask run lengths overflow")?;
            if new_len > expected {
                return Err(format!(
                    "mask runs cover more than the {expected} declared pixels"
                ));
            }
            data.resize(new_len, value);
        }
        if data.len() != expected {
            return Err(format!(
                "mask runs cover {} of the {expected} declared pixels",
                data.len()
            ));
        }
        Ok(Mask {
            width: self.width,
            height: self.height,
            data,
        })
    }
}

fn push_varint(out: &mut Vec<u8>, mut value: u32) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn read_varint(bytes: &[u8], cursor: &mut usize) -> std::result::Result<u32, String> {
    let mut value = 0u32;
    let mut shift = 0u32;
    loop {
        let byte = *bytes.get(*cursor).ok_or("truncated mask run")?;
        *cursor += 1;
        if shift == 28 && (byte & 0x7f) > 0x0f {
            return Err("mask run length overflows u32".into());
        }
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift > 28 {
            return Err("mask run length overflows u32".into());
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(text: &str) -> std::result::Result<Vec<u8>, String> {
    let bytes = text.as_bytes();
    if bytes.len() % 2 != 0 {
        return Err("mask run blob has an odd number of hex digits".into());
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let high = hex_nibble(pair[0])?;
        let low = hex_nibble(pair[1])?;
        out.push((high << 4) | low);
    }
    Ok(out)
}

fn hex_nibble(byte: u8) -> std::result::Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(format!("byte 0x{byte:02x} is not a hex digit")),
    }
}

mod compact_mask {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::{CompactMask, Mask};

    pub fn serialize<S>(mask: &Option<Mask>, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match mask {
            Some(mask) => CompactMask::from_mask(mask).serialize(serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> std::result::Result<Option<Mask>, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Option::<CompactMask>::deserialize(deserializer)? {
            Some(compact) => compact
                .into_mask()
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::InputEncoding;
    use crate::stack::StackMethod;
    use crate::tonemap::Curve;

    fn workdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hdr-core-recipe-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn painted_mask() -> Mask {
        let mut mask = Mask::new(1024, 1024);
        mask.paint_circle(200.0, 300.0, 90.0, 0.5, 255);
        mask.paint_circle(700.0, 600.0, 50.0, 0.0, 96);
        mask.data[123_456] = 37;
        mask
    }

    fn sample_recipe(dir: &Path, mask: Option<Mask>) -> Recipe {
        let stack = MergeOptions {
            method: StackMethod::TrimmedMax(2),
            align: true,
            reference: Some(1),
            max_dim: Some(1600),
            input_encoding: InputEncoding::Linear,
            max_shift: Some(12),
            crop_to_intersection: false,
            use_gpu: true,
        };
        let tone = ToneMapOptions {
            exposure_ev: 1.25,
            curve: Curve::Hable,
            srgb: false,
            ..Default::default()
        };
        Recipe {
            export: ExportSettings {
                path: Some(dir.join("master.exr")),
                max_edge: Some(4000),
            },
            ..Recipe::new(
                vec![
                    RecipeFrame {
                        path: dir.join("frame_0001.tif"),
                        included: true,
                        mask,
                    },
                    RecipeFrame {
                        path: dir.join("frame_0002.tif"),
                        included: false,
                        mask: None,
                    },
                ],
                stack,
                tone,
            )
        }
    }

    #[test]
    fn round_trip_keeps_settings_and_mask_pixels() {
        let dir = workdir("roundtrip");
        let mask = painted_mask();
        let recipe = sample_recipe(&dir, Some(mask.clone()));
        let path = dir.join("run.recipe.json");

        recipe.save(&path).unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        assert!(
            size < 200_000,
            "a painted 1 MP mask should stay compact, got {size} bytes"
        );

        let loaded = Recipe::load(&path).unwrap();
        assert_eq!(loaded, recipe);
        assert_eq!(loaded.stack.max_shift, Some(12));
        assert!(!loaded.stack.crop_to_intersection);
        let loaded_mask = loaded.frames[0].mask.as_ref().unwrap();
        assert_eq!(
            (loaded_mask.width, loaded_mask.height),
            (mask.width, mask.height)
        );
        assert_eq!(loaded_mask.data, mask.data);
        assert!(loaded.frames[1].mask.is_none());
    }

    #[test]
    fn load_rejects_a_wrong_schema() {
        let dir = workdir("schema");
        let mut recipe = sample_recipe(&dir, None);
        recipe.schema = RECIPE_SCHEMA + 1;
        let path = dir.join("future.recipe.json");
        recipe.save(&path).unwrap();

        let error = Recipe::load(&path).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("schema 2") && message.contains("schema 1"),
            "schema mismatch must name both versions, got: {message}"
        );
    }
}
