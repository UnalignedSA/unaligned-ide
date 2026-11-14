//! The `openssh-key-v1` container: what lies inside
//! `-----BEGIN OPENSSH PRIVATE KEY-----`. Read to describe a key that is kept
//! as it is, written to hold a key converted from another format.
//!
//! Layout (PROTOCOL.key in the OpenSSH sources):
//!
//! ```text
//! "openssh-key-v1\0"
//! string  cipher name          "none" or "aes256-ctr"…
//! string  kdf name             "none" or "bcrypt"
//! string  kdf options          empty, or: string salt, uint32 rounds
//! uint32  number of keys       always 1
//! string  public key blob
//! string  private section      encrypted with the cipher; holds:
//!     uint32 check, uint32 check   the same random number twice
//!     string key type, the numbers of the key, string comment
//!     padding bytes 1, 2, 3… up to the cipher block size
//! ```

use aes::cipher::{KeyIvInit as _, StreamCipher as _};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rand::TryRngCore as _;
use rand::rngs::OsRng;
use zeroize::Zeroizing;

use crate::KeyError;
use crate::wire::{Reader, describe_public_blob, put_string, put_u32};

const MAGIC: &[u8] = b"openssh-key-v1\0";
const LINE_WIDTH: usize = 70;

/// What `ssh-keygen` uses for a new key (OpenSSH 9.4 and later).
pub(crate) const BCRYPT_ROUNDS: u32 = 24;
const SALT_LENGTH: usize = 16;
const AES_BLOCK: usize = 16;
const AES256_KEY: usize = 32;
/// Padding unit of an unencrypted private section.
const PLAIN_BLOCK: usize = 8;

type Aes256Ctr = ctr::Ctr128BE<aes::Aes256>;

/// What an OpenSSH private key file says without its passphrase.
pub(crate) struct Container {
    pub(crate) encrypted: bool,
    pub(crate) key_type: String,
    pub(crate) algorithm: String,
    /// Only readable when the key is not encrypted: the comment lives in the
    /// private section.
    pub(crate) comment: Option<String>,
    pub(crate) public_blob: Vec<u8>,
}

fn damaged() -> KeyError {
    KeyError::Damaged(
        "This OpenSSH key is damaged, part of it is missing or was changed: copy the whole key again.".to_string(),
    )
}

/// Block size and, for the ciphers that authenticate, the length of the tag
/// that follows the private section. `None` for a cipher this code does not
/// know: its key is kept without these checks.
fn cipher_shape(name: &str) -> Option<(usize, usize)> {
    match name {
        "aes128-ctr" | "aes192-ctr" | "aes256-ctr" | "aes128-cbc" | "aes192-cbc" | "aes256-cbc" => {
            Some((16, 0))
        }
        "aes128-gcm@openssh.com" | "aes256-gcm@openssh.com" => Some((16, 16)),
        "chacha20-poly1305@openssh.com" => Some((8, 16)),
        "3des-cbc" => Some((8, 0)),
        _ => None,
    }
}

pub(crate) fn parse(data: &[u8]) -> Result<Container, KeyError> {
    let body = data.strip_prefix(MAGIC).ok_or_else(damaged)?;
    let mut reader = Reader::new(body);
    let cipher = reader.text().ok_or_else(damaged)?;
    let kdf = reader.text().ok_or_else(damaged)?;
    let kdf_options = reader.string().ok_or_else(damaged)?;
    let key_count = reader.u32().ok_or_else(damaged)?;
    if key_count != 1 {
        return Err(KeyError::Unsupported(format!(
            "This OpenSSH file holds {key_count} keys, and only files with one key can be used."
        )));
    }
    let public_blob = reader.string().ok_or_else(damaged)?;
    let private_section = reader.string().ok_or_else(damaged)?;
    let (key_type, algorithm) = describe_public_blob(public_blob).ok_or_else(damaged)?;

    let encrypted = cipher != "none";
    let comment = if encrypted {
        match kdf {
            "bcrypt" => {
                let mut options = Reader::new(kdf_options);
                let salt = options.string().ok_or_else(damaged)?;
                let rounds = options.u32().ok_or_else(damaged)?;
                if salt.is_empty() || rounds == 0 || !options.is_empty() {
                    return Err(damaged());
                }
            }
            // A cipher needs a key, and a key needs a derivation.
            "none" => return Err(damaged()),
            _ => {}
        }
        if private_section.is_empty() {
            return Err(damaged());
        }
        if let Some((block, tag)) = cipher_shape(cipher)
            && (private_section.len() % block != 0 || reader.rest().len() != tag)
        {
            return Err(damaged());
        }
        None
    } else {
        if kdf != "none" || !kdf_options.is_empty() || !reader.is_empty() {
            return Err(damaged());
        }
        read_plain_private_section(private_section, key_type)?
    };

    Ok(Container {
        encrypted,
        key_type: key_type.to_string(),
        algorithm,
        comment,
        public_blob: public_blob.to_vec(),
    })
}

