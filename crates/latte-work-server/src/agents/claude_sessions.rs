//! Claude session metadata: bounded incremental reads and append-only offline rename.
//! Matches the official SDK customTitle > aiTitle > summary ordering. No transcript rewrite.
use anyhow::{Context, Result, bail};
use latte_work_platform::fs::is_link;
use serde_json::{Value, json};
#[cfg(test)]
use std::fs::OpenOptions;
use std::{
    collections::HashMap,
    fs::Metadata,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Instant, SystemTime},
};
const WINDOW: u64 = 64 * 1024;
const SCAN_BUDGET: u64 = 4 * 1024 * 1024;
const INDEX_LIMIT: usize = 256;

#[derive(Default)]
struct Titles {
    custom: Option<String>,
    generated: Option<String>,
    summary: Option<String>,
}
impl Titles {
    fn record(&mut self, line: &[u8]) {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        // Only native metadata records may name the parent session. User messages
        // and tool payloads can contain identically named fields.
        let slot = match value.get("type").and_then(Value::as_str) {
            Some("custom-title") => (&mut self.custom, "customTitle"),
            Some("ai-title") => (&mut self.generated, "aiTitle"),
            Some("summary") => (&mut self.summary, "summary"),
            _ => return,
        };
        if let Some(text) = value
            .get(slot.1)
            .and_then(Value::as_str)
            .filter(|s| valid_title(s))
        {
            *slot.0 = Some(text.trim().to_owned());
        }
    }
    fn title(&self) -> Option<Title> {
        self.custom
            .as_ref()
            .map(|text| Title {
                text: text.clone(),
                custom: true,
            })
            .or_else(|| {
                self.generated
                    .as_ref()
                    .or(self.summary.as_ref())
                    .map(|text| Title {
                        text: text.clone(),
                        custom: false,
                    })
            })
    }
}
struct TitleIndex {
    identity: String,
    size: u64,
    modified: SystemTime,
    offset: u64,
    line: Vec<u8>,
    oversized: bool,
    titles: Titles,
    touched: Instant,
}
impl TitleIndex {
    fn new(meta: &Metadata, identity: String, modified: SystemTime) -> Self {
        Self {
            identity,
            size: meta.len(),
            modified,
            offset: 0,
            line: Vec::new(),
            oversized: false,
            titles: Titles::default(),
            touched: Instant::now(),
        }
    }
    fn read(&mut self, opened: &mut native::SessionFile, meta: &Metadata) -> Result<Option<Title>> {
        let identity = native::identity(opened)?;
        let modified = meta.modified()?;
        if self.identity != identity
            || meta.len() < self.size
            || (meta.len() == self.size && self.modified != modified)
        {
            *self = Self::new(meta, identity, modified);
        }
        self.size = meta.len();
        self.modified = modified;
        self.touched = Instant::now();
        let file = opened.file_mut();
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        file.take(SCAN_BUDGET.min(self.size - self.offset))
            .read_to_end(&mut bytes)?;
        for part in bytes.split_inclusive(|b| *b == b'\n') {
            if !self.oversized && self.line.len() + part.len() <= WINDOW as usize {
                self.line.extend_from_slice(part);
            } else {
                self.line.clear();
                self.oversized = true;
            }
            if part.ends_with(b"\n") {
                if !self.oversized {
                    self.titles.record(&self.line);
                }
                self.line.clear();
                self.oversized = false;
            }
        }
        self.offset += bytes.len() as u64;
        // A partial scan must never roll a confirmed name back to an older record.
        // Subsequent metadata reads continue from this offset, not from the head.
        Ok(
            (self.offset == self.size && self.line.is_empty() && !self.oversized)
                .then(|| self.titles.title())
                .flatten(),
        )
    }
}
fn indexes() -> &'static Mutex<HashMap<PathBuf, TitleIndex>> {
    static INDEXES: OnceLock<Mutex<HashMap<PathBuf, TitleIndex>>> = OnceLock::new();
    INDEXES.get_or_init(|| Mutex::new(HashMap::new()))
}
#[derive(Clone)]
pub struct Title {
    pub text: String,
    pub custom: bool,
}
fn valid_title(value: &str) -> bool {
    !value.trim().is_empty() && value.chars().count() <= 200 && !value.chars().any(char::is_control)
}
fn projects() -> Result<PathBuf> {
    let config = crate::agent_environment::variable("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            crate::agent_environment::variable("HOME").map(|p| PathBuf::from(p).join(".claude"))
        })
        .or_else(|| {
            latte_work_platform::dirs::home_dir()
                .ok()
                .map(|p| p.join(".claude"))
        })
        .context("无法确定 Claude 会话目录")?;
    Ok(config.join("projects"))
}
fn locate_in(projects: &Path, cwd: &Path, id: &str) -> Result<Option<PathBuf>> {
    uuid::Uuid::parse_str(id).context("原生会话 ID 无效")?;
    // Require the canonical UUID spelling; never use an input as a free-form path.
    let filename = format!("{}.jsonl", uuid::Uuid::parse_str(id)?);
    let sanitized = native::project_key(&cwd.canonicalize()?);
    let root = match projects.canonicalize() {
        Ok(root) => root,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let search_root = native::search_root(projects, &root)?;
    let mut candidates = if sanitized.len() <= 200 {
        vec![search_root.join(&sanitized).join(&filename)]
    } else {
        // Native Bun and SDK hash suffixes differ for long paths; match the exact UUID
        // beneath the shared 200-character prefix, bounded and ambiguity checked.
        let prefix = format!("{}-", &sanitized[..200]);
        let mut paths = vec![];
        for (n, entry) in search_root.read_dir()?.enumerate() {
            if n >= 1024 {
                bail!("Claude 会话目录超出读取范围");
            }
            let entry = entry?;
            if entry.file_type()?.is_dir()
                && entry.file_name().to_string_lossy().starts_with(&prefix)
            {
                paths.push(entry.path().join(&filename));
            }
        }
        paths
    };
    candidates.retain(|p| p.is_file());
    if candidates.len() > 1 {
        bail!("原生会话目录存在歧义");
    }
    let Some(path) = candidates.pop() else {
        return Ok(None);
    };
    let target = path.canonicalize()?;
    if !target.starts_with(&root) || is_link(&path.symlink_metadata()?) {
        bail!("拒绝访问 Claude 会话目录外的路径");
    }
    Ok(Some(native::located_path(path, target)))
}
fn sanitized_project_key(path: &str) -> String {
    path.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect()
}
fn read_path(path: &Path) -> Result<Option<Title>> {
    let mut opened = native::SessionFile::open(path, false)?;
    let metadata = opened.file().metadata()?;
    if !metadata.is_file() {
        bail!("Claude 会话不是普通文件");
    }
    let mut indexes = indexes()
        .lock()
        .map_err(|_| anyhow::anyhow!("原生标题索引锁异常"))?;
    if !indexes.contains_key(path)
        && indexes.len() >= INDEX_LIMIT
        && let Some(oldest) = indexes
            .iter()
            .filter(|(_, entry)| entry.offset == entry.size)
            .min_by_key(|(_, entry)| entry.touched)
            .map(|(key, _)| key.clone())
    {
        indexes.remove(&oldest);
    }
    if !indexes.contains_key(path) && indexes.len() >= INDEX_LIMIT {
        // Finish active scans before admitting more files; otherwise a long list
        // could repeatedly evict every unfinished scan before it reaches its end.
        return Ok(None);
    }
    let identity = native::identity(&opened)?;
    let modified = metadata.modified()?;
    indexes
        .entry(path.to_owned())
        .or_insert_with(|| TitleIndex::new(&metadata, identity, modified))
        .read(&mut opened, &metadata)
}
/// Offline rename always verifies the latest bounded tail, even if a large file's
/// incremental index has not caught up yet. Partial tail records are ignored.
fn read_tail(path: &Path) -> Result<Option<Title>> {
    let mut opened = native::SessionFile::open(path, false)?;
    let file = opened.file_mut();
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("Claude 会话不是普通文件");
    }
    let start = metadata.len().saturating_sub(WINDOW);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(WINDOW).read_to_end(&mut bytes)?;
    let mut records = bytes.split_inclusive(|b| *b == b'\n');
    if start != 0 {
        records.next();
    }
    let mut titles = Titles::default();
    for line in records.filter(|line| line.ends_with(b"\n")) {
        titles.record(line);
    }
    Ok(titles.title())
}

