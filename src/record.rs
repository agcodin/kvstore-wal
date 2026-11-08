//! On-disk record format.
//!
//! ```text
//! +-----------+-----------+---------------------------+
//! | len: u32  | crc: u32  | payload (len bytes)       |
//! +-----------+-----------+---------------------------+
//! payload = op: u8 | key_len: u32 | key | value        (op 1 = put)
//!         = op: u8 | key                                (op 2 = delete)
//! ```
//!
//! All integers are little endian. `crc` is CRC-32 of the payload only.

pub const HEADER_LEN: usize = 8;
/// Upper bound on a payload. A corrupted length field larger than this is
/// rejected without trying to read that many bytes.
pub const MAX_PAYLOAD: usize = 16 << 20;

const OP_PUT: u8 = 1;
const OP_DELETE: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Put { key: Vec<u8>, value: Vec<u8> },
    Delete { key: Vec<u8> },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decoded {
    /// A valid record and the number of bytes it occupied.
    Record(Op, usize),
    /// The buffer ends before the record does (torn write).
    Incomplete,
    /// Length, checksum or payload structure is invalid.
    Corrupt,
}

/// Encoded size of a put record, used for live-size accounting.
pub fn put_len(key: &[u8], value: &[u8]) -> usize {
    HEADER_LEN + 1 + 4 + key.len() + value.len()
}

pub fn encode(op: &Op, out: &mut Vec<u8>) {
    let start = out.len();
    out.extend_from_slice(&[0; HEADER_LEN]);
    match op {
        Op::Put { key, value } => {
            out.push(OP_PUT);
            out.extend_from_slice(&(key.len() as u32).to_le_bytes());
            out.extend_from_slice(key);
            out.extend_from_slice(value);
        }
        Op::Delete { key } => {
            out.push(OP_DELETE);
            out.extend_from_slice(key);
        }
    }
    let payload = &out[start + HEADER_LEN..];
    let len = (payload.len() as u32).to_le_bytes();
    let crc = crc32fast::hash(payload).to_le_bytes();
    out[start..start + 4].copy_from_slice(&len);
    out[start + 4..start + 8].copy_from_slice(&crc);
}

pub fn decode(buf: &[u8]) -> Decoded {
    if buf.len() < HEADER_LEN {
        return Decoded::Incomplete;
    }
    let len = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
    let crc = u32::from_le_bytes(buf[4..8].try_into().unwrap());
    if len == 0 || len > MAX_PAYLOAD {
        return Decoded::Corrupt;
    }
    if buf.len() < HEADER_LEN + len {
        return Decoded::Incomplete;
    }
    let payload = &buf[HEADER_LEN..HEADER_LEN + len];
    if crc32fast::hash(payload) != crc {
        return Decoded::Corrupt;
    }
    let op = match payload[0] {
        OP_PUT => {
            if payload.len() < 5 {
                return Decoded::Corrupt;
            }
            let klen = u32::from_le_bytes(payload[1..5].try_into().unwrap()) as usize;
            if payload.len() < 5 + klen {
                return Decoded::Corrupt;
            }
            Op::Put {
                key: payload[5..5 + klen].to_vec(),
                value: payload[5 + klen..].to_vec(),
            }
        }
        OP_DELETE => Op::Delete {
            key: payload[1..].to_vec(),
        },
        _ => return Decoded::Corrupt,
    };
    Decoded::Record(op, HEADER_LEN + len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(k: &[u8], v: &[u8]) -> Op {
        Op::Put {
            key: k.to_vec(),
            value: v.to_vec(),
        }
    }

    #[test]
    fn roundtrip() {
        for op in [
            put(b"a", b"b"),
            put(b"", b""),
            put(b"key", &[0u8; 1000]),
            Op::Delete {
                key: b"zz".to_vec(),
            },
        ] {
            let mut buf = Vec::new();
            encode(&op, &mut buf);
            assert_eq!(decode(&buf), Decoded::Record(op, buf.len()));
        }
    }

    #[test]
    fn put_len_matches_encoding() {
        let mut buf = Vec::new();
        encode(&put(b"abc", b"defgh"), &mut buf);
        assert_eq!(buf.len(), put_len(b"abc", b"defgh"));
    }

    #[test]
    fn every_truncation_is_incomplete() {
        let mut buf = Vec::new();
        encode(&put(b"abc", b"defgh"), &mut buf);
        for n in 0..buf.len() {
            assert_eq!(decode(&buf[..n]), Decoded::Incomplete, "n={n}");
        }
    }

    #[test]
    fn bit_flip_is_detected() {
        let mut buf = Vec::new();
        encode(&put(b"abc", b"defgh"), &mut buf);
        for i in 0..buf.len() {
            let mut b = buf.clone();
            b[i] ^= 0x10;
            assert!(!matches!(decode(&b), Decoded::Record(..)), "byte {i}");
        }
    }

    #[test]
    fn huge_length_is_corrupt_not_incomplete() {
        let mut buf = vec![0xff; 16];
        buf[4..8].fill(0);
        assert_eq!(decode(&buf), Decoded::Corrupt);
    }
}
