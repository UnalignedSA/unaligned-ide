//! Every key used here is a throwaway from `tests/fixtures` (see the README
//! there). The values expected of a converted PuTTY key were recorded from
//! the real tools: the public key is the one `puttygen -O public-openssh`
//! prints, and the hash is that of the private part of PuTTYgen's own OpenSSH
//! export, as rewritten by `ssh-keygen -p`.

use std::fs;

use aes::cipher::{KeyIvInit as _, StreamCipher as _};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, Mac as _};
use sha1::Sha1;
use sha2::{Digest as _, Sha256};

use crate::wire::{Reader, put_mpint, put_string, put_u32};
use crate::{
    KeyError, KeyFormat, KeyInfo, KeySeed, generate_with_rounds, import, import_with_rounds,
    inspect, openssh, text_from_bytes, write_key_file,
};

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("../tests/fixtures/", $name))
    };
}

const PASSPHRASE: &str = "correct horse battery";
const UNICODE_PASSPHRASE: &str = "pässwörd ключ";
/// Far fewer rounds than real use: the key derivation is very slow in a
/// debug build.
const TEST_ROUNDS: u32 = 2;

struct PuttyFixture {
    name: &'static str,
    text: &'static str,
    passphrase: Option<&'static str>,
    algorithm: &'static str,
    comment: Option<&'static str>,
    /// What `puttygen -O public-openssh` prints for the file.
    public_key: &'static str,
    /// SHA-256 of the private section (between the check numbers and the
    /// padding) of PuTTYgen's own OpenSSH export, rewritten by `ssh-keygen -p`.
    private_section_sha256: &'static str,
}

const PUTTY_FIXTURES: &[PuttyFixture] = &[
    PuttyFixture {
        name: "putty_v3_ed25519.ppk",
        text: fixture!("putty_v3_ed25519.ppk"),
        passphrase: None,
        algorithm: "ED25519",
        comment: Some("clé de Zoë – 東京 laptop"),
        public_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGDSzOFdDe1KJbS1vqWoJ/L1jlr7HTLC5Wa72ho80Esu clé de Zoë – 東京 laptop",
        private_section_sha256: "aa1b01aeb6ac08d3d0c9377a77843f4b3570a37a7aa11d2e1c84c6311f5bc0d3",
    },
    PuttyFixture {
        name: "putty_v3_ed25519_encrypted.ppk",
        text: fixture!("putty_v3_ed25519_encrypted.ppk"),
        passphrase: Some(PASSPHRASE),
        algorithm: "ED25519",
        comment: Some("work laptop"),
        public_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIP4kxQGZM6hP4EBT2FHniztrgl22fKN+VCi/mD5o+j4S work laptop",
        private_section_sha256: "8f3529cf011367c7c87302f0439c90874b85f00bb285a17b7ef4d670bbe272aa",
    },
    PuttyFixture {
        name: "putty_v3_rsa_encrypted.ppk",
        text: fixture!("putty_v3_rsa_encrypted.ppk"),
        passphrase: Some(UNICODE_PASSPHRASE),
        algorithm: "RSA 2048",
        comment: Some("rsa key (Argon2i)"),
        public_key: "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQCVRCD0Ly4J1AZHlYd7ABnG+ZrDUltNCektQWQcDXLwmnoDBTOQhXqAh8SUBOgveFGarq9tJbGxUnD97oWWY7qB0jbiBL0PKp0n/uBe00n2xod3mKOsoiD0ox+K9lvboXcg/1AOfOCcpB6hFSP6Ai7Pj7CihQ9MXIWIjdaLdieRLQfaDPpb1LAUgBz4vEhlzVK+HdKChOyZttgWQh2XsnZRqfMYCUOVRvWHnVfiaOV2ex7qYtdspgrMr8G43taVjIAaH0q7kvIa7cLNOoTUMNS4x0EI6opVltuo9958rFMwnNQkCfIQlBKCm9YdVxQX3y4Fa2yWf/rDaqeYyQ9imHYV rsa key (Argon2i)",
        private_section_sha256: "eb32863014537fb62b62c24c3d45e3d94ab80f0ff9820a2c8f58ec8dd71a7776",
    },
    PuttyFixture {
        name: "putty_v3_ecdsa384_encrypted.ppk",
        text: fixture!("putty_v3_ecdsa384_encrypted.ppk"),
        passphrase: Some(PASSPHRASE),
        algorithm: "ECDSA P-384",
        comment: Some("ecdsa key (Argon2d)"),
        public_key: "ecdsa-sha2-nistp384 AAAAE2VjZHNhLXNoYTItbmlzdHAzODQAAAAIbmlzdHAzODQAAABhBI+GeSN3yrAgr0DtbCKg7DaBA7hF/M8tM4jvVAihKNkSnqFztJwjLTik/3LkWUbVIHHd59qmBmicEk4V/YZ+BL87f4D06st1lXniHilkbuSgeJUVbSJ4biBWxjV/AHIAMw== ecdsa key (Argon2d)",
        private_section_sha256: "07cd7fd0f50cbe64c37aa8c80f84eb990b331eed346498c1b5d9c80fd21df47b",
    },
    PuttyFixture {
        name: "putty_v3_ecdsa521.ppk",
        text: fixture!("putty_v3_ecdsa521.ppk"),
        passphrase: None,
        algorithm: "ECDSA P-521",
        comment: Some("ecdsa-key-20261114"),
        public_key: "ecdsa-sha2-nistp521 AAAAE2VjZHNhLXNoYTItbmlzdHA1MjEAAAAIbmlzdHA1MjEAAACFBACKpDzmBT5caDS3BcE9eqOTTjMzjdwfTU38Xczbtv4XLFePiNugx8KpSNtOtXvRKFTgZT1TBcX2xgjnjqRzCiZMIQHMLQ/wqS3uEGG4Fr5f1kZmNtNFfVA+dTNy/r7i57cByuMql3pGmRCbTvOVytNzoSLUO9PZFxHg6fn5Xc/YdfO3FA== ecdsa-key-20261114",
        private_section_sha256: "b7090a8c8820897f4ed416c66f01cb6af19674eabbff198d8394721fe21028df",
    },
    PuttyFixture {
        name: "putty_v2_rsa.ppk",
        text: fixture!("putty_v2_rsa.ppk"),
        passphrase: None,
        algorithm: "RSA 2048",
        comment: Some("rsa-key-20261114"),
        public_key: "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQCP3fXACgMVfHVjBvSP5DmapYa0WJ/6kWWVm0whZ9Zrig9yWvx4n9uQWlgNfo55IfME6qUKP9fufL5pNH2nrKXNLaUq5vjDBwt+b/6+IN7Ht0wihHe85g8wXl+sj6ZMqI0GLLA9/bQdeVlRDEH7FhoIoZyApLvc8PEPoe2ZzNksxX1n/8DfjbMdWDItbYmHXWqdU4l5KP1PYKwjcn2sc3cc7jjPjQQUFpsXTSB57Tp0djVw6/YcAkzxYUz1TPKdjdpBJ7SFpus9rNDcgAoZkMk8CWYEcqoI6j2KCyZLWNAjZUWd/veJGYvwMna+OCS1v+yycuyMtDE1yXZxGxXxknNP rsa-key-20261114",
        private_section_sha256: "87fca98ee7f580b411341c9cc2bcda1f9ef7979d7eb2dc42bb18fd08635ae3f9",
    },
    PuttyFixture {
        name: "putty_v2_ed25519_encrypted.ppk",
        text: fixture!("putty_v2_ed25519_encrypted.ppk"),
        passphrase: Some(PASSPHRASE),
        algorithm: "ED25519",
        comment: Some("old laptop"),
        public_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJGXlFjk/mM1LT/Y+OiSsdsb2UwD/iYWifgVDt/KkXd+ old laptop",
        private_section_sha256: "1ebf483f95005598e0dc03e09ff872ae5a88cfd0c4d814d2f86f57bd2e0062f0",
    },
    PuttyFixture {
        name: "putty_v2_ecdsa256.ppk",
        text: fixture!("putty_v2_ecdsa256.ppk"),
        passphrase: None,
        algorithm: "ECDSA P-256",
        comment: None,
        // PuTTYgen ends this line with a space when there is no comment.
        public_key: "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBDrs6Qobp6FRBtkfzrLCrRffoX1dzeXIofh3kUflOt7tVLG+0a7mKn5F/y7TpyD2Z5ra4ZqgQVKEdTR8AYjCmic=",
        private_section_sha256: "233e1dddda3e848831613a4016131f6694e35babfc1c5592fd2812adf9cd7616",
    },
    PuttyFixture {
        name: "putty_v2_dsa_encrypted.ppk",
        text: fixture!("putty_v2_dsa_encrypted.ppk"),
        passphrase: Some(PASSPHRASE),
        algorithm: "DSA",
        comment: Some("dsa key"),
        public_key: "ssh-dss AAAAB3NzaC1kc3MAAACBAPLs2PlSj680IWhmchVhjwTDeN0ZJLjN7CKp1NcxJgLZT1Ro7PAx3BbFSlGSWCE+lrgGxlbIwdquQp26RuXmYBKrHeyTd15wrzWTcNYe8nQv4IaPevUJXVzr3+FQ1v/RPWo8lTWPfCKwRccPSTULL783V+T2QhQO0i9t83jYSQJVAAAAFQC8EKbIewJY7HqB2N2pqQpE6BftEQAAAIEA5pmKFJXGmsnO1ObmDgMoWT6PfucLspj83H54l5HtZcA+dvo2A5ViORDF4wyZFI4nOGrKL5YUeystwoyBt8jy9XOkEefbO9ksmS07DjZEjVmN7+VyzzSY6k593SAaB1sEJRMPDJu6vRWvPtU1pO3fw9sJ4/LX6M3eTKr+BpLbmwIAAACBAKRx5uoR43+BdZC9pm40JmGjehbB7ymgjQvfKIhCDFXBRI1EJWr1ZX37peENWFl73o5gq+R69QZTGwuR70yrLFPn239GyWk6oIiDFdPwvrhT1oe7rK0qC7S+aVC9CM5TuKOBS+zOIRHc+yknmohnyOCQQ+mOvzbUArWCAobtiygW dsa key",
        private_section_sha256: "57f3773f73bbf6c312b08016f3eff26c7a3eb7bf5045ae4542af1ed21546c546",
    },
];

