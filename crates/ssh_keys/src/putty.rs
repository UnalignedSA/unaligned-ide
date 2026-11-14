//! PuTTY's own private key files (`.ppk`, versions 2 and 3), read and turned
//! into the numbers an OpenSSH key file is made of.
//!
//! A file looks like this (PuTTY manual, appendix "PPK file format"):
//!
//! ```text
//! PuTTY-User-Key-File-3: ssh-ed25519
//! Encryption: aes256-cbc            or "none"
//! Comment: laptop key
//! Public-Lines: 2
//! <base64 of the public key blob>
//! Key-Derivation: Argon2id          these five lines only in version 3,
//! Argon2-Memory: 8192               and only when the key is encrypted
//! Argon2-Passes: 13
//! Argon2-Parallelism: 1
//! Argon2-Salt: <hex>
//! Private-Lines: 1
//! <base64 of the private key blob, encrypted or not>
//! Private-MAC: <hex>
//! ```
//!
//! The MAC covers the algorithm, the encryption, the comment, the public blob
//! and the private blob in the clear, so it tells a wrong passphrase or a
//! damaged file apart from a good one.

use aes::cipher::{BlockDecryptMut as _, KeyIvInit as _, block_padding::NoPadding};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::Hmac;
use sha1::{Digest as _, Sha1};
use sha2::Sha256;
use zeroize::{Zeroize as _, Zeroizing};

use crate::wire::{Reader, describe_public_blob, put_mpint, put_string};
use crate::{KeyError, is_padding, openssh};

pub(crate) const FIRST_LINE: &str = "PuTTY-User-Key-File-";

const AES_BLOCK: usize = 16;
const AES256_KEY: usize = 32;
const SHA1_LENGTH: usize = 20;
/// PuTTY writes 64 base64 characters per line.
const LINE_WIDTH: usize = 64;
/// No key comes near a megabyte of base64; a file that announces more is not
/// given the memory it asks for.
const LONGEST_BLOB: usize = 1 << 20;

/// Limits on the Argon2 parameters of a file, checked before any memory is
/// reserved: a file is free to ask for anything.
const MAX_ARGON2_MEMORY_KIB: u32 = 1024 * 1024;
const MAX_ARGON2_PASSES: u32 = 1000;
const MAX_ARGON2_PARALLELISM: u32 = 64;

type Aes256CbcDecryptor = cbc::Decryptor<aes::Aes256>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Version {
    Two,
    Three,
}

struct Argon2Parameters {
    flavour: argon2::Algorithm,
    memory_kib: u32,
    passes: u32,
    parallelism: u32,
    salt: Vec<u8>,
}

pub(crate) struct PuttyKey {
    version: Version,
    /// The SSH name of the key type, such as `ssh-ed25519`.
    algorithm: String,
    /// The name people know the key type by, such as "RSA 4096".
    algorithm_name: String,
    encrypted: bool,
    /// The comment as the file has it, then (when the line ended with spaces
    /// that a mail or chat tool may have added) without them. The MAC says
    /// which one is the real comment.
    comments: Vec<String>,
    public_blob: Vec<u8>,
    /// As stored in the file: encrypted when `encrypted` is set.
    private_blob: Zeroizing<Vec<u8>>,
    mac: Vec<u8>,
    argon2: Option<Argon2Parameters>,
}

/// A private blob in the clear, and the comment its MAC was computed with.
struct Opened<'a> {
    private_blob: Zeroizing<Vec<u8>>,
    comment: &'a str,
}

fn damaged(what: &str) -> KeyError {
    KeyError::Damaged(format!(
        "This PuTTY key is damaged, {what}: copy the whole file again."
    ))
}

fn incomplete() -> KeyError {
    damaged("part of it is missing")
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some((high * 16 + low) as u8)
        })
        .collect()
}

/// The lines of a text, whatever its line ends, without empty lines.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.split(['\n', '\r'])
        .filter(|line| !line.trim_matches(is_padding).is_empty())
}

/// Splits `Name: value`. The value keeps what follows the one space PuTTY
/// writes after the colon.
fn header(line: &str) -> Option<(&str, &str)> {
    let (name, value) = line.trim_start_matches(is_padding).split_once(':')?;
    let is_name = !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-');
    is_name.then(|| (name, value.strip_prefix(' ').unwrap_or(value)))
}

