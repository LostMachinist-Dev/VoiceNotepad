//! Fetching a speech model on first run.
//!
//! A model is a folder of several ONNX files rather than one blob, and the
//! big one is a couple of gigabytes, so the download reports progress across
//! the whole set rather than per file. After it finishes the app never
//! touches the network again.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::config::{model_is_complete, SpeechModel};

/// The Parakeet ONNX exports.
pub const MODEL_BASE_URL: &str = "https://huggingface.co/altunenes/parakeet-rs/resolve/main/";

/// Report progress roughly this often. Frequent enough to look alive, rare
/// enough not to flood the channel on a 2.4 GB file.
const CHUNK: usize = 1024 * 1024;

/// What the download thread tells the UI.
#[derive(Clone, Debug)]
pub enum Progress {
    /// Working out how big the download is.
    Connecting,
    /// `total` is `None` until every file's size is known.
    Downloading {
        received: u64,
        total: Option<u64>,
        /// Which file is in flight, for the caption.
        file: String,
    },
    /// Finished and verified; the model folder is at this path.
    Done(PathBuf),
    /// Gave up. The string is already fit to show the user.
    Failed(String),
}

impl Progress {
    /// Fraction complete, when that is known.
    pub fn fraction(&self) -> Option<f32> {
        match self {
            Progress::Downloading {
                received,
                total: Some(total),
                ..
            } if *total > 0 => Some((*received as f64 / *total as f64) as f32),
            Progress::Done(_) => Some(1.0),
            _ => None,
        }
    }
}

pub fn file_url(model: SpeechModel, file: &str) -> String {
    format!("{MODEL_BASE_URL}{}/{file}", model.remote_dir())
}

/// Download `model` into `dir`, reporting progress through `on_progress`.
///
/// Each file lands as `.part` and is renamed once complete, so an interrupted
/// download never looks like a usable model. Files already present and the
/// right size are skipped, which makes a retry after a failure cheap.
pub fn download(
    model: SpeechModel,
    dir: &Path,
    mut on_progress: impl FnMut(Progress),
) -> Result<PathBuf, String> {
    let target = dir.join(model.dir_name());
    if model_is_complete(model, &target) {
        on_progress(Progress::Done(target.clone()));
        return Ok(target);
    }

    std::fs::create_dir_all(&target)
        .map_err(|e| format!("Could not create the model folder {}: {e}", target.display()))?;

    on_progress(Progress::Connecting);

    // Ask how big everything is first, so there is one honest progress bar
    // rather than five that each restart at zero.
    let mut sizes: Vec<(String, Option<u64>)> = Vec::new();
    for file in model.files() {
        let size = head_length(&file_url(model, file));
        sizes.push(((*file).to_string(), size));
    }
    let total: Option<u64> = if sizes.iter().all(|(_, s)| s.is_some()) {
        Some(sizes.iter().filter_map(|(_, s)| *s).sum())
    } else {
        None
    };

    let mut received: u64 = 0;
    for (file, size) in &sizes {
        let final_path = target.join(file);

        // Already there from a previous attempt.
        if std::fs::metadata(&final_path)
            .map(|m| m.is_file() && size.map(|s| m.len() == s).unwrap_or(m.len() > 1024))
            .unwrap_or(false)
        {
            received += size.unwrap_or(0);
            on_progress(Progress::Downloading {
                received,
                total,
                file: file.clone(),
            });
            continue;
        }

        received = download_one(
            model,
            file,
            &final_path,
            received,
            total,
            &mut on_progress,
        )?;
    }

    if !model_is_complete(model, &target) {
        return Err(format!(
            "The {} model downloaded but some of its files are missing. \
             Try again, or pick a different model in Settings.",
            model.label()
        ));
    }

    on_progress(Progress::Done(target.clone()));
    Ok(target)
}

