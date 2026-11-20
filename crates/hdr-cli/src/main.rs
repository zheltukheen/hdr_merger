use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use hdr_core::io::InputEncoding;
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
    Merge(MergeArgs),
    /// Print image facts.
    Info { file: PathBuf },
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

    /// Exposure in EV, or `auto`.
    #[arg(long, default_value = "auto")]
    exposure: String,

    /// Tone curve: none, aces, reinhard, hable.
    #[arg(long, default_value = "aces")]
    curve: String,

    /// How the inputs are interpreted: auto, srgb, linear.
    #[arg(long, default_value = "auto")]
    input: String,

    /// Directory with `<frame-stem>.mask.json` files.
    #[arg(long)]
    mask_dir: Option<PathBuf>,

    /// Target luminance for `--exposure auto`.
    #[arg(long, default_value_t = 0.55)]
    target: f32,
}

fn main() -> ExitCode {
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
        Command::Merge(args) => merge_command(args),
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
    eprintln!(
        "stacked {} frame(s) with {} -> {}x{}, exposure {:.2} EV",
        frames.len(),
        options.method.label(),
        merged.image.width,
        merged.image.height,
        tonemap.exposure_ev
    );

    hdr_core::io::save(&args.out, &merged.image, &tonemap)?;
    println!("{}", args.out.display());
    Ok(())
}

fn parse_curve(text: &str) -> Result<Curve> {
    match text.to_ascii_lowercase().as_str() {
        "none" | "clip" => Ok(Curve::None),
        "aces" => Ok(Curve::Aces),
        "reinhard" => Ok(Curve::Reinhard),
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
