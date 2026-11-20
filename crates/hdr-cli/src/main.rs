use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use hdr_core::io::InputEncoding;
use hdr_core::recipe::{ExportSettings, Recipe, RecipeFrame};
use hdr_core::{
    merge, merge_paths, Curve, FrameInput, Mask, MergeOptions, StackMethod, ToneMapOptions,
};

#[derive(Parser)]
#[command(name = "hdr-merger", version, about = "HDR / light-paint stacker")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Merge frames into one HDR image.
    Merge(Box<MergeArgs>),
    /// Print image facts.
    Info { file: PathBuf },
    /// Write every frame as a PSD/PSB layer with its mask.
    Layers(LayersArgs),
    /// Group a folder into stacks and merge each one.
    Batch(BatchArgs),
    /// List stack methods.
    Methods,
}

#[derive(Parser)]
struct MergeArgs {
    /// Input frames (tiff, png, jpeg, exr).
    inputs: Vec<PathBuf>,

    /// Output file. Extension picks the format: exr, tif, png, jpg.
    #[arg(short, long)]
    out: PathBuf,

    /// Stack method: median, mean, sum, max, p90, percentile:75, trimmed:3.
    #[arg(short, long, default_value = "median")]
    method: String,

    /// Disable translation alignment.
    #[arg(long)]
    no_align: bool,

    /// Frame index used as the alignment anchor.
    #[arg(long)]
    reference: Option<usize>,

    /// Longest edge of the result (proxy / preview).
    #[arg(long)]
    max_dim: Option<u32>,

    /// Refuse a frame whose estimated shift exceeds this many pixels (|dx|+|dy|).
    #[arg(long)]
    max_shift: Option<u32>,

    /// Keep the full frame instead of cropping to the area valid in every frame.
    #[arg(long)]
    no_crop: bool,

    /// Write a recipe JSON describing this run.
    #[arg(long)]
    save_recipe: Option<PathBuf>,

    /// Exposure in EV, or `auto`.
    #[arg(long, default_value = "auto")]
    exposure: String,

    /// Tone curve: none, aces, reinhard, hable.
    #[arg(long, default_value = "aces")]
    curve: String,

    /// How the inputs are interpreted: auto, srgb, linear.
    #[arg(long, default_value = "auto")]
    input: String,

    /// Worker threads for the CPU stacker (default: all cores).
    #[arg(long)]
    threads: Option<usize>,

    /// Focus stacking feather radius in pixels (0 = hard pick).
    #[arg(long, default_value_t = 8)]
    focus_feather: usize,

    /// Dark frame to subtract from every frame.
    #[arg(long)]
    dark: Option<PathBuf>,

    /// Save the focus depth map next to the output (.png).
    #[arg(long)]
    depth: bool,

    /// Use the wgpu compute stacker instead of the CPU one (slower on the
    /// sets measured so far, kept for experimentation).
    #[arg(long)]
    gpu: bool,

    /// Directory with `<frame-stem>.mask.json` files.
    #[arg(long)]
    mask_dir: Option<PathBuf>,

    /// Target luminance for `--exposure auto`.
    #[arg(long, default_value_t = 0.55)]
    target: f32,
}

#[derive(Parser)]
struct BatchArgs {
    /// Folder with the whole shoot.
    folder: PathBuf,
    /// Output folder for the merged files.
    #[arg(short, long)]
    out: PathBuf,
    /// Stack method: median, mean, sum, max, p90, focus ...
    #[arg(short, long, default_value = "median")]
    method: String,
    /// Output extension: jpg, tif, exr, psd.
    #[arg(long, default_value = "jpg")]
    format: String,
    /// Longest edge of every result.
    #[arg(long)]
    max_dim: Option<u32>,
    /// Exposure in EV, or `auto`.
    #[arg(long, default_value = "auto")]
    exposure: String,
    /// Tone curve: none, aces, reinhard, reinhard02, hable.
    #[arg(long, default_value = "aces")]
    curve: String,
    /// Focus stacking feather radius in pixels (0 = hard pick).
    #[arg(long, default_value_t = 8)]
    focus_feather: usize,
}

