//! Reads SSH private keys in whatever form a person has them and turns them
//! into a key file that OpenSSH's `ssh -i` accepts.
//!
//! - OpenSSH keys (`-----BEGIN OPENSSH PRIVATE KEY-----`) and PEM keys
//!   (`RSA`, `EC`, `DSA`, PKCS#8) are kept: OpenSSH reads them itself. They are
//!   only tidied up, because keys are pasted from mails, chat tools and
//!   Windows editors that change line ends and line breaks.
//! - PuTTY keys (`.ppk`, versions 2 and 3) are converted to OpenSSH keys. An
//!   encrypted PuTTY key gives an encrypted OpenSSH key with the same
//!   passphrase.
//!
//! Nothing here touches the network, starts a process or keeps state; the
//! only function that touches the disk is [`write_key_file`]. Buffers that
//! hold private key material are wiped when dropped. The texts handed back
//! ([`ImportedKey::text`], and what [`text_from_bytes`] returns) are ordinary
//! `String`s the caller owns: wiping them, if wanted, is up to the caller.
//!
//! [`inspect`] and [`import`] take text. A key read from a file should go
//! through [`text_from_bytes`] first: key files written on Windows are not
//! always UTF-8.
//!
//! Unlocking an encrypted PuTTY key and protecting its OpenSSH copy both run a
//! deliberately slow key derivation: [`import`] can take a second or more, and
//! as much memory as the PuTTY file asks for (8 MiB by default, at most
//! 1 GiB). Call it off the main thread.

mod armor;
mod der;
mod key_file;
mod openssh;
mod putty;
mod wire;

#[cfg(test)]
mod tests;

use std::fmt;
use std::ops::RangeInclusive;

pub use key_file::write_key_file;

use armor::Armor;
use putty::PuttyKey;

/// OpenSSH writes its own format 70 characters to a line and reads any line
/// length, so a regular body written by another tool is kept as it is.
const OPENSSH_LINE_WIDTH: usize = 70;
const OPENSSH_KEPT_WIDTHS: RangeInclusive<usize> = 4..=76;
/// PEM is read by the cryptography library OpenSSH is built with, and some
/// (LibreSSL, older OpenSSL) take any body line that is not exactly 64
/// characters long for the last one. Nothing but 64 is kept.
const PEM_LINE_WIDTH: usize = 64;
const PEM_KEPT_WIDTHS: RangeInclusive<usize> = PEM_LINE_WIDTH..=PEM_LINE_WIDTH;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyFormat {
    OpenSsh,
    Pem,
    Putty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyInfo {
    pub format: KeyFormat,
    /// For people: "ED25519", "RSA", "RSA 4096" when the size is known,
    /// "ECDSA P-256", "DSA". `None` when the file does not say (an encrypted
    /// PKCS#8 PEM).
    pub algorithm: Option<String>,
    /// Protected by a passphrase.
    pub encrypted: bool,
    pub comment: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum KeyError {
    Empty,
    /// The text is a PUBLIC key (`ssh-ed25519 AAAA…`,
    /// `-----BEGIN PUBLIC KEY-----`, `---- BEGIN SSH2 PUBLIC KEY ----`, a
    /// `.pub` file): a very common mistake.
    PublicKey,
    /// A certificate, or anything else that is not a private key.
    NotAKey,
    /// An encrypted PuTTY key: converting it needs its passphrase.
    NeedsPassphrase,
    WrongPassphrase,
    /// A private key that cannot be used. The text is a whole sentence for
    /// the person, saying why and what to do.
    Unsupported(String),
    /// A private key that is incomplete or was changed. The text is a whole
    /// sentence for the person, saying what is wrong and what to do.
    Damaged(String),
}

impl fmt::Display for KeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sentence = match self {
            Self::Empty => "There is no key here yet: paste your private key or choose its file.",
            Self::PublicKey => {
                "This is the public key. Choose the private key: the file without \".pub\"."
            }
            Self::NotAKey => {
                "This is not an SSH private key: choose the private key file, such as \"id_ed25519\" or a \".ppk\" file."
            }
            Self::NeedsPassphrase => {
                "This key is protected by a passphrase: enter it to use the key."
            }
            Self::WrongPassphrase => {
                "This passphrase does not open the key: check it and try again."
            }
            Self::Unsupported(sentence) | Self::Damaged(sentence) => sentence,
        };
        formatter.write_str(sentence)
    }
}

impl std::error::Error for KeyError {}

