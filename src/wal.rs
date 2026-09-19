use crate::error::{Error, Result};
use crate::key::{InternalKey, ValueKind};
use crate::options::FsyncMode;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAGIC: u32 = 0x4A_41_44_45; // "JADE"

pub struct Wal {
    path: PathBuf,
    file: File,
    fsync: FsyncMode,
    pending: usize,
    batch_bytes: usize,
}

impl Wal {
    pub fn create(path: impl AsRef<Path>, fsync: FsyncMode) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
        Ok(Self {
            path,
            file,
            fsync,
            pending: 0,
            batch_bytes: 256 * 1024,
        })
    }

    pub fn open_append(path: impl AsRef<Path>, fsync: FsyncMode) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            path,
            file,
            fsync,
            pending: 0,
            batch_bytes: 256 * 1024,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&mut self, key: &InternalKey, value: &[u8]) -> Result<()> {
        let mut rec = Vec::new();
        rec.extend_from_slice(&MAGIC.to_le_bytes());
        rec.extend_from_slice(&(key.user_key.len() as u32).to_le_bytes());
        rec.extend_from_slice(&key.user_key);
        rec.extend_from_slice(&key.seq.to_le_bytes());
        rec.push(key.kind as u8);
        rec.extend_from_slice(&(value.len() as u32).to_le_bytes());
        rec.extend_from_slice(value);
        let crc = crc32fast::hash(&rec);
        self.file.write_all(&(rec.len() as u32).to_le_bytes())?;
        self.file.write_all(&crc.to_le_bytes())?;
        self.file.write_all(&rec)?;
        self.pending += rec.len() + 8;
        match self.fsync {
            FsyncMode::Always => self.file.sync_all()?,
            FsyncMode::Batch if self.pending >= self.batch_bytes => {
                self.file.sync_all()?;
                self.pending = 0;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn sync(&mut self) -> Result<()> {
        if self.fsync != FsyncMode::Never {
            self.file.sync_all()?;
            self.pending = 0;
        }
        Ok(())
    }

    pub fn recover(path: impl AsRef<Path>) -> Result<Vec<(InternalKey, Vec<u8>)>> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let mut file = File::open(path)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        let mut out = Vec::new();
        let mut i = 0;
        while i + 8 <= buf.len() {
            let len = u32::from_le_bytes(buf[i..i + 4].try_into().unwrap()) as usize;
            let crc = u32::from_le_bytes(buf[i + 4..i + 8].try_into().unwrap());
            i += 8;
            if i + len > buf.len() {
                break; // truncated record after crash — stop
            }
            let rec = &buf[i..i + len];
            if crc32fast::hash(rec) != crc {
                return Err(Error::Corruption("wal checksum mismatch".into()));
            }
            i += len;
            let entry = decode_record(rec)?;
            out.push(entry);
        }
        Ok(out)
    }
}

fn decode_record(rec: &[u8]) -> Result<(InternalKey, Vec<u8>)> {
    if rec.len() < 4 + 4 + 8 + 1 + 4 {
        return Err(Error::Corruption("wal record too short".into()));
    }
    let mut o = 0;
    let magic = u32::from_le_bytes(rec[o..o + 4].try_into().unwrap());
    o += 4;
    if magic != MAGIC {
        return Err(Error::Corruption("wal bad magic".into()));
    }
    let klen = u32::from_le_bytes(rec[o..o + 4].try_into().unwrap()) as usize;
    o += 4;
    if o + klen + 8 + 1 + 4 > rec.len() {
        return Err(Error::Corruption("wal key truncated".into()));
    }
    let user_key = rec[o..o + klen].to_vec();
    o += klen;
    let seq = u64::from_le_bytes(rec[o..o + 8].try_into().unwrap());
    o += 8;
    let kind = ValueKind::from_u8(rec[o]).ok_or_else(|| Error::Corruption("wal kind".into()))?;
    o += 1;
    let vlen = u32::from_le_bytes(rec[o..o + 4].try_into().unwrap()) as usize;
    o += 4;
    if o + vlen != rec.len() {
        return Err(Error::Corruption("wal value length".into()));
    }
    let value = rec[o..o + vlen].to_vec();
    Ok((
        InternalKey {
            user_key,
            seq,
            kind,
        },
        value,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn wal_roundtrip_and_truncate() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("000001.log");
        {
            let mut wal = Wal::create(&path, FsyncMode::Always).unwrap();
            wal.append(&InternalKey::put(b"a".to_vec(), 1), b"1")
                .unwrap();
            wal.append(&InternalKey::put(b"b".to_vec(), 2), b"2")
                .unwrap();
        }
        let entries = Wal::recover(&path).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0.user_key, b"a");
    }
}
