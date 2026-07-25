//! Minimal standard base64 encoder (RFC 4648). Hand-rolled to avoid a runtime
//! dependency; used only to encode kitty-graphics image payloads.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode `bytes` as standard base64 with `=` padding.
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18 & 63) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Decode a standard base64 string (with `=` padding) back to bytes.
/// Test-only: used to verify transmitted kitty-graphics image payloads.
#[cfg(test)]
pub fn decode(s: &str) -> Vec<u8> {
    fn val(c: u8) -> u32 {
        ALPHABET.iter().position(|&a| a == c).unwrap() as u32
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for chunk in s.as_bytes().chunks(4) {
        let pad = chunk.iter().filter(|&&c| c == b'=').count();
        let c0 = val(chunk[0]);
        let c1 = val(chunk[1]);
        let c2 = if chunk[2] == b'=' { 0 } else { val(chunk[2]) };
        let c3 = if chunk[3] == b'=' { 0 } else { val(chunk[3]) };
        let n = (c0 << 18) | (c1 << 12) | (c2 << 6) | c3;
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};
    #[test]
    fn encodes_rfc4648_vectors() {
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"foob"), "Zm9vYg==");
        assert_eq!(encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn decodes_rfc4648_vectors() {
        assert_eq!(decode(""), b"");
        assert_eq!(decode("Zg=="), b"f");
        assert_eq!(decode("Zm8="), b"fo");
        assert_eq!(decode("Zm9v"), b"foo");
        assert_eq!(decode("Zm9vYg=="), b"foob");
        assert_eq!(decode("Zm9vYmE="), b"fooba");
        assert_eq!(decode("Zm9vYmFy"), b"foobar");
    }

    #[test]
    fn decode_reverses_encode_for_arbitrary_bytes() {
        let bytes: Vec<u8> = (0..=255).collect();
        assert_eq!(decode(&encode(&bytes)), bytes);
    }
}