pub struct ImportedKey {
    /// The content of a private key file that `ssh -i` accepts: OpenSSH or
    /// PEM text, LF line ends, one trailing newline.
    pub text: String,
    /// What was given (before any conversion).
    pub info: KeyInfo,
    /// True when the text had to be converted (a PuTTY key).
    pub converted: bool,
    /// `ssh-ed25519 AAAA… comment`: the line to put in a server's
    /// `authorized_keys`, when it can be read without a passphrase (always
    /// for OpenSSH and PuTTY keys; `None` for PEM).
    pub public_key: Option<String>,
}

/// What the text is, without needing a passphrase.
pub fn inspect(text: &str) -> Result<KeyInfo, KeyError> {
    match read(text)? {
        Source::Kept(kept) => Ok(kept.info),
        Source::Putty(key) => Ok(putty_info(&key)),
    }
}

/// Reads a private key and gives back the content of a key file `ssh -i`
/// accepts.
///
/// `passphrase` is used only for an encrypted PuTTY key (`None` gives
/// `Err(NeedsPassphrase)`). OpenSSH and PEM keys are kept as they are,
/// encrypted or not, so they never need one here.
pub fn import(text: &str, passphrase: Option<&str>) -> Result<ImportedKey, KeyError> {
    import_with_rounds(text, passphrase, openssh::BCRYPT_ROUNDS)
}

/// `import`, with the cost of the key derivation that protects a converted
/// key as a parameter: tests in debug builds use fewer rounds than real use.
fn import_with_rounds(
    text: &str,
    passphrase: Option<&str>,
    bcrypt_rounds: u32,
) -> Result<ImportedKey, KeyError> {
    match read(text)? {
        Source::Kept(kept) => Ok(ImportedKey {
            text: kept.text,
            info: kept.info,
            converted: false,
            public_key: kept.public_key,
        }),
        Source::Putty(key) => Ok(ImportedKey {
            text: key.to_openssh(passphrase, bcrypt_rounds)?,
            info: putty_info(&key),
            converted: true,
            public_key: Some(key.public_key_line()),
        }),
    }
}

/// What a new key is made from: 32 random bytes. The same seed always gives
/// the same key, so a key can be shown (its public half) before it is known
/// whether it will be protected by a passphrase. Wiped when dropped.
#[derive(Clone)]
pub struct KeySeed(zeroize::Zeroizing<[u8; 32]>);

impl KeySeed {
    /// A seed from the system's random number generator.
    pub fn new() -> Result<Self, KeyError> {
        Ok(Self(
            zeroize::Zeroizing::new(openssh::random_bytes::<32>()?),
        ))
    }
}

/// Makes the ED25519 key of a seed: the kind of key `ssh-keygen` makes by
/// default, accepted by every OpenSSH server since 2014.
///
/// With a passphrase the key file is protected the way `ssh-keygen` protects
/// one. `comment` is the label that ends the public key line, usually what
/// the key is used from. The `public_key` of the result is the line to add to
/// the server's `~/.ssh/authorized_keys`.
pub fn generate(
    seed: &KeySeed,
    comment: &str,
    passphrase: Option<&str>,
) -> Result<ImportedKey, KeyError> {
    generate_with_rounds(seed, comment, passphrase, openssh::BCRYPT_ROUNDS)
}

fn generate_with_rounds(
    seed: &KeySeed,
    comment: &str,
    passphrase: Option<&str>,
    bcrypt_rounds: u32,
) -> Result<ImportedKey, KeyError> {
    const KEY_TYPE: &str = "ssh-ed25519";
    // A comment is one line of the public key file.
    let comment: String = comment
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    let comment = comment.trim();

    let seed = &seed.0;
    let pair =
        ring::signature::Ed25519KeyPair::from_seed_unchecked(seed.as_slice()).map_err(|_| {
            KeyError::Unsupported("This computer could not make a key: try again.".to_string())
        })?;
    let public = ring::signature::KeyPair::public_key(&pair).as_ref();

    let mut public_blob = Vec::new();
    wire::put_string(&mut public_blob, KEY_TYPE.as_bytes());
    wire::put_string(&mut public_blob, public);

    // OpenSSH keeps the public half after the seed, in one string.
    let mut seed_and_public = zeroize::Zeroizing::new(Vec::with_capacity(64));
    seed_and_public.extend_from_slice(seed.as_slice());
    seed_and_public.extend_from_slice(public);
    let mut private_key = zeroize::Zeroizing::new(Vec::with_capacity(
        public_blob.len() + 4 + seed_and_public.len(),
    ));
    private_key.extend_from_slice(&public_blob);
    wire::put_string(&mut private_key, &seed_and_public);

    let text = openssh::write(
        &public_blob,
        &private_key,
        comment,
        passphrase.map(str::as_bytes),
        bcrypt_rounds,
    )?;
    Ok(ImportedKey {
        text,
        info: KeyInfo {
            format: KeyFormat::OpenSsh,
            algorithm: Some("ED25519".to_string()),
            encrypted: passphrase.is_some(),
            comment: (!comment.is_empty()).then(|| comment.to_string()),
        },
        converted: false,
        public_key: Some(openssh::public_key_line(
            KEY_TYPE,
            &public_blob,
            Some(comment),
        )),
    })
}

