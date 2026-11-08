//! Jellyfin's image files (spec 5.9): where a Jellyfin image path lives, whether a file is a usable
//! image, and the copy Rustyfin makes of it.

use std::fs;
use std::io::{Read, Seek};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rustyfin_model::image::ImageType;

use super::paths::{PathMapping, pair_for};

/// The largest image file that is copied (spec 5.9).
const MAX_BYTES: u64 = 32 * 1024 * 1024;

/// FNV-1a 64-bit offset basis and prime.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// The extension of an image by its first bytes (spec 5.9); `None` for anything else.
pub fn signature(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if head.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("png")
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("webp")
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Some("gif")
    } else {
        None
    }
}

/// Where a Jellyfin image path lives (spec 5.9): the local file and its root folder. Lexical only:
/// no file system access. `None` for paths that are neither absolute nor placeholder-based (relative
/// paths, URLs), for any `..` component (before or after translation), and for paths that no pair
/// or placeholder covers.
pub fn resolve(
    jf_path: &str,
    program_data: &Path,
    pairs: &[PathMapping],
) -> Option<(PathBuf, PathBuf)> {
    let first = jf_path.split('/').next().unwrap_or_default();
    let placeholder = first == "%MetadataPath%" || first == "%AppDataPath%";
    if !(jf_path.starts_with('/') || placeholder) || has_parent(jf_path) {
        return None;
    }
    let (root, file) = if let Some((pair, rest)) = pair_for(jf_path, pairs) {
        let trimmed = pair.to.trim_end_matches('/');
        let root = if trimmed.is_empty() { "/" } else { trimmed };
        let file = format!("{trimmed}{rest}");
        (PathBuf::from(root), file)
    } else {
        let (root, rest) = if let Some(rest) = jf_path.strip_prefix("%MetadataPath%") {
            (program_data.join("metadata"), rest)
        } else {
            (
                program_data.join("data"),
                jf_path.strip_prefix("%AppDataPath%")?,
            )
        };
        let file = format!("{}{rest}", root.display());
        (root, file)
    };
    (!has_parent(&file)).then(|| (PathBuf::from(file), root))
}

/// Whether a path has a `..` component.
fn has_parent(path: &str) -> bool {
    path.split('/').any(|part| part == "..")
}

/// A Jellyfin image file that can be copied (spec 5.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsableImage {
    /// The file's canonical path.
    pub source: PathBuf,
    /// The copy's extension, from the file's signature.
    pub ext: &'static str,
    /// FNV-1a 64 over the file's length and modification time (see `check`).
    pub hash: u64,
    /// The file as `check` saw it; `copy` requires the same file.
    identity: FileIdentity,
}

/// What makes a file the same file (spec 5.9): its length and modification time, and on unix its
/// device and inode, so a file that is replaced or moved is another one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    len: u64,
    /// Nanoseconds since the epoch, negative before it.
    modified: i128,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
}

/// The identity of the file with metadata `meta`; `None` when it has no modification time.
fn identity(meta: &fs::Metadata) -> Option<FileIdentity> {
    let modified = match meta.modified().ok()?.duration_since(UNIX_EPOCH) {
        Ok(after) => i128::try_from(after.as_nanos()).ok()?,
        Err(before) => -i128::try_from(before.duration().as_nanos()).ok()?,
    };
    Some(FileIdentity {
        len: meta.len(),
        modified,
        #[cfg(unix)]
        dev: meta.dev(),
        #[cfg(unix)]
        ino: meta.ino(),
    })
}

