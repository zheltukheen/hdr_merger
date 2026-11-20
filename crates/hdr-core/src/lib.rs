//! HDR / light-paint stacking engine.
//!
//! The crate is a pure library: no UI, no global state. Everything a caller
//! needs goes through [`pipeline::merge`].

pub mod align;
pub mod color;
pub mod error;
pub mod image;
pub mod io;
pub mod mask;
pub mod pipeline;
pub mod recipe;
pub mod stack;
pub mod tonemap;

pub use align::Shift;
pub use error::{HdrError, Result};
pub use image::LinearImage;
pub use mask::Mask;
pub use pipeline::{merge, merge_paths, FrameInput, MergeOptions, MergeOutput};
pub use recipe::{ExportSettings, Recipe, RecipeFrame, RECIPE_SCHEMA};
pub use stack::StackMethod;
pub use tonemap::{Curve, ToneMapOptions};
