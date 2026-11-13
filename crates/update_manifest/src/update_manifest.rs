//! The signed list of a release's files.
//!
//! A release carries two small files beside its installers:
//!
//! - `update.json`, the manifest: the product, the version, and for each
//!   installer its name, its size and its SHA-256;
//! - `update.json.sig`, an Ed25519 signature of the manifest, in base64.
//!
//! The application holds the public key (`UPDATE_PUBLIC_KEYS` in the brand
//! file) and installs a downloaded file only when the manifest is signed by
//! that key and names the file's exact size and hash. Whoever can publish a
//! release but does not hold the private key cannot make the application
//! install anything.
//!
//! This crate is the single place that writes, signs and checks the two
//! files: the application uses [`verify`], the `update-manifest` program of
//! this crate does the rest.

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use ring::{
    rand::SystemRandom,
    signature::{ED25519, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// The manifest's file name in a release.
pub const MANIFEST_FILE_NAME: &str = "update.json";
/// The signature's file name in a release.
pub const SIGNATURE_FILE_NAME: &str = "update.json.sig";

/// A manifest is a few hundred bytes. Anything larger is refused unread.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
/// A signature is 88 base64 characters and a line end.
pub const MAX_SIGNATURE_BYTES: usize = 1024;

/// Signed in front of the manifest's bytes, so that a signature made for
/// anything else with the same key is never a valid manifest signature.
const SIGNATURE_CONTEXT: &[u8] = b"update-manifest-v1\n";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The application identifier (`APP_ID` of the brand file).
    pub product: String,
    /// The version of the release, without a leading `v`.
    pub version: String,
    pub files: Vec<ManifestFile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFile {
    /// The file's name in the release. A plain name: no directory.
    pub name: String,
    pub size: u64,
    /// 64 lowercase hexadecimal characters.
    pub sha256: String,
}

impl Manifest {
    /// Describes `paths` as the files of `version`: reads each one for its
    /// size and its hash.
    pub fn describe(product: &str, version: &str, paths: &[PathBuf]) -> Result<Self> {
        let mut files = Vec::new();
        for path in paths {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .with_context(|| format!("{path:?} has no usable file name"))?
                .to_string();
            let (size, sha256) = hash_file(path)?;
            files.push(ManifestFile { name, size, sha256 });
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));
        let manifest = Self {
            product: product.to_string(),
            version: version.to_string(),
            files,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// Reads a manifest and refuses one that is not well formed. This does
    /// not check who wrote it: see [`verify`].
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_MANIFEST_BYTES,
            "the manifest is larger than {MAX_MANIFEST_BYTES} bytes"
        );
        let manifest: Self = serde_json::from_slice(bytes).context("the manifest is not valid")?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn to_json(&self) -> Result<String> {
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        Ok(json)
    }

    pub fn file(&self, name: &str) -> Option<&ManifestFile> {
        self.files.iter().find(|file| file.name == name)
    }

    fn validate(&self) -> Result<()> {
        ensure!(!self.product.is_empty(), "the manifest names no product");
        ensure!(
            is_plain_version(&self.version),
            "the manifest's version {:?} is not three numbers separated by dots",
            self.version
        );
        ensure!(!self.files.is_empty(), "the manifest lists no file");
        for (index, file) in self.files.iter().enumerate() {
            ensure!(
                is_plain_file_name(&file.name),
                "{:?} is not a plain file name",
                file.name
            );
            ensure!(file.size > 0, "{} is empty", file.name);
            ensure!(
                file.sha256.len() == 64
                    && file
                        .sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "the hash of {} is not 64 lowercase hexadecimal characters",
                file.name
            );
            ensure!(
                self.files[..index]
                    .iter()
                    .all(|other| other.name != file.name),
                "{} is listed twice",
                file.name
            );
        }
        Ok(())
    }
}

/// `1.2.3`: release versions carry no pre-release or build part.
fn is_plain_version(version: &str) -> bool {
    let mut parts = 0;
    for part in version.split('.') {
        parts += 1;
        if part.is_empty()
            || part.len() > 9
            || !part.bytes().all(|byte| byte.is_ascii_digit())
            || (part.len() > 1 && part.starts_with('0'))
        {
            return false;
        }
    }
    parts == 3
}

fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// The size and the SHA-256 (lowercase hexadecimal) of a file.
pub fn hash_file(path: &Path) -> Result<(u64, String)> {
    let mut file = File::open(path).with_context(|| format!("could not open {path:?}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut size = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("could not read {path:?}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((size, hex(&hasher.finalize())))
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[(byte >> 4) as usize] as char);
        text.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    text
}

fn signed_message(manifest: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(SIGNATURE_CONTEXT.len() + manifest.len());
    message.extend_from_slice(SIGNATURE_CONTEXT);
    message.extend_from_slice(manifest);
    message
}

/// Checks that `manifest` is signed by one of `public_keys`, then reads it.
///
/// `signature` is the content of the signature file; `public_keys` are
/// base64 Ed25519 public keys (32 bytes each). Nothing of the manifest may be
/// trusted before this returns `Ok`.
pub fn verify(manifest: &[u8], signature: &str, public_keys: &[&str]) -> Result<Manifest> {
    ensure!(
        manifest.len() <= MAX_MANIFEST_BYTES,
        "the manifest is larger than {MAX_MANIFEST_BYTES} bytes"
    );
    ensure!(!public_keys.is_empty(), "no public key to check against");
    let signature = BASE64
        .decode(signature.trim())
        .ok()
        .filter(|signature| signature.len() == 64)
        .context("the signature file does not hold an Ed25519 signature")?;
    let message = signed_message(manifest);

    let mut signed = false;
    for public_key in public_keys {
        let public_key = decode_public_key(public_key)?;
        if UnparsedPublicKey::new(&ED25519, &public_key)
            .verify(&message, &signature)
            .is_ok()
        {
            signed = true;
            break;
        }
    }
    ensure!(
        signed,
        "the manifest is not signed by this product's update key"
    );
    Manifest::parse(manifest)
}

fn decode_public_key(public_key: &str) -> Result<Vec<u8>> {
    BASE64
        .decode(public_key.trim())
        .ok()
        .filter(|key| key.len() == 32)
        .ok_or_else(|| anyhow!("a public key is not 32 bytes in base64"))
}

/// Splits the brand file's `UPDATE_PUBLIC_KEYS` value: keys separated by
/// commas. Every key must be well formed, or none is returned.
pub fn parse_public_keys(value: &str) -> Result<Vec<&str>> {
    let keys: Vec<&str> = value
        .split(',')
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .collect();
    for key in &keys {
        decode_public_key(key)?;
    }
    Ok(keys)
}

/// Creates a private key: a PKCS#8 document in base64. Keep it out of the
/// repository; whoever holds it can sign updates.
pub fn generate_private_key() -> Result<String> {
    let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .map_err(|_| anyhow!("the system gave no random bytes for a key"))?;
    Ok(BASE64.encode(document.as_ref()))
}

fn key_pair(private_key: &str) -> Result<Ed25519KeyPair> {
    let document = BASE64
        .decode(private_key.trim())
        .map_err(|_| anyhow!("the private key is not base64"))?;
    Ed25519KeyPair::from_pkcs8(&document)
        .map_err(|_| anyhow!("the private key is not an Ed25519 key in PKCS#8"))
}

/// The public key (base64) that goes with a private key.
pub fn public_key(private_key: &str) -> Result<String> {
    Ok(BASE64.encode(key_pair(private_key)?.public_key().as_ref()))
}

/// Signs a manifest: the content of the signature file. A manifest that is
/// not well formed is refused, so that nothing unreadable is ever signed.
pub fn sign(manifest: &[u8], private_key: &str) -> Result<String> {
    Manifest::parse(manifest)?;
    let key_pair = key_pair(private_key)?;
    let signature = key_pair.sign(&signed_message(manifest));
    let mut text = BASE64.encode(signature.as_ref());
    text.push('\n');
    Ok(text)
}

/// Compares the files of a directory with the manifest: every listed file
/// that is present must have the listed size and hash. Returns the names that
/// are listed but absent.
pub fn check_files(manifest: &Manifest, directory: &Path) -> Result<Vec<String>> {
    let mut absent = Vec::new();
    for file in &manifest.files {
        let path = directory.join(&file.name);
        if !path.exists() {
            absent.push(file.name.clone());
            continue;
        }
        let (size, sha256) = hash_file(&path)?;
        if size != file.size || sha256 != file.sha256 {
            bail!("{} is not the file the manifest describes", file.name);
        }
    }
    Ok(absent)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    fn manifest_json(version: &str) -> String {
        format!(
            r#"{{"product":"example-ide","version":"{version}","files":[{{"name":"example-ide-linux-x86_64.AppImage","size":4,"sha256":"{HASH}"}}]}}"#
        )
    }

    fn keys() -> (String, String) {
        let private_key = generate_private_key().unwrap();
        let public_key = public_key(&private_key).unwrap();
        (private_key, public_key)
    }

    #[test]
    fn test_a_signed_manifest_is_accepted() {
        let (private_key, public_key) = keys();
        let json = manifest_json("1.2.3");
        let signature = sign(json.as_bytes(), &private_key).unwrap();

        let manifest = verify(json.as_bytes(), &signature, &[&public_key]).unwrap();
        assert_eq!(manifest.product, "example-ide");
        assert_eq!(manifest.version, "1.2.3");
        let file = manifest.file("example-ide-linux-x86_64.AppImage").unwrap();
        assert_eq!((file.size, file.sha256.as_str()), (4, HASH));
        assert!(manifest.file("other").is_none());
    }

    #[test]
    fn test_any_listed_key_is_enough() {
        let (private_key, public_key) = keys();
        let (_, other_public_key) = keys();
        let json = manifest_json("1.2.3");
        let signature = sign(json.as_bytes(), &private_key).unwrap();
        verify(
            json.as_bytes(),
            &signature,
            &[&other_public_key, &public_key],
        )
        .unwrap();
    }

    #[test]
    fn test_a_changed_manifest_is_refused() {
        let (private_key, public_key) = keys();
        let json = manifest_json("1.2.3");
        let signature = sign(json.as_bytes(), &private_key).unwrap();

        let changed = manifest_json("1.2.4");
        let error = verify(changed.as_bytes(), &signature, &[&public_key]).unwrap_err();
        assert!(error.to_string().contains("not signed"), "{error}");

        // One more byte of whitespace is a different manifest too.
        let padded = format!("{json} ");
        verify(padded.as_bytes(), &signature, &[&public_key]).unwrap_err();
    }

    #[test]
    fn test_another_keys_signature_is_refused() {
        let (_, public_key) = keys();
        let (other_private_key, _) = keys();
        let json = manifest_json("1.2.3");
        let signature = sign(json.as_bytes(), &other_private_key).unwrap();
        let error = verify(json.as_bytes(), &signature, &[&public_key]).unwrap_err();
        assert!(error.to_string().contains("not signed"), "{error}");
    }

    #[test]
    fn test_a_signature_of_the_bare_bytes_is_refused() {
        // A signature made without the context line (by another tool, for
        // another purpose) must not pass.
        let private_key = generate_private_key().unwrap();
        let public_key = public_key(&private_key).unwrap();
        let json = manifest_json("1.2.3");
        let bare = BASE64.encode(key_pair(&private_key).unwrap().sign(json.as_bytes()));
        verify(json.as_bytes(), &bare, &[&public_key]).unwrap_err();
    }

    #[test]
    fn test_malformed_signatures_and_keys_are_refused() {
        let (private_key, public_key) = keys();
        let json = manifest_json("1.2.3");
        let signature = sign(json.as_bytes(), &private_key).unwrap();

        for bad_signature in ["", "not base64 !", "AAAA"] {
            verify(json.as_bytes(), bad_signature, &[&public_key]).unwrap_err();
        }
        verify(json.as_bytes(), &signature, &[]).unwrap_err();
        verify(json.as_bytes(), &signature, &["AAAA"]).unwrap_err();
    }

    #[test]
    fn test_only_well_formed_manifests_are_read_or_signed() {
        let private_key = generate_private_key().unwrap();
        let file = format!(r#"{{"name":"a.dmg","size":4,"sha256":"{HASH}"}}"#);
        let bad = [
            "{}".to_string(),
            "not json".to_string(),
            format!(r#"{{"product":"","version":"1.2.3","files":[{file}]}}"#),
            r#"{"product":"p","version":"1.2.3","files":[]}"#.to_string(),
            format!(r#"{{"product":"p","version":"1.2","files":[{file}]}}"#),
            format!(r#"{{"product":"p","version":"v1.2.3","files":[{file}]}}"#),
            format!(r#"{{"product":"p","version":"1.2.3-rc1","files":[{file}]}}"#),
            format!(r#"{{"product":"p","version":"1.02.3","files":[{file}]}}"#),
            format!(r#"{{"product":"p","version":"1.2.3","files":[{file},{file}]}}"#),
            format!(
                r#"{{"product":"p","version":"1.2.3","files":[{{"name":"../a.dmg","size":4,"sha256":"{HASH}"}}]}}"#
            ),
            format!(
                r#"{{"product":"p","version":"1.2.3","files":[{{"name":".hidden","size":4,"sha256":"{HASH}"}}]}}"#
            ),
            format!(
                r#"{{"product":"p","version":"1.2.3","files":[{{"name":"a.dmg","size":0,"sha256":"{HASH}"}}]}}"#
            ),
            r#"{"product":"p","version":"1.2.3","files":[{"name":"a.dmg","size":4,"sha256":"ABCD"}]}"#
                .to_string(),
            format!(
                r#"{{"product":"p","version":"1.2.3","files":[{{"name":"a.dmg","size":4,"sha256":"{}"}}]}}"#,
                HASH.to_uppercase()
            ),
        ];
        for json in &bad {
            assert!(Manifest::parse(json.as_bytes()).is_err(), "{json}");
            assert!(sign(json.as_bytes(), &private_key).is_err(), "{json}");
        }

        let too_large = vec![b' '; MAX_MANIFEST_BYTES + 1];
        assert!(Manifest::parse(&too_large).is_err());
    }

    #[test]
    fn test_unknown_fields_are_ignored() {
        // A later version of the format may add fields; an older application
        // must still read what it knows.
        let json = format!(
            r#"{{"product":"p","version":"1.2.3","notes":"x","files":[{{"name":"a.dmg","size":4,"sha256":"{HASH}","kind":"dmg"}}]}}"#
        );
        Manifest::parse(json.as_bytes()).unwrap();
    }

    #[test]
    fn test_describing_files_and_checking_them() {
        let directory = tempfile::tempdir().unwrap();
        let installer = directory
            .path()
            .join("example-ide-windows-x86_64-setup.exe");
        let image = directory.path().join("example-ide-macos-aarch64.dmg");
        std::fs::write(&installer, b"test").unwrap();
        std::fs::write(&image, b"another").unwrap();

        let manifest =
            Manifest::describe("example-ide", "0.2.0", &[installer.clone(), image.clone()])
                .unwrap();
        assert_eq!(
            manifest
                .files
                .iter()
                .map(|file| file.name.as_str())
                .collect::<Vec<_>>(),
            [
                "example-ide-macos-aarch64.dmg",
                "example-ide-windows-x86_64-setup.exe"
            ]
        );
        let described = manifest
            .file("example-ide-windows-x86_64-setup.exe")
            .unwrap();
        assert_eq!((described.size, described.sha256.as_str()), (4, HASH));

        // What is written is what is read back.
        let json = manifest.to_json().unwrap();
        assert_eq!(Manifest::parse(json.as_bytes()).unwrap(), manifest);

        assert_eq!(
            check_files(&manifest, directory.path()).unwrap(),
            Vec::<String>::new()
        );
        std::fs::remove_file(&image).unwrap();
        assert_eq!(
            check_files(&manifest, directory.path()).unwrap(),
            ["example-ide-macos-aarch64.dmg"]
        );
        std::fs::write(&installer, b"tesT").unwrap();
        assert!(check_files(&manifest, directory.path()).is_err());
    }

    #[test]
    fn test_public_key_lists() {
        let (_, first) = keys();
        let (_, second) = keys();
        assert_eq!(parse_public_keys("").unwrap(), Vec::<&str>::new());
        assert_eq!(parse_public_keys(&first).unwrap(), [first.as_str()]);
        assert_eq!(
            parse_public_keys(&format!("{first}, {second}")).unwrap(),
            [first.as_str(), second.as_str()]
        );
        assert!(parse_public_keys(&format!("{first},nonsense")).is_err());
    }
}
