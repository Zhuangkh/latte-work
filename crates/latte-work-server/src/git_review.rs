//! Bounded, read-only Git review. Branch diffs use resolved commit IDs, not live refs.
use crate::{files, managed_recovery::ManagedRecoveryExclusions};
use anyhow::{Context, Result, bail};
use latte_work_protocol::{
    ChangeSection, GitCommit, GitInfo, GitRef, GitReview, GitReviewFile, GitReviewScope,
};
use std::{
    collections::HashMap,
    path::{Component, Path},
    time::Duration,
};
use tokio::io::AsyncReadExt;
const FILE_LIMIT: usize = 1000;
const CONTENT_LIMIT: usize = 512 * 1024;

async fn command(root: &Path, args: &[&str]) -> Result<String> {
    let (text, truncated) = files::git(root, args).await?;
    if truncated {
        bail!("Git 元数据超出读取上限，请缩小项目范围");
    }
    Ok(text.trim_end_matches('\n').into())
}
async fn commit(root: &Path, reference: &str) -> Result<String> {
    if reference.is_empty()
        || reference.len() > 256
        || reference.starts_with('-')
        || reference.chars().any(char::is_control)
        || reference.contains([':', '~', '^', '*', '?', '[', '\\'])
        || reference.contains("..")
    {
        bail!("无效的 Git 比较基准");
    }
    command(
        root,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
    )
    .await
    .context("比较基准不存在或不是提交")
}
fn valid_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn valid_path(path: &str) -> Result<()> {
    latte_work_platform::paths::relative(Path::new(path))?;
    if path.is_empty()
        || path.len() > 4096
        || path.contains('\0')
        || Path::new(path).is_absolute()
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("路径必须位于项目目录内");
    }
    Ok(())
}
pub async fn info(root: &Path) -> Result<GitInfo> {
    tokio::time::timeout(Duration::from_secs(20), async {
        command(root, &["rev-parse", "--show-prefix"]).await?;
        let branch = command(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .await
            .ok();
        let head = commit(root, "HEAD").await.ok();
        let (text, mut truncated) = files::git(
            root,
            &[
                "for-each-ref",
                "--format=%(refname)%09%(refname:short)%09%(symref)",
                "refs/heads",
                "refs/remotes",
            ],
        )
        .await?;
        let mut refs = Vec::new();
        for line in text.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            if fields.len() != 3 || !fields[2].is_empty() {
                continue;
            }
            if refs.len() == FILE_LIMIT {
                truncated = true;
                break;
            }
            refs.push(GitRef {
                full_name: fields[0].into(),
                name: fields[1].into(),
            });
        }
        let upstream = command(root, &["rev-parse", "--symbolic-full-name", "@{upstream}"])
            .await
            .ok();
        let default_base = upstream
            .filter(|refname| refs.iter().any(|r| &r.full_name == refname))
            .or_else(|| {
                [
                    "refs/remotes/origin/main",
                    "refs/remotes/origin/master",
                    "refs/heads/main",
                    "refs/heads/master",
                ]
                .iter()
                .find(|name| refs.iter().any(|r| r.full_name == **name))
                .map(|name| (*name).into())
            })
            .or_else(|| head.as_ref().map(|_| "HEAD".into()));
        Ok(GitInfo {
            branch,
            head,
            default_base,
            refs,
            truncated,
        })
    })
    .await
    .context("读取分支超时")?
}
/// Pages follow a fixed first-parent lineage even when the working branch moves.
pub async fn commits(
    root: &Path,
    head: Option<&str>,
    offset: u32,
) -> Result<(Option<String>, Vec<GitCommit>, bool)> {
    tokio::time::timeout(Duration::from_secs(20), async {
        command(root, &["rev-parse", "--show-prefix"]).await?;
        let head = match head {
            Some(oid) if valid_oid(oid) => Some(commit(root, oid).await?),
            Some(_) => bail!("无效的提交游标"),
            None => commit(root, "HEAD").await.ok(),
        };
        let Some(oid) = &head else {
            return Ok((None, Vec::new(), false));
        };
        let text = command(
            root,
            &[
                "log",
                "--first-parent",
                "--no-decorate",
                "--no-show-signature",
                "--format=%H%x09%s",
                "--max-count=51",
                &format!("--skip={offset}"),
                oid,
                "--",
            ],
        )
        .await?;
        let mut rows = Vec::new();
        for line in text.lines() {
            let (oid, subject) = line.split_once('\t').context("无法读取提交记录")?;
            if !valid_oid(oid) {
                bail!("无效的提交记录");
            }
            rows.push(GitCommit {
                oid: oid.into(),
                subject: subject.into(),
            });
        }
        let has_more = rows.len() > 50;
        rows.truncate(50);
        Ok((head, rows, has_more))
    })
    .await
    .context("读取提交历史超时")?
}
fn arguments(scope: GitReviewScope, base: Option<&str>, head: Option<&str>) -> Result<Vec<String>> {
    let mut args: Vec<String> = [
        "diff",
        "--relative",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--find-renames",
    ]
    .map(str::to_owned)
    .into();
    match scope {
        GitReviewScope::Commit if base.is_none() => {
            let head = head
                .filter(|oid| valid_oid(oid))
                .context("缺少有效的提交，请刷新")?;
            args[0] = "diff-tree".into();
            args.extend([
                "--root".into(),
                "--no-commit-id".into(),
                "-r".into(),
                head.into(),
            ]);
        }
        GitReviewScope::Branch | GitReviewScope::Commit => {
            let base = base
                .filter(|oid| valid_oid(oid))
                .context("缺少有效的基准提交，请刷新")?;
            let head = head
                .filter(|oid| valid_oid(oid))
                .context("缺少有效的分支提交，请刷新")?;
            args.extend([base.into(), head.into()]);
        }
        GitReviewScope::Worktree => args.push(
            head.filter(|oid| valid_oid(oid))
                .context("仓库尚无提交，请使用已暂存或未暂存范围")?
                .into(),
        ),
        GitReviewScope::Staged => {
            args.push("--cached".into());
            if let Some(head) = head {
                if !valid_oid(head) {
                    bail!("无效的提交");
                }
                args.push(head.into());
            }
        }
        GitReviewScope::Unstaged => {}
    }
    Ok(args)
}
async fn listing(
    root: &Path,
    args: &[String],
    exclusions: Option<&ManagedRecoveryExclusions>,
) -> Result<(Vec<GitReviewFile>, bool)> {
    let veto = if let Some(exclusions) = exclusions {
        files::private_rename_paths(root, exclusions).await?
    } else {
        Default::default()
    };
    let mut names = args.to_vec();
    names.extend(["--name-status".into(), "-z".into(), "--".into(), ".".into()]);
    let (text, mut truncated) =
        files::git(root, &names.iter().map(String::as_str).collect::<Vec<_>>()).await?;
    let mut records = text.split('\0');
    let mut entries: Vec<GitReviewFile> = Vec::new();
    while let Some(status) = records.next() {
        if status.is_empty() {
            continue;
        }
        let Some(first) = records.next() else {
            truncated = true;
            break;
        };
        let (path, previous_path) = if status.starts_with(['R', 'C']) {
            let Some(second) = records.next() else {
                truncated = true;
                break;
            };
            (second, Some(first.into()))
        } else {
            (first, None)
        };
        if valid_path(path).is_err()
            || previous_path
                .as_deref()
                .is_some_and(|p| valid_path(p).is_err())
        {
            truncated = true;
            continue;
        }
        if let Some(exclusions) = exclusions
            && (exclusions.contains(root, path)?
                || previous_path
                    .as_deref()
                    .map(|path| exclusions.contains(root, path))
                    .transpose()?
                    .unwrap_or(false))
        {
            continue;
        }
        if veto.contains(path)
            || previous_path
                .as_deref()
                .is_some_and(|path| veto.contains(path))
        {
            continue;
        }
        if let Some(existing) = entries.iter_mut().find(|entry| entry.path == path) {
            if status.starts_with('U') {
                existing.status = "U".into();
            }
            continue;
        }
        if entries.len() == FILE_LIMIT {
            truncated = true;
            break;
        }
        entries.push(GitReviewFile {
            path: path.into(),
            previous_path,
            status: status[..1].into(),
            added: None,
            removed: None,
            binary: false,
            untracked: false,
        });
    }
    Ok((entries, truncated))
}
async fn untracked_content(root: &Path, path: &str) -> Result<(Vec<u8>, bool)> {
    valid_path(path)?;
    let metadata = tokio::fs::symlink_metadata(root.join(path)).await?;
    if latte_work_platform::fs::is_link(&metadata) {
        bail!("未跟踪符号链接不支持内容预览");
    }
    let target = files::resolve(root, path)?;
    if !target.is_file() {
        bail!("只能预览普通文件");
    }
    let file = tokio::fs::File::open(target).await?;
    let mut data = Vec::new();
    file.take((CONTENT_LIMIT + 1) as u64)
        .read_to_end(&mut data)
        .await?;
    let truncated = data.len() > CONTENT_LIMIT;
    data.truncate(CONTENT_LIMIT);
    Ok((data, truncated))
}
pub async fn review_excluding(
    root: &Path,
    scope: GitReviewScope,
    reference: Option<&str>,
    exclusions: Option<&ManagedRecoveryExclusions>,
) -> Result<GitReview> {
    let exclusions = if matches!(scope, GitReviewScope::Branch | GitReviewScope::Commit) {
        None
    } else {
        Some(exclusions.context("Live Git review requires managed recovery exclusions")?)
    };
    tokio::time::timeout(Duration::from_secs(25), async {
        let head = if scope == GitReviewScope::Commit {
            let reference = reference.context("请选择要查看的提交")?;
            if !valid_oid(reference) {
                bail!("无效的提交");
            }
            Some(commit(root, reference).await?)
        } else {
            commit(root, "HEAD").await.ok()
        };
        let base = if scope == GitReviewScope::Branch {
            let info = info(root).await?;
            let reference = reference
                .or(info.default_base.as_deref())
                .context("仓库尚无提交，无法比较分支")?;
            let tip = commit(root, reference).await?;
            Some(
                command(
                    root,
                    &["merge-base", head.as_deref().context("仓库尚无提交")?, &tip],
                )
                .await
                .context("两个分支没有共同祖先")?,
            )
        } else if scope == GitReviewScope::Commit {
            let parents = command(
                root,
                &[
                    "rev-list",
                    "--parents",
                    "--max-count=1",
                    head.as_deref().context("缺少提交")?,
                    "--",
                ],
            )
            .await?;
            parents.split_whitespace().nth(1).map(str::to_owned)
        } else {
            None
        };
        let args = arguments(scope, base.as_deref(), head.as_deref())?;
        let (mut entries, mut truncated) = listing(root, &args, exclusions).await?;
        let mut stat_args = args;
        stat_args.extend(["--numstat".into(), "-z".into(), "--".into(), ".".into()]);
        let (text, partial) = files::git(
            root,
            &stat_args.iter().map(String::as_str).collect::<Vec<_>>(),
        )
        .await?;
        truncated |= partial;
        let mut records = text.split('\0');
        let mut stats = HashMap::new();
        while let Some(record) = records.next() {
            if record.is_empty() {
                continue;
            }
            let mut parts = record.splitn(3, '\t');
            let (Some(added), Some(removed), Some(path)) =
                (parts.next(), parts.next(), parts.next())
            else {
                truncated = true;
                break;
            };
            let path = if path.is_empty() {
                records.next();
                let Some(new) = records.next() else {
                    truncated = true;
                    break;
                };
                new
            } else {
                path
            };
            stats.insert(
                path,
                (
                    added.parse::<u32>().ok(),
                    removed.parse::<u32>().ok(),
                    added == "-",
                ),
            );
        }
        for entry in &mut entries {
            if let Some((added, removed, binary)) = stats.get(entry.path.as_str()) {
                entry.added = *added;
                entry.removed = *removed;
                entry.binary = *binary;
            } else {
                truncated = true;
            }
        }
        if matches!(scope, GitReviewScope::Worktree | GitReviewScope::Unstaged) {
            let (changes, partial) = files::changes_excluding(
                root,
                exclusions.context("Missing live review exclusions")?,
            )
            .await?;
            truncated |= partial;
            let mut bytes = 0;
            for change in changes
                .into_iter()
                .filter(|entry| entry.section == ChangeSection::Untracked)
            {
                if entries.len() == FILE_LIMIT {
                    truncated = true;
                    break;
                }
                let mut entry = GitReviewFile {
                    path: change.path,
                    status: "A".into(),
                    previous_path: None,
                    added: None,
                    removed: None,
                    binary: false,
                    untracked: true,
                };
                if bytes < 2 * CONTENT_LIMIT {
                    match untracked_content(root, &entry.path).await {
                        Ok((data, partial)) => {
                            bytes += data.len();
                            truncated |= partial;
                            entry.binary = data.contains(&0);
                            if !partial && !entry.binary {
                                entry.added =
                                    Some(data.split(|b| *b == b'\n').count().saturating_sub(
                                        usize::from(data.ends_with(b"\n") || data.is_empty()),
                                    ) as u32);
                                entry.removed = Some(0);
                            }
                        }
                        Err(_) => truncated = true,
                    }
                } else {
                    truncated = true;
                }
                entries.push(entry);
            }
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(GitReview {
            base,
            head,
            added: entries
                .iter()
                .fold(0_u32, |n, e| n.saturating_add(e.added.unwrap_or(0))),
            removed: entries
                .iter()
                .fold(0_u32, |n, e| n.saturating_add(e.removed.unwrap_or(0))),
            entries,
            truncated,
        })
    })
    .await
    .context("读取变更超时，请缩小项目范围")?
}
pub(crate) fn quote_path(path: &str) -> String {
    let mut quoted = String::from("\"");
    for byte in path.bytes() {
        match byte {
            b'"' | b'\\' => {
                quoted.push('\\');
                quoted.push(char::from(byte));
            }
            32..=126 => quoted.push(char::from(byte)),
            _ => quoted.push_str(&format!("\\{byte:03o}")),
        }
    }
    quoted.push('"');
    quoted
}
#[allow(clippy::too_many_arguments)]
pub async fn diff_excluding(
    root: &Path,
    scope: GitReviewScope,
    base: Option<&str>,
    head: Option<&str>,
    path: &str,
    full_context: bool,
    exclusions: Option<&ManagedRecoveryExclusions>,
) -> Result<(String, bool)> {
    let exclusions = if matches!(scope, GitReviewScope::Branch | GitReviewScope::Commit) {
        None
    } else {
        Some(exclusions.context("Live Git patch requires managed recovery exclusions")?)
    };
    tokio::time::timeout(Duration::from_secs(20), async {
        valid_path(path)?;
        if let Some(exclusions) = exclusions {
            exclusions.require_visible(root, path)?;
        }
        if matches!(scope, GitReviewScope::Worktree | GitReviewScope::Staged)
            && commit(root, "HEAD").await.ok().as_deref() != head
        {
            bail!("分支提交已变化，请刷新变更后重试");
        }
        let mut args = arguments(scope, base, head)?;
        let (entries, _) = listing(root, &args, exclusions).await?;
        if let Some(entry) = entries.iter().find(|e| e.path == path) {
            args.push(
                if full_context {
                    "--unified=100000"
                } else {
                    "--unified=3"
                }
                .into(),
            );
            args.extend(["--".into(), path.into()]);
            if let Some(previous) = &entry.previous_path {
                args.push(previous.clone());
            }
            return files::git(root, &args.iter().map(String::as_str).collect::<Vec<_>>()).await;
        }
        if matches!(scope, GitReviewScope::Worktree | GitReviewScope::Unstaged) {
            let (entries, _) = files::changes_excluding(
                root,
                exclusions.context("Missing live review exclusions")?,
            )
            .await?;
            if entries
                .iter()
                .any(|e| e.path == path && e.section == ChangeSection::Untracked)
            {
                let (data, truncated) = untracked_content(root, path).await?;
                if data.contains(&0) {
                    return Ok(("Binary files differ\n".into(), truncated));
                }
                let text = String::from_utf8_lossy(&data);
                let quoted_path = quote_path(&format!("b/{path}"));
                let mut patch = format!(
                    "--- /dev/null\n+++ {quoted_path}\n@@ -0,0 +1,{} @@\n",
                    text.lines().count()
                );
                for line in text.lines() {
                    patch.push('+');
                    patch.push_str(line);
                    patch.push('\n');
                }
                if !data.is_empty() && !data.ends_with(b"\n") {
                    patch.push_str("\\ No newline at end of file\n");
                }
                return Ok((patch, truncated));
            }
        }
        Ok((String::new(), false))
    })
    .await
    .context("读取 diff 超时")?
}

#[cfg(test)]
async fn review(root: &Path, scope: GitReviewScope, reference: Option<&str>) -> Result<GitReview> {
    review_excluding(
        root,
        scope,
        reference,
        Some(&ManagedRecoveryExclusions::empty()),
    )
    .await
}
#[cfg(test)]
async fn diff(
    root: &Path,
    scope: GitReviewScope,
    base: Option<&str>,
    head: Option<&str>,
    path: &str,
    full_context: bool,
) -> Result<(String, bool)> {
    diff_excluding(
        root,
        scope,
        base,
        head,
        path,
        full_context,
        Some(&ManagedRecoveryExclusions::empty()),
    )
    .await
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    fn run(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        run(root.path(), &["init", "-q", "-b", "main"]);
        run(root.path(), &["config", "user.name", "Fixture"]);
        run(
            root.path(),
            &["config", "user.email", "fixture@example.test"],
        );
        std::fs::create_dir(root.path().join("project")).unwrap();
        for (name, text) in [
            ("file.txt", "original\n"),
            ("old.txt", "rename\n"),
            ("base.txt", "base\n"),
        ] {
            std::fs::write(root.path().join("project").join(name), text).unwrap();
        }
        std::fs::write(root.path().join("outside.txt"), "outside\n").unwrap();
        run(root.path(), &["add", "."]);
        run(root.path(), &["commit", "-qm", "initial"]);
        root
    }
    #[tokio::test]
    async fn committed_reviews_pin_first_parent_and_support_root_commits() {
        let root = fixture();
        let sub = root.path().join("project");
        let (_, history, _) = commits(&sub, None, 0).await.unwrap();
        let initial = history[0].oid.clone();
        let saved = review(&sub, GitReviewScope::Commit, Some(&initial))
            .await
            .unwrap();
        assert!(saved.base.is_none());
        assert_eq!(saved.entries.len(), 3);
        let patch = diff(
            &sub,
            GitReviewScope::Commit,
            None,
            saved.head.as_deref(),
            "file.txt",
            false,
        )
        .await
        .unwrap()
        .0;
        assert!(patch.contains("+original"));
        assert!(!patch.contains("outside"));
        run(root.path(), &["checkout", "-qb", "feature"]);
        std::fs::write(sub.join("file.txt"), "feature\n").unwrap();
        run(root.path(), &["add", "."]);
        run(root.path(), &["commit", "-qm", "feature"]);
        run(root.path(), &["checkout", "-q", "main"]);
        std::fs::write(sub.join("base.txt"), "main\n").unwrap();
        run(root.path(), &["add", "."]);
        run(root.path(), &["commit", "-qm", "main"]);
        let parent = commit(&sub, "HEAD").await.unwrap();
        run(
            root.path(),
            &["merge", "--no-ff", "-qm", "merge feature", "feature"],
        );
        let (head, history, _) = commits(&sub, None, 0).await.unwrap();
        assert_eq!(
            history
                .iter()
                .map(|c| c.subject.as_str())
                .collect::<Vec<_>>(),
            ["merge feature", "main", "initial"]
        );
        let index = std::fs::read(root.path().join(".git/index")).unwrap();
        let saved = review(&sub, GitReviewScope::Commit, head.as_deref())
            .await
            .unwrap();
        assert_eq!(saved.base.as_deref(), Some(parent.as_str()));
        assert_eq!(saved.entries.len(), 1);
        assert_eq!(saved.entries[0].path, "file.txt");
        assert_eq!(
            std::fs::read(root.path().join(".git/index")).unwrap(),
            index
        );
        run(root.path(), &["checkout", "-q", "feature"]);
        std::fs::write(sub.join("file.txt"), "working\n").unwrap();
        let patch = diff(
            &sub,
            GitReviewScope::Commit,
            saved.base.as_deref(),
            saved.head.as_deref(),
            "file.txt",
            false,
        )
        .await
        .unwrap()
        .0;
        assert!(patch.contains("+feature"));
        assert!(!patch.contains("+working"));
        // Inspection never writes the index; check before the explicit checkout changes it.
        assert_eq!(saved.head, head);
        assert!(!index.is_empty());
        for value in ["HEAD", "--output=escape", "HEAD:outside.txt", "../main"] {
            assert!(
                review(&sub, GitReviewScope::Commit, Some(value))
                    .await
                    .is_err()
            );
        }
        assert!(
            diff(
                &sub,
                GitReviewScope::Commit,
                saved.base.as_deref(),
                saved.head.as_deref(),
                "../outside.txt",
                false
            )
            .await
            .is_err()
        );
    }
    #[tokio::test]
    async fn commit_pages_keep_their_lineage_after_head_moves() {
        let root = fixture();
        // Construct enough history for two pages in one deterministic Git process.
        let initial = commit(root.path(), "HEAD").await.unwrap();
        let mut input = String::new();
        for n in 0..54 {
            let message = format!("commit {n}");
            let parent = if n == 0 {
                initial.clone()
            } else {
                format!(":{n}")
            };
            input.push_str(&format!("commit refs/heads/main\nmark :{}\ncommitter Fixture <fixture@example.test> {} +0000\ndata {}\n{}\nfrom {}\n\n", n + 1, n + 1, message.len(), message, parent));
        }
        let mut child = Command::new("git")
            .args(["fast-import", "--quiet"])
            .current_dir(root.path())
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(&mut child.stdin.take().unwrap(), input.as_bytes()).unwrap();
        assert!(child.wait().unwrap().success());
        let (head, first, more) = commits(root.path(), None, 0).await.unwrap();
        assert_eq!(first.len(), 50);
        assert!(more);
        run(
            root.path(),
            &["commit", "--allow-empty", "-qm", "late commit"],
        );
        let (_, second, more) = commits(root.path(), head.as_deref(), 50).await.unwrap();
        assert_eq!(second.len(), 5);
        assert!(!more);
        assert_eq!(second.last().unwrap().subject, "initial");
        assert!(!second.iter().any(|c| c.subject == "late commit"));
        assert!(!first.iter().any(|a| second.iter().any(|b| a.oid == b.oid)));
        assert!(commits(root.path(), Some("HEAD~1"), 0).await.is_err());
        let empty = tempfile::tempdir().unwrap();
        assert!(commits(empty.path(), None, 0).await.is_err());
        run(empty.path(), &["init", "-q", "-b", "main"]);
        assert_eq!(commits(empty.path(), None, 0).await.unwrap().0, None);
    }
    #[tokio::test]
    async fn branch_review_uses_merge_base_and_pinned_commits_with_renames() {
        let root = fixture();
        let sub = root.path().join("project");
        run(root.path(), &["checkout", "-qb", "feature"]);
        std::fs::write(sub.join("file.txt"), "feature\nextra\n").unwrap();
        run(
            root.path(),
            &["mv", "project/old.txt", "project/中文 renamed.txt"],
        );
        std::fs::write(root.path().join("outside.txt"), "do not expose outside\n").unwrap();
        run(root.path(), &["add", "."]);
        run(root.path(), &["commit", "-qm", "feature"]);
        run(root.path(), &["checkout", "-q", "main"]);
        std::fs::write(sub.join("base.txt"), "base branch only\n").unwrap();
        run(root.path(), &["add", "."]);
        run(root.path(), &["commit", "-qm", "base advanced"]);
        run(
            root.path(),
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
        );
        run(root.path(), &["checkout", "-q", "feature"]);
        let index = std::fs::read(root.path().join(".git/index")).unwrap();
        let metadata = info(&sub).await.unwrap();
        assert_eq!(metadata.branch.as_deref(), Some("feature"));
        assert_eq!(
            metadata.default_base.as_deref(),
            Some("refs/remotes/origin/main")
        );
        let snapshot = review(&sub, GitReviewScope::Branch, None).await.unwrap();
        assert_eq!(snapshot.entries.len(), 2);
        assert_eq!((snapshot.added, snapshot.removed), (2, 1));
        assert!(
            !snapshot
                .entries
                .iter()
                .any(|e| e.path.contains("outside") || e.path == "base.txt")
        );
        assert!(
            snapshot
                .entries
                .iter()
                .any(|e| e.path == "中文 renamed.txt"
                    && e.previous_path.as_deref() == Some("old.txt"))
        );
        assert_eq!(
            std::fs::read(root.path().join(".git/index")).unwrap(),
            index
        );
        run(root.path(), &["checkout", "-q", "main"]);
        run(
            root.path(),
            &["update-ref", "refs/remotes/origin/main", "feature"],
        );
        let patch = diff(
            &sub,
            GitReviewScope::Branch,
            snapshot.base.as_deref(),
            snapshot.head.as_deref(),
            "file.txt",
            false,
        )
        .await
        .unwrap()
        .0;
        assert!(patch.contains("+feature"));
        assert!(!patch.contains("base branch only"));
        let patch = diff(
            &sub,
            GitReviewScope::Branch,
            snapshot.base.as_deref(),
            snapshot.head.as_deref(),
            "中文 renamed.txt",
            false,
        )
        .await
        .unwrap()
        .0;
        assert!(patch.contains("rename from old.txt"));
        run(root.path(), &["checkout", "-q", "feature"]);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn index_worktree_binary_untracked_and_literal_paths_are_distinct() {
        let root = fixture();
        let sub = root.path().join("project");
        std::fs::write(sub.join("file.txt"), "staged\n").unwrap();
        run(root.path(), &["add", "project/file.txt"]);
        std::fs::write(sub.join("file.txt"), "working\nextra\n").unwrap();
        std::fs::write(sub.join("binary"), [0_u8, 1, 2]).unwrap();
        let unusual = "中文 space\nfile.txt";
        std::fs::write(sub.join(unusual), "literal\n").unwrap();
        std::fs::write(root.path().join("outside.txt"), "outside edits\n").unwrap();
        std::os::unix::fs::symlink(root.path().join("outside.txt"), sub.join("link")).unwrap();
        let index = std::fs::read(root.path().join(".git/index")).unwrap();
        for (scope, added, removed) in [
            (GitReviewScope::Staged, 1, 1),
            (GitReviewScope::Unstaged, 3, 1),
            (GitReviewScope::Worktree, 3, 1),
        ] {
            let snapshot = review(&sub, scope, None).await.unwrap();
            assert_eq!((snapshot.added, snapshot.removed), (added, removed));
            assert!(
                snapshot
                    .entries
                    .iter()
                    .all(|e| !e.path.contains("outside") && !e.path.starts_with("project/"))
            );
            let patch = diff(
                &sub,
                scope,
                snapshot.base.as_deref(),
                snapshot.head.as_deref(),
                "file.txt",
                false,
            )
            .await
            .unwrap()
            .0;
            assert!(patch.contains(if scope == GitReviewScope::Staged {
                "+staged"
            } else {
                "+working"
            }));
            if scope != GitReviewScope::Staged {
                assert!(snapshot.truncated); // The external symlink has no readable statistics.
                assert!(
                    snapshot
                        .entries
                        .iter()
                        .any(|e| e.path == "binary" && e.binary)
                );
                assert!(
                    diff(&sub, scope, None, snapshot.head.as_deref(), "link", false)
                        .await
                        .is_err()
                );
                assert!(
                    diff(&sub, scope, None, snapshot.head.as_deref(), unusual, false)
                        .await
                        .unwrap()
                        .0
                        .contains("+literal")
                );
            }
        }
        assert_eq!(
            std::fs::read(root.path().join(".git/index")).unwrap(),
            index
        );
        for path in ["../outside.txt", "/etc/passwd", ""] {
            assert!(
                diff(&sub, GitReviewScope::Unstaged, None, None, path, false)
                    .await
                    .is_err()
            );
        }
        assert_eq!(
            diff(&sub, GitReviewScope::Unstaged, None, None, ":(top)*", false)
                .await
                .unwrap()
                .0,
            ""
        );
        for base in [
            "--output=outside",
            "HEAD:outside.txt",
            "HEAD~1",
            "missing",
            "refs/heads/../main",
        ] {
            assert!(
                review(&sub, GitReviewScope::Branch, Some(base))
                    .await
                    .is_err()
            );
        }
    }
    #[tokio::test]
    async fn review_is_bounded_and_does_not_execute_external_diff() {
        let root = fixture();
        let sub = root.path().join("project");
        let marker = root.path().join("executed");
        run(
            root.path(),
            &[
                "config",
                "diff.external",
                &format!("touch {}", marker.display()),
            ],
        );
        std::fs::write(sub.join("file.txt"), "changed\n").unwrap();
        let snapshot = review(&sub, GitReviewScope::Worktree, None).await.unwrap();
        assert!(
            diff(
                &sub,
                GitReviewScope::Worktree,
                None,
                snapshot.head.as_deref(),
                "file.txt",
                true
            )
            .await
            .unwrap()
            .0
            .contains("+changed")
        );
        assert!(!marker.exists());
        for n in 0..1005 {
            std::fs::write(sub.join(format!("untracked-{n}")), "").unwrap();
        }
        let snapshot = review(&sub, GitReviewScope::Unstaged, None).await.unwrap();
        assert_eq!(snapshot.entries.len(), FILE_LIMIT);
        assert!(snapshot.truncated);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn untracked_patches_quote_paths_preserve_eof_and_reject_internal_symlinks() {
        let root = fixture();
        let sub = root.path().join("project");
        let path = "new\n\"name";
        std::fs::write(sub.join(path), "no newline").unwrap();
        std::os::unix::fs::symlink(sub.join("file.txt"), sub.join("inside-link")).unwrap();
        let snapshot = review(&sub, GitReviewScope::Unstaged, None).await.unwrap();
        assert!(snapshot.truncated);
        let patch = diff(
            &sub,
            GitReviewScope::Unstaged,
            None,
            snapshot.head.as_deref(),
            path,
            false,
        )
        .await
        .unwrap()
        .0;
        assert!(patch.contains("\\012"));
        assert!(patch.contains("\\ No newline at end of file"));
        assert!(
            diff(
                &sub,
                GitReviewScope::Unstaged,
                None,
                snapshot.head.as_deref(),
                "inside-link",
                false
            )
            .await
            .is_err()
        );
        run(root.path(), &["commit", "--allow-empty", "-qm", "advance"]);
        assert!(
            diff(
                &sub,
                GitReviewScope::Worktree,
                None,
                snapshot.head.as_deref(),
                "file.txt",
                false
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("请刷新")
        );
    }
    #[tokio::test]
    async fn unmerged_files_are_listed_once_and_preserve_combined_diff() {
        let root = fixture();
        let sub = root.path().join("project");
        run(root.path(), &["checkout", "-qb", "feature"]);
        std::fs::write(sub.join("file.txt"), "feature\n").unwrap();
        run(root.path(), &["add", "."]);
        run(root.path(), &["commit", "-qm", "feature"]);
        run(root.path(), &["checkout", "-q", "main"]);
        std::fs::write(sub.join("file.txt"), "main\n").unwrap();
        run(root.path(), &["add", "."]);
        run(root.path(), &["commit", "-qm", "main"]);
        let result = Command::new("git")
            .args(["merge", "feature"])
            .current_dir(root.path())
            .output()
            .unwrap();
        assert!(!result.status.success());
        let snapshot = review(&sub, GitReviewScope::Unstaged, None).await.unwrap();
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].status, "U");
        assert!(
            diff(
                &sub,
                GitReviewScope::Unstaged,
                None,
                snapshot.head.as_deref(),
                "file.txt",
                false
            )
            .await
            .unwrap()
            .0
            .contains("diff --cc")
        );
    }
    #[tokio::test]
    async fn non_repo_unborn_and_detached_head_have_explicit_states() {
        let empty = tempfile::tempdir().unwrap();
        assert!(info(empty.path()).await.is_err());
        run(empty.path(), &["init", "-q", "-b", "main"]);
        let metadata = info(empty.path()).await.unwrap();
        assert!(metadata.head.is_none());
        assert_eq!(metadata.branch.as_deref(), Some("main"));
        assert!(
            review(empty.path(), GitReviewScope::Branch, None)
                .await
                .is_err()
        );
        assert!(
            review(empty.path(), GitReviewScope::Worktree, None)
                .await
                .is_err()
        );
        assert!(
            review(empty.path(), GitReviewScope::Staged, None)
                .await
                .unwrap()
                .entries
                .is_empty()
        );
        let root = fixture();
        run(root.path(), &["checkout", "-q", "--detach"]);
        let metadata = info(root.path()).await.unwrap();
        assert!(metadata.branch.is_none());
        assert!(metadata.head.is_some());
    }
}