pub fn read(cwd: &Path, id: &str) -> Result<Option<Title>> {
    let Some(path) = locate_in(&projects()?, cwd, id)? else {
        return Ok(None);
    };
    read_path(&path)
}

pub(super) struct ChildBytes {
    pub bytes: Vec<u8>,
    pub start: u64,
    pub truncated: bool,
}

/// Construct the native child path from registered identities, never output_file.
pub(super) fn read_child(cwd: &Path, parent: &str, child: &str) -> Result<Option<ChildBytes>> {
    if child.is_empty()
        || child.len() > 128
        || !child
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Ok(None);
    }
    let Some(parent_path) = locate_in(&projects()?, cwd, parent)? else {
        return Ok(None);
    };
    read_child_path(&parent_path, child)
}

fn open_child_path(parent: &Path, child: &str) -> Result<Option<native::SessionFile>> {
    let session_dir = parent.with_extension("");
    let directory = session_dir.join("subagents");
    for path in [&session_dir, &directory] {
        match path.symlink_metadata() {
            Ok(meta) if meta.is_dir() && !is_link(&meta) => {}
            Ok(_) => bail!("子会话目录不是普通目录"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        }
    }
    let path = directory.join(format!("agent-{child}.jsonl"));
    match path.symlink_metadata() {
        Ok(meta) if meta.is_file() && !is_link(&meta) => {}
        Ok(_) => bail!("子会话记录不是普通文件"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    Ok(Some(native::open_child(&path)?))
}
fn read_child_path(parent: &Path, child: &str) -> Result<Option<ChildBytes>> {
    const LIMIT: u64 = 512 * 1024;
    let Some(mut opened) = open_child_path(parent, child)? else {
        return Ok(None);
    };
    let file = opened.file_mut();
    let start = file.metadata()?.len().saturating_sub(LIMIT);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(LIMIT).read_to_end(&mut bytes)?;
    Ok(Some(ChildBytes {
        bytes,
        start,
        truncated: start != 0,
    }))
}

pub(super) struct ChildPageBytes {
    pub bytes: Vec<u8>,
    pub cursor: super::ChildHistoryCursor,
    pub has_more: bool,
}
/// Read whole records in bounded forward pages. A partial last line is retried
/// after the producer appends its newline, never skipped or treated as complete.
pub(super) fn read_child_page(
    cwd: &Path,
    parent: &str,
    child: &str,
    cursor: super::ChildHistoryCursor,
) -> Result<Option<ChildPageBytes>> {
    if child.is_empty()
        || child.len() > 128
        || !child
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Ok(None);
    }
    let Some(parent_path) = locate_in(&projects()?, cwd, parent)? else {
        return Ok(None);
    };
    read_child_page_path(&parent_path, child, cursor)
}
fn read_child_page_path(
    parent: &Path,
    child: &str,
    mut cursor: super::ChildHistoryCursor,
) -> Result<Option<ChildPageBytes>> {
    let Some(mut opened) = open_child_path(parent, child)? else {
        return Ok(None);
    };
    let identity = native::identity(&opened)?;
    let file = opened.file_mut();
    let metadata = file.metadata()?;
    if !cursor.identity.is_empty() && cursor.identity != identity {
        bail!("子会话原始文件已替换，请核对记录");
    }
    if cursor.offset > metadata.len() {
        bail!("子会话原始记录缩短，请核对记录");
    }
    cursor.identity = identity;
    file.seek(SeekFrom::Start(cursor.offset))?;
    let mut reader = BufReader::new(file.take(metadata.len() - cursor.offset));
    let mut bytes = Vec::new();
    let mut partial_tail = false;
    while bytes.len() < 512 * 1024 {
        let mut line = Vec::new();
        // Each native record is independently bounded, with an explicit error
        // instead of silently clipping its text or skipping its remaining bytes.
        (&mut reader)
            .take((latte_work_protocol::MAX_FRAME - 4096 + 1) as u64)
            .read_until(b'\n', &mut line)?;
        if line.is_empty() {
            break;
        }
        if line.strip_suffix(b"\n").unwrap_or(&line).len() > latte_work_protocol::MAX_FRAME - 4096 {
            bail!("子会话单条原始记录超出安全读取范围，内容未截断");
        }
        if !line.ends_with(b"\n") {
            partial_tail = true;
            break;
        }
        bytes.extend(line);
    }
    cursor.pending_tail = partial_tail;
    let has_more = !partial_tail && cursor.offset + (bytes.len() as u64) < metadata.len();
    Ok(Some(ChildPageBytes {
        bytes,
        cursor,
        has_more,
    }))
}
pub fn rename(cwd: &Path, id: &str, title: &str) -> Result<String> {
    let path = locate_in(&projects()?, cwd, id)?.context("未找到原生会话，标题未修改")?;
    append(&path, id, title)?;
    let actual = read_tail(&path)?.context("原生标题写入后未能确认")?;
    if !actual.custom || actual.text != title.trim() {
        bail!("原生标题写入后发生变化，请核对后重试");
    }
    Ok(actual.text)
}
fn append(path: &Path, id: &str, title: &str) -> Result<()> {
    if !valid_title(title) {
        bail!("原生会话标题无效");
    }
    let mut opened = native::SessionFile::open(path, true)?;
    let file = opened.file_mut();
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() == 0 {
        bail!("原生会话文件不可用，标题未修改");
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0];
    file.read_exact(&mut last)?;
    if last != *b"\n" {
        bail!("原生会话记录尚未写入完整，标题未修改");
    }
    // Same append-only mutation as SDK rename_session; never create a missing session.
    let mut record = serde_json::to_vec(
        &json!({"type":"custom-title","customTitle":title.trim(),"sessionId":id}),
    )?;
    record.push(b'\n');
    if file.write(&record)? != record.len() {
        bail!("原生标题写入未完成，请核对会话");
    }
    file.sync_data()?;
    Ok(())
}
#[cfg(unix)]
mod native {
    use super::*;
    use std::{
        fs::{File, OpenOptions},
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };

    pub(super) struct SessionFile(File);

    impl SessionFile {
        pub(super) fn open(path: &Path, append: bool) -> Result<Self> {
            let expected = path.symlink_metadata()?;
            if !expected.is_file() || path.canonicalize()? != path {
                bail!("原生会话路径已改变");
            }
            let file = OpenOptions::new()
                .read(true)
                .append(append)
                .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
                .open(path)?;
            let actual = file.metadata()?;
            if !actual.is_file()
                || actual.dev() != expected.dev()
                || actual.ino() != expected.ino()
                || path.canonicalize()? != path
            {
                bail!("原生会话文件身份已改变");
            }
            Ok(Self(file))
        }
        pub(super) fn file(&self) -> &File {
            &self.0
        }
        pub(super) fn file_mut(&mut self) -> &mut File {
            &mut self.0
        }
    }

    pub(super) fn identity(file: &SessionFile) -> Result<String> {
        let metadata = file.file().metadata()?;
        Ok(format!("{}:{}", metadata.dev(), metadata.ino()))
    }
    pub(super) fn open_child(path: &Path) -> Result<SessionFile> {
        let file = SessionFile::open(path, false)?;
        if file.file().metadata()?.uid() != nix::unistd::Uid::current().as_raw() {
            bail!("子会话记录属于其他用户");
        }
        Ok(file)
    }
    pub(super) fn project_key(path: &Path) -> String {
        sanitized_project_key(&path.to_string_lossy())
    }
    pub(super) fn search_root(_projects: &Path, canonical: &Path) -> Result<PathBuf> {
        Ok(canonical.to_owned())
    }
    pub(super) fn located_path(_path: PathBuf, canonical: PathBuf) -> PathBuf {
        canonical
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    pub(super) use latte_work_platform::fs::NativeSessionFile as SessionFile;

    pub(super) fn identity(file: &SessionFile) -> Result<String> {
        let identity = file.identity()?;
        Ok(format!(
            "{}:{}",
            identity.volume,
            uuid::Uuid::from_bytes(identity.file_id).simple()
        ))
    }
    pub(super) fn open_child(path: &Path) -> Result<SessionFile> {
        // The constructor verifies the owner SID of every opened native file.
        Ok(SessionFile::open(path, false)?)
    }
    pub(super) fn project_key(path: &Path) -> String {
        let text = path.to_string_lossy();
        // canonicalize adds a Win32 namespace prefix absent from Claude's cwd.
        let text = if matches!(path.components().next(), Some(std::path::Component::Prefix(p))
            if matches!(p.kind(), std::path::Prefix::VerbatimDisk(_)))
        {
            text.strip_prefix(r"\\?\").unwrap_or(&text)
        } else {
            &text
        };
        sanitized_project_key(text)
    }
    pub(super) fn search_root(projects: &Path, _canonical: &Path) -> Result<PathBuf> {
        Ok(std::path::absolute(projects)?)
    }
    pub(super) fn located_path(path: PathBuf, _canonical: PathBuf) -> PathBuf {
        path
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn child_record_reads_are_bounded_and_reject_non_files_and_invalid_ids() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let parent = root.join("11111111-1111-4111-8111-111111111111.jsonl");
        let child_dir = parent.with_extension("").join("subagents");
        std::fs::create_dir_all(&child_dir).unwrap();
        let file = child_dir.join("agent-a123.jsonl");
        std::fs::write(&file, vec![b'x'; 600 * 1024]).unwrap();
        let bytes = super::read_child_path(&parent, "a123").unwrap().unwrap();
        assert_eq!(bytes.bytes.len(), 512 * 1024);
        assert!(bytes.truncated && bytes.start > 0);
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        assert!(super::read_child_path(&parent, "a123").is_err());
        #[cfg(unix)]
        {
            std::fs::remove_dir(&file).unwrap();
            let secret = root.join("secret.jsonl");
            std::fs::write(&secret, "private").unwrap();
            std::os::unix::fs::symlink(&secret, &file).unwrap();
            assert!(super::read_child_path(&parent, "a123").is_err());
            std::fs::remove_file(&file).unwrap();
            std::fs::remove_dir(&child_dir).unwrap();
            std::os::unix::fs::symlink(&root, &child_dir).unwrap();
            assert!(super::read_child_path(&parent, "a123").is_err());
        }
        assert!(
            super::read_child(&root, "parent", "../../secret")
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn child_pages_cover_entire_file_and_retry_partial_tail_without_skipping() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().canonicalize().unwrap().join("parent.jsonl");
        std::fs::write(&parent, "").unwrap();
        let children = parent.with_extension("").join("subagents");
        std::fs::create_dir_all(&children).unwrap();
        let path = children.join("agent-a123.jsonl");
        let records = (0..700)
            .map(|i| format!("{{\"n\":{i},\"text\":\"{}\"}}\n", "x".repeat(1024)))
            .collect::<String>();
        std::fs::write(&path, format!("{records}{{\"partial\":true}}")).unwrap();
        let mut cursor = super::super::ChildHistoryCursor::default();
        let mut found = Vec::new();
        loop {
            let page = super::read_child_page_path(&parent, "a123", cursor)
                .unwrap()
                .unwrap();
            cursor = page.cursor;
            cursor.offset += page.bytes.len() as u64;
            found.extend(page.bytes);
            if !page.has_more {
                break;
            }
        }
        assert_eq!(found, records.as_bytes());
        assert_eq!(cursor.offset, records.len() as u64);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        let page = super::read_child_page_path(&parent, "a123", cursor)
            .unwrap()
            .unwrap();
        assert_eq!(page.bytes, b"{\"partial\":true}\n");
        let mut invalid = page.cursor;
        invalid.identity = "replaced".into();
        assert!(super::read_child_page_path(&parent, "a123", invalid).is_err());
    }
    #[test]
    fn child_record_limit_excludes_newline_and_retries_an_exact_boundary_tail() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().canonicalize().unwrap().join("parent.jsonl");
        std::fs::write(&parent, "").unwrap();
        let children = parent.with_extension("").join("subagents");
        std::fs::create_dir_all(&children).unwrap();
        let path = children.join("agent-a123.jsonl");
        let limit = latte_work_protocol::MAX_FRAME - 4096;
        let record = format!("{{\"text\":\"{}\"}}", "x".repeat(limit - 11));
        assert_eq!(record.len(), limit);
        std::fs::write(&path, &record).unwrap();
        let cursor = super::super::ChildHistoryCursor::default();
        let partial = super::read_child_page_path(&parent, "a123", Default::default())
            .unwrap()
            .unwrap();
        assert!(partial.bytes.is_empty() && partial.cursor.pending_tail);
        std::fs::write(&path, format!("{record}\n")).unwrap();
        let page = super::read_child_page_path(&parent, "a123", partial.cursor)
            .unwrap()
            .unwrap();
        assert_eq!(page.bytes, format!("{record}\n").as_bytes());
        std::fs::write(&path, format!("{record} \n")).unwrap();
        assert!(super::read_child_page_path(&parent, "a123", cursor).is_err());
    }
    use super::*;
    #[test]
    fn native_titles_follow_sdk_priority_and_rename_preserves_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap().join("session.jsonl");
        let original = "{\"type\":\"user\",\"message\":{\"content\":\"prompt\"}}\n{\"type\":\"ai-title\",\"aiTitle\":\"Generated title\"}\n";
        std::fs::write(&path, original).unwrap();
        assert_eq!(read_path(&path).unwrap().unwrap().text, "Generated title");
        append(
            &path,
            "11111111-1111-4111-8111-111111111111",
            "Manual title",
        )
        .unwrap();
        let title = read_path(&path).unwrap().unwrap();
        assert!(title.custom);
        assert_eq!(title.text, "Manual title");
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .starts_with(original)
        );
        append(&path, "id", "Second title").unwrap();
        assert_eq!(read_path(&path).unwrap().unwrap().text, "Second title");
        assert!(append(&dir.path().join("missing.jsonl"), "id", "No fake session").is_err());
    }
    #[test]
    fn titles_in_the_middle_survive_transcript_growth_and_ignore_payload_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap().join("session.jsonl");
        let padding = format!(
            "{{\"type\":\"assistant\",\"text\":\"{}\"}}\n",
            "x".repeat(2048)
        )
        .repeat(40);
        std::fs::write(
            &path,
            format!("{padding}{{\"type\":\"ai-title\",\"aiTitle\":\"AI name\"}}\n{padding}"),
        )
        .unwrap();
        assert_eq!(read_path(&path).unwrap().unwrap().text, "AI name");
        append(&path, "id", "Native name").unwrap();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(padding.as_bytes()).unwrap();
        writeln!(
            file,
            "{{\"type\":\"user\",\"customTitle\":\"Not metadata\"}}"
        )
        .unwrap();
        writeln!(
            file,
            "{{\"type\":\"ai-title\",\"aiTitle\":\"Later AI name\"}}"
        )
        .unwrap();
        let title = read_path(&path).unwrap().unwrap();
        assert_eq!(title.text, "Native name");
        assert!(title.custom);
    }
    #[test]
    fn large_title_scans_resume_without_exposing_stale_names_and_retry_partial_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap().join("session.jsonl");
        let mut file = OpenOptions::new()
            .append(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        writeln!(
            file,
            "{{\"type\":\"custom-title\",\"customTitle\":\"Old name\"}}"
        )
        .unwrap();
        // An oversized transcript record crosses the scan boundary and must not
        // consume unbounded memory or prevent subsequent metadata from being read.
        file.write_all(&vec![b'x'; SCAN_BUDGET as usize]).unwrap();
        writeln!(
            file,
            "\n{{\"type\":\"custom-title\",\"customTitle\":\"Latest name\"}}"
        )
        .unwrap();
        assert!(read_path(&path).unwrap().is_none());
        // Offline verification must also work before the index catches up.
        append(&path, "id", "Offline latest").unwrap();
        assert_eq!(read_tail(&path).unwrap().unwrap().text, "Offline latest");
        assert_eq!(read_path(&path).unwrap().unwrap().text, "Offline latest");
        file.write_all(b"{\"type\":\"custom-title\",\"customTitle\":\"Partial")
            .unwrap();
        assert!(read_path(&path).unwrap().is_none());
        assert!(append(&path, "id", "Rejected").is_err());
        file.write_all(b" name\"}\n").unwrap();
        assert_eq!(read_path(&path).unwrap().unwrap().text, "Partial name");
        std::fs::write(&path, "{\"type\":\"ai-title\",\"aiTitle\":\"Replaced\"}\n").unwrap();
        let title = read_path(&path).unwrap().unwrap();
        assert_eq!(title.text, "Replaced");
        assert!(!title.custom);
        let replacement = path.with_extension("new");
        std::fs::write(
            &replacement,
            "{\"type\":\"custom-title\",\"customTitle\":\"New inode\"}\n",
        )
        .unwrap();
        std::fs::rename(replacement, &path).unwrap();
        assert_eq!(read_path(&path).unwrap().unwrap().text, "New inode");
    }
    #[test]
    fn session_paths_reject_invalid_ids_symlinks_and_keep_reads_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("project");
        std::fs::create_dir(&cwd).unwrap();
        let projects = dir.path().join("projects");
        std::fs::create_dir(&projects).unwrap();
        assert!(locate_in(&projects, &cwd, "../escape").is_err());
        let name = native::project_key(&cwd.canonicalize().unwrap());
        let folder = projects.join(name);
        std::fs::create_dir(&folder).unwrap();
        let id = "11111111-1111-4111-8111-111111111111";
        #[cfg(unix)]
        {
            let external = dir.path().join("private.jsonl");
            std::fs::write(&external, "private").unwrap();
            let link = folder.join(format!("{id}.jsonl"));
            std::os::unix::fs::symlink(&external, &link).unwrap();
            assert!(locate_in(&projects, &cwd, id).is_err());
            assert!(read_path(&link).is_err());
            assert!(append(&link, id, "Rejected").is_err());
            assert_eq!(std::fs::read(&external).unwrap(), b"private");
            std::fs::remove_file(link).unwrap();
        }
        assert!(locate_in(&projects, &cwd, id).unwrap().is_none());
        let large = folder.join(format!("{id}.jsonl"));
        let mut data = vec![b'x'; (WINDOW * 3) as usize];
        data.extend(b"\n{\"type\":\"ai-title\",\"aiTitle\":\"Tail title\"}\n");
        std::fs::write(&large, data).unwrap();
        let located = locate_in(&projects, &cwd, id).unwrap().unwrap();
        assert_eq!(read_path(&located).unwrap().unwrap().text, "Tail title");
    }
    #[cfg(windows)]
    #[test]
    fn project_keys_match_native_drive_paths_without_verbatim_prefix() {
        assert_eq!(
            native::project_key(Path::new(r"C:\work\project")),
            "C--work-project"
        );
        assert_eq!(
            native::project_key(Path::new(r"\\?\C:\work\project")),
            "C--work-project"
        );
    }

    #[test]
    fn child_pages_reject_real_replacement_and_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().canonicalize().unwrap().join("parent.jsonl");
        let children = parent.with_extension("").join("subagents");
        std::fs::create_dir_all(&children).unwrap();
        let path = children.join("agent-a123.jsonl");
        std::fs::write(&path, b"{\"n\":1}\n").unwrap();
        let first = read_child_page_path(&parent, "a123", Default::default())
            .unwrap()
            .unwrap();
        let mut cursor = first.cursor;
        cursor.offset = first.bytes.len() as u64;
        std::fs::write(&path, b"x").unwrap();
        let truncated_cursor = super::super::ChildHistoryCursor {
            identity: cursor.identity.clone(),
            offset: cursor.offset,
            ..Default::default()
        };
        assert!(read_child_page_path(&parent, "a123", truncated_cursor).is_err());
        // Keep the old object alive by name so the replacement cannot reuse its ID.
        std::fs::rename(&path, children.join("old.jsonl")).unwrap();
        std::fs::write(&path, b"{\"n\":2}\n").unwrap();
        assert!(read_child_page_path(&parent, "a123", cursor).is_err());
    }

    #[test]
    fn invalid_title_and_malformed_metadata_do_not_rewrite_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap().join("session.jsonl");
        let contents = b"not json\n{\"type\":\"summary\",\"summary\":\"Summary\"}\n{\"type\":\"custom-title\",\"customTitle\":\"\\n\"}\n{\"type\":\"ai-title\",\"aiTitle\":\"Generated\"}\n";
        std::fs::write(&path, contents).unwrap();
        let title = read_path(&path).unwrap().unwrap();
        assert_eq!(title.text, "Generated");
        assert!(!title.custom);
        assert_eq!(std::fs::read(&path).unwrap(), contents);
        for invalid in [" ", "line\nbreak", &"x".repeat(201)] {
            assert!(append(&path, "id", invalid).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), contents);
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn native_session_and_child_reads_reject_junction_ancestry() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_owned();
        let cwd = root.join("project");
        std::fs::create_dir(&cwd).unwrap();
        let projects = root.join("projects");
        let folder = projects.join(native::project_key(&cwd.canonicalize().unwrap()));
        std::fs::create_dir_all(&folder).unwrap();
        let id = "11111111-1111-4111-8111-111111111111";
        let parent = folder.join(format!("{id}.jsonl"));
        let original = b"{\"aiTitle\":\"Original\"}\n";
        std::fs::write(&parent, original).unwrap();
        let children = parent.with_extension("").join("subagents");
        std::fs::create_dir_all(&children).unwrap();
        std::fs::write(children.join("agent-a123.jsonl"), b"{\"n\":1}\n").unwrap();
        let saved = children.with_extension("saved");
        std::fs::rename(&children, &saved).unwrap();
        let alias = root.join("projects-alias");
        let mut command = tokio::process::Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:LATTE_SESSION_TEST_ALIAS -Target $env:LATTE_SESSION_TEST_PROJECTS | Out-Null; New-Item -ItemType Junction -Path $env:LATTE_SESSION_TEST_CHILDREN -Target $env:LATTE_SESSION_TEST_SAVED | Out-Null"])
            .env("LATTE_SESSION_TEST_ALIAS", &alias)
            .env("LATTE_SESSION_TEST_PROJECTS", &projects)
            .env("LATTE_SESSION_TEST_CHILDREN", &children)
            .env("LATTE_SESSION_TEST_SAVED", &saved);
        let (status, _) = latte_work_platform::process::output(
            &mut command,
            std::time::Duration::from_secs(10),
            4096,
        )
        .await
        .unwrap();
        assert!(status.success());
        let located = locate_in(&alias, &cwd, id).unwrap().unwrap();
        assert!(
            located.starts_with(&alias),
            "retain the original ancestry until safe open"
        );
        assert!(read_path(&located).is_err());
        assert!(append(&located, id, "Rejected").is_err());
        assert!(read_child_path(&parent, "a123").is_err());
        assert!(read_child_page_path(&parent, "a123", Default::default()).is_err());
        assert_eq!(std::fs::read(&parent).unwrap(), original);
        std::fs::remove_dir(children).unwrap();
        std::fs::remove_dir(alias).unwrap();
    }
}