/// Stream one file to disk, returning the new running total.
fn download_one(
    model: SpeechModel,
    file: &str,
    final_path: &Path,
    mut received: u64,
    total: Option<u64>,
    on_progress: &mut impl FnMut(Progress),
) -> Result<u64, String> {
    let url = file_url(model, file);
    let mut response = ureq::get(&url).call().map_err(|e| describe(&e))?;
    if response.status() != 200 {
        return Err(format!(
            "The model server answered {} for {file}.",
            response.status()
        ));
    }

    let part_path = final_path.with_extension("part");
    let mut part = std::fs::File::create(&part_path)
        .map_err(|e| format!("Could not write to {}: {e}", part_path.display()))?;

    let mut reader = response.body_mut().as_reader();
    let mut buf = vec![0u8; CHUNK];

    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                drop(part);
                let _ = std::fs::remove_file(&part_path);
                return Err(format!("The download of {file} was interrupted: {e}"));
            }
        };
        if let Err(e) = part.write_all(&buf[..n]) {
            drop(part);
            let _ = std::fs::remove_file(&part_path);
            return Err(format!("Could not write {file} to disk: {e}"));
        }
        received += n as u64;
        on_progress(Progress::Downloading {
            received,
            total,
            file: file.to_string(),
        });
    }

    part.flush()
        .and_then(|_| part.sync_all())
        .map_err(|e| format!("Could not finish writing {file}: {e}"))?;
    drop(part);

    std::fs::rename(&part_path, final_path)
        .map_err(|e| format!("Could not move {file} into place: {e}"))?;
    Ok(received)
}

/// Content length of a URL, if the server will say.
fn head_length(url: &str) -> Option<u64> {
    let response = ureq::head(url).call().ok()?;
    response
        .headers()
        .get("content-length")?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

/// Turn a transport error into something worth showing a person.
fn describe(err: &ureq::Error) -> String {
    let detail = err.to_string();
    if detail.contains("dns") || detail.contains("resolve") {
        format!("Could not reach huggingface.co ({detail}). Check the internet connection.")
    } else {
        format!("Could not download the model: {detail}")
    }
}

/// Human-readable byte count for the progress dialog.
pub fn format_bytes(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.0} MB", b / MB)
    } else {
        format!("{:.0} KB", b / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_point_at_the_parakeet_exports() {
        let url = file_url(SpeechModel::Nemotron, "encoder.onnx");
        assert_eq!(
            url,
            "https://huggingface.co/altunenes/parakeet-rs/resolve/main/\
             nemotron-speech-streaming-en-0.6b/encoder.onnx"
        );
        for model in SpeechModel::ALL {
            for file in model.files() {
                let url = file_url(model, file);
                assert!(url.starts_with("https://"));
                assert!(url.ends_with(file));
            }
        }
    }

    #[test]
    fn progress_fraction_needs_a_known_total() {
        let downloading = |received, total| Progress::Downloading {
            received,
            total,
            file: "encoder.onnx".into(),
        };
        assert_eq!(downloading(50, Some(100)).fraction(), Some(0.5));
        assert_eq!(downloading(50, None).fraction(), None);
        assert_eq!(downloading(0, Some(0)).fraction(), None);
        assert_eq!(Progress::Connecting.fraction(), None);
        assert_eq!(Progress::Done(PathBuf::from("x")).fraction(), Some(1.0));
    }

    #[test]
    fn byte_counts_read_naturally() {
        assert_eq!(format_bytes(466 * 1024 * 1024), "466 MB");
        assert_eq!(format_bytes(2516582400), "2.34 GB");
        assert_eq!(format_bytes(2048), "2 KB");
    }

    #[test]
    fn the_biggest_file_is_downloaded_last() {
        // So that a failure part way through wastes the least time, and a
        // retry skips what already arrived.
        let files = SpeechModel::Nemotron.files();
        assert_eq!(*files.last().unwrap(), "encoder.onnx.data");
    }
}
