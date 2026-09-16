//! Local staging for images attached to a post: validated once, copied into
//! Pluk-owned storage named by content hash, and never touched again.
//!
//! Copying — not linking or remembering the source path — is what makes a
//! staged image immutable: an edit to the original file after this call
//! never reaches the bytes a draft was approved with. Naming the copy by its
//! own hash is what dedups it: two requests for the same bytes, even from
//! different source files, land on the same staged file and are never
//! written twice.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::BrowserError;

/// How many images a single post can carry.
pub const MAX_IMAGES: usize = 4;
/// Largest a single source file may be, matching X's own per-image limit.
pub const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;
/// Longest a source path may be before it is rejected outright.
pub const MAX_SOURCE_PATH_LENGTH: usize = 4_096;

/// One image copied into staging, in the order it was given.
#[derive(Clone, Debug)]
pub struct StagedImage {
    pub content_type: &'static str,
    pub bytes: i64,
    pub sha256: String,
    pub staged_path: PathBuf,
}

/// Validate and copy `sources` (1 to [`MAX_IMAGES`] absolute local paths)
/// into `<root>/<integration_id>/`, returned in the given order.
///
/// A failure partway through — an unreadable file, one over the size cap, one
/// that is not actually a PNG or JPEG — removes only the files this call
/// itself copied; bytes staged by an earlier, unrelated call are never
/// touched.
pub fn stage_images(
    root: &Path,
    integration_id: &str,
    sources: &[String],
) -> Result<Vec<StagedImage>, BrowserError> {
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    if sources.len() > MAX_IMAGES {
        return Err(BrowserError::InvalidData(format!(
            "A post can carry at most {MAX_IMAGES} images."
        )));
    }
    let dir = root.join(integration_id);
    fs::create_dir_all(&dir).map_err(io_error)?;
    let mut staged = Vec::with_capacity(sources.len());
    let mut newly_copied = Vec::new();
    for (ordinal, source) in sources.iter().enumerate() {
        match stage_one(&dir, source, &mut newly_copied) {
            Ok(image) => staged.push(image),
            Err(error) => {
                for path in &newly_copied {
                    let _ = fs::remove_file(path);
                }
                return Err(prefix_ordinal(error, ordinal));
            }
        }
    }
    Ok(staged)
}

fn prefix_ordinal(error: BrowserError, ordinal: usize) -> BrowserError {
    match error {
        BrowserError::InvalidData(message) => {
            BrowserError::InvalidData(format!("Image {}: {message}", ordinal + 1))
        }
        other => other,
    }
}

fn stage_one(
    dir: &Path,
    source: &str,
    newly_copied: &mut Vec<PathBuf>,
) -> Result<StagedImage, BrowserError> {
    if source.is_empty() || source.len() > MAX_SOURCE_PATH_LENGTH || !Path::new(source).is_absolute() {
        return Err(BrowserError::InvalidData(
            "path must be an absolute local file path.".to_owned(),
        ));
    }
    let metadata = fs::metadata(source)
        .map_err(|error| BrowserError::InvalidData(format!("could not read \"{source}\": {error}")))?;
    if !metadata.is_file() {
        return Err(BrowserError::InvalidData(format!(
            "\"{source}\" is not a regular file."
        )));
    }
    if metadata.len() > MAX_IMAGE_BYTES {
        return Err(BrowserError::InvalidData(format!(
            "\"{source}\" is over the {} MiB limit.",
            MAX_IMAGE_BYTES / (1024 * 1024)
        )));
    }
    let bytes = fs::read(source)
        .map_err(|error| BrowserError::InvalidData(format!("could not read \"{source}\": {error}")))?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(BrowserError::InvalidData(format!(
            "\"{source}\" is over the {} MiB limit.",
            MAX_IMAGE_BYTES / (1024 * 1024)
        )));
    }
    let content_type = sniff_format(&bytes).ok_or_else(|| {
        BrowserError::InvalidData(format!("\"{source}\" is not a PNG or JPEG file."))
    })?;
    let sha256 = hex_digest(&bytes);
    let staged_path = dir.join(format!("{sha256}{}", extension_for(content_type)));
    if !staged_path.exists() {
        let temp_path = dir.join(format!(".{sha256}-{}.tmp", std::process::id()));
        fs::write(&temp_path, &bytes).map_err(io_error)?;
        fs::rename(&temp_path, &staged_path).map_err(io_error)?;
        newly_copied.push(staged_path.clone());
    }
    Ok(StagedImage {
        content_type,
        bytes: bytes.len() as i64,
        sha256,
        staged_path,
    })
}