/// Reads the base64 lines that follow a `Public-Lines:` or `Private-Lines:`
/// header. The line breaks may have been lost or moved, so the announced
/// number of lines is only used to check that nothing is missing.
fn base64_lines<'a>(
    announced: &str,
    lines: &mut std::iter::Peekable<impl Iterator<Item = &'a str>>,
) -> Result<Zeroizing<Vec<u8>>, KeyError> {
    let announced: usize = announced
        .trim_matches(is_padding)
        .parse()
        .map_err(|_| damaged("a line count cannot be read"))?;
    let longest = announced.saturating_mul(LINE_WIDTH);
    if longest > LONGEST_BLOB {
        return Err(damaged("a line count cannot be right"));
    }
    // Sized for the announced lines, so that the buffer never grows: a
    // buffer that grows leaves its old content behind, out of reach of the
    // wipe.
    let mut joined = Zeroizing::new(String::with_capacity(longest));
    while let Some(line) = lines.peek() {
        if header(line).is_some() {
            break;
        }
        for character in line.chars().filter(|character| !is_padding(*character)) {
            if joined.len() + character.len_utf8() > longest {
                return Err(damaged("it holds more lines than it announces"));
            }
            joined.push(character);
        }
        lines.next();
    }
    if joined.len() + LINE_WIDTH <= longest {
        return Err(damaged("some of its lines are missing"));
    }
    STANDARD
        .decode(joined.as_bytes())
        .map(Zeroizing::new)
        .map_err(|_| damaged("some of its characters are missing or were changed"))
}

impl PuttyKey {
    /// Reads a PuTTY key file. `text` starts at the `PuTTY-User-Key-File-`
    /// line; whatever comes after the `Private-MAC:` line is ignored.
    pub(crate) fn parse(text: &str) -> Result<Self, KeyError> {
        let mut lines = lines(text).peekable();
        let first_line = lines.next().ok_or_else(incomplete)?;
        let (name, algorithm) = header(first_line).ok_or_else(incomplete)?;
        let algorithm = algorithm.trim_matches(is_padding);
        let version = match name.strip_prefix(FIRST_LINE) {
            Some("2") => Version::Two,
            Some("3") => Version::Three,
            Some("1") => {
                return Err(KeyError::Unsupported(
                    "This PuTTY key is in a very old file format: open it in PuTTYgen, save it again, and use the new file."
                        .to_string(),
                ));
            }
            _ => {
                return Err(KeyError::Unsupported(
                    "This PuTTY key is in a file format that is newer than this application understands: in PuTTYgen, export it as an OpenSSH key and use that file."
                        .to_string(),
                ));
            }
        };
        if !matches!(
            algorithm,
            "ssh-ed25519"
                | "ssh-rsa"
                | "ssh-dss"
                | "ecdsa-sha2-nistp256"
                | "ecdsa-sha2-nistp384"
                | "ecdsa-sha2-nistp521"
        ) {
            let shown: String = algorithm.chars().take(64).collect();
            return Err(KeyError::Unsupported(format!(
                "This PuTTY key is of a type ({shown}) that cannot be converted: use an ED25519, RSA or ECDSA key."
            )));
        }

        let mut encryption = None;
        let mut comments = None;
        let mut public_blob = None;
        let mut private_blob = None;
        let mut mac = None;
        let mut derivation = None;
        let mut memory_kib = None;
        let mut passes = None;
        let mut parallelism = None;
        let mut salt = None;
        while let Some(line) = lines.next() {
            let Some((name, value)) = header(line) else {
                continue;
            };
            let trimmed = value.trim_matches(is_padding);
            let number = || {
                trimmed
                    .parse::<u32>()
                    .map_err(|_| damaged("one of its numbers cannot be read"))
            };
            match name {
                "Encryption" => encryption = Some(trimmed),
                "Comment" => {
                    let mut candidates = vec![value.to_string()];
                    let without_trailing_spaces = value.trim_end_matches(is_padding);
                    if without_trailing_spaces != value {
                        candidates.push(without_trailing_spaces.to_string());
                    }
                    comments = Some(candidates);
                }
                "Public-Lines" => public_blob = Some(base64_lines(trimmed, &mut lines)?),
                "Private-Lines" => private_blob = Some(base64_lines(trimmed, &mut lines)?),
                "Key-Derivation" => derivation = Some(trimmed),
                "Argon2-Memory" => memory_kib = Some(number()?),
                "Argon2-Passes" => passes = Some(number()?),
                "Argon2-Parallelism" => parallelism = Some(number()?),
                "Argon2-Salt" => {
                    salt = Some(
                        decode_hex(trimmed).ok_or_else(|| damaged("its salt cannot be read"))?,
                    )
                }
                "Private-MAC" => {
                    mac = Some(
                        decode_hex(trimmed)
                            .ok_or_else(|| damaged("its last line cannot be read"))?,
                    );
                    break;
                }
                _ => {}
            }
        }

        let encrypted = match encryption.ok_or_else(incomplete)? {
            "none" => false,
            "aes256-cbc" => true,
            other => {
                let shown: String = other.chars().take(64).collect();
                return Err(KeyError::Unsupported(format!(
                    "This PuTTY key is protected in a way ({shown}) that is not supported: in PuTTYgen, export it as an OpenSSH key and use that file."
                )));
            }
        };
        let comments = comments.ok_or_else(incomplete)?;
        let public_blob = public_blob.ok_or_else(incomplete)?.to_vec();
        let private_blob = private_blob.ok_or_else(incomplete)?;
        let mac = mac.ok_or_else(incomplete)?;

        let mac_length = match version {
            Version::Two => 20,
            Version::Three => 32,
        };
        if mac.len() != mac_length {
            return Err(damaged("its last line is not complete"));
        }
        let algorithm_name = match describe_public_blob(&public_blob) {
            Some((key_type, name)) if key_type == algorithm => name,
            _ => return Err(damaged("its public part does not match its type")),
        };
        if encrypted && (private_blob.is_empty() || private_blob.len() % AES_BLOCK != 0) {
            return Err(damaged("its private part is not complete"));
        }

        let argon2 = if encrypted && version == Version::Three {
            let flavour = match derivation.ok_or_else(incomplete)? {
                "Argon2id" => argon2::Algorithm::Argon2id,
                "Argon2i" => argon2::Algorithm::Argon2i,
                "Argon2d" => argon2::Algorithm::Argon2d,
                other => {
                    let shown: String = other.chars().take(64).collect();
                    return Err(KeyError::Unsupported(format!(
                        "This PuTTY key is protected in a way ({shown}) that is not supported: in PuTTYgen, export it as an OpenSSH key and use that file."
                    )));
                }
            };
            let parameters = Argon2Parameters {
                flavour,
                memory_kib: memory_kib.ok_or_else(incomplete)?,
                passes: passes.ok_or_else(incomplete)?,
                parallelism: parallelism.ok_or_else(incomplete)?,
                salt: salt.ok_or_else(incomplete)?,
            };
            if parameters.memory_kib > MAX_ARGON2_MEMORY_KIB
                || parameters.passes > MAX_ARGON2_PASSES
                || parameters.parallelism > MAX_ARGON2_PARALLELISM
            {
                return Err(KeyError::Unsupported(
                    "This PuTTY key asks for too much memory or time to be unlocked here: in PuTTYgen, export it as an OpenSSH key and use that file."
                        .to_string(),
                ));
            }
            Some(parameters)
        } else {
            None
        };

        let key = Self {
            version,
            algorithm: algorithm.to_string(),
            algorithm_name,
            encrypted,
            comments,
            public_blob,
            private_blob,
            mac,
            argon2,
        };
        // An unencrypted key can be checked at once, so a damaged file is
        // reported as soon as it is looked at.
        if !key.encrypted {
            let opened = key.open(None)?;
            key.openssh_private_key(&opened.private_blob)?;
        }
        Ok(key)
    }

