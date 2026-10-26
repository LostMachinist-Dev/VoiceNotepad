//! Fetching the Whisper model on first run.
//!
//! The installer stays small by downloading the model once, into the user's
//! app-data folder, with progress reported back to the UI. After that the app
//! never touches the network again -- transcription is entirely local.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::config::{is_plausible_model, ModelSize};

/// Official whisper.cpp ggml models.
pub const MODEL_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/";

/// How often to report progress. Frequent enough to look live, rare enough
/// not to flood the channel.
const CHUNK: usize = 128 * 1024;

/// What the download thread tells the UI.
#[derive(Clone, Debug)]
pub enum Progress {
    /// Contacting Hugging Face; no byte counts yet.
    Connecting,
    /// `total` is `None` if the server did not send a content length.
    Downloading { received: u64, total: Option<u64> },
    /// Finished and verified; the model is at this path.
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
            } if *total > 0 => Some((*received as f64 / *total as f64) as f32),
            Progress::Done(_) => Some(1.0),
            _ => None,
        }
    }
}

pub fn model_url(size: ModelSize) -> String {
    format!("{MODEL_BASE_URL}{}", size.file_name())
}

/// Download `size` into `dir`, reporting progress through `on_progress`.
///
/// Downloads to a `.part` file and renames on success, so an interrupted
/// download never looks like a usable model. Returns the final path.
pub fn download(
    size: ModelSize,
    dir: &Path,
    mut on_progress: impl FnMut(Progress),
) -> Result<PathBuf, String> {
    let final_path = dir.join(size.file_name());
    if is_plausible_model(&final_path) {
        on_progress(Progress::Done(final_path.clone()));
        return Ok(final_path);
    }

    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Could not create the model folder {}: {e}", dir.display()))?;

    on_progress(Progress::Connecting);

    let url = model_url(size);
    let mut response = ureq::get(&url)
        .call()
        .map_err(|e| describe_network_error(&e))?;

    if response.status() != 200 {
        return Err(format!(
            "The model server answered {} for {}.",
            response.status(),
            size.file_name()
        ));
    }

    let total = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    let part_path = final_path.with_extension("part");
    let mut part = std::fs::File::create(&part_path)
        .map_err(|e| format!("Could not write to {}: {e}", part_path.display()))?;

    let mut reader = response.body_mut().as_reader();
    let mut buf = vec![0u8; CHUNK];
    let mut received: u64 = 0;

    on_progress(Progress::Downloading { received, total });

    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                drop(part);
                let _ = std::fs::remove_file(&part_path);
                return Err(format!("The download was interrupted: {e}"));
            }
        };
        if let Err(e) = part.write_all(&buf[..n]) {
            drop(part);
            let _ = std::fs::remove_file(&part_path);
            return Err(format!("Could not write the model to disk: {e}"));
        }
        received += n as u64;
        on_progress(Progress::Downloading { received, total });
    }

    part.flush()
        .and_then(|_| part.sync_all())
        .map_err(|e| format!("Could not finish writing the model: {e}"))?;
    drop(part);

    // A truncated download is worse than none: it would fail later, inside
    // whisper, where the cause would not be obvious.
    if let Some(total) = total {
        if received != total {
            let _ = std::fs::remove_file(&part_path);
            return Err(format!(
                "The download stopped early ({} of {} bytes). Check the connection and try again.",
                received, total
            ));
        }
    }
    if received < 10 * 1024 * 1024 {
        let _ = std::fs::remove_file(&part_path);
        return Err("The downloaded file is too small to be a Whisper model.".to_string());
    }

    std::fs::rename(&part_path, &final_path).map_err(|e| {
        format!(
            "Could not move the model into place at {}: {e}",
            final_path.display()
        )
    })?;

    on_progress(Progress::Done(final_path.clone()));
    Ok(final_path)
}

/// Turn a transport error into something worth showing a person.
fn describe_network_error(err: &ureq::Error) -> String {
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
    fn urls_point_at_the_official_ggml_models() {
        assert_eq!(
            model_url(ModelSize::Small),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin"
        );
        for size in ModelSize::ALL {
            let url = model_url(size);
            assert!(url.starts_with("https://"));
            assert!(url.ends_with(size.file_name()));
        }
    }

    #[test]
    fn progress_fraction_needs_a_known_total() {
        assert_eq!(
            Progress::Downloading { received: 50, total: Some(100) }.fraction(),
            Some(0.5)
        );
        assert_eq!(
            Progress::Downloading { received: 50, total: None }.fraction(),
            None
        );
        assert_eq!(Progress::Connecting.fraction(), None);
        assert_eq!(
            Progress::Done(PathBuf::from("x")).fraction(),
            Some(1.0)
        );
    }

    #[test]
    fn progress_fraction_survives_a_zero_total() {
        assert_eq!(
            Progress::Downloading { received: 0, total: Some(0) }.fraction(),
            None
        );
    }

    #[test]
    fn byte_counts_read_naturally() {
        assert_eq!(format_bytes(466 * 1024 * 1024), "466 MB");
        assert_eq!(format_bytes(1536 * 1024 * 1024), "1.50 GB");
        assert_eq!(format_bytes(2048), "2 KB");
    }
}