#[derive(Parser)]
struct LayersArgs {
    /// Input frames (tiff, png, jpeg, exr).
    inputs: Vec<PathBuf>,
    /// Output .psd or .psb.
    #[arg(short, long)]
    out: PathBuf,
    /// Exposure in EV, or `auto`.
    #[arg(long, default_value = "auto")]
    exposure: String,
    /// Tone curve: none, aces, reinhard, reinhard02, hable.
    #[arg(long, default_value = "aces")]
    curve: String,
    /// Longest edge of every layer.
    #[arg(long)]
    max_dim: Option<u32>,
    /// Target luminance for `--exposure auto`.
    #[arg(long, default_value_t = 0.55)]
    target: f32,
    /// Directory with `<frame-stem>.mask.json` files.
    #[arg(long)]
    mask_dir: Option<PathBuf>,
}

fn main() -> ExitCode {
    // clap's `--version` cannot include a runtime constant, and the recipe
    // schema is part of the contract (v1.0), so answer it explicitly.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 1 && (args[0] == "--version" || args[0] == "-V") {
        println!("hdr-merger {}", env!("CARGO_PKG_VERSION"));
        println!("recipe schema {}", hdr_core::recipe::RECIPE_SCHEMA);
        return ExitCode::SUCCESS;
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Merge(args) => merge_command(*args),
        Command::Info { file } => {
            let image = hdr_core::io::load(&file, InputEncoding::Auto)?;
            println!(
                "{}: {}x{}, max {:.4}, mean luminance {:.4}",
                file.display(),
                image.width,
                image.height,
                image.max_channel(),
                image.mean_luminance()
            );
            Ok(())
        }
        Command::Layers(args) => layers_command(args),
        Command::Batch(args) => batch_command(args),
        Command::Methods => {
            for method in [
                "median",
                "mean",
                "sum",
                "max",
                "p90",
                "percentile:75",
                "trimmed:3",
            ] {
                println!("{method}");
            }
            Ok(())
        }
    }
}