    pub(crate) fn algorithm_name(&self) -> &str {
        &self.algorithm_name
    }

    pub(crate) fn is_encrypted(&self) -> bool {
        self.encrypted
    }

    /// The comment, as far as it is known without the passphrase.
    pub(crate) fn comment(&self) -> Option<&str> {
        let comment = match self.open(None) {
            Ok(opened) => opened.comment,
            Err(_) => self.comments.last()?.as_str(),
        };
        (!comment.is_empty()).then_some(comment)
    }

    pub(crate) fn public_key_line(&self) -> String {
        openssh::public_key_line(&self.algorithm, &self.public_blob, self.comment())
    }

    /// The same key as an OpenSSH private key file. An encrypted PuTTY key
    /// gives an encrypted OpenSSH key with the same passphrase: a key its
    /// owner protected is never written in the clear.
    pub(crate) fn to_openssh(
        &self,
        passphrase: Option<&str>,
        bcrypt_rounds: u32,
    ) -> Result<String, KeyError> {
        let opened = self.open(passphrase)?;
        let private_key = self.openssh_private_key(&opened.private_blob)?;
        let passphrase = passphrase.filter(|_| self.encrypted).map(str::as_bytes);
        openssh::write(
            &self.public_blob,
            &private_key,
            opened.comment,
            passphrase,
            bcrypt_rounds,
        )
    }