/// Sniff PNG or JPEG off the leading bytes. Nothing else is an accepted
/// image, whatever the source file's extension claims.
fn sniff_format(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else {
        None
    }
}

fn extension_for(content_type: &str) -> &'static str {
    if content_type == "image/png" { ".png" } else { ".jpg" }
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn io_error(error: std::io::Error) -> BrowserError {
    BrowserError::InvalidData(format!("Image staging failed: {error}"))
}

/// Delete one staged file. The caller is the one who knows nothing else
/// still references its content hash; this only refuses to reach outside the
/// integration's own staging directory.
pub fn remove_staged_file(root: &Path, integration_id: &str, staged_path: &Path) {
    if staged_path.starts_with(root.join(integration_id)) {
        let _ = fs::remove_file(staged_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_MAGIC: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

    fn write_source(dir: &Path, name: &str, bytes: &[u8]) -> String {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn stages_a_valid_png_and_names_it_by_hash() {
        let source_dir = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut bytes = PNG_MAGIC.to_vec();
        bytes.extend_from_slice(b"pixels");
        let source = write_source(source_dir.path(), "a.png", &bytes);
        let staged = stage_images(root.path(), "int-1", &[source]).unwrap();
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].content_type, "image/png");
        assert!(staged[0].staged_path.exists());
        assert_eq!(staged[0].sha256, hex_digest(&bytes));
    }

    #[test]
    fn identical_bytes_reuse_the_same_staged_file() {
        let source_dir = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut bytes = PNG_MAGIC.to_vec();
        bytes.extend_from_slice(b"same pixels");
        let first = write_source(source_dir.path(), "a.png", &bytes);
        let second = write_source(source_dir.path(), "b.png", &bytes);
        let staged_first = stage_images(root.path(), "int-1", &[first]).unwrap();
        let staged_second = stage_images(root.path(), "int-1", &[second]).unwrap();
        assert_eq!(staged_first[0].staged_path, staged_second[0].staged_path);
    }

    #[test]
    fn rejects_a_file_that_is_not_actually_an_image() {
        let source_dir = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let source = write_source(source_dir.path(), "a.png", b"not an image");
        let error = stage_images(root.path(), "int-1", &[source]).unwrap_err();
        assert!(matches!(error, BrowserError::InvalidData(_)));
    }

    #[test]
    fn rejects_more_than_the_maximum() {
        let source_dir = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut bytes = PNG_MAGIC.to_vec();
        bytes.extend_from_slice(b"pixels");
        let sources: Vec<String> = (0..5)
            .map(|index| write_source(source_dir.path(), &format!("{index}.png"), &bytes))
            .collect();
        let error = stage_images(root.path(), "int-1", &sources).unwrap_err();
        assert!(matches!(error, BrowserError::InvalidData(_)));
    }

    #[test]
    fn a_failure_partway_cleans_up_only_what_this_call_copied() {
        let source_dir = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut good = PNG_MAGIC.to_vec();
        good.extend_from_slice(b"pixels");
        let first = write_source(source_dir.path(), "a.png", &good);
        let bad = write_source(source_dir.path(), "b.png", b"not an image");
        let error = stage_images(root.path(), "int-1", &[first, bad]);
        assert!(error.is_err());
        let leftovers = fs::read_dir(root.path().join("int-1"))
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn rejects_a_relative_path() {
        let root = tempfile::tempdir().unwrap();
        let error = stage_images(root.path(), "int-1", &["relative.png".to_owned()]).unwrap_err();
        assert!(matches!(error, BrowserError::InvalidData(_)));
    }
}