/// The text of a key file read from disk, whatever its encoding.
///
/// A key is ASCII, but the file around it is not always: PowerShell and
/// Notepad write UTF-16, and an old PuTTY for Windows wrote the comment of a
/// key in the code page of the system. UTF-8 and UTF-16 (with its byte order
/// mark) are decoded; anything else is read one byte per character
/// (Latin-1), which is what [`inspect`] and [`import`] expect of such a
/// PuTTY file to check it.
///
/// Reading a key file with `std::fs::read_to_string` instead fails on these
/// files.
pub fn text_from_bytes(bytes: &[u8]) -> String {
    let utf16 = |rest: &[u8], unit: fn([u8; 2]) -> u16| {
        let units = rest.chunks_exact(2).map(|pair| unit([pair[0], pair[1]]));
        let mut text = String::with_capacity(rest.len());
        text.extend(
            char::decode_utf16(units)
                .map(|character| character.unwrap_or(char::REPLACEMENT_CHARACTER)),
        );
        text
    };
    if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe]) {
        utf16(rest, u16::from_le_bytes)
    } else if let Some(rest) = bytes.strip_prefix(&[0xfe, 0xff]) {
        utf16(rest, u16::from_be_bytes)
    } else if let Ok(text) = std::str::from_utf8(bytes) {
        text.to_string()
    } else {
        let mut text = String::with_capacity(bytes.len() * 2);
        text.extend(bytes.iter().map(|byte| char::from(*byte)));
        text
    }
}

/// White space, and the invisible characters editors and chat tools add: the
/// byte order mark and the zero width space.
pub(crate) fn is_padding(character: char) -> bool {
    character.is_whitespace() || character == '\u{feff}' || character == '\u{200b}'
}

/// A key that is kept in its own format, tidied up.
struct Kept {
    text: String,
    info: KeyInfo,
    public_key: Option<String>,
}

enum Source {
    Kept(Kept),
    Putty(PuttyKey),
}

fn putty_info(key: &PuttyKey) -> KeyInfo {
    KeyInfo {
        format: KeyFormat::Putty,
        algorithm: Some(key.algorithm_name().to_string()),
        encrypted: key.is_encrypted(),
        comment: key.comment().map(str::to_string),
    }
}

fn read(text: &str) -> Result<Source, KeyError> {
    if text.chars().all(is_padding) {
        return Err(KeyError::Empty);
    }
    if let Some(start) = find_line_start(text, putty::FIRST_LINE) {
        return PuttyKey::parse(&text[start..]).map(Source::Putty);
    }
    if let Some(armor) = armor::find_private_key(text)? {
        return read_armored(&armor).map(Source::Kept);
    }
    Err(explain_other_text(text))
}

/// Where a line that starts with `prefix` (after any spaces) begins.
fn find_line_start(text: &str, prefix: &str) -> Option<usize> {
    let mut offset = 0;
    for line in text.split_inclusive(['\n', '\r']) {
        let content = line.trim_start_matches(is_padding);
        if content.starts_with(prefix) {
            return Some(offset + (line.len() - content.len()));
        }
        offset += line.len();
    }
    None
}