    /// Decrypts the private blob when it is encrypted, and checks the MAC.
    fn open(&self, passphrase: Option<&str>) -> Result<Opened<'_>, KeyError> {
        if !self.encrypted {
            return self
                .open_with(b"")?
                .ok_or_else(|| damaged("part of it was changed"));
        }
        let passphrase = match passphrase {
            Some(passphrase) if !passphrase.is_empty() => passphrase,
            _ => return Err(KeyError::NeedsPassphrase),
        };
        if let Some(opened) = self.open_with(passphrase.as_bytes())? {
            return Ok(opened);
        }
        // PuTTY for Windows took a passphrase in the code page of the system
        // before it learned Unicode: for western Europe, one byte for each
        // accented letter.
        if let Some(single_bytes) = latin1_bytes(passphrase)
            && let Some(opened) = self.open_with(&single_bytes)?
        {
            return Ok(opened);
        }
        Err(KeyError::WrongPassphrase)
    }

    /// `None` when the MAC does not match: a wrong passphrase, or a file
    /// that was changed.
    fn open_with(&self, passphrase: &[u8]) -> Result<Option<Opened<'_>>, KeyError> {
        let mut private_blob = self.private_blob.clone();
        let mut mac_key = Zeroizing::new([0u8; 32]);
        let mac_key: &[u8] = match (self.version, &self.argon2) {
            (Version::Two, _) => {
                if self.encrypted {
                    // Two SHA-1 hashes of a counter and the passphrase, end to
                    // end, give the cipher key; the IV is all zeros.
                    let mut key = Zeroizing::new([0u8; 2 * SHA1_LENGTH]);
                    let (first, second) = key.split_at_mut(SHA1_LENGTH);
                    sha1_into(&[&0u32.to_be_bytes(), passphrase], first);
                    sha1_into(&[&1u32.to_be_bytes(), passphrase], second);
                    decrypt(&key[..AES256_KEY], &[0u8; AES_BLOCK], &mut private_blob)?;
                }
                let mac_key = &mut mac_key[..SHA1_LENGTH];
                sha1_into(&[b"putty-private-key-file-mac-key", passphrase], mac_key);
                mac_key
            }
            (Version::Three, Some(parameters)) => {
                // One Argon2 run gives the cipher key, the IV and the MAC key.
                let mut derived = Zeroizing::new([0u8; AES256_KEY + AES_BLOCK + 32]);
                parameters.derive(passphrase, derived.as_mut_slice())?;
                let (key, rest) = derived.split_at(AES256_KEY);
                let (iv, derived_mac_key) = rest.split_at(AES_BLOCK);
                decrypt(key, iv, &mut private_blob)?;
                mac_key.copy_from_slice(derived_mac_key);
                mac_key.as_slice()
            }
            // An unencrypted version 3 file has an empty MAC key.
            (Version::Three, None) => &[],
        };

        let encryption = if self.encrypted { "aes256-cbc" } else { "none" };
        let matches = |comment: &[u8]| {
            let parts: [&[u8]; 5] = [
                self.algorithm.as_bytes(),
                encryption.as_bytes(),
                comment,
                &self.public_blob,
                &private_blob,
            ];
            match self.version {
                Version::Two => mac_matches::<Hmac<Sha1>>(mac_key, &parts, &self.mac),
                Version::Three => mac_matches::<Hmac<Sha256>>(mac_key, &parts, &self.mac),
            }
        };
        // The MAC was computed over the bytes of the comment, which an old
        // file may hold in a single-byte encoding (see `text_from_bytes`).
        let comment = self.comments.iter().find(|comment| {
            matches(comment.as_bytes())
                || latin1_bytes(comment).is_some_and(|single_bytes| matches(&single_bytes))
        });
        Ok(comment.map(|comment| Opened {
            private_blob,
            comment,
        }))
    }

    /// The key type and the numbers of the key, in the order OpenSSH stores
    /// them in its private section.
    fn openssh_private_key(&self, private_blob: &[u8]) -> Result<Zeroizing<Vec<u8>>, KeyError> {
        let unexpected = || damaged("its content is not what its type announces");
        // Room for every value of both blobs, plus a possible sign byte in
        // front of each number: the buffer never has to grow.
        let mut out = Zeroizing::new(Vec::with_capacity(
            self.public_blob.len() + private_blob.len() + 128,
        ));
        let mut public = Reader::new(&self.public_blob);
        let mut private = Reader::new(private_blob);
        let key_type = public.string().ok_or_else(unexpected)?;
        put_string(&mut out, key_type);
        let mut public_value = || public.string().ok_or_else(unexpected);
        let mut private_value = || private.string().ok_or_else(unexpected);
        match self.algorithm.as_str() {
            "ssh-ed25519" => {
                let public_key = public_value()?;
                let seed = private_value()?;
                if public_key.len() != 32 || seed.len() > 32 {
                    return Err(unexpected());
                }
                put_string(&mut out, public_key);
                // OpenSSH stores the 32-byte seed followed by the public key.
                // PuTTY stores the seed alone, and its older versions dropped
                // the zero bytes at its end.
                let mut seed_and_public = Zeroizing::new([0u8; 64]);
                seed_and_public[..seed.len()].copy_from_slice(seed);
                seed_and_public[32..].copy_from_slice(public_key);
                put_string(&mut out, seed_and_public.as_slice());
            }
            "ssh-rsa" => {
                // PuTTY: e, n in the public blob; d, p, q, iqmp in the private one.
                // OpenSSH: n, e, d, iqmp, p, q.
                let exponent = public_value()?;
                let modulus = public_value()?;
                let private_exponent = private_value()?;
                let prime_p = private_value()?;
                let prime_q = private_value()?;
                let inverse_q = private_value()?;
                for number in [
                    modulus,
                    exponent,
                    private_exponent,
                    inverse_q,
                    prime_p,
                    prime_q,
                ] {
                    put_mpint(&mut out, number);
                }
            }
            "ssh-dss" => {
                // Both: p, q, g, y (public), then x (private).
                for _ in 0..4 {
                    put_mpint(&mut out, public_value()?);
                }
                put_mpint(&mut out, private_value()?);
            }
            _ => {
                // ECDSA. Both: curve name, public point, then the private number.
                let curve = public_value()?;
                let point = public_value()?;
                if !self.algorithm.as_bytes().ends_with(curve) {
                    return Err(unexpected());
                }
                put_string(&mut out, curve);
                put_string(&mut out, point);
                put_mpint(&mut out, private_value()?);
            }
        }
        Ok(out)
    }
}

