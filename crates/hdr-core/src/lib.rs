//! HDR / light-paint stacking engine.
//!
//! The crate is a pure library: no UI, no global state. Everything a caller
//! needs goes through [`pipeline::merge`].

pub mod align;
pub mod color;
pub mod deghost;
pub mod error;
pub mod exr_stream;
pub mod focus;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod grouping;
pub mod image;
pub mod io;
pub mod layers;
pub mod mask;
pub mod pipeline;
pub mod psd;
pub mod recipe;
pub mod stack;
pub mod tonemap;

pub use align::Shift;
pub use deghost::{detect_movers, DeghostOptions, DeghostResult};
pub use error::{HdrError, Result};
pub use exr_stream::ExrStreamWriter;
pub use grouping::{group_paths, Group, GroupOptions};
pub use image::LinearImage;
pub use mask::Mask;
pub use pipeline::{merge, merge_paths, FrameInput, MergeOptions, MergeOutput, MergeTimings};
pub use psd::PsdLayer;
pub use recipe::{ExportSettings, Recipe, RecipeFrame, RECIPE_SCHEMA};
pub use stack::StackMethod;
pub use tonemap::{Curve, ToneMapOptions};