fn read_armored(armor: &Armor<'_>) -> Result<Kept, KeyError> {
    let unreadable = || {
        KeyError::Damaged(
            "This key is damaged, part of it is missing or was changed: copy the whole key again."
                .to_string(),
        )
    };
    let is_openssh = armor.label == "OPENSSH PRIVATE KEY";
    let has_passphrase_headers = armor
        .headers
        .iter()
        .any(|header| header.starts_with("Proc-Type:") && header.contains("ENCRYPTED"));
    let headers_allowed = matches!(
        armor.label,
        "RSA PRIVATE KEY" | "EC PRIVATE KEY" | "DSA PRIVATE KEY"
    );
    if !armor.headers.is_empty() && !headers_allowed {
        return Err(unreadable());
    }
    if has_passphrase_headers
        && !armor
            .headers
            .iter()
            .any(|header| header.starts_with("DEK-Info:"))
    {
        return Err(unreadable());
    }
    let data = armor.decode()?;

    if is_openssh {
        let container = openssh::parse(&data)?;
        let public_key = openssh::public_key_line(
            &container.key_type,
            &container.public_blob,
            container.comment.as_deref(),
        );
        return Ok(Kept {
            text: armor.to_file_text(OPENSSH_KEPT_WIDTHS, OPENSSH_LINE_WIDTH),
            info: KeyInfo {
                format: KeyFormat::OpenSsh,
                algorithm: Some(container.algorithm),
                encrypted: container.encrypted,
                comment: container.comment,
            },
            public_key: Some(public_key),
        });
    }

    // A PEM body is one DER SEQUENCE, except for the old encrypted form
    // (`Proc-Type: 4,ENCRYPTED`), where the whole body is cipher text.
    let sequence = if has_passphrase_headers {
        None
    } else {
        Some(der::whole_sequence(&data).ok_or_else(unreadable)?)
    };
    let (algorithm, encrypted) = match armor.label {
        "RSA PRIVATE KEY" => {
            let algorithm = match sequence.and_then(der::rsa_bits) {
                Some(bits) => format!("RSA {bits}"),
                None => "RSA".to_string(),
            };
            (Some(algorithm), has_passphrase_headers)
        }
        "EC PRIVATE KEY" => {
            let algorithm = sequence.and_then(der::ec_curve).unwrap_or("ECDSA");
            (Some(algorithm.to_string()), has_passphrase_headers)
        }
        "DSA PRIVATE KEY" => (Some("DSA".to_string()), has_passphrase_headers),
        "PRIVATE KEY" => (sequence.and_then(der::pkcs8_algorithm), false),
        "ENCRYPTED PRIVATE KEY" => (None, true),
        other => {
            let shown: String = other.chars().take(64).collect();
            return Err(KeyError::Unsupported(format!(
                "This kind of private key ({shown}) is not one that SSH can use: choose an OpenSSH, PEM or PuTTY key."
            )));
        }
    };
    Ok(Kept {
        text: armor.to_file_text(PEM_KEPT_WIDTHS, PEM_LINE_WIDTH),
        info: KeyInfo {
            format: KeyFormat::Pem,
            algorithm,
            encrypted,
            comment: None,
        },
        public_key: None,
    })
}

/// Says what a text without any private key block is: the mistakes people
/// make most are a public key and a key that lost its first line.
fn explain_other_text(text: &str) -> KeyError {
    let first_line_missing = || {
        KeyError::Damaged(
            "This key is incomplete, its first line is missing: copy the whole key again, from its very first character."
                .to_string(),
        )
    };
    if text.contains("---- BEGIN SSH2 ENCRYPTED PRIVATE KEY") {
        return KeyError::Unsupported(
            "This key is in the ssh.com format, which is not supported: in PuTTYgen, export it as an OpenSSH key and use that file."
                .to_string(),
        );
    }
    if text.contains("---- BEGIN SSH2 PUBLIC KEY") {
        return KeyError::PublicKey;
    }
    let mut public_key = false;
    let mut certificate = false;
    for line in text.split(['\n', '\r']) {
        let line = line.trim_matches(is_padding);
        // The end of a private key, or the middle of a PuTTY file, without
        // the line that starts it.
        if (line.starts_with("-----END ") && line.contains("PRIVATE KEY"))
            || line.starts_with("Private-Lines:")
            || line.starts_with("Private-MAC:")
            // "openssh-key-v1" in base64: the body of an OpenSSH private key.
            || line.starts_with("b3BlbnNzaC1rZXktdjE")
        {
            return first_line_missing();
        }
        let mut words = line
            .split(is_padding)
            .filter(|word| !word.is_empty())
            .peekable();
        while let Some(word) = words.next() {
            // A public key blob starts with the length of its type name:
            // three zero bytes, "AAAA" in base64.
            if is_public_blob_start(word) {
                public_key = true;
            } else if is_key_type(word) && words.peek().is_some_and(|next| next.starts_with("AAAA"))
            {
                if word.contains("-cert-") {
                    certificate = true;
                } else {
                    public_key = true;
                }
            }
        }
    }
    if certificate {
        KeyError::NotAKey
    } else if public_key {
        KeyError::PublicKey
    } else {
        KeyError::NotAKey
    }
}

fn is_key_type(word: &str) -> bool {
    ["ssh-", "ecdsa-sha2-", "sk-ssh-", "sk-ecdsa-"]
        .iter()
        .any(|prefix| word.starts_with(prefix))
}

/// The first characters of the base64 of a public key blob of a common type
/// (`ssh-rsa`, `ssh-dss`, `ssh-ed25519`, `ecdsa-sha2-…`), for a public key
/// pasted without the type name in front.
fn is_public_blob_start(word: &str) -> bool {
    [
        "AAAAB3NzaC1yc2E",
        "AAAAB3NzaC1kc3M",
        "AAAAC3NzaC1lZDI1NTE5",
        "AAAAE2VjZHNhLXNoYTItbmlzdH",
    ]
    .iter()
    .any(|prefix| word.starts_with(prefix))
}
