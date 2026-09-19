use std::cmp::Ordering;

/// Put = 1, Delete (tombstone) = 0. Higher kind wins for same seq (shouldn't happen).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ValueKind {
    Delete = 0,
    Put = 1,
}

impl ValueKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Delete),
            1 => Some(Self::Put),
            _ => None,
        }
    }
}

/// Logical record ordered by (user_key ASC, seq DESC).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalKey {
    pub user_key: Vec<u8>,
    pub seq: u64,
    pub kind: ValueKind,
}

impl InternalKey {
    pub fn put(user_key: Vec<u8>, seq: u64) -> Self {
        Self {
            user_key,
            seq,
            kind: ValueKind::Put,
        }
    }

    pub fn delete(user_key: Vec<u8>, seq: u64) -> Self {
        Self {
            user_key,
            seq,
            kind: ValueKind::Delete,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.user_key.len() + 9);
        buf.extend_from_slice(&self.user_key);
        let tag = (self.seq << 8) | u64::from(self.kind as u8);
        buf.extend_from_slice(&tag.to_be_bytes());
        buf
    }

    pub fn decode(bytes: &[u8]) -> crate::error::Result<Self> {
        if bytes.len() < 8 {
            return Err(crate::error::Error::Corruption(
                "internal key too short".into(),
            ));
        }
        let (user_key, tag_bytes) = bytes.split_at(bytes.len() - 8);
        let tag = u64::from_be_bytes(tag_bytes.try_into().unwrap());
        let kind = ValueKind::from_u8((tag & 0xff) as u8).ok_or_else(|| {
            crate::error::Error::Corruption(format!("bad value kind {}", tag & 0xff))
        })?;
        Ok(Self {
            user_key: user_key.to_vec(),
            seq: tag >> 8,
            kind,
        })
    }
}

impl Ord for InternalKey {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.user_key.cmp(&other.user_key) {
            Ordering::Equal => match other.seq.cmp(&self.seq) {
                Ordering::Equal => (self.kind as u8).cmp(&(other.kind as u8)),
                o => o,
            },
            o => o,
        }
    }
}

impl PartialOrd for InternalKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Compare encoded internal keys without full decode (user_key + tag).
pub fn compare_encoded(a: &[u8], b: &[u8]) -> Ordering {
    let a_key = &a[..a.len().saturating_sub(8)];
    let b_key = &b[..b.len().saturating_sub(8)];
    match a_key.cmp(b_key) {
        Ordering::Equal if a.len() >= 8 && b.len() >= 8 => {
            let a_tag = u64::from_be_bytes(a[a.len() - 8..].try_into().unwrap());
            let b_tag = u64::from_be_bytes(b[b.len() - 8..].try_into().unwrap());
            let a_seq = a_tag >> 8;
            let b_seq = b_tag >> 8;
            match b_seq.cmp(&a_seq) {
                Ordering::Equal => (a_tag & 0xff).cmp(&(b_tag & 0xff)),
                o => o,
            }
        }
        o => o,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_seq_sorts_first_same_key() {
        let a = InternalKey::put(b"k".to_vec(), 10);
        let b = InternalKey::put(b"k".to_vec(), 5);
        assert!(a < b);
    }

    #[test]
    fn encode_roundtrip() {
        let k = InternalKey::delete(b"hello".to_vec(), 42);
        let enc = k.encode();
        let dec = InternalKey::decode(&enc).unwrap();
        assert_eq!(k, dec);
    }
}
