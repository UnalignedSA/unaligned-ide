//! Writing a private key to disk the way `ssh` wants to find it: readable by
//! its owner only, and never in place of a file that was already there.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use rand::TryRngCore as _;
use rand::rngs::OsRng;

const LONGEST_NAME: usize = 48;
const FALLBACK_NAME: &str = "key";
/// How many names are tried (`name`, `name-2`, `name-3`…) before giving up.
const MOST_ATTEMPTS: u32 = 10_000;

/// Writes `text` as a new private key file in `directory` and returns its path.
///
/// The directory is created when missing (mode 0700 on Unix) and the file gets
/// mode 0600 on Unix: `ssh` refuses a key that others can read. The file name
/// comes from `name_hint`, reduced to lowercase letters, digits, `-` and `_`.
/// An existing file is never replaced: a counter is added to the name instead.
/// The text is written under a temporary name first, so a half-written key
/// never has the final name.
pub fn write_key_file(directory: &Path, name_hint: &str, text: &str) -> io::Result<PathBuf> {
    create_private_directory(directory)?;
    let name = file_name_from(name_hint);

    let mut suffix = [0u8; 8];
    OsRng
        .try_fill_bytes(&mut suffix)
        .map_err(io::Error::other)?;
    let suffix: String = suffix.iter().map(|byte| format!("{byte:02x}")).collect();
    let temporary = directory.join(format!(".{name}.{suffix}.tmp"));

    let written =
        write_private_file(&temporary, text).and_then(|()| publish(&temporary, directory, &name));
    if written.is_err() {
        // Nothing useful can be done if this fails too; the first error is
        // the one worth reporting.
        fs::remove_file(&temporary).ok();
    }
    written
}

fn create_private_directory(directory: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(directory)
}

fn write_private_file(path: &Path, text: &str) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

/// Gives the temporary file its final name, the first free one of `name`,
/// `name-2`, `name-3`… A hard link is used where the file system has them:
/// unlike a rename, creating a link fails when the name is taken, so two
/// writers can never end up replacing each other's key.
fn publish(temporary: &Path, directory: &Path, name: &str) -> io::Result<PathBuf> {
    for attempt in 1..=MOST_ATTEMPTS {
        let candidate = if attempt == 1 {
            directory.join(name)
        } else {
            directory.join(format!("{name}-{attempt}"))
        };
        match fs::hard_link(temporary, &candidate) {
            Ok(()) => {
                fs::remove_file(temporary)?;
                return Ok(candidate);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => {
                // No hard links here (some removable or network drives):
                // check, then rename.
                if candidate.symlink_metadata().is_ok() {
                    continue;
                }
                fs::rename(temporary, &candidate)?;
                return Ok(candidate);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "every file name tried for this key is already taken",
    ))
}

/// A file name made of lowercase letters, digits, `-` and `_` only. Anything
/// else becomes a `-`; the name never starts with a `-` or a `.`, is never
/// empty, and is never one Windows reserves for a device.
fn file_name_from(hint: &str) -> String {
    let mut name = String::new();
    for character in hint.chars() {
        let character = character.to_ascii_lowercase();
        if character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_' {
            name.push(character);
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
        if name.len() >= LONGEST_NAME {
            break;
        }
    }
    let name = name.trim_matches('-');
    let reserved = matches!(name, "con" | "prn" | "aux" | "nul")
        || (name.len() == 4
            && (name.starts_with("com") || name.starts_with("lpt"))
            && name.ends_with(|last: char| last.is_ascii_digit()));
    if name.is_empty() {
        FALLBACK_NAME.to_string()
    } else if reserved {
        format!("{name}-{FALLBACK_NAME}")
    } else {
        name.to_string()
    }
}