/// Checks an unencrypted private section and returns its comment. For a key
/// type whose private numbers this code cannot step over, the section is
/// accepted as it is and no comment is returned.
fn read_plain_private_section(section: &[u8], key_type: &str) -> Result<Option<String>, KeyError> {
    let mut reader = Reader::new(section);
    let check = reader.u32().ok_or_else(damaged)?;
    if reader.u32() != Some(check) || reader.text() != Some(key_type) {
        return Err(damaged());
    }
    // How many length-prefixed values, then (for hardware-backed keys) a
    // flags byte and more values, come before the comment.
    let (values, security_key_values) = match key_type {
        "ssh-ed25519" => (2, 0),
        "ssh-rsa" => (6, 0),
        "ssh-dss" => (5, 0),
        "ecdsa-sha2-nistp256" | "ecdsa-sha2-nistp384" | "ecdsa-sha2-nistp521" => (3, 0),
        "sk-ssh-ed25519@openssh.com" => (2, 2),
        "sk-ecdsa-sha2-nistp256@openssh.com" => (3, 2),
        _ => return Ok(None),
    };
    for _ in 0..values {
        reader.string().ok_or_else(damaged)?;
    }
    if security_key_values > 0 {
        reader.bytes(1).ok_or_else(damaged)?;
        for _ in 0..security_key_values {
            reader.string().ok_or_else(damaged)?;
        }
    }
    let comment = reader.string().ok_or_else(damaged)?;
    let padding = reader.rest();
    let padding_is_right = padding.len() < 256
        && padding
            .iter()
            .enumerate()
            .all(|(index, byte)| usize::from(*byte) == index + 1);
    if !padding_is_right {
        return Err(damaged());
    }
    let comment = String::from_utf8_lossy(comment);
    Ok((!comment.is_empty()).then(|| comment.into_owned()))
}

/// `ssh-ed25519 AAAA… comment`: the line for a server's `authorized_keys`.
pub(crate) fn public_key_line(key_type: &str, public_blob: &[u8], comment: Option<&str>) -> String {
    let mut line = format!("{key_type} {}", STANDARD.encode(public_blob));
    if let Some(comment) = comment.filter(|comment| !comment.is_empty()) {
        line.push(' ');
        line.push_str(comment);
    }
    line
}

fn random_bytes<const N: usize>() -> Result<[u8; N], KeyError> {
    let mut bytes = [0u8; N];
    OsRng.try_fill_bytes(&mut bytes).map_err(|_| {
        KeyError::Unsupported(
            "This computer could not provide the random numbers needed to protect the key: try again.".to_string(),
        )
    })?;
    Ok(bytes)
}

/// Writes an OpenSSH private key file.
///
/// `private_key` is the middle of the private section: the key type and the
/// numbers of the key, already in OpenSSH's order. With a passphrase the
/// section is encrypted the way `ssh-keygen` does it (bcrypt key derivation,
/// AES-256 in counter mode); without one it is left in the clear.
pub(crate) fn write(
    public_blob: &[u8],
    private_key: &[u8],
    comment: &str,
    passphrase: Option<&[u8]>,
    rounds: u32,
) -> Result<String, KeyError> {
    let block = if passphrase.is_some() {
        AES_BLOCK
    } else {
        PLAIN_BLOCK
    };

    // Every buffer below is given its final size up front: a buffer that
    // grows leaves its old content behind, out of reach of the wipe.
    let unpadded = 8 + private_key.len() + 4 + comment.len();
    let padding = (block - unpadded % block) % block;
    let mut section = Zeroizing::new(Vec::with_capacity(unpadded + padding));
    let check = random_bytes::<4>()?;
    section.extend_from_slice(&check);
    section.extend_from_slice(&check);
    section.extend_from_slice(private_key);
    put_string(&mut section, comment.as_bytes());
    section.extend((1..=padding).map(|index| index as u8));

    let mut kdf_options = Vec::new();
    let (cipher_name, kdf_name) = match passphrase {
        Some(passphrase) => {
            let salt = random_bytes::<SALT_LENGTH>()?;
            let mut key_and_iv = Zeroizing::new([0u8; AES256_KEY + AES_BLOCK]);
            bcrypt_pbkdf::bcrypt_pbkdf(passphrase, &salt, rounds, key_and_iv.as_mut_slice())
                .map_err(|_| {
                    KeyError::Unsupported(
                        "This passphrase cannot be used to protect an OpenSSH key.".to_string(),
                    )
                })?;
            let (key, iv) = key_and_iv.split_at(AES256_KEY);
            let mut cipher = Aes256Ctr::new_from_slices(key, iv).map_err(|_| damaged())?;
            cipher.apply_keystream(&mut section);
            put_string(&mut kdf_options, &salt);
            put_u32(&mut kdf_options, rounds);
            ("aes256-ctr", "bcrypt")
        }
        None => ("none", "none"),
    };

    let mut container = Zeroizing::new(Vec::with_capacity(
        MAGIC.len()
            + 4
            + cipher_name.len()
            + 4
            + kdf_name.len()
            + 4
            + kdf_options.len()
            + 4
            + 4
            + public_blob.len()
            + 4
            + section.len(),
    ));
    container.extend_from_slice(MAGIC);
    put_string(&mut container, cipher_name.as_bytes());
    put_string(&mut container, kdf_name.as_bytes());
    put_string(&mut container, &kdf_options);
    put_u32(&mut container, 1);
    put_string(&mut container, public_blob);
    put_string(&mut container, &section);

    let mut encoded = Zeroizing::new(String::with_capacity(container.len().div_ceil(3) * 4));
    STANDARD.encode_string(container.as_slice(), &mut encoded);

    let mut text = String::with_capacity(encoded.len() + encoded.len() / LINE_WIDTH + 80);
    text.push_str("-----BEGIN OPENSSH PRIVATE KEY-----\n");
    for line in encoded.as_bytes().chunks(LINE_WIDTH) {
        // Base64 is ASCII, so every chunk of bytes is valid text.
        text.push_str(std::str::from_utf8(line).unwrap_or_default());
        text.push('\n');
    }
    text.push_str("-----END OPENSSH PRIVATE KEY-----\n");
    Ok(text)
}