fn merge_command(args: MergeArgs) -> Result<()> {
    let options = MergeOptions {
        method: StackMethod::parse(&args.method)?,
        align: !args.no_align,
        reference: args.reference,
        max_dim: args.max_dim,
        input_encoding: parse_input_encoding(&args.input)?,
        use_gpu: args.gpu,
        threads: args.threads,
        focus_feather: args.focus_feather,
        dark_frame: args.dark.clone(),
        max_shift: args.max_shift,
        crop_to_intersection: !args.no_crop,
    };
    let curve = parse_curve(&args.curve)?;

    let frames: Vec<FrameInput> = args
        .inputs
        .iter()
        .map(|path| {
            let mask = load_mask(&args.mask_dir, path).ok().flatten();
            FrameInput {
                path: path.clone(),
                mask,
            }
        })
        .collect();

    let is_exr = args
        .out
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("exr"))
        .unwrap_or(false);

    // EXR is written straight from the stacker: no 60 MP master in RAM.
    if is_exr {
        let timings = hdr_core::pipeline::merge_to_exr(&frames, &options, &args.out, &progress)?;
        eprintln!(
            "stacked {} frame(s) with {} -> {} (streamed)",
            frames.len(),
            options.method.label(),
            args.out.display()
        );
        eprintln!(
            "time: decode {} ms, cache {} ms, stack {} ms, total {} ms ({:.0}% before the stack, CPU stacker)",
            timings.decode_ms,
            timings.cache_ms,
            timings.stack_ms,
            timings.total_ms,
            timings.decode_share() * 100.0
        );
        println!("{}", args.out.display());
        return Ok(());
    }

    let merged = if frames.iter().any(|f| f.mask.is_some()) {
        merge(&frames, &options, &progress)?
    } else {
        let paths: Vec<PathBuf> = frames.iter().map(|f| f.path.clone()).collect();
        merge_paths(&paths, &options, &progress)?
    };

    let mut tonemap = ToneMapOptions {
        exposure_ev: 0.0,
        curve,
        srgb: true,
        ..Default::default()
    };
    tonemap.exposure_ev = match args.exposure.as_str() {
        "auto" => hdr_core::tonemap::auto_exposure_ev(&merged.image, args.target),
        other => other
            .parse::<f32>()
            .context("--exposure must be a number or `auto`")?,
    };

    for warning in &merged.warnings {
        eprintln!("warning: {warning}");
    }
    let timings = merged.timings;
    eprintln!(
        "stacked {} frame(s) with {} -> {}x{}, exposure {:.2} EV",
        frames.len(),
        options.method.label(),
        merged.image.width,
        merged.image.height,
        tonemap.exposure_ev
    );
    eprintln!(
        "time: decode {} ms, cache {} ms, stack {} ms, total {} ms ({:.0}% before the stack, {} stacker)",
        timings.decode_ms,
        timings.cache_ms,
        timings.stack_ms,
        timings.total_ms,
        timings.decode_share() * 100.0,
        if timings.gpu { "GPU" } else { "CPU" }
    );

    hdr_core::io::save(&args.out, &merged.image, &tonemap)?;

    if args.depth {
        if let Some(depth) = &merged.depth {
            let depth_path = args.out.with_extension("depth.png");
            let linear = hdr_core::ToneMapOptions {
                exposure_ev: 0.0,
                curve: hdr_core::Curve::None,
                srgb: false,
                ..Default::default()
            };
            hdr_core::io::save(&depth_path, depth, &linear)?;
            eprintln!("depth map -> {}", depth_path.display());
        } else {
            eprintln!("--depth only applies to --method focus");
        }
    }

    if let Some(path) = &args.save_recipe {
        let recipe = Recipe {
            export: ExportSettings {
                path: Some(args.out.clone()),
                max_edge: None,
            },
            ..Recipe::new(
                frames
                    .into_iter()
                    .map(|frame| RecipeFrame {
                        path: frame.path,
                        included: true,
                        mask: frame.mask,
                    })
                    .collect(),
                options,
                tonemap,
            )
        };
        recipe.save(path)?;
        eprintln!("recipe: {}", path.display());
    }

    println!("{}", args.out.display());
    Ok(())
}

fn batch_command(args: BatchArgs) -> Result<()> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&args.folder)
        .with_context(|| format!("read {}", args.folder.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            matches!(
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext.to_ascii_lowercase())
                    .as_deref(),
                Some("tif") | Some("tiff") | Some("png") | Some("jpg") | Some("jpeg") | Some("exr")
            )
        })
        .collect();
    paths.sort();
    if paths.is_empty() {
        anyhow::bail!("no supported images in {}", args.folder.display());
    }
    std::fs::create_dir_all(&args.out)?;

    let groups = hdr_core::grouping::group_paths(
        &paths,
        &hdr_core::grouping::GroupOptions::default(),
        &progress,
    )?;
    eprintln!("{} frame(s) in {} stack(s)", paths.len(), groups.len());

    let options = MergeOptions {
        method: StackMethod::parse(&args.method)?,
        align: true,
        reference: None,
        max_dim: args.max_dim,
        input_encoding: InputEncoding::Auto,
        max_shift: None,
        crop_to_intersection: true,
        threads: None,
        focus_feather: args.focus_feather,
        dark_frame: None,
        use_gpu: false,
    };
    let curve = parse_curve(&args.curve)?;
    let folder_name = args
        .folder
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "shoot".to_owned());

    for (index, group) in groups.iter().enumerate() {
        let frames: Vec<FrameInput> = group
            .frames
            .iter()
            .map(|frame| FrameInput::new(paths[*frame].clone()))
            .collect();
        let output = args
            .out
            .join(format!("{folder_name}_{:02}.{}", index + 1, args.format));
        eprintln!(
            "stack {}/{}: {} frame(s), {}",
            index + 1,
            groups.len(),
            frames.len(),
            group.reason
        );
        if args.format.eq_ignore_ascii_case("psd") || args.format.eq_ignore_ascii_case("psb") {
            let exposure = exposure_for(&args.exposure, &frames, 0.55)?;
            let tonemap = hdr_core::ToneMapOptions {
                exposure_ev: exposure,
                curve,
                srgb: true,
                ..Default::default()
            };
            hdr_core::layers::export(&output, &frames, &tonemap, args.max_dim, &progress)?;
            continue;
        }
        let merged = merge(&frames, &options, &progress)?;
        let mut tonemap = hdr_core::ToneMapOptions {
            exposure_ev: 0.0,
            curve,
            srgb: true,
            ..Default::default()
        };
        tonemap.exposure_ev = match args.exposure.as_str() {
            "auto" => hdr_core::tonemap::auto_exposure_ev(&merged.image, 0.55),
            other => other
                .parse::<f32>()
                .context("--exposure must be a number or `auto`")?,
        };
        hdr_core::io::save(&output, &merged.image, &tonemap)?;
    }
    println!("{}", args.out.display());
    Ok(())
}

