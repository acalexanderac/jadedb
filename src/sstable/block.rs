use crate::error::{Error, Result};
use crate::key::InternalKey;

/// Uncompressed data block: [num:u32][entries...]
/// entry: [ik_len:u32][ik][v_len:u32][v]
#[derive(Debug, Clone)]
pub struct Block {
    data: Vec<u8>,
}

impl Block {
    pub fn from_raw(data: Vec<u8>) -> Self {
        Self { data }
    }

    pub fn raw(&self) -> &[u8] {
        &self.data
    }

    pub fn get(&self, user_key: &[u8], max_seq: u64) -> Option<(InternalKey, Vec<u8>)> {
        let mut best: Option<(InternalKey, Vec<u8>)> = None;
        for (ik, v) in self.iter_entries() {
            if ik.user_key.as_slice() == user_key && ik.seq <= max_seq {
                match &best {
                    None => best = Some((ik, v)),
                    Some((b, _)) if ik.seq > b.seq => best = Some((ik, v)),
                    _ => {}
                }
            }
        }
        best
    }

    pub fn iter_entries(&self) -> BlockEntryIter<'_> {
        let num = if self.data.len() >= 4 {
            u32::from_le_bytes(self.data[..4].try_into().unwrap()) as usize
        } else {
            0
        };
        BlockEntryIter {
            data: &self.data,
            offset: 4,
            remaining: num,
        }
    }
}

pub struct BlockEntryIter<'a> {
    data: &'a [u8],
    offset: usize,
    remaining: usize,
}

impl<'a> Iterator for BlockEntryIter<'a> {
    type Item = (InternalKey, Vec<u8>);

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        if self.offset + 4 > self.data.len() {
            return None;
        }
        let ik_len = u32::from_le_bytes(self.data[self.offset..self.offset + 4].try_into().ok()?)
            as usize;
        self.offset += 4;
        if self.offset + ik_len + 4 > self.data.len() {
            return None;
        }
        let ik = InternalKey::decode(&self.data[self.offset..self.offset + ik_len]).ok()?;
        self.offset += ik_len;
        let v_len =
            u32::from_le_bytes(self.data[self.offset..self.offset + 4].try_into().ok()?) as usize;
        self.offset += 4;
        if self.offset + v_len > self.data.len() {
            return None;
        }
        let v = self.data[self.offset..self.offset + v_len].to_vec();
        self.offset += v_len;
        self.remaining -= 1;
        Some((ik, v))
    }
}

#[derive(Debug, Default)]
pub struct BlockBuilder {
    entries: Vec<(Vec<u8>, Vec<u8>)>,
    size: usize,
}

impl BlockBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, ikey: &[u8], value: &[u8]) {
        self.size += ikey.len() + value.len() + 8;
        self.entries.push((ikey.to_vec(), value.to_vec()));
    }

    pub fn estimated_size(&self) -> usize {
        self.size
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn finish(self) -> Block {
        let mut data = Vec::new();
        data.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for (ik, v) in self.entries {
            data.extend_from_slice(&(ik.len() as u32).to_le_bytes());
            data.extend_from_slice(&ik);
            data.extend_from_slice(&(v.len() as u32).to_le_bytes());
            data.extend_from_slice(&v);
        }
        Block { data }
    }
}

/// Index block: sorted (separator_key, offset, length)
#[derive(Debug, Clone)]
pub struct IndexBlock {
    entries: Vec<(Vec<u8>, u64, u32)>,
}

impl IndexBlock {
    pub fn encode(&self) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for (k, off, len) in &self.entries {
            data.extend_from_slice(&(k.len() as u32).to_le_bytes());
            data.extend_from_slice(k);
            data.extend_from_slice(&off.to_le_bytes());
            data.extend_from_slice(&len.to_le_bytes());
        }
        data
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        if data.len() < 4 {
            return Err(Error::Corruption("index too short".into()));
        }
        let n = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;
        let mut o = 4;
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            if o + 4 > data.len() {
                return Err(Error::Corruption("index truncated".into()));
            }
            let klen = u32::from_le_bytes(data[o..o + 4].try_into().unwrap()) as usize;
            o += 4;
            if o + klen + 8 + 4 > data.len() {
                return Err(Error::Corruption("index entry truncated".into()));
            }
            let k = data[o..o + klen].to_vec();
            o += klen;
            let off = u64::from_le_bytes(data[o..o + 8].try_into().unwrap());
            o += 8;
            let len = u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
            o += 4;
            entries.push((k, off, len));
        }
        Ok(Self { entries })
    }

    pub fn find_block(&self, user_key: &[u8]) -> Option<(u64, u32)> {
        // separator is last key of block (encoded internal); find first sep >= user_key prefix
        let mut best = None;
        for (sep, off, len) in &self.entries {
            let sep_user = if sep.len() >= 8 {
                &sep[..sep.len() - 8]
            } else {
                sep.as_slice()
            };
            best = Some((*off, *len));
            if sep_user >= user_key {
                break;
            }
        }
        best
    }

    pub fn entries(&self) -> &[(Vec<u8>, u64, u32)] {
        &self.entries
    }

    pub fn push(&mut self, separator: Vec<u8>, offset: u64, length: u32) {
        self.entries.push((separator, offset, length));
    }

    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}