/// The usable image at `file` (spec 5.9): its canonical path lies under the canonical `root`, it is
/// a regular file of at most 32 MiB, and its first 12 bytes carry a known signature. The file is
/// opened once, and its size, identity and signature are read from that handle; on Linux the handle
/// must still be at the canonical path. The hash covers the length (u64) and the modification time
/// in nanoseconds since the epoch (i128), both little endian; a changed file gets a new hash.
pub fn check(file: &Path, root: &Path) -> Option<UsableImage> {
    let source = fs::canonicalize(file).ok()?;
    let root = fs::canonicalize(root).ok()?;
    if !source.starts_with(&root) {
        return None;
    }
    // Only a regular file is opened: opening a FIFO would wait for a writer.
    if !fs::metadata(&source).ok()?.is_file() {
        return None;
    }
    let handle = fs::File::open(&source).ok()?;
    if !opened_at(&handle, &source) {
        return None;
    }
    let meta = handle.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return None;
    }
    let mut head = Vec::with_capacity(12);
    (&handle).take(12).read_to_end(&mut head).ok()?;
    let ext = signature(&head)?;
    let identity = identity(&meta)?;
    let mut seed = Vec::with_capacity(24);
    seed.extend_from_slice(&identity.len.to_le_bytes());
    seed.extend_from_slice(&identity.modified.to_le_bytes());
    Some(UsableImage {
        source,
        ext,
        hash: fnv1a(&seed),
        identity,
    })
}

/// Whether `handle` is still the file at `path`. On Linux the handle's `/proc/self/fd` link must be
/// `path`, so a link swapped in after `path` was canonicalized is refused.
#[cfg(target_os = "linux")]
fn opened_at(handle: &fs::File, path: &Path) -> bool {
    use std::os::fd::AsRawFd;
    fs::read_link(format!("/proc/self/fd/{}", handle.as_raw_fd()))
        .is_ok_and(|link| link.as_path() == path)
}

/// Elsewhere the link is not checked: `copy` still refuses a file that was swapped after `check`.
#[cfg(not(target_os = "linux"))]
fn opened_at(_handle: &fs::File, _path: &Path) -> bool {
    true
}

/// FNV-1a 64 over `bytes`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV_OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

/// The file name of a copy (spec 5.9): `jf-<Type>-<index>-<16 lowercase hex digits>.<ext>`.
pub fn copy_name(image_type: ImageType, index: usize, image: &UsableImage) -> String {
    format!(
        "jf-{}-{}-{:016x}.{}",
        image_type.as_str(),
        index,
        image.hash,
        image.ext
    )
}

/// Copies `image` to `dest`: creates the folder, writes a temporary file beside `dest`, then renames
/// it into place. The copy reads the file that `check` saw: it must have the same identity and
/// signature, else it is an error before any folder or file is created. Its bytes come from that
/// handle, from the first one, at most `MAX_BYTES`. The temporary file is removed when the copy
/// fails.
pub fn copy(image: &UsableImage, dest: &Path) -> std::io::Result<()> {
    let folder = dest
        .parent()
        .ok_or_else(|| std::io::Error::other("the copy has no folder"))?;
    let name = dest
        .file_name()
        .ok_or_else(|| std::io::Error::other("the copy has no name"))?;
    let changed = || std::io::Error::other("the image changed after it was checked");
    // Only a regular file is opened: opening a FIFO would wait for a writer.
    if !fs::metadata(&image.source)?.is_file() {
        return Err(changed());
    }
    let mut handle = fs::File::open(&image.source)?;
    let meta = handle.metadata()?;
    let mut head = Vec::with_capacity(12);
    (&handle).take(12).read_to_end(&mut head)?;
    if identity(&meta).as_ref() != Some(&image.identity) || signature(&head) != Some(image.ext) {
        return Err(changed());
    }
    handle.rewind()?;
    fs::create_dir_all(folder)?;
    let temp = folder.join(format!(".{}.tmp", name.to_string_lossy()));
    let result =
        write_copy(&mut handle, image.identity.len, &temp).and_then(|()| fs::rename(&temp, dest));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Writes the bytes of `source` from its current position, at most `MAX_BYTES`, to a new file at
/// `temp`. The copy must be `len` bytes, the checked length.
fn write_copy(source: &mut fs::File, len: u64, temp: &Path) -> std::io::Result<()> {
    let mut out = fs::File::create(temp)?;
    let copied = std::io::copy(&mut Read::by_ref(source).take(MAX_BYTES), &mut out)?;
    if copied != len {
        return Err(std::io::Error::other(
            "the image changed while it was copied",
        ));
    }
    Ok(())
}
