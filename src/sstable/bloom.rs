//! Bloom filter with double hashing — never false negatives.

#[derive(Debug, Clone)]
pub struct BloomFilter {
    bits: Vec<u8>,
    nbits: usize,
    k: u32,
}

impl BloomFilter {
    pub fn build(keys: &[Vec<u8>], bits_per_key: usize) -> Self {
        let bits_per_key = bits_per_key.max(1);
        let nbits = (keys.len() * bits_per_key).max(64);
        let nbytes = (nbits + 7) / 8;
        let k = ((bits_per_key as f64) * 0.69).max(1.0).min(30.0) as u32;
        let mut bits = vec![0u8; nbytes];
        for key in keys {
            let (h1, h2) = hash_pair(key);
            for i in 0..k {
                let h = h1.wrapping_add(u32::wrapping_mul(i, h2)) as usize % nbits;
                bits[h / 8] |= 1 << (h % 8);
            }
        }
        Self { bits, nbits, k }
    }

    pub fn may_contain(&self, key: &[u8]) -> bool {
        if self.bits.is_empty() || self.nbits == 0 {
            return true;
        }
        let (h1, h2) = hash_pair(key);
        for i in 0..self.k {
            let h = h1.wrapping_add(u32::wrapping_mul(i, h2)) as usize % self.nbits;
            if self.bits[h / 8] & (1 << (h % 8)) == 0 {
                return false;
            }
        }
        true
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 8 + self.bits.len());
        out.extend_from_slice(&self.k.to_le_bytes());
        out.extend_from_slice(&(self.nbits as u64).to_le_bytes());
        out.extend_from_slice(&self.bits);
        out
    }

    pub fn decode(data: &[u8]) -> crate::error::Result<Self> {
        if data.len() < 12 {
            return Err(crate::error::Error::Corruption("bloom too short".into()));
        }
        let k = u32::from_le_bytes(data[..4].try_into().unwrap());
        let nbits = u64::from_le_bytes(data[4..12].try_into().unwrap()) as usize;
        Ok(Self {
            k,
            nbits,
            bits: data[12..].to_vec(),
        })
    }
}

fn hash_pair(key: &[u8]) -> (u32, u32) {
    let h = crc32fast::hash(key);
    let mut buf = Vec::with_capacity(4 + key.len());
    buf.extend_from_slice(&h.to_le_bytes());
    buf.extend_from_slice(key);
    let h2 = crc32fast::hash(&buf).max(1);
    (h, h2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_false_negative() {
        let keys: Vec<_> = (0..100).map(|i| format!("key{i}").into_bytes()).collect();
        let bloom = BloomFilter::build(&keys, 10);
        let round = BloomFilter::decode(&bloom.encode()).unwrap();
        for k in &keys {
            assert!(bloom.may_contain(k));
            assert!(round.may_contain(k));
        }
        assert!(!bloom.may_contain(b"missing-key-xyz"));
    }
}
