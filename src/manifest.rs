use crate::error::{Error, Result};
use crate::options::FsyncMode;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FileMeta {
    pub file_num: u64,
    pub level: u32,
    pub smallest: Vec<u8>,
    pub largest: Vec<u8>,
    pub file_size: u64,
}

#[derive(Debug, Clone)]
pub enum ManifestEdit {
    AddFile(FileMeta),
    DeleteFile { level: u32, file_num: u64 },
    SetNextFileNum(u64),
    SetLastSeq(u64),
}

pub struct Manifest {
    path: PathBuf,
    file: File,
    fsync: FsyncMode,
}

impl Manifest {
    pub fn create(dir: impl AsRef<Path>, fsync: FsyncMode) -> Result<Self> {
        let path = dir.as_ref().join("MANIFEST");
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
        Ok(Self { path, file, fsync })
    }

    pub fn open(dir: impl AsRef<Path>, fsync: FsyncMode) -> Result<Self> {
        let path = dir.as_ref().join("MANIFEST");
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self { path, file, fsync })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn log(&mut self, edits: &[ManifestEdit]) -> Result<()> {
        for e in edits {
            let bytes = encode_edit(e);
            let crc = crc32fast::hash(&bytes);
            self.file.write_all(&(bytes.len() as u32).to_le_bytes())?;
            self.file.write_all(&crc.to_le_bytes())?;
            self.file.write_all(&bytes)?;
        }
        if self.fsync != FsyncMode::Never {
            self.file.sync_all()?;
        }
        Ok(())
    }

    pub fn recover(dir: impl AsRef<Path>) -> Result<RecoveredState> {
        let path = dir.as_ref().join("MANIFEST");
        let mut state = RecoveredState::default();
        if !path.exists() {
            return Ok(state);
        }
        let mut file = File::open(&path)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        let mut i = 0;
        while i + 8 <= buf.len() {
            let len = u32::from_le_bytes(buf[i..i + 4].try_into().unwrap()) as usize;
            let crc = u32::from_le_bytes(buf[i + 4..i + 8].try_into().unwrap());
            i += 8;
            if i + len > buf.len() {
                break;
            }
            let rec = &buf[i..i + len];
            if crc32fast::hash(rec) != crc {
                return Err(Error::Corruption("manifest checksum".into()));
            }
            i += len;
            apply_edit(&mut state, decode_edit(rec)?);
        }
        Ok(state)
    }
}

#[derive(Debug, Default)]
pub struct RecoveredState {
    pub files: Vec<FileMeta>,
    pub next_file_num: u64,
    pub last_seq: u64,
}

fn apply_edit(state: &mut RecoveredState, edit: ManifestEdit) {
    match edit {
        ManifestEdit::AddFile(m) => state.files.push(m),
        ManifestEdit::DeleteFile { level, file_num } => {
            state
                .files
                .retain(|f| !(f.level == level && f.file_num == file_num));
        }
        ManifestEdit::SetNextFileNum(n) => state.next_file_num = n,
        ManifestEdit::SetLastSeq(s) => state.last_seq = s,
    }
}

fn encode_edit(e: &ManifestEdit) -> Vec<u8> {
    let mut b = Vec::new();
    match e {
        ManifestEdit::AddFile(m) => {
            b.push(1);
            b.extend_from_slice(&m.file_num.to_le_bytes());
            b.extend_from_slice(&m.level.to_le_bytes());
            b.extend_from_slice(&(m.smallest.len() as u32).to_le_bytes());
            b.extend_from_slice(&m.smallest);
            b.extend_from_slice(&(m.largest.len() as u32).to_le_bytes());
            b.extend_from_slice(&m.largest);
            b.extend_from_slice(&m.file_size.to_le_bytes());
        }
        ManifestEdit::DeleteFile { level, file_num } => {
            b.push(2);
            b.extend_from_slice(&level.to_le_bytes());
            b.extend_from_slice(&file_num.to_le_bytes());
        }
        ManifestEdit::SetNextFileNum(n) => {
            b.push(3);
            b.extend_from_slice(&n.to_le_bytes());
        }
        ManifestEdit::SetLastSeq(s) => {
            b.push(4);
            b.extend_from_slice(&s.to_le_bytes());
        }
    }
    b
}

fn decode_edit(b: &[u8]) -> Result<ManifestEdit> {
    if b.is_empty() {
        return Err(Error::Corruption("empty edit".into()));
    }
    let mut o = 1;
    match b[0] {
        1 => {
            let file_num = u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
            o += 8;
            let level = u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
            o += 4;
            let sl = u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
            o += 4;
            let smallest = b[o..o + sl].to_vec();
            o += sl;
            let ll = u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
            o += 4;
            let largest = b[o..o + ll].to_vec();
            o += ll;
            let file_size = u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
            Ok(ManifestEdit::AddFile(FileMeta {
                file_num,
                level,
                smallest,
                largest,
                file_size,
            }))
        }
        2 => {
            let level = u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
            o += 4;
            let file_num = u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
            Ok(ManifestEdit::DeleteFile { level, file_num })
        }
        3 => Ok(ManifestEdit::SetNextFileNum(u64::from_le_bytes(
            b[o..o + 8].try_into().unwrap(),
        ))),
        4 => Ok(ManifestEdit::SetLastSeq(u64::from_le_bytes(
            b[o..o + 8].try_into().unwrap(),
        ))),
        t => Err(Error::Corruption(format!("unknown edit {t}"))),
    }
}