impl Argon2Parameters {
    fn derive(&self, passphrase: &[u8], out: &mut [u8]) -> Result<(), KeyError> {
        let invalid = || damaged("its protection settings are not valid");
        let parameters = argon2::Params::new(
            self.memory_kib,
            self.passes,
            self.parallelism,
            Some(out.len()),
        )
        .map_err(|_| invalid())?;
        let block_count = parameters.block_count();
        let argon2 = argon2::Argon2::new(self.flavour, argon2::Version::V0x13, parameters);
        let mut memory: Vec<argon2::Block> = Vec::new();
        memory.try_reserve_exact(block_count).map_err(|_| {
            KeyError::Unsupported(
                "This computer does not have enough free memory to unlock this PuTTY key: close other applications and try again."
                    .to_string(),
            )
        })?;
        memory.resize(block_count, argon2::Block::default());
        let result =
            argon2.hash_password_into_with_memory(passphrase, &self.salt, out, &mut memory);
        memory.zeroize();
        result.map_err(|_| invalid())
    }
}

/// The text in Latin-1 (one byte for each character), when it has characters
/// outside ASCII and all of them fit.
fn latin1_bytes(text: &str) -> Option<Zeroizing<Vec<u8>>> {
    if text.is_ascii() {
        return None;
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(text.len()));
    for character in text.chars() {
        bytes.push(u8::try_from(u32::from(character)).ok()?);
    }
    Some(bytes)
}

/// SHA-1 of the parts end to end, written straight into `out` so that no
/// copy of it is left elsewhere.
fn sha1_into(parts: &[&[u8]], out: &mut [u8]) {
    let mut hash = Sha1::new();
    for part in parts {
        hash.update(part);
    }
    hash.finalize_into(out.into());
}

fn decrypt(key: &[u8], iv: &[u8], data: &mut [u8]) -> Result<(), KeyError> {
    let unreadable = || damaged("its private part is not complete");
    let cipher = Aes256CbcDecryptor::new_from_slices(key, iv).map_err(|_| unreadable())?;
    cipher
        .decrypt_padded_mut::<NoPadding>(data)
        .map_err(|_| unreadable())?;
    Ok(())
}

/// Computes the MAC over the parts, each one preceded by its length, and
/// compares it with the expected one in constant time.
fn mac_matches<M>(key: &[u8], parts: &[&[u8]], expected: &[u8]) -> bool
where
    M: hmac::Mac + hmac::digest::KeyInit,
{
    let Ok(mut mac) = <M as hmac::Mac>::new_from_slice(key) else {
        return false;
    };
    for part in parts {
        mac.update(&(part.len() as u32).to_be_bytes());
        mac.update(part);
    }
    mac.verify_slice(expected).is_ok()
}
