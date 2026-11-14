//! Just enough DER to check that a PEM body is what its label says, and to
//! name the algorithm of an unencrypted key. Nothing here is used to convert
//! a key: OpenSSH reads PEM files itself.

use crate::wire::bit_length;

const SEQUENCE: u8 = 0x30;
const INTEGER: u8 = 0x02;
const OCTET_STRING: u8 = 0x04;
const OBJECT_IDENTIFIER: u8 = 0x06;
const CONTEXT_0: u8 = 0xa0;

const OID_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
const OID_EC: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const OID_DSA: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x38, 0x04, 0x01];
const OID_ED25519: &[u8] = &[0x2b, 0x65, 0x70];
const OID_P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const OID_P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];
const OID_P521: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x23];

/// One DER element: its tag, its content, and what follows it.
fn element(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, after_tag) = data.split_first()?;
    let (&first, after_first) = after_tag.split_first()?;
    let (length, after_length) = if first < 0x80 {
        (usize::from(first), after_first)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 || count > after_first.len() {
            return None;
        }
        let (length_bytes, rest) = after_first.split_at(count);
        let length = length_bytes
            .iter()
            .fold(0usize, |length, byte| (length << 8) | usize::from(*byte));
        (length, rest)
    };
    if length > after_length.len() {
        return None;
    }
    let (content, rest) = after_length.split_at(length);
    Some((tag, content, rest))
}

fn expect(data: &[u8], wanted: u8) -> Option<(&[u8], &[u8])> {
    let (tag, content, rest) = element(data)?;
    (tag == wanted).then_some((content, rest))
}

/// The content of the SEQUENCE the data must consist of, when the data is
/// exactly one whole SEQUENCE: a cut or padded body fails here.
pub(crate) fn whole_sequence(data: &[u8]) -> Option<&[u8]> {
    let (content, rest) = expect(data, SEQUENCE)?;
    rest.is_empty().then_some(content)
}

/// `RSAPrivateKey` (PKCS#1): the size of the modulus.
pub(crate) fn rsa_bits(sequence: &[u8]) -> Option<usize> {
    let (_version, rest) = expect(sequence, INTEGER)?;
    let (modulus, _) = expect(rest, INTEGER)?;
    Some(bit_length(modulus))
}

fn curve_name(oid: &[u8]) -> Option<&'static str> {
    match oid {
        OID_P256 => Some("ECDSA P-256"),
        OID_P384 => Some("ECDSA P-384"),
        OID_P521 => Some("ECDSA P-521"),
        _ => None,
    }
}

/// `ECPrivateKey` (RFC 5915): the curve, when the key names it. A key that
/// spells its curve out as numbers instead is only known as "ECDSA".
pub(crate) fn ec_curve(sequence: &[u8]) -> Option<&'static str> {
    let (_version, rest) = expect(sequence, INTEGER)?;
    let (_private, rest) = expect(rest, OCTET_STRING)?;
    let (parameters, _) = expect(rest, CONTEXT_0)?;
    let (oid, _) = expect(parameters, OBJECT_IDENTIFIER)?;
    curve_name(oid)
}

/// `PrivateKeyInfo` (PKCS#8, unencrypted): the algorithm it holds.
pub(crate) fn pkcs8_algorithm(sequence: &[u8]) -> Option<String> {
    let (_version, rest) = expect(sequence, INTEGER)?;
    let (algorithm, rest) = expect(rest, SEQUENCE)?;
    let (oid, parameters) = expect(algorithm, OBJECT_IDENTIFIER)?;
    match oid {
        OID_RSA => {
            let bits = expect(rest, OCTET_STRING)
                .and_then(|(key, _)| whole_sequence(key))
                .and_then(rsa_bits);
            Some(match bits {
                Some(bits) => format!("RSA {bits}"),
                None => "RSA".to_string(),
            })
        }
        OID_EC => {
            let curve = expect(parameters, OBJECT_IDENTIFIER).and_then(|(oid, _)| curve_name(oid));
            Some(curve.unwrap_or("ECDSA").to_string())
        }
        OID_DSA => Some("DSA".to_string()),
        OID_ED25519 => Some("ED25519".to_string()),
        _ => None,
    }
}