fn exposure_for(exposure: &str, frames: &[FrameInput], target: f32) -> Result<f32> {
    match exposure {
        "auto" => Ok(hdr_core::layers::auto_exposure(frames, target)?),
        other => other
            .parse::<f32>()
            .context("--exposure must be a number or `auto`"),
    }
}

fn layers_command(args: LayersArgs) -> Result<()> {
    let frames: Vec<FrameInput> = args
        .inputs
        .iter()
        .map(|path| FrameInput {
            path: path.clone(),
            mask: load_mask(&args.mask_dir, path).ok().flatten(),
        })
        .collect();
    let exposure_ev = match args.exposure.as_str() {
        "auto" => hdr_core::layers::auto_exposure(&frames, args.target)?,
        other => other
            .parse::<f32>()
            .context("--exposure must be a number or `auto`")?,
    };
    let tonemap = hdr_core::ToneMapOptions {
        exposure_ev,
        curve: parse_curve(&args.curve)?,
        srgb: true,
        ..Default::default()
    };
    hdr_core::layers::export(&args.out, &frames, &tonemap, args.max_dim, &progress)?;
    eprintln!(
        "wrote {} layer(s) at {:.2} EV -> {}",
        frames.len(),
        exposure_ev,
        args.out.display()
    );
    println!("{}", args.out.display());
    Ok(())
}

fn parse_curve(text: &str) -> Result<Curve> {
    match text.to_ascii_lowercase().as_str() {
        "none" | "clip" => Ok(Curve::None),
        "aces" => Ok(Curve::Aces),
        "reinhard" => Ok(Curve::Reinhard),
        "reinhard02" | "reinhard2" => Ok(Curve::Reinhard02),
        "hable" | "filmic" => Ok(Curve::Hable),
        other => anyhow::bail!("unknown curve `{other}`"),
    }
}

fn parse_input_encoding(text: &str) -> Result<InputEncoding> {
    match text.to_ascii_lowercase().as_str() {
        "auto" => Ok(InputEncoding::Auto),
        "srgb" => Ok(InputEncoding::Srgb),
        "linear" => Ok(InputEncoding::Linear),
        other => anyhow::bail!("unknown input encoding `{other}`"),
    }
}

fn load_mask(dir: &Option<PathBuf>, frame: &std::path::Path) -> Result<Option<Mask>> {
    let Some(dir) = dir else { return Ok(None) };
    let stem = frame.file_stem().unwrap_or_default().to_string_lossy();
    let path = dir.join(format!("{stem}.mask.json"));
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)?;
    Ok(Some(serde_json::from_str(&text)?))
}

fn progress(done: u32, total: u32) {
    if total == 0 {
        return;
    }
    let percent = (done as f32 / total as f32 * 100.0).min(100.0);
    eprint!("\r{done}/{total} ({percent:.0}%)");
    if done >= total {
        eprintln!();
    }
}
