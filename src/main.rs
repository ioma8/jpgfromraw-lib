use anyhow::{bail, Result};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use jpgfromraw::parser::process_file;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::{self};
use tokio::sync::Semaphore;

#[derive(Parser)]
#[command(author, version, about)]
struct Args {
    /// Input directory containing RAW files
    input_dir: PathBuf,

    /// Output directory to store extracted JPEGs
    #[arg(default_value = ".")]
    output_dir: PathBuf,

    /// How many files to process at once
    #[arg(short, long, default_value_t = 8)]
    transfers: usize,

    /// Look for this extension in addition to the default list.
    ///
    /// Default list: arw, cr2, crw, dng, erf, kdc, mef, mrw, nef, nrw, orf, pef, raf, raw, rw2,
    /// rwl, sr2, srf, srw, x3f
    #[arg(short, long)]
    extension: Option<OsString>,
}

struct ProcessingResult {
    result: Result<()>,
    path: PathBuf,
}

/// Recursively process a directory of RAW files, extracting embedded JPEGs and writing them to the
/// output directory.
///
/// This function recursively searches the input directory for RAW files with valid extensions,
/// processes each file to extract the embedded JPEG, and writes the JPEGs to the corresponding
/// location in the output directory. The directory structure relative to the input directory is
/// maintained.
async fn process_directory(
    in_dir: &Path,
    out_dir: &'static Path,
    ext: Option<OsString>,
    transfers: usize,
) -> Result<()> {
    let valid_extensions = [
        "arw", "cr2", "crw", "dng", "erf", "kdc", "mef", "mrw", "nef", "nrw", "orf", "pef", "raf",
        "raw", "rw2", "rwl", "sr2", "srf", "srw", "x3f",
    ]
    .iter()
    .flat_map(|&ext| [OsString::from(ext), OsString::from(ext.to_uppercase())])
    .chain(ext.into_iter())
    .collect::<HashSet<_>>();

    let mut entries = Vec::new();
    let mut dir_queue = vec![in_dir.to_path_buf()];

    while let Some(current_dir) = dir_queue.pop() {
        let mut read_dir = fs::read_dir(&current_dir).await?;
        let mut found_raw = false;

        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            if entry.file_type().await?.is_dir() {
                dir_queue.push(path);
            } else if path
                .extension()
                .is_some_and(|ext| valid_extensions.contains(ext))
            {
                found_raw = true;
                entries.push(path);
            }
        }

        if found_raw {
            let relative_dir = current_dir.strip_prefix(in_dir)?;
            let output_subdir = out_dir.join(relative_dir);
            fs::create_dir_all(&output_subdir).await?;
        }
    }

    let progress_bar = ProgressBar::new(entries.len().try_into()?);
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template("{pos}/{len} [{bar}] (ETA: {eta})")?
            .progress_chars("##-"),
    );

    let semaphore = Arc::new(Semaphore::new(transfers));
    let mut tasks = Vec::with_capacity(entries.len());

    for in_path in entries {
        let semaphore = semaphore.clone();
        let relative_path = in_path.strip_prefix(in_dir)?.to_path_buf();
        let progress_bar = progress_bar.clone();
        let task: tokio::task::JoinHandle<Result<ProcessingResult>> = tokio::spawn(async move {
            let permit = semaphore.acquire_owned().await?;
            let result = process_file(&in_path, out_dir, &relative_path, jpgfromraw::FindJpegType::Largest).await;
            drop(permit);
            progress_bar.inc(1);
            Ok(ProcessingResult {
                result,
                path: in_path,
            })
        });
        tasks.push(task);
    }

    let mut nr_failed = 0;
    for task in tasks {
        let pr_res = task.await??;
        if let Err(e) = pr_res.result {
            nr_failed += 1;
            let msg = format!("Error processing file {}: {:?}", pr_res.path.display(), e);
            progress_bar.println(msg);
        }
    }

    progress_bar.abandon();

    if nr_failed != 0 {
        bail!("Failed to process {} files", nr_failed);
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // We would need a copy for each task otherwise, so better just to make it &'static
    let output_dir = Box::leak(Box::new(args.output_dir));

    fs::create_dir_all(&output_dir).await?;
    process_directory(&args.input_dir, output_dir, args.extension, args.transfers).await?;

    Ok(())
}
