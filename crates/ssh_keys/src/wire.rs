//! The SSH wire encoding (RFC 4251): big-endian lengths, length-prefixed
//! strings and multiple precision integers. Both the PuTTY and the OpenSSH
//! key files are built from these.

/// Reads SSH wire values from the front of a byte slice.
pub(crate) struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub(crate) fn rest(&self) -> &'a [u8] {
        self.data
    }

    pub(crate) fn bytes(&mut self, count: usize) -> Option<&'a [u8]> {
        if count > self.data.len() {
            return None;
        }
        let (head, tail) = self.data.split_at(count);
        self.data = tail;
        Some(head)
    }

    pub(crate) fn u32(&mut self) -> Option<u32> {
        let bytes = self.bytes(4)?;
        Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(crate) fn string(&mut self) -> Option<&'a [u8]> {
        let length = self.u32()?;
        self.bytes(usize::try_from(length).ok()?)
    }

    pub(crate) fn text(&mut self) -> Option<&'a str> {
        std::str::from_utf8(self.string()?).ok()
    }
}

pub(crate) fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

pub(crate) fn put_string(out: &mut Vec<u8>, bytes: &[u8]) {
    // A key file is a few kilobytes: a length never comes near 4 GiB.
    put_u32(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

/// Writes an unsigned integer given as big-endian bytes, in its shortest
/// form: no needless leading zero, and one zero byte in front when the top bit
/// is set, so that it is not read as a negative number.
pub(crate) fn put_mpint(out: &mut Vec<u8>, magnitude: &[u8]) {
    let magnitude = strip_leading_zeros(magnitude);
    let needs_zero = magnitude.first().is_some_and(|byte| byte & 0x80 != 0);
    put_u32(out, (magnitude.len() + usize::from(needs_zero)) as u32);
    if needs_zero {
        out.push(0);
    }
    out.extend_from_slice(magnitude);
}

pub(crate) fn strip_leading_zeros(bytes: &[u8]) -> &[u8] {
    let zeros = bytes.iter().take_while(|byte| **byte == 0).count();
    &bytes[zeros..]
}

/// The number of significant bits of an unsigned big-endian integer.
pub(crate) fn bit_length(magnitude: &[u8]) -> usize {
    let magnitude = strip_leading_zeros(magnitude);
    match magnitude.first() {
        Some(first) => magnitude.len() * 8 - first.leading_zeros() as usize,
        None => 0,
    }
}

/// The key type a public key blob starts with, and the name people know that
/// kind of key by ("RSA 4096", "ECDSA P-256"...).
pub(crate) fn describe_public_blob(blob: &[u8]) -> Option<(&str, String)> {
    let mut reader = Reader::new(blob);
    let key_type = reader.text()?;
    let name = match key_type {
        "ssh-ed25519" => "ED25519".to_string(),
        "ssh-ed448" => "ED448".to_string(),
        "ssh-dss" => "DSA".to_string(),
        "ssh-rsa" => {
            let _exponent = reader.string()?;
            let modulus = reader.string()?;
            format!("RSA {}", bit_length(modulus))
        }
        "ecdsa-sha2-nistp256" => "ECDSA P-256".to_string(),
        "ecdsa-sha2-nistp384" => "ECDSA P-384".to_string(),
        "ecdsa-sha2-nistp521" => "ECDSA P-521".to_string(),
        "sk-ssh-ed25519@openssh.com" => "ED25519-SK".to_string(),
        "sk-ecdsa-sha2-nistp256@openssh.com" => "ECDSA-SK".to_string(),
        other => other.to_string(),
    };
    Some((key_type, name))
}