struct KeptFixture {
    name: &'static str,
    text: &'static str,
    format: KeyFormat,
    algorithm: Option<&'static str>,
    encrypted: bool,
    comment: Option<&'static str>,
    /// What `ssh-keygen` wrote in the `.pub` file next to the key. For an
    /// encrypted OpenSSH key the comment cannot be read, so it is not here.
    public_key: Option<&'static str>,
}

const KEPT_FIXTURES: &[KeptFixture] = &[
    KeptFixture {
        name: "openssh_ed25519.key",
        text: fixture!("openssh_ed25519.key"),
        format: KeyFormat::OpenSsh,
        algorithm: Some("ED25519"),
        encrypted: false,
        comment: Some("zoe@laptop"),
        public_key: Some(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAINUjVZAo0SICWBwt04Z1m1NppS6Fy3ItjaA6W3as/2N0 zoe@laptop",
        ),
    },
    KeptFixture {
        name: "openssh_ed25519_encrypted.key",
        text: fixture!("openssh_ed25519_encrypted.key"),
        format: KeyFormat::OpenSsh,
        algorithm: Some("ED25519"),
        encrypted: true,
        comment: None,
        public_key: Some(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOECQnbxAZWnDX5IAvkyZa6cevP9TZ9Tsuw5rjk8PD1G",
        ),
    },
    KeptFixture {
        name: "openssh_rsa.key",
        text: fixture!("openssh_rsa.key"),
        format: KeyFormat::OpenSsh,
        algorithm: Some("RSA 2048"),
        encrypted: false,
        comment: None,
        public_key: Some(
            "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQDw2iJmBo72JfmBZcT1DiqyM+oZ4x9ueR0nwZoiQk+V07w630dllZ1EcbqrK+NU4u44CNPQcVUxWtd745GTh4Uz1KIr19YSG3PLXogZbgFpP8XEZpV8+OXbQqR+TLCW9vGJ99ri6UeEw5SKvDSEJDr7Vfu8VCnQdS7oBlPwVp+HDdlS+/SlWntsokTFU4ykRB+7GVh7iPlsbqJ+2OXSNUHo3MhmSh6yo8h7n41BwVrSiIwiMwmDn33ptC2qbOTcPKqNqrmnuSY6gQFyp0rR8VR40T4Sy7788/M8nTPx+Gx50XOVmZUXd0PocJBYpG4v6Rdctu9oTzkQdTyftfVX+eoz",
        ),
    },
    KeptFixture {
        name: "openssh_ecdsa256.key",
        text: fixture!("openssh_ecdsa256.key"),
        format: KeyFormat::OpenSsh,
        algorithm: Some("ECDSA P-256"),
        encrypted: false,
        comment: Some("deploy key"),
        public_key: Some(
            "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBHLnTVoq6y52ft780paC+NrvInpsSq4VT+KRH5tZ3hSV1bcGc2PydeP5LL1Rv/TxPbXNeUS2F3N7kM+0Xg8WoE8= deploy key",
        ),
    },
    KeptFixture {
        name: "pem_rsa.pem",
        text: fixture!("pem_rsa.pem"),
        format: KeyFormat::Pem,
        algorithm: Some("RSA 3072"),
        encrypted: false,
        comment: None,
        public_key: None,
    },
    KeptFixture {
        name: "pem_rsa_encrypted.pem",
        text: fixture!("pem_rsa_encrypted.pem"),
        format: KeyFormat::Pem,
        algorithm: Some("RSA"),
        encrypted: true,
        comment: None,
        public_key: None,
    },
    KeptFixture {
        name: "pem_ec.pem",
        text: fixture!("pem_ec.pem"),
        format: KeyFormat::Pem,
        algorithm: Some("ECDSA P-256"),
        encrypted: false,
        comment: None,
        public_key: None,
    },
    KeptFixture {
        // Written by an ssh-keygen built with LibreSSL, which spells the
        // curve out as numbers instead of naming it.
        name: "pem_ec_explicit_curve.pem",
        text: fixture!("pem_ec_explicit_curve.pem"),
        format: KeyFormat::Pem,
        algorithm: Some("ECDSA"),
        encrypted: false,
        comment: None,
        public_key: None,
    },
    KeptFixture {
        name: "pem_dsa.pem",
        text: fixture!("pem_dsa.pem"),
        format: KeyFormat::Pem,
        algorithm: Some("DSA"),
        encrypted: false,
        comment: None,
        public_key: None,
    },
    KeptFixture {
        name: "pem_pkcs8_rsa.pem",
        text: fixture!("pem_pkcs8_rsa.pem"),
        format: KeyFormat::Pem,
        algorithm: Some("RSA 2048"),
        encrypted: false,
        comment: None,
        public_key: None,
    },
    KeptFixture {
        name: "pem_pkcs8_ecdsa384.pem",
        text: fixture!("pem_pkcs8_ecdsa384.pem"),
        format: KeyFormat::Pem,
        algorithm: Some("ECDSA P-384"),
        encrypted: false,
        comment: None,
        public_key: None,
    },
    KeptFixture {
        name: "pem_pkcs8_encrypted.pem",
        text: fixture!("pem_pkcs8_encrypted.pem"),
        format: KeyFormat::Pem,
        algorithm: None,
        encrypted: true,
        comment: None,
        public_key: None,
    },
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn import_error(text: &str, passphrase: Option<&str>) -> KeyError {
    match import_with_rounds(text, passphrase, TEST_ROUNDS) {
        Ok(_) => panic!("the key was accepted"),
        Err(error) => error,
    }
}

/// Both entry points must say the same thing about a text that is refused
/// before any passphrase matters.
fn refused(text: &str) -> KeyError {
    let error = inspect(text).expect_err("inspect accepted the text");
    assert_eq!(import_error(text, None), error);
    error
}

fn assert_damaged(text: &str) {
    let error = refused(text);
    assert!(
        matches!(error, KeyError::Damaged(_)),
        "expected Damaged, got {error:?}"
    );
}

fn assert_unsupported(text: &str, mention: &str) {
    match refused(text) {
        KeyError::Unsupported(sentence) => {
            assert!(
                sentence.contains(mention),
                "{sentence:?} does not mention {mention:?}"
            )
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

/// An armored key split into its first line, its header lines, its base64
/// body in one piece, and its last line.
struct Pieces {
    begin: String,
    headers: Vec<String>,
    body: String,
    end: String,
}

fn pieces(text: &str) -> Pieces {
    let lines: Vec<&str> = text.lines().collect();
    let (begin, rest) = lines.split_first().unwrap();
    let (end, middle) = rest.split_last().unwrap();
    Pieces {
        begin: begin.to_string(),
        headers: middle
            .iter()
            .filter(|line| line.contains(':'))
            .map(|line| line.to_string())
            .collect(),
        body: middle
            .iter()
            .filter(|line| !line.contains(':'))
            .copied()
            .collect(),
        end: end.to_string(),
    }
}

impl Pieces {
    /// The key again, its body cut every `width` characters, its lines ended
    /// with `line_end`.
    fn wrapped(&self, width: usize, line_end: &str) -> String {
        let mut lines = vec![self.begin.clone()];
        if !self.headers.is_empty() {
            lines.extend(self.headers.iter().cloned());
            lines.push(String::new());
        }
        lines.extend(
            self.body
                .as_bytes()
                .chunks(width)
                .map(|chunk| String::from_utf8(chunk.to_vec()).unwrap()),
        );
        lines.push(self.end.clone());
        lines.join(line_end) + line_end
    }
}

/// The parts of an OpenSSH private key file, for checking what `import`
/// wrote without the help of the code under test.
struct OpenSshFile {
    cipher: String,
    kdf: String,
    kdf_options: Vec<u8>,
    public_blob: Vec<u8>,
    private_section: Vec<u8>,
}

fn read_openssh_file(text: &str) -> OpenSshFile {
    assert!(text.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----\n"));
    assert!(text.ends_with("\n-----END OPENSSH PRIVATE KEY-----\n"));
    let body = pieces(text).body;
    let data = STANDARD.decode(body).unwrap();
    let mut reader = Reader::new(data.strip_prefix(b"openssh-key-v1\0").unwrap());
    let cipher = reader.text().unwrap().to_string();
    let kdf = reader.text().unwrap().to_string();
    let kdf_options = reader.string().unwrap().to_vec();
    assert_eq!(reader.u32(), Some(1));
    let public_blob = reader.string().unwrap().to_vec();
    let private_section = reader.string().unwrap().to_vec();
    assert!(reader.is_empty());
    OpenSshFile {
        cipher,
        kdf,
        kdf_options,
        public_blob,
        private_section,
    }
}

impl OpenSshFile {
    /// The private section in the clear, without its check numbers and its
    /// padding. `None` when the passphrase does not open it.
    fn private_key(&self, passphrase: Option<&str>) -> Option<Vec<u8>> {
        let mut section = self.private_section.clone();
        let block = match self.cipher.as_str() {
            "none" => {
                assert_eq!(self.kdf, "none");
                assert!(self.kdf_options.is_empty());
                8
            }
            "aes256-ctr" => {
                assert_eq!(self.kdf, "bcrypt");
                let mut options = Reader::new(&self.kdf_options);
                let salt = options.string().unwrap();
                let rounds = options.u32().unwrap();
                assert_eq!(salt.len(), 16);
                let mut key_and_iv = [0u8; 48];
                bcrypt_pbkdf::bcrypt_pbkdf(passphrase.unwrap(), salt, rounds, &mut key_and_iv)
                    .unwrap();
                let mut cipher = ctr::Ctr128BE::<aes::Aes256>::new_from_slices(
                    &key_and_iv[..32],
                    &key_and_iv[32..],
                )
                .unwrap();
                cipher.apply_keystream(&mut section);
                16
            }
            other => panic!("unexpected cipher {other}"),
        };
        assert_eq!(section.len() % block, 0);
        if section[..4] != section[4..8] {
            return None;
        }
        let padding = usize::from(*section.last().unwrap());
        let padding =
            if padding < block && section.ends_with(&(1..=padding as u8).collect::<Vec<u8>>()) {
                padding
            } else {
                0
            };
        Some(section[8..section.len() - padding].to_vec())
    }

    fn rounds(&self) -> u32 {
        let mut options = Reader::new(&self.kdf_options);
        options.string().unwrap();
        options.u32().unwrap()
    }
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn armor_openssh(data: &[u8]) -> String {
    let encoded = STANDARD.encode(data);
    let mut text = String::from("-----BEGIN OPENSSH PRIVATE KEY-----\n");
    for line in encoded.as_bytes().chunks(70) {
        text.push_str(std::str::from_utf8(line).unwrap());
        text.push('\n');
    }
    text.push_str("-----END OPENSSH PRIVATE KEY-----\n");
    text
}

/// The bytes inside an OpenSSH key file, changed by `change`, as a file again.
fn openssh_with(text: &str, change: impl FnOnce(&mut Vec<u8>)) -> String {
    let mut data = STANDARD.decode(pieces(text).body).unwrap();
    change(&mut data);
    armor_openssh(&data)
}

/// An unencrypted version 2 PuTTY file with a valid MAC, for contents
/// PuTTYgen cannot be made to write.
fn putty_v2_file(
    algorithm: &str,
    comment: &str,
    public_blob: &[u8],
    private_blob: &[u8],
) -> String {
    let mac_key = Sha1::digest(b"putty-private-key-file-mac-key");
    let mut mac = Hmac::<Sha1>::new_from_slice(&mac_key).unwrap();
    for part in [
        algorithm.as_bytes(),
        b"none",
        comment.as_bytes(),
        public_blob,
        private_blob,
    ] {
        mac.update(&(part.len() as u32).to_be_bytes());
        mac.update(part);
    }
    let mac: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let lines = |blob: &[u8]| -> Vec<String> {
        STANDARD
            .encode(blob)
            .as_bytes()
            .chunks(64)
            .map(|chunk| String::from_utf8(chunk.to_vec()).unwrap())
            .collect()
    };
    let public_lines = lines(public_blob);
    let private_lines = lines(private_blob);
    format!(
        "PuTTY-User-Key-File-2: {algorithm}\nEncryption: none\nComment: {comment}\nPublic-Lines: {}\n{}\nPrivate-Lines: {}\n{}\nPrivate-MAC: {mac}\n",
        public_lines.len(),
        public_lines.join("\n"),
        private_lines.len(),
        private_lines.join("\n"),
    )
}

fn replace_line(text: &str, prefix: &str, replacement: &str) -> String {
    let mut found = false;
    let lines: Vec<&str> = text
        .lines()
        .map(|line| {
            if line.starts_with(prefix) {
                found = true;
                replacement
            } else {
                line
            }
        })
        .collect();
    assert!(found, "no line starts with {prefix:?}");
    lines.join("\n") + "\n"
}

// ---------------------------------------------------------------------------
// What is not a private key
// ---------------------------------------------------------------------------

#[test]
fn empty_text() {
    for text in ["", " ", "\n\n", "\r\n  \t\r\n", "\u{feff}", "\u{feff}\r\n"] {
        assert_eq!(refused(text), KeyError::Empty, "{text:?}");
    }
}

#[test]
fn public_keys() {
    let public_file = fixture!("openssh_ed25519.pub");
    let blob = public_file.split(' ').nth(1).unwrap();
    let texts = [
        public_file.to_string(),
        public_file.replace('\n', "\r\n"),
        // A line of `authorized_keys`, with its options.
        format!("command=\"/usr/bin/true\",no-pty {public_file}"),
        // Two lines of `authorized_keys`.
        format!("# my keys\n{public_file}{}\n", PUTTY_FIXTURES[5].public_key),
        // Only the middle of the line.
        blob.to_string(),
        PUTTY_FIXTURES[8].public_key.to_string(),
        "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEA1SNVkCjRIgJYHC3ThnWbU2mlLoXLci2NoDpbdqz/Y3Q=\n-----END PUBLIC KEY-----\n"
            .to_string(),
        "-----BEGIN RSA PUBLIC KEY-----\nMAA=\n-----END RSA PUBLIC KEY-----\n".to_string(),
        format!(
            "---- BEGIN SSH2 PUBLIC KEY ----\nComment: \"zoe@laptop\"\n{blob}\n---- END SSH2 PUBLIC KEY ----\n"
        ),
    ];
    for text in texts {
        assert_eq!(refused(&text), KeyError::PublicKey, "{text:?}");
    }
}

#[test]
fn things_that_are_not_keys() {
    let texts = [
        fixture!("openssh_ed25519-cert.pub").to_string(),
        "-----BEGIN CERTIFICATE-----\nMAA=\n-----END CERTIFICATE-----\n".to_string(),
        "-----BEGIN PGP PRIVATE KEY BLOCK-----\n\nlQOYBF\n-----END PGP PRIVATE KEY BLOCK-----\n"
            .to_string(),
        "hunter2".to_string(),
        "Hello,\n\nPlease find my key attached.\n".to_string(),
        "MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQCba77VmO0cMA6q".to_string(),
        "Host example\n  HostName example.com\n  User zoe\n  IdentityFile ~/.ssh/id_ed25519\n"
            .to_string(),
    ];
    for text in texts {
        assert_eq!(refused(&text), KeyError::NotAKey, "{text:?}");
    }
}

#[test]
fn a_private_key_wins_over_what_is_pasted_with_it() {
    let private_key = fixture!("openssh_ed25519.key");
    let text = format!(
        "{}\n{private_key}\n-----BEGIN CERTIFICATE-----\nMAA=\n-----END CERTIFICATE-----\n",
        fixture!("openssh_ed25519.pub")
    );
    assert_eq!(import(&text, None).unwrap().text, private_key);
}

#[test]
fn every_error_is_one_plain_sentence() {
    let mut errors = vec![
        KeyError::Empty,
        KeyError::PublicKey,
        KeyError::NotAKey,
        KeyError::NeedsPassphrase,
        KeyError::WrongPassphrase,
    ];
    let refused_texts = [
        fixture!("putty_v3_ed448.ppk").to_string(),
        "PuTTY-User-Key-File-1: ssh-rsa\nEncryption: none\n".to_string(),
        "PuTTY-User-Key-File-4: ssh-rsa\nEncryption: none\n".to_string(),
        "---- BEGIN SSH2 ENCRYPTED PRIVATE KEY ----\nAAAA\n---- END SSH2 ENCRYPTED PRIVATE KEY ----\n".to_string(),
        "-----BEGIN FANCY PRIVATE KEY-----\nMAA=\n-----END FANCY PRIVATE KEY-----\n".to_string(),
        replace_line(fixture!("putty_v3_rsa_encrypted.ppk"), "Argon2-Memory:", "Argon2-Memory: 4194304"),
        replace_line(fixture!("putty_v3_rsa_encrypted.ppk"), "Key-Derivation:", "Key-Derivation: Scrypt"),
        replace_line(fixture!("putty_v2_rsa.ppk"), "Encryption:", "Encryption: des-cbc"),
        replace_line(fixture!("putty_v2_rsa.ppk"), "Private-MAC:", "Private-MAC: 00"),
        replace_line(fixture!("putty_v2_rsa.ppk"), "Private-Lines:", "Private-Lines: 99"),
        fixture!("openssh_ed25519.key").replace("-----BEGIN OPENSSH PRIVATE KEY-----\n", ""),
        fixture!("openssh_ed25519.key").replace("-----END OPENSSH PRIVATE KEY-----\n", ""),
        fixture!("openssh_ed25519.key").replace("QyNTUx", "QyN!Ux"),
        fixture!("pem_ec.pem").replace("AwEHoUQDQgAEMJev+WoafSO1qNbjq18YSQEsufiwpNRmgAAeyrgODeZrfnA2a/O2\n", ""),
        "-----BEGIN OPENSSH PRIVATE KEY-----\n-----END OPENSSH PRIVATE KEY-----\n".to_string(),
        "-----BEGIN OPENSSH PRIVATE KEY".to_string(),
    ];
    for text in &refused_texts {
        let error = refused(text);
        assert!(
            matches!(error, KeyError::Unsupported(_) | KeyError::Damaged(_)),
            "{text:?} gave {error:?}"
        );
        errors.push(error);
    }
    for error in errors {
        let sentence = error.to_string();
        assert!(sentence.ends_with('.'), "{sentence:?}");
        assert!(
            sentence.chars().next().unwrap().is_uppercase(),
            "{sentence:?}"
        );
        assert!(!sentence.contains('\n'), "{sentence:?}");
        assert!(sentence.len() > 30 && sentence.len() < 200, "{sentence:?}");
        // One sentence: the only full stops allowed inside are those of a
        // file name in quotes, such as ".pub".
        let inner_stops = sentence[..sentence.len() - 1].matches(". ").count();
        assert!(inner_stops <= 1, "{sentence:?}");
    }
    assert_eq!(
        KeyError::PublicKey.to_string(),
        "This is the public key. Choose the private key: the file without \".pub\"."
    );
    let boxed: Box<dyn std::error::Error> = Box::new(KeyError::NotAKey);
    assert!(boxed.to_string().contains("not an SSH private key"));
}

// ---------------------------------------------------------------------------
// Keys that are kept: OpenSSH and PEM
// ---------------------------------------------------------------------------

#[test]
fn kept_keys_are_described() {
    for fixture in KEPT_FIXTURES {
        let expected = KeyInfo {
            format: fixture.format,
            algorithm: fixture.algorithm.map(str::to_string),
            encrypted: fixture.encrypted,
            comment: fixture.comment.map(str::to_string),
        };
        assert_eq!(
            inspect(fixture.text),
            Ok(expected.clone()),
            "{}",
            fixture.name
        );

        let imported = import(fixture.text, None).unwrap();
        assert_eq!(imported.info, expected, "{}", fixture.name);
        assert!(!imported.converted, "{}", fixture.name);
        assert_eq!(
            imported.public_key.as_deref(),
            fixture.public_key,
            "{}",
            fixture.name
        );
        // A passphrase is of no use for these, and does no harm.
        assert_eq!(
            import(fixture.text, Some("unused")).unwrap().text,
            fixture.text
        );
    }
    assert_eq!(
        import(fixture!("openssh_ed25519.key"), None)
            .unwrap()
            .public_key
            .unwrap(),
        fixture!("openssh_ed25519.pub").trim_end()
    );
}

#[test]
fn a_tidy_key_comes_back_byte_for_byte() {
    for fixture in KEPT_FIXTURES {
        assert_eq!(
            import(fixture.text, None).unwrap().text,
            fixture.text,
            "{}",
            fixture.name
        );
    }
}

#[test]
fn mangled_keys_are_tidied_up() {
    for fixture in KEPT_FIXTURES {
        let text = fixture.text;
        let pieces = pieces(text);
        let lines: Vec<&str> = text.lines().collect();
        let mut variants = vec![
            ("CRLF", text.replace('\n', "\r\n")),
            ("CR", text.replace('\n', "\r")),
            ("byte order mark", format!("\u{feff}{text}")),
            (
                "byte order mark and CRLF",
                format!("\u{feff}{}", text.replace('\n', "\r\n")),
            ),
            ("no last line end", text.trim_end().to_string()),
            ("blank lines around", format!("\n\n   \n{text}\n\n\n")),
            ("blank lines between", lines.join("\n\n") + "\n"),
            (
                "spaces around lines",
                format!("  {}  \n", lines.join(" \t\n   ")),
            ),
            (
                "text around",
                format!(
                    "Hi,\n\nHere is the key you asked for:\n\n{text}\n-- \nZoë\nSent from my phone\n"
                ),
            ),
            ("body on one line", pieces.wrapped(usize::MAX, "\n")),
            ("body on one line, CRLF", pieces.wrapped(usize::MAX, "\r\n")),
            ("body cut at 61", pieces.wrapped(61, "\n")),
            ("body cut at 100", pieces.wrapped(100, "\r\n")),
            (
                "last dashes lost",
                text.trim_end().trim_end_matches('-').to_string(),
            ),
        ];
        if fixture.format == KeyFormat::Pem {
            // Regular, but not a width every PEM reader takes.
            variants.push(("body cut at 40", pieces.wrapped(40, "\n")));
            variants.push(("body cut at 76", pieces.wrapped(76, "\n")));
        }
        if pieces.headers.is_empty() {
            // What a chat tool makes of a key: one line, spaces for line breaks.
            variants.push(("everything on one line", lines.join(" ")));
        }
        for (what, variant) in variants {
            let imported = import(&variant, None)
                .unwrap_or_else(|error| panic!("{}, {what}: {error:?}", fixture.name));
            assert_eq!(imported.text, text, "{}, {what}", fixture.name);
            assert_eq!(
                imported.public_key.as_deref(),
                fixture.public_key,
                "{}, {what}",
                fixture.name
            );
            assert_eq!(inspect(&variant), inspect(text), "{}, {what}", fixture.name);
        }
    }
}

#[test]
fn a_regular_openssh_body_of_another_width_is_kept() {
    // OpenSSH reads its own format at any line length, and other tools write
    // it 64 or 76 characters wide: such a file is not rewritten.
    let pieces = pieces(fixture!("openssh_rsa.key"));
    for width in [64, 76] {
        let text = pieces.wrapped(width, "\n");
        assert_eq!(import(&text, None).unwrap().text, text);
        assert_eq!(
            import(&pieces.wrapped(width, "\r\n"), None).unwrap().text,
            text
        );
    }
}

#[test]
fn pem_header_lines_are_kept_as_they_are() {
    let text = fixture!("pem_rsa_encrypted.pem");
    let imported = import(&pieces(text).wrapped(48, "\r\n"), None).unwrap();
    assert_eq!(imported.text, text);
    let lines: Vec<&str> = imported.text.lines().collect();
    assert_eq!(lines[1], "Proc-Type: 4,ENCRYPTED");
    assert_eq!(
        lines[2],
        "DEK-Info: AES-128-CBC,EC6DF03CE1CBFB52232D956167CAE965"
    );
    assert_eq!(lines[3], "");
    assert!(
        lines[4..lines.len() - 2]
            .iter()
            .all(|line| line.len() == 64)
    );
}

#[test]
fn pkcs8_ed25519_is_named() {
    // PrivateKeyInfo { version 0, algorithm Ed25519, OCTET STRING { OCTET STRING seed } }
    let mut der = vec![
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20,
    ];
    der.extend(0u8..32);
    let text = format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
        STANDARD.encode(&der)
    );
    let info = inspect(&text).unwrap();
    assert_eq!(info.format, KeyFormat::Pem);
    assert_eq!(info.algorithm.as_deref(), Some("ED25519"));
    assert!(!info.encrypted);
    assert_eq!(import(&text, None).unwrap().text, text);
}

#[test]
fn incomplete_or_changed_kept_keys() {
    for fixture in KEPT_FIXTURES {
        let text = fixture.text;
        let lines: Vec<&str> = text.lines().collect();
        let (first, last) = (lines[0], lines[lines.len() - 1]);

        // The first line was not selected.
        assert_damaged(&lines[1..].join("\n"));
        // The last line was not selected.
        assert_damaged(&lines[..lines.len() - 1].join("\n"));
        // Only the body was selected.
        if fixture.format == KeyFormat::OpenSsh {
            assert_damaged(&lines[1..lines.len() - 1].join("\n"));
        }
        // Nothing between the first and last lines.
        assert_damaged(&format!("{first}\n{last}\n"));
        // A character that base64 does not have.
        let body_line = lines.len() - 2;
        let mut changed = lines.clone();
        let broken = changed[body_line].replacen(|_: char| true, "!", 1);
        changed[body_line] = &broken;
        assert_damaged(&changed.join("\n"));
        // The last characters of the body were lost.
        let mut changed = lines.clone();
        let shorter = &lines[body_line][..lines[body_line].len() - 1];
        changed[body_line] = shorter;
        assert_damaged(&changed.join("\n"));
    }
}

#[test]
fn a_cut_body_is_damaged() {
    // Whole lines lost from the middle or the end of the body: the base64 is
    // still valid, what it holds is not. The old encrypted PEM form is cipher
    // text all through, so nothing can tell there.
    for fixture in KEPT_FIXTURES {
        if fixture.name == "pem_rsa_encrypted.pem" {
            continue;
        }
        let lines: Vec<&str> = fixture.text.lines().collect();
        if lines.len() < 5 {
            continue;
        }
        let mut without_last_body_lines = lines.clone();
        without_last_body_lines.drain(lines.len() - 3..lines.len() - 1);
        assert_damaged(&without_last_body_lines.join("\n"));
        let mut without_a_middle_line = lines.clone();
        without_a_middle_line.remove(2);
        assert_damaged(&without_a_middle_line.join("\n"));
    }
}

#[test]
fn damaged_openssh_containers() {
    let text = fixture!("openssh_ed25519.key");
    // Not an OpenSSH key inside.
    assert_damaged(&openssh_with(text, |data| data[3] ^= 1));
    // The two check numbers differ.
    let section_start = {
        let file = read_openssh_file(text);
        STANDARD.decode(pieces(text).body).unwrap().len() - file.private_section.len()
    };
    assert_damaged(&openssh_with(text, |data| data[section_start] ^= 1));
    // The padding is not 1, 2, 3…
    assert_damaged(&openssh_with(text, |data| *data.last_mut().unwrap() ^= 1));
    // Bytes after the end.
    assert_damaged(&openssh_with(text, |data| data.push(0)));
    // The private part is of another type than the public part.
    assert_damaged(&openssh_with(text, |data| {
        data[section_start + 8 + 4 + 4] ^= 1
    }));

    // An encrypted key whose private part is not a whole number of blocks.
    let encrypted = fixture!("openssh_ed25519_encrypted.key");
    assert_damaged(&openssh_with(encrypted, |data| {
        let length = data.len();
        data.truncate(length - 1);
    }));
    assert_damaged(&openssh_with(encrypted, |data| data.push(0)));

    // Two keys in one file: OpenSSH itself does not read these.
    let two_keys = openssh_with(text, |data| {
        let position = b"openssh-key-v1\0".len() + 4 + 4 + 4 + 4 + 4;
        assert_eq!(data[position..position + 4], [0, 0, 0, 1]);
        data[position + 3] = 2;
    });
    assert_unsupported(&two_keys, "2 keys");
}

#[test]
fn openssh_keys_of_other_kinds() {
    let comment = "token key";
    // A key held by a hardware token: its type, public key, application,
    // flags, key handle and a reserved field.
    let mut public_blob = Vec::new();
    put_string(&mut public_blob, b"sk-ssh-ed25519@openssh.com");
    put_string(&mut public_blob, &[7; 32]);
    put_string(&mut public_blob, b"ssh:");
    let mut private_key = Vec::new();
    put_string(&mut private_key, b"sk-ssh-ed25519@openssh.com");
    put_string(&mut private_key, &[7; 32]);
    put_string(&mut private_key, b"ssh:");
    private_key.push(1);
    put_string(&mut private_key, &[9; 96]);
    put_string(&mut private_key, b"");
    let text = openssh::write(&public_blob, &private_key, comment, None, TEST_ROUNDS).unwrap();
    let info = inspect(&text).unwrap();
    assert_eq!(info.algorithm.as_deref(), Some("ED25519-SK"));
    assert_eq!(info.comment.as_deref(), Some(comment));
    assert_eq!(import(&text, None).unwrap().text, text);

    // A type this code has never heard of is kept, without its comment.
    let mut public_blob = Vec::new();
    put_string(&mut public_blob, b"ssh-future@example.org");
    put_string(&mut public_blob, &[1; 40]);
    let mut private_key = Vec::new();
    put_string(&mut private_key, b"ssh-future@example.org");
    private_key.extend_from_slice(&[5; 77]);
    let text = openssh::write(&public_blob, &private_key, comment, None, TEST_ROUNDS).unwrap();
    let info = inspect(&text).unwrap();
    assert_eq!(info.algorithm.as_deref(), Some("ssh-future@example.org"));
    assert_eq!(info.comment, None);
    assert_eq!(import(&text, None).unwrap().text, text);

    // A cipher this code has never heard of is kept too.
    let encrypted = openssh_with(fixture!("openssh_ed25519_encrypted.key"), |data| {
        let position = b"openssh-key-v1\0".len() + 4;
        assert_eq!(&data[position..position + 10], b"aes256-ctr");
        data[position..position + 10].copy_from_slice(b"future-256");
    });
    assert!(inspect(&encrypted).unwrap().encrypted);
}

// ---------------------------------------------------------------------------
// PuTTY keys
// ---------------------------------------------------------------------------

#[test]
fn putty_keys_are_described() {
    for fixture in PUTTY_FIXTURES {
        let info = inspect(fixture.text).unwrap();
        assert_eq!(info.format, KeyFormat::Putty, "{}", fixture.name);
        assert_eq!(
            info.algorithm.as_deref(),
            Some(fixture.algorithm),
            "{}",
            fixture.name
        );
        assert_eq!(
            info.encrypted,
            fixture.passphrase.is_some(),
            "{}",
            fixture.name
        );
        assert_eq!(info.comment.as_deref(), fixture.comment, "{}", fixture.name);
    }
}

#[test]
fn putty_keys_are_converted() {
    for fixture in PUTTY_FIXTURES {
        let imported = import_with_rounds(fixture.text, fixture.passphrase, TEST_ROUNDS)
            .unwrap_or_else(|error| panic!("{}: {error:?}", fixture.name));
        assert!(imported.converted, "{}", fixture.name);
        assert_eq!(
            imported.info,
            inspect(fixture.text).unwrap(),
            "{}",
            fixture.name
        );
        assert_eq!(
            imported.public_key.as_deref(),
            Some(fixture.public_key),
            "{}",
            fixture.name
        );

        // The result is an OpenSSH key this crate reads back as the same key…
        let converted = inspect(&imported.text).unwrap();
        assert_eq!(converted.format, KeyFormat::OpenSsh, "{}", fixture.name);
        assert_eq!(
            converted.algorithm.as_deref(),
            Some(fixture.algorithm),
            "{}",
            fixture.name
        );
        assert_eq!(
            converted.encrypted,
            fixture.passphrase.is_some(),
            "{}",
            fixture.name
        );
        let again = import(&imported.text, None).unwrap();
        assert_eq!(again.text, imported.text, "{}", fixture.name);
        assert!(!again.converted);
        let (key_type, blob) = {
            let mut words = fixture.public_key.split(' ');
            (words.next().unwrap(), words.next().unwrap())
        };
        assert_eq!(
            again.public_key.unwrap().split(' ').nth(1),
            Some(blob),
            "{}",
            fixture.name
        );

        // …and, read without this crate, holds exactly what PuTTYgen's own
        // export holds.
        let file = read_openssh_file(&imported.text);
        assert_eq!(STANDARD.encode(&file.public_blob), blob, "{}", fixture.name);
        let private_key = match fixture.passphrase {
            Some(passphrase) => {
                // A protected key stays protected, by the same passphrase.
                assert_eq!(file.cipher, "aes256-ctr", "{}", fixture.name);
                assert_eq!(file.kdf, "bcrypt", "{}", fixture.name);
                assert_eq!(converted.comment, None, "{}", fixture.name);
                assert_eq!(
                    file.private_key(Some("another passphrase")),
                    None,
                    "{}",
                    fixture.name
                );
                file.private_key(Some(passphrase))
            }
            None => {
                assert_eq!(file.cipher, "none", "{}", fixture.name);
                assert_eq!(
                    converted.comment.as_deref(),
                    fixture.comment,
                    "{}",
                    fixture.name
                );
                file.private_key(None)
            }
        }
        .unwrap_or_else(|| panic!("{}: the converted key does not open", fixture.name));
        assert_eq!(
            sha256_hex(&private_key),
            fixture.private_section_sha256,
            "{}",
            fixture.name
        );

        let mut reader = Reader::new(&private_key);
        assert_eq!(reader.text(), Some(key_type), "{}", fixture.name);
        assert!(private_key.ends_with(fixture.comment.unwrap_or_default().as_bytes()));
    }
}

#[test]
fn a_converted_key_is_protected_as_ssh_keygen_does() {
    let fixture = &PUTTY_FIXTURES[6];
    assert_eq!(fixture.name, "putty_v2_ed25519_encrypted.ppk");
    let imported = import(fixture.text, fixture.passphrase).unwrap();
    let file = read_openssh_file(&imported.text);
    assert_eq!(
        (file.cipher.as_str(), file.kdf.as_str()),
        ("aes256-ctr", "bcrypt")
    );
    assert_eq!(file.rounds(), 24);
    assert_eq!(
        sha256_hex(&file.private_key(fixture.passphrase).unwrap()),
        fixture.private_section_sha256
    );
    assert!(imported.text.lines().all(|line| line.len() <= 70));
    assert!(
        imported
            .text
            .ends_with("-----END OPENSSH PRIVATE KEY-----\n")
    );

    // Each conversion draws a new salt: the same key never gives the same
    // file twice, and no part of the clear key shows in it.
    let second = import_with_rounds(fixture.text, fixture.passphrase, TEST_ROUNDS).unwrap();
    assert_ne!(second.text, imported.text);
    let clear = file.private_key(fixture.passphrase).unwrap();
    assert!(
        !file
            .private_section
            .windows(16)
            .any(|window| clear.windows(16).any(|clear| clear == window))
    );
}

#[test]
fn an_encrypted_putty_key_needs_its_passphrase() {
    for fixture in PUTTY_FIXTURES
        .iter()
        .filter(|fixture| fixture.passphrase.is_some())
    {
        assert_eq!(
            import_error(fixture.text, None),
            KeyError::NeedsPassphrase,
            "{}",
            fixture.name
        );
        assert_eq!(
            import_error(fixture.text, Some("")),
            KeyError::NeedsPassphrase,
            "{}",
            fixture.name
        );
        // Looking at it needs none.
        assert!(inspect(fixture.text).unwrap().encrypted, "{}", fixture.name);
    }
}

#[test]
fn wrong_passphrase() {
    for name in [
        "putty_v2_ed25519_encrypted.ppk",
        "putty_v2_dsa_encrypted.ppk",
        "putty_v3_rsa_encrypted.ppk",
        "putty_v3_ecdsa384_encrypted.ppk",
    ] {
        let fixture = PUTTY_FIXTURES
            .iter()
            .find(|fixture| fixture.name == name)
            .unwrap();
        let passphrase = fixture.passphrase.unwrap();
        for wrong in [
            "wrong",
            &passphrase.to_uppercase(),
            &format!("{passphrase} "),
            &passphrase[1..],
        ] {
            assert_eq!(
                import_error(fixture.text, Some(wrong)),
                KeyError::WrongPassphrase,
                "{name}, {wrong:?}"
            );
        }
    }
}

#[test]
fn a_passphrase_is_ignored_for_an_unencrypted_putty_key() {
    let fixture = &PUTTY_FIXTURES[0];
    let imported = import(fixture.text, Some("not needed")).unwrap();
    let file = read_openssh_file(&imported.text);
    assert_eq!(file.cipher, "none");
    assert_eq!(
        sha256_hex(&file.private_key(None).unwrap()),
        fixture.private_section_sha256
    );
}

#[test]
fn changed_putty_keys() {
    let plain = fixture!("putty_v3_ed25519.ppk");
    let plain_v2 = fixture!("putty_v2_rsa.ppk");
    let flip_last_digit = |text: &str| {
        let text = text.trim_end();
        let last = if text.ends_with('0') { '1' } else { '0' };
        format!("{}{last}\n", &text[..text.len() - 1])
    };

    // The MAC itself, the comment, the public part, the private part: any
    // change to an unencrypted key shows.
    assert_damaged(&flip_last_digit(plain));
    assert_damaged(&flip_last_digit(plain_v2));
    assert_damaged(&replace_line(
        plain,
        "Comment:",
        "Comment: someone else's laptop",
    ));
    assert_damaged(&replace_line(
        plain_v2,
        "Comment:",
        "Comment: rsa-key-20261115",
    ));
    let lines: Vec<&str> = plain_v2.lines().collect();
    let public_line = 5;
    let private_line = lines
        .iter()
        .position(|line| line.starts_with("Private-Lines:"))
        .unwrap()
        + 2;
    for line in [public_line, private_line] {
        let mut changed = lines.clone();
        let swapped: String = {
            let mut characters: Vec<char> = lines[line].chars().collect();
            characters.swap(10, 11);
            assert_ne!(characters[10], characters[11]);
            characters.into_iter().collect()
        };
        changed[line] = &swapped;
        assert_damaged(&changed.join("\n"));
    }

    // With an encrypted key the MAC cannot be told from a wrong passphrase.
    let encrypted = PUTTY_FIXTURES
        .iter()
        .find(|fixture| fixture.name == "putty_v3_ecdsa384_encrypted.ppk")
        .unwrap();
    assert_eq!(
        import_error(&flip_last_digit(encrypted.text), encrypted.passphrase),
        KeyError::WrongPassphrase
    );
    let encrypted_v2 = PUTTY_FIXTURES
        .iter()
        .find(|fixture| fixture.name == "putty_v2_ed25519_encrypted.ppk")
        .unwrap();
    assert_eq!(
        import_error(&flip_last_digit(encrypted_v2.text), encrypted_v2.passphrase),
        KeyError::WrongPassphrase
    );
}

#[test]
fn incomplete_putty_keys() {
    for fixture in PUTTY_FIXTURES {
        let lines: Vec<&str> = fixture.text.lines().collect();
        // The first line was not selected.
        assert_damaged(&lines[1..].join("\n"));
        // The last line was not selected, or the file was cut anywhere.
        for kept in 1..lines.len() {
            let error = refused(&lines[..kept].join("\n"));
            assert!(
                matches!(error, KeyError::Damaged(_)),
                "{}, {kept} lines: {error:?}",
                fixture.name
            );
        }
        // A line lost from the public or the private part.
        for (index, line) in lines.iter().enumerate() {
            if line.contains(':') {
                continue;
            }
            let mut shorter = lines.clone();
            shorter.remove(index);
            let error = refused(&shorter.join("\n"));
            assert!(
                matches!(error, KeyError::Damaged(_)),
                "{}, without line {index}: {error:?}",
                fixture.name
            );
        }
    }
    assert_damaged(&replace_line(
        fixture!("putty_v2_rsa.ppk"),
        "Private-MAC:",
        "Private-MAC: xyz",
    ));
    assert_damaged(&replace_line(
        fixture!("putty_v2_rsa.ppk"),
        "Private-MAC:",
        "Private-MAC: 0123456789abcdef",
    ));
    assert_damaged(&replace_line(
        fixture!("putty_v2_rsa.ppk"),
        "Public-Lines:",
        "Public-Lines: six",
    ));
    assert_damaged(&replace_line(
        fixture!("putty_v3_rsa_encrypted.ppk"),
        "Argon2-Salt:",
        "Argon2-Salt: 12zz",
    ));
    assert_damaged(&replace_line(
        fixture!("putty_v3_rsa_encrypted.ppk"),
        "Argon2-Passes:",
        "Argon2-Passes: many",
    ));
    // The header announces one type, the key inside is of another.
    assert_damaged(&replace_line(
        fixture!("putty_v3_ed25519.ppk"),
        "PuTTY-User-Key-File-3:",
        "PuTTY-User-Key-File-3: ssh-rsa",
    ));
}

#[test]
fn mangled_putty_keys_are_read() {
    for fixture in PUTTY_FIXTURES {
        let text = fixture.text;
        let lines: Vec<&str> = text.lines().collect();
        // All the base64 lines of each part joined into one, as a tool that
        // drops line breaks leaves them; the announced counts stay as they were.
        let mut joined = String::new();
        for line in &lines {
            let is_header = line.contains(':');
            if is_header && !joined.is_empty() && !joined.ends_with('\n') {
                joined.push('\n');
            }
            joined.push_str(line);
            if is_header {
                joined.push('\n');
            }
        }
        let variants = [
            ("CRLF", text.replace('\n', "\r\n")),
            ("CR", text.replace('\n', "\r")),
            ("byte order mark", format!("\u{feff}{text}")),
            ("no last line end", text.trim_end().to_string()),
            ("blank lines", lines.join("\n\n")),
            ("spaces at line ends", lines.join("  \n") + "  \n"),
            ("indented", format!("    {}\n", lines.join("\n    "))),
            (
                "text around",
                format!("Here it is:\n\n{text}\nRegards,\nZoë: see you\n"),
            ),
            ("joined lines", joined),
        ];
        // This one has the cost PuTTYgen gives a key by default, which takes
        // seconds in a debug build: one variant is enough for it.
        let tried = if fixture.name == "putty_v3_ed25519_encrypted.ppk" {
            1
        } else {
            variants.len()
        };
        for (what, variant) in variants.into_iter().take(tried) {
            let imported = import_with_rounds(&variant, fixture.passphrase, TEST_ROUNDS)
                .unwrap_or_else(|error| panic!("{}, {what}: {error:?}", fixture.name));
            assert_eq!(
                imported.public_key.as_deref(),
                Some(fixture.public_key),
                "{}, {what}",
                fixture.name
            );
            assert_eq!(
                imported.info.comment.as_deref(),
                fixture.comment,
                "{}, {what}",
                fixture.name
            );
            let file = read_openssh_file(&imported.text);
            assert_eq!(
                sha256_hex(&file.private_key(fixture.passphrase).unwrap()),
                fixture.private_section_sha256,
                "{}, {what}",
                fixture.name
            );
        }
    }
}

#[test]
fn putty_comments_with_spaces_at_their_ends() {
    let mut public_blob = Vec::new();
    put_string(&mut public_blob, b"ssh-ed25519");
    put_string(&mut public_blob, &[3; 32]);
    let mut private_blob = Vec::new();
    put_string(&mut private_blob, &[4; 32]);

    // A comment that really ends with spaces is kept with them.
    let text = putty_v2_file("ssh-ed25519", "my key  ", &public_blob, &private_blob);
    let imported = import(&text, None).unwrap();
    assert_eq!(imported.info.comment.as_deref(), Some("my key  "));
    assert!(
        read_openssh_file(&imported.text)
            .private_key(None)
            .unwrap()
            .ends_with(b"my key  ")
    );

    // Spaces a tool added after the comment are not part of it.
    let text = putty_v2_file("ssh-ed25519", "my key", &public_blob, &private_blob)
        .replace("my key\n", "my key \t \n");
    let imported = import(&text, None).unwrap();
    assert_eq!(imported.info.comment.as_deref(), Some("my key"));
    assert_eq!(
        imported
            .public_key
            .unwrap()
            .split(' ')
            .skip(2)
            .collect::<Vec<_>>(),
        ["my", "key"]
    );

    // A comment with a colon in it, and an empty one whose line lost its space.
    let text = putty_v2_file("ssh-ed25519", "zoe: laptop", &public_blob, &private_blob);
    assert_eq!(
        inspect(&text).unwrap().comment.as_deref(),
        Some("zoe: laptop")
    );
    let text = putty_v2_file("ssh-ed25519", "", &public_blob, &private_blob)
        .replace("Comment: \n", "Comment:\n");
    assert_eq!(inspect(&text).unwrap().comment, None);
}

#[test]
fn an_old_putty_ed25519_key_with_a_short_seed() {
    // Old PuTTY versions dropped the zero bytes at the end of the seed.
    let public_key = [0xa5u8; 32];
    let mut public_blob = Vec::new();
    put_string(&mut public_blob, b"ssh-ed25519");
    put_string(&mut public_blob, &public_key);
    let short_seed = [0x11u8; 30];
    let mut private_blob = Vec::new();
    put_string(&mut private_blob, &short_seed);

    let imported = import(
        &putty_v2_file("ssh-ed25519", "old", &public_blob, &private_blob),
        None,
    )
    .unwrap();
    let private_key = read_openssh_file(&imported.text).private_key(None).unwrap();
    let mut reader = Reader::new(&private_key);
    assert_eq!(reader.text(), Some("ssh-ed25519"));
    assert_eq!(reader.string(), Some(public_key.as_slice()));
    let seed_and_public = reader.string().unwrap();
    assert_eq!(seed_and_public.len(), 64);
    assert_eq!(seed_and_public[..30], short_seed);
    assert_eq!(seed_and_public[30..32], [0, 0]);
    assert_eq!(seed_and_public[32..], public_key);
    assert_eq!(reader.string(), Some(b"old".as_slice()));
    assert!(reader.is_empty());
}

#[test]
fn putty_numbers_are_written_in_their_shortest_form() {
    // Numbers with needless zero bytes in front, or without the zero byte
    // that keeps a number with its top bit set positive.
    let mut public_blob = Vec::new();
    put_string(&mut public_blob, b"ssh-dss");
    for number in [&[0, 0, 0x7f, 1][..], &[0x80, 2], &[0, 0x80, 3], &[4]] {
        put_string(&mut public_blob, number);
    }
    let mut private_blob = Vec::new();
    put_string(&mut private_blob, &[0, 0, 0, 5]);
    let imported = import(
        &putty_v2_file("ssh-dss", "", &public_blob, &private_blob),
        None,
    )
    .unwrap();
    let private_key = read_openssh_file(&imported.text).private_key(None).unwrap();

    let mut expected = Vec::new();
    put_string(&mut expected, b"ssh-dss");
    for number in [&[0x7f, 1][..], &[0, 0x80, 2], &[0, 0x80, 3], &[4], &[5]] {
        put_string(&mut expected, number);
    }
    put_string(&mut expected, b"");
    assert_eq!(private_key, expected);

    let mut direct = Vec::new();
    put_mpint(&mut direct, &[0, 0]);
    put_mpint(&mut direct, &[0xff]);
    assert_eq!(direct, [0, 0, 0, 0, 0, 0, 0, 2, 0, 0xff]);
}

#[test]
fn putty_keys_with_unexpected_content() {
    let mut ed25519_public = Vec::new();
    put_string(&mut ed25519_public, b"ssh-ed25519");
    put_string(&mut ed25519_public, &[3; 32]);

    // The MAC is right, the content is not what the type announces.
    let mut long_seed = Vec::new();
    put_string(&mut long_seed, &[4; 33]);
    assert_damaged(&putty_v2_file(
        "ssh-ed25519",
        "",
        &ed25519_public,
        &long_seed,
    ));
    assert_damaged(&putty_v2_file("ssh-ed25519", "", &ed25519_public, &[]));
    assert_damaged(&putty_v2_file(
        "ssh-ed25519",
        "",
        &ed25519_public,
        &[0, 0, 0, 9, 1],
    ));

    let mut short_public_key = Vec::new();
    put_string(&mut short_public_key, b"ssh-ed25519");
    put_string(&mut short_public_key, &[3; 31]);
    let mut seed = Vec::new();
    put_string(&mut seed, &[4; 32]);
    assert_damaged(&putty_v2_file("ssh-ed25519", "", &short_public_key, &seed));

    // An ECDSA key whose curve is not the one of its type.
    let mut ecdsa_public = Vec::new();
    put_string(&mut ecdsa_public, b"ecdsa-sha2-nistp256");
    put_string(&mut ecdsa_public, b"nistp384");
    put_string(&mut ecdsa_public, &[4; 65]);
    assert_damaged(&putty_v2_file(
        "ecdsa-sha2-nistp256",
        "",
        &ecdsa_public,
        &seed,
    ));

    // An RSA key that lost one of its numbers.
    let mut rsa_public = Vec::new();
    put_string(&mut rsa_public, b"ssh-rsa");
    put_string(&mut rsa_public, &[1, 0, 1]);
    put_string(&mut rsa_public, &[0xc1; 256]);
    let mut rsa_private = Vec::new();
    for _ in 0..3 {
        put_string(&mut rsa_private, &[0x55; 128]);
    }
    assert_damaged(&putty_v2_file("ssh-rsa", "", &rsa_public, &rsa_private));
    put_string(&mut rsa_private, &[0x55; 128]);
    assert_eq!(
        inspect(&putty_v2_file("ssh-rsa", "", &rsa_public, &rsa_private))
            .unwrap()
            .algorithm
            .as_deref(),
        Some("RSA 2048")
    );
}

#[test]
fn unsupported_putty_keys() {
    assert_unsupported(fixture!("putty_v3_ed448.ppk"), "ssh-ed448");
    assert_unsupported(
        &replace_line(
            fixture!("putty_v3_ed25519.ppk"),
            "PuTTY-User-Key-File-3:",
            "PuTTY-User-Key-File-3: ssh-ed25519-cert-v01@openssh.com",
        ),
        "ssh-ed25519-cert-v01@openssh.com",
    );
    assert_unsupported(
        &replace_line(
            fixture!("putty_v2_rsa.ppk"),
            "PuTTY-User-Key-File-2:",
            "PuTTY-User-Key-File-1: ssh-rsa",
        ),
        "very old",
    );
    assert_unsupported(
        &replace_line(
            fixture!("putty_v2_rsa.ppk"),
            "PuTTY-User-Key-File-2:",
            "PuTTY-User-Key-File-4: ssh-rsa",
        ),
        "newer",
    );
    assert_unsupported(
        &replace_line(
            fixture!("putty_v2_rsa.ppk"),
            "Encryption:",
            "Encryption: chacha20",
        ),
        "chacha20",
    );
    assert_unsupported(
        &replace_line(
            fixture!("putty_v3_rsa_encrypted.ppk"),
            "Key-Derivation:",
            "Key-Derivation: Scrypt",
        ),
        "Scrypt",
    );
    assert_unsupported(
        "---- BEGIN SSH2 ENCRYPTED PRIVATE KEY ----\nComment: \"rsa-key\"\nP8/vee8AAAAA\n---- END SSH2 ENCRYPTED PRIVATE KEY ----\n",
        "ssh.com",
    );
    assert_unsupported(
        "-----BEGIN FANCY PRIVATE KEY-----\nMAA=\n-----END FANCY PRIVATE KEY-----\n",
        "FANCY PRIVATE KEY",
    );
}

#[test]
fn absurd_argon2_settings_are_refused_before_any_work() {
    let text = fixture!("putty_v3_rsa_encrypted.ppk");
    let started = std::time::Instant::now();
    for (line, value) in [
        ("Argon2-Memory:", "Argon2-Memory: 1048577"),
        ("Argon2-Memory:", "Argon2-Memory: 4294967295"),
        ("Argon2-Passes:", "Argon2-Passes: 1001"),
        ("Argon2-Parallelism:", "Argon2-Parallelism: 65"),
    ] {
        let changed = replace_line(text, line, value);
        assert_unsupported(&changed, "too much memory or time");
        match import_error(&changed, Some(UNICODE_PASSPHRASE)) {
            KeyError::Unsupported(_) => {}
            other => panic!("{value}: {other:?}"),
        }
    }
    assert!(started.elapsed() < std::time::Duration::from_secs(5));

    // Settings Argon2 itself does not allow are a damaged file; they are only
    // found when the passphrase is tried.
    for (line, value) in [
        ("Argon2-Memory:", "Argon2-Memory: 4"),
        ("Argon2-Passes:", "Argon2-Passes: 0"),
        ("Argon2-Parallelism:", "Argon2-Parallelism: 0"),
        ("Argon2-Salt:", "Argon2-Salt: 0011"),
    ] {
        let changed = replace_line(text, line, value);
        match import_error(&changed, Some(UNICODE_PASSPHRASE)) {
            KeyError::Damaged(_) => {}
            other => panic!("{value}: {other:?}"),
        }
    }
}

#[test]
fn a_putty_key_from_an_old_windows_putty() {
    // Comment and passphrase in a single-byte code page, as PuTTY for Windows
    // wrote them before it learned Unicode: the file is not valid UTF-8.
    let bytes = include_bytes!("../tests/fixtures/putty_v2_ed25519_latin1.ppk");
    assert!(String::from_utf8(bytes.to_vec()).is_err());
    let text = text_from_bytes(bytes);

    let info = inspect(&text).unwrap();
    assert_eq!(info.comment.as_deref(), Some("clé de Zoë"));
    assert!(info.encrypted);

    // The passphrase is typed as text; the file was protected with its
    // single-byte form.
    let imported = import_with_rounds(&text, Some("pässwörd"), TEST_ROUNDS).unwrap();
    assert_eq!(
        imported.public_key.as_deref(),
        Some(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIEsY7wp5q8f7g9qPMSWCQGSYszjTUJPTDX8nPajFW5yg clé de Zoë"
        )
    );
    assert_eq!(
        import_error(&text, Some("passwörd")),
        KeyError::WrongPassphrase
    );
    assert_eq!(
        import_error(&text, Some("pässwörd€")),
        KeyError::WrongPassphrase
    );

    // The OpenSSH key is protected by the passphrase as it is typed today,
    // and its comment is UTF-8.
    let file = read_openssh_file(&imported.text);
    assert_eq!(
        sha256_hex(&file.private_key(Some("pässwörd")).unwrap()),
        "ff65f04a8dcbcdc9b8c74319b8b416feea99c45041825ed1742bf80863fa7943"
    );
}

#[test]
fn key_files_in_other_encodings() {
    let text = fixture!("openssh_ed25519.key");
    assert_eq!(text_from_bytes(text.as_bytes()), text);
    assert_eq!(text_from_bytes(b""), "");

    // What PowerShell's `>` and Notepad's "Unicode" write.
    let crlf = text.replace('\n', "\r\n");
    let mut utf16_le = vec![0xff, 0xfe];
    utf16_le.extend(crlf.encode_utf16().flat_map(u16::to_le_bytes));
    let mut utf16_be = vec![0xfe, 0xff];
    utf16_be.extend(crlf.encode_utf16().flat_map(u16::to_be_bytes));
    let mut utf8_with_mark = vec![0xef, 0xbb, 0xbf];
    utf8_with_mark.extend_from_slice(crlf.as_bytes());
    for bytes in [utf16_le, utf16_be, utf8_with_mark] {
        assert_eq!(import(&text_from_bytes(&bytes), None).unwrap().text, text);
    }

    // A PuTTY key with a comment outside ASCII, saved as UTF-16.
    let fixture = &PUTTY_FIXTURES[0];
    let mut utf16 = vec![0xff, 0xfe];
    utf16.extend(fixture.text.encode_utf16().flat_map(u16::to_le_bytes));
    let imported = import(&text_from_bytes(&utf16), None).unwrap();
    assert_eq!(imported.public_key.as_deref(), Some(fixture.public_key));

    // Odd lengths and broken pairs do not panic.
    assert_eq!(text_from_bytes(&[0xff, 0xfe, 0x41]), "");
    assert_eq!(text_from_bytes(&[0xff, 0xfe, 0x00, 0xd8]), "\u{fffd}");
    assert_eq!(text_from_bytes(&[0x41, 0xe9]), "Aé");
}

#[test]
fn no_text_makes_the_reader_panic() {
    // Every fixture, cut at every character and with every line removed,
    // doubled or swapped with its neighbour: the answer may be anything, as
    // long as there is one.
    let latin1 = text_from_bytes(include_bytes!(
        "../tests/fixtures/putty_v2_ed25519_latin1.ppk"
    ));
    let texts = KEPT_FIXTURES
        .iter()
        .map(|fixture| fixture.text)
        .chain(PUTTY_FIXTURES.iter().map(|fixture| fixture.text))
        .chain([
            fixture!("putty_v3_ed448.ppk"),
            fixture!("openssh_ed25519-cert.pub"),
            latin1.as_str(),
        ]);
    for text in texts {
        for (position, _) in text.char_indices() {
            inspect(&text[..position]).ok();
            inspect(&text[position..]).ok();
        }
        let lines: Vec<&str> = text.lines().collect();
        for index in 0..lines.len() {
            let mut removed = lines.clone();
            removed.remove(index);
            import_with_rounds(&removed.join("\n"), Some("x"), TEST_ROUNDS).ok();
            let mut doubled = lines.clone();
            doubled.insert(index, lines[index]);
            inspect(&doubled.join("\n")).ok();
            if index + 1 < lines.len() {
                let mut swapped = lines.clone();
                swapped.swap(index, index + 1);
                inspect(&swapped.join("\r\n")).ok();
            }
        }
    }

    // Every OpenSSH container with one byte changed, and cut at every length.
    for name in [
        "openssh_ed25519.key",
        "openssh_ed25519_encrypted.key",
        "openssh_ecdsa256.key",
    ] {
        let text = KEPT_FIXTURES
            .iter()
            .find(|fixture| fixture.name == name)
            .unwrap()
            .text;
        let data = STANDARD.decode(pieces(text).body).unwrap();
        for position in 0..data.len() {
            for change in [0x01, 0x80, 0xff] {
                let mut changed = data.clone();
                changed[position] ^= change;
                inspect(&armor_openssh(&changed)).ok();
            }
            inspect(&armor_openssh(&data[..position])).ok();
        }
    }

    // PuTTY files whose MAC is right and whose content is cut anywhere.
    let mut public_blob = Vec::new();
    put_string(&mut public_blob, b"ssh-rsa");
    put_string(&mut public_blob, &[1, 0, 1]);
    put_string(&mut public_blob, &[0xc1; 64]);
    let mut private_blob = Vec::new();
    for _ in 0..4 {
        put_string(&mut private_blob, &[0x55; 32]);
    }
    for algorithm in ["ssh-rsa", "ssh-dss", "ssh-ed25519", "ecdsa-sha2-nistp256"] {
        let mut public_blob = public_blob.clone();
        public_blob.splice(4..11, algorithm.bytes());
        public_blob[3] = algorithm.len() as u8;
        for length in 0..public_blob.len() {
            import(
                &putty_v2_file(algorithm, "", &public_blob[..length], &private_blob),
                None,
            )
            .ok();
        }
        for length in 0..private_blob.len() {
            import(
                &putty_v2_file(algorithm, "", &public_blob, &private_blob[..length]),
                None,
            )
            .ok();
        }
    }
}

// ---------------------------------------------------------------------------
// Writing the key file
// ---------------------------------------------------------------------------

#[test]
fn key_files_are_private_and_never_replaced() {
    let directory = tempfile::tempdir().unwrap();
    let keys = directory.path().join("nested").join("keys");
    let text = fixture!("openssh_ed25519.key");

    let first = write_key_file(&keys, "Zoë's Laptop (prod).ppk", text).unwrap();
    assert_eq!(first, keys.join("zo-s-laptop-prod-ppk"));
    assert_eq!(fs::read_to_string(&first).unwrap(), text);

    let second = write_key_file(&keys, "Zoë's Laptop (prod).ppk", "second").unwrap();
    assert_eq!(second, keys.join("zo-s-laptop-prod-ppk-2"));
    let third = write_key_file(&keys, "zo-s-laptop-prod-ppk", "third").unwrap();
    assert_eq!(third, keys.join("zo-s-laptop-prod-ppk-3"));
    assert_eq!(fs::read_to_string(&first).unwrap(), text);
    assert_eq!(fs::read_to_string(&second).unwrap(), "second");
    assert_eq!(fs::read_to_string(&third).unwrap(), "third");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        for file in [&first, &second, &third] {
            assert_eq!(
                fs::metadata(file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        for created in [&keys, &directory.path().join("nested")] {
            assert_eq!(
                fs::metadata(created).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    // Nothing but the three keys is left behind.
    let mut names: Vec<String> = fs::read_dir(&keys)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "zo-s-laptop-prod-ppk",
            "zo-s-laptop-prod-ppk-2",
            "zo-s-laptop-prod-ppk-3"
        ]
    );
}

#[test]
fn key_file_names_are_tame() {
    let directory = tempfile::tempdir().unwrap();
    let name = |hint: &str| {
        let path = write_key_file(directory.path(), hint, "key").unwrap();
        assert_eq!(path.parent(), Some(directory.path()));
        path.file_name().unwrap().to_str().unwrap().to_string()
    };
    assert_eq!(name("id_ed25519"), "id_ed25519");
    assert_eq!(name("ID_ED25519"), "id_ed25519-2");
    assert_eq!(name("Prod Server #1"), "prod-server-1");
    assert_eq!(name("../../etc/passwd"), "etc-passwd");
    assert_eq!(name("/absolute/path"), "absolute-path");
    assert_eq!(name("..\\..\\windows\\system32"), "windows-system32");
    assert_eq!(name(".hidden"), "hidden");
    assert_eq!(name("--option"), "option");
    assert_eq!(name("a   b___c---d"), "a-b___c-d");
    assert_eq!(name(""), "key");
    assert_eq!(name("   "), "key-2");
    assert_eq!(name("東京"), "key-3");
    assert_eq!(name("NUL"), "nul-key");
    assert_eq!(name("com1"), "com1-key");
    assert_eq!(name("command"), "command");
    let long = name(&"x".repeat(300));
    assert_eq!(long, "x".repeat(48));
    assert_eq!(name("null\0byte"), "null-byte");
}

#[test]
fn a_key_file_is_not_written_over_anything() {
    let directory = tempfile::tempdir().unwrap();
    // A directory and a dangling link already have the names wanted.
    fs::create_dir(directory.path().join("server")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        directory.path().join("nowhere"),
        directory.path().join("server-2"),
    )
    .unwrap();
    #[cfg(not(unix))]
    fs::write(directory.path().join("server-2"), "taken").unwrap();

    let path = write_key_file(directory.path(), "server", "key").unwrap();
    assert_eq!(path, directory.path().join("server-3"));
    assert!(directory.path().join("server").is_dir());
    assert!(!directory.path().join("nowhere").exists());

    // The directory is a file: nothing is written, and the error says so.
    let file = directory.path().join("server-3");
    assert!(write_key_file(&file, "key", "key").is_err());
    assert_eq!(fs::read_to_string(&file).unwrap(), "key");
}

#[cfg(unix)]
#[test]
fn an_existing_directory_keeps_its_mode() {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o750)).unwrap();
    let path = write_key_file(directory.path(), "key", "text").unwrap();
    assert_eq!(
        fs::metadata(directory.path()).unwrap().permissions().mode() & 0o777,
        0o750
    );
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

// ---------------------------------------------------------------------------
// The low level
// ---------------------------------------------------------------------------

#[test]
fn written_openssh_files_are_read_back() {
    let mut public_blob = Vec::new();
    put_string(&mut public_blob, b"ssh-ed25519");
    put_string(&mut public_blob, &[1; 32]);
    let mut private_key = Vec::new();
    put_string(&mut private_key, b"ssh-ed25519");
    put_string(&mut private_key, &[1; 32]);
    put_string(&mut private_key, &[2; 64]);

    // Every comment length, so every amount of padding.
    for length in 0..40 {
        let comment = "c".repeat(length);
        let text = openssh::write(&public_blob, &private_key, &comment, None, TEST_ROUNDS).unwrap();
        let info = inspect(&text).unwrap();
        assert_eq!(info.comment.unwrap_or_default(), comment);
        assert!(!info.encrypted);
        let file = read_openssh_file(&text);
        assert_eq!(file.private_section.len() % 8, 0);
        let mut expected = private_key.clone();
        put_string(&mut expected, comment.as_bytes());
        assert_eq!(file.private_key(None).unwrap(), expected);

        let text = openssh::write(
            &public_blob,
            &private_key,
            &comment,
            Some(b"secret"),
            TEST_ROUNDS,
        )
        .unwrap();
        let info = inspect(&text).unwrap();
        assert!(info.encrypted);
        assert_eq!(info.comment, None);
        let file = read_openssh_file(&text);
        assert_eq!(file.private_section.len() % 16, 0);
        assert_eq!(file.rounds(), TEST_ROUNDS);
        assert_eq!(file.private_key(Some("secret")).unwrap(), expected);
    }

    let mut options = Vec::new();
    put_string(&mut options, &[0; 16]);
    put_u32(&mut options, 24);
    assert_eq!(options.len(), 24);
}

// --- generate ----------------------------------------------------------------

#[test]
fn test_a_generated_key_is_an_openssh_ed25519_key_with_its_public_line() {
    let key =
        generate_with_rounds(&KeySeed::new().unwrap(), "laptop of Zoé", None, TEST_ROUNDS).unwrap();
    assert!(
        key.text
            .starts_with("-----BEGIN OPENSSH PRIVATE KEY-----\n")
    );
    assert!(key.text.ends_with("-----END OPENSSH PRIVATE KEY-----\n"));
    assert!(!key.converted);

    // What was written reads back as what was meant.
    let info = inspect(&key.text).unwrap();
    assert_eq!(info.format, KeyFormat::OpenSsh);
    assert_eq!(info.algorithm.as_deref(), Some("ED25519"));
    assert!(!info.encrypted);
    assert_eq!(info.comment.as_deref(), Some("laptop of Zoé"));
    assert_eq!(key.info, info);

    let public = key.public_key.unwrap();
    assert!(
        public.starts_with("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA"),
        "{public}"
    );
    assert!(public.ends_with(" laptop of Zoé"), "{public}");
    // The file's own public half is the one given.
    let read_back = import_with_rounds(&key.text, None, TEST_ROUNDS).unwrap();
    assert_eq!(read_back.public_key.as_deref(), Some(public.as_str()));
    assert_eq!(read_back.text, key.text);
}

#[test]
fn test_a_generated_key_with_a_passphrase_is_protected_and_two_keys_differ() {
    let seed = KeySeed::new().unwrap();
    let protected =
        generate_with_rounds(&seed, "work", Some("correct horse"), TEST_ROUNDS).unwrap();
    let info = inspect(&protected.text).unwrap();
    assert!(info.encrypted);
    assert!(protected.info.encrypted);
    // The public half is readable without the passphrase.
    assert!(protected.public_key.as_deref().unwrap().ends_with(" work"));

    // The same seed without a passphrase: the same key, in the clear.
    let clear = generate_with_rounds(&seed, "work", None, TEST_ROUNDS).unwrap();
    assert_eq!(protected.public_key, clear.public_key);
    assert!(!clear.info.encrypted);
    assert_ne!(protected.text, clear.text);

    let other = generate_with_rounds(&KeySeed::new().unwrap(), "work", None, TEST_ROUNDS).unwrap();
    assert_ne!(protected.public_key, other.public_key);
}

#[test]
fn test_a_generated_keys_comment_is_one_line() {
    let key = generate_with_rounds(
        &KeySeed::new().unwrap(),
        "  two\nlines\t ",
        None,
        TEST_ROUNDS,
    )
    .unwrap();
    assert_eq!(key.info.comment.as_deref(), Some("twolines"));
    let key = generate_with_rounds(&KeySeed::new().unwrap(), "", None, TEST_ROUNDS).unwrap();
    assert_eq!(key.info.comment, None);
    assert_eq!(key.public_key.unwrap().split(' ').count(), 2);
}
