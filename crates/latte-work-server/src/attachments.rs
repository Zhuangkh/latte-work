//! App-owned paste staging. Completed files remain available to saved prompts;
//! only incomplete, expired transfers are discarded automatically.
use anyhow::{Context, Result, bail};
use latte_work_platform::fs::PrivateDir;
use latte_work_protocol::FileEntry;
use std::{
    collections::HashMap,
    io::Write,
    path::Path,
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;

pub const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_CACHE: u64 = 2 * 1024 * 1024 * 1024;
struct Pending {
    file: NamedTempFile,
    name: String,
    size: u64,
    written: u64,
    touched: Instant,
}
pub struct Attachments {
    root: PrivateDir,
    pending: HashMap<String, Pending>,
}
impl Attachments {
    pub fn new(state: &Path) -> Result<Self> {
        Ok(Self {
            root: PrivateDir::prepare(&state.join("paste-tmp"))?,
            pending: HashMap::new(),
        })
    }
    pub fn is_completed(&self, path: &Path) -> bool {
        path.parent() == Some(self.root.path())
            && path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.len() > 37
                    && n.as_bytes()[36] == b'-'
                    && n.get(..36)
                        .is_some_and(|prefix| uuid::Uuid::parse_str(prefix).is_ok())
            })
    }
    pub fn begin(&mut self, name: String, size: u64) -> Result<String> {
        self.pending
            .retain(|_, p| p.touched.elapsed() < Duration::from_secs(600));
        if size > MAX_FILE {
            bail!("单个粘贴附件不能超过 64 MiB");
        }
        if name.is_empty()
            || name.len() > 180
            || name.contains(['/', '\\', '\0'])
            || name == "."
            || name == ".."
            || name.chars().any(char::is_control)
        {
            bail!("附件文件名无效");
        }
        latte_work_platform::paths::file_name(&name).context("附件文件名无效")?;
        if self.pending.len() >= 8 {
            bail!("正在处理的附件过多，请稍后重试");
        }
        let mut used = 0u64;
        for (i, entry) in self.root.path().read_dir()?.enumerate() {
            if i >= 4096 {
                bail!("附件临时目录已满");
            }
            let metadata = entry?.path().symlink_metadata()?;
            if metadata.is_file() {
                used += metadata.len();
            }
        }
        let reserved: u64 = self.pending.values().map(|p| p.size - p.written).sum();
        if used + reserved + size > MAX_CACHE {
            bail!("附件临时目录已达到 2 GiB 上限");
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.pending.insert(
            id.clone(),
            Pending {
                file: self.root.tempfile("attachment-", ".tmp")?,
                name,
                size,
                written: 0,
                touched: Instant::now(),
            },
        );
        Ok(id)
    }
    pub fn chunk(&mut self, id: &str, offset: u64, data: &[u8]) -> Result<()> {
        let p = self
            .pending
            .get_mut(id)
            .context("附件上传已失效，请重新粘贴")?;
        if p.touched.elapsed() >= Duration::from_secs(600)
            || data.is_empty()
            || data.len() > 65536
            || offset != p.written
            || p.written + data.len() as u64 > p.size
        {
            bail!("附件分块或偏移无效");
        }
        p.file.write_all(data)?;
        p.written += data.len() as u64;
        p.touched = Instant::now();
        Ok(())
    }
    pub fn finish(&mut self, id: &str) -> Result<FileEntry> {
        let p = self.pending.get(id).context("附件上传已失效，请重新粘贴")?;
        if p.written != p.size || p.touched.elapsed() >= Duration::from_secs(600) {
            bail!("附件内容不完整，请重新粘贴");
        }
        let p = self.pending.remove(id).context("附件上传已失效")?;
        p.file.as_file().sync_all()?;
        let path = self.root.path().join(format!("{id}-{}", p.name));
        p.file.persist_noclobber(&path)?;
        Ok(FileEntry {
            name: p.name,
            path: path.to_str().context("附件路径不是 UTF-8")?.into(),
            directory: false,
        })
    }
    pub fn abort(&mut self, id: &str) {
        self.pending.remove(id);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limits_pending_streams_and_preserves_completed_files_on_restart() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Attachments::new(root.path()).unwrap();
        let mut ids = Vec::new();
        for _ in 0..8 {
            ids.push(store.begin("test.bin".into(), 65537).unwrap());
        }
        assert!(store.begin("ninth".into(), 1).is_err());
        assert!(store.chunk(&ids[0], 0, &vec![0; 65537]).is_err());
        store.pending.get_mut(&ids[0]).unwrap().touched = Instant::now() - Duration::from_secs(601);
        assert!(store.chunk(&ids[0], 0, &[0]).is_err());
        let done = store.begin("empty.txt".into(), 0).unwrap();
        let file = store.finish(&done).unwrap();
        drop(store);
        let store = Attachments::new(root.path()).unwrap();
        assert!(Path::new(&file.path).is_file());
        assert_eq!(store.root.path().read_dir().unwrap().count(), 1);
    }
    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_cache_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("paste-tmp")).unwrap();
        assert!(Attachments::new(root.path()).is_err());
    }
    #[test]
    fn bounded_upload_is_atomic_private_and_cannot_escape() {
        let root = tempfile::tempdir().unwrap();
        let mut store = Attachments::new(root.path()).unwrap();
        for name in ["../secret", "/tmp/escape", "a\\b", "bad\nname"] {
            assert!(store.begin(name.into(), 1).is_err());
        }
        assert!(store.begin("huge".into(), MAX_FILE + 1).is_err());
        let id = store.begin("图片.png".into(), 3).unwrap();
        assert!(store.finish(&id).is_err());
        assert!(store.chunk(&id, 1, &[1]).is_err());
        assert!(store.chunk(&id, 0, &[0; 4]).is_err());
        store.chunk(&id, 0, &[1, 2, 3]).unwrap();
        assert!(store.chunk(&id, 0, &[1]).is_err());
        let entry = store.finish(&id).unwrap();
        assert_eq!(std::fs::read(&entry.path).unwrap(), [1, 2, 3]);
        latte_work_platform::fs::verify_private_file(&std::fs::File::open(&entry.path).unwrap())
            .unwrap();
        assert!(Path::new(&entry.path).starts_with(root.path().canonicalize().unwrap()));
        assert!(store.finish(&id).is_err());
        let aborted = store.begin("cancel.txt".into(), 1).unwrap();
        store.abort(&aborted);
        assert!(store.chunk(&aborted, 0, &[1]).is_err());
        assert_eq!(store.root.path().read_dir().unwrap().count(), 1);
    }
}
