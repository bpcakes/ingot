use std::collections::BTreeMap;
use std::path::{Component, Path};
use std::process::Stdio;

use ingot_domain::commit_oid::CommitOid;
use ingot_domain::git_operation::{ConvergenceConflictFile, ConvergenceConflictStage};
use ingot_domain::ids::{ConvergenceId, GitOperationId, ItemId, JobId};
use tokio::process::Command;

use crate::commands::{GitCommandError, git, head_oid};

#[derive(Clone, Debug)]
pub struct JobCommitTrailers {
    pub operation_id: GitOperationId,
    pub item_id: ItemId,
    pub revision_no: u32,
    pub job_id: JobId,
}

#[derive(Clone, Debug)]
pub struct ConvergenceCommitTrailers {
    pub operation_id: GitOperationId,
    pub item_id: ItemId,
    pub revision_no: u32,
    pub convergence_id: ConvergenceId,
    pub source_commit_oid: CommitOid,
}

pub async fn working_tree_has_changes(repo_path: &Path) -> Result<bool, GitCommandError> {
    let output = git(repo_path, &["status", "--porcelain"]).await?;
    Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

pub async fn create_daemon_job_commit(
    repo_path: &Path,
    subject: &str,
    summary: &str,
    trailers: &JobCommitTrailers,
) -> Result<CommitOid, GitCommandError> {
    let message = format!(
        "{subject}\n\n{summary}\n\nIngot-Operation: {}\nIngot-Item: {}\nIngot-Revision: {}\nIngot-Job: {}",
        trailers.operation_id, trailers.item_id, trailers.revision_no, trailers.job_id
    );
    create_daemon_commit_from_staged(repo_path, &message).await
}

pub async fn create_daemon_convergence_commit(
    repo_path: &Path,
    original_message: &str,
    trailers: &ConvergenceCommitTrailers,
) -> Result<CommitOid, GitCommandError> {
    let message = format!(
        "{}\n\nIngot-Operation: {}\nIngot-Item: {}\nIngot-Revision: {}\nIngot-Convergence: {}\nIngot-Source-Commit: {}",
        original_message.trim_end(),
        trailers.operation_id,
        trailers.item_id,
        trailers.revision_no,
        trailers.convergence_id,
        trailers.source_commit_oid
    );
    create_daemon_commit_from_staged(repo_path, &message).await
}

pub async fn create_daemon_commit_from_staged(
    repo_path: &Path,
    message: &str,
) -> Result<CommitOid, GitCommandError> {
    git(repo_path, &["add", "-A"]).await?;

    let mut child = Command::new("git")
        .args(["commit", "--no-verify", "-F", "-"])
        .current_dir(repo_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(message.as_bytes()).await?;
    }

    let output = child.wait_with_output().await?;

    if !output.status.success() {
        return Err(GitCommandError::command_failed(
            repo_path,
            &["commit", "--no-verify", "-F", "-"],
            output,
        ));
    }

    head_oid(repo_path).await
}

pub async fn commit_message(
    repo_path: &Path,
    commit_oid: &CommitOid,
) -> Result<String, GitCommandError> {
    let output = git(
        repo_path,
        &["show", "-s", "--format=%B", commit_oid.as_str()],
    )
    .await?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string())
}

pub async fn list_commits_oldest_first(
    repo_path: &Path,
    base_commit_oid: &CommitOid,
    head_commit_oid: &CommitOid,
) -> Result<Vec<CommitOid>, GitCommandError> {
    if base_commit_oid == head_commit_oid {
        return Ok(vec![]);
    }

    let range = format!("{base_commit_oid}..{head_commit_oid}");
    let output = git(repo_path, &["rev-list", "--reverse", &range]).await?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(CommitOid::new)
        .collect())
}

pub async fn cherry_pick_no_commit(
    repo_path: &Path,
    commit_oid: &CommitOid,
) -> Result<(), GitCommandError> {
    git(
        repo_path,
        &["cherry-pick", "--no-commit", commit_oid.as_str()],
    )
    .await?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct CollectedConvergenceConflictFiles {
    pub total_count: usize,
    pub files: Vec<ConvergenceConflictFile>,
}

pub async fn collect_convergence_conflict_files(
    repo_path: &Path,
) -> Result<CollectedConvergenceConflictFiles, GitCommandError> {
    // Bounds persisted metadata to at most 20 file records; excerpts have their own cap below.
    const MAX_CONFLICT_FILES: usize = 20;

    let output = git(repo_path, &["diff", "--name-only", "--diff-filter=U", "-z"]).await?;
    let (total_count, paths) = conflict_paths_from_diff_name_only_z(&output.stdout);

    let stages_output = git(repo_path, &["ls-files", "-u", "-z"]).await?;
    let mut stages_by_path = BTreeMap::<String, Vec<ConvergenceConflictStage>>::new();
    for record in stages_output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let Some(tab_index) = record.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        let prefix = String::from_utf8_lossy(&record[..tab_index]);
        let Ok(path) = String::from_utf8(record[(tab_index + 1)..].to_vec()) else {
            continue;
        };
        // git ls-files -u records are "mode oid stage<TAB>path".
        let stage = prefix.split_whitespace().nth(2).unwrap_or_default();
        let Some(stage) = (match stage {
            "1" => Some(ConvergenceConflictStage::Base),
            "2" => Some(ConvergenceConflictStage::Ours),
            "3" => Some(ConvergenceConflictStage::Theirs),
            _ => None,
        }) else {
            continue;
        };
        let stages = stages_by_path.entry(path).or_default();
        if !stages.contains(&stage) {
            stages.push(stage);
        }
    }

    let mut files = Vec::with_capacity(paths.len().min(MAX_CONFLICT_FILES));
    for path in paths.into_iter().take(MAX_CONFLICT_FILES) {
        let excerpt = conflict_file_excerpt(repo_path, &path).await;
        let mut stages = stages_by_path.remove(&path).unwrap_or_default();
        stages.sort();
        files.push(ConvergenceConflictFile {
            path,
            stages,
            excerpt,
        });
    }

    Ok(CollectedConvergenceConflictFiles { total_count, files })
}

fn conflict_paths_from_diff_name_only_z(stdout: &[u8]) -> (usize, Vec<String>) {
    let path_bytes = stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();
    // Count raw Git path records before UTF-8 filtering. Detailed metadata only stores
    // representable paths, but callers still need an accurate total so the file list is marked
    // incomplete when Git reports paths this API cannot safely surface as strings.
    let total_count = path_bytes.len();
    let paths = path_bytes
        .into_iter()
        .filter_map(|path| String::from_utf8(path.to_vec()).ok())
        .collect::<Vec<_>>();

    (total_count, paths)
}

pub async fn abort_cherry_pick(repo_path: &Path) -> Result<(), GitCommandError> {
    git(repo_path, &["cherry-pick", "--abort"])
        .await
        .map(|_| ())
}

async fn conflict_file_excerpt(repo_path: &Path, path: &str) -> Option<String> {
    use tokio::io::AsyncReadExt;

    // The collector caps calls to 20 paths, so intermediate reads stay below 1.25 MiB per
    // failed attempt before excerpts are trimmed to the persisted character cap.
    const MAX_BYTES: usize = 64 * 1024;
    // Worst-case stored text is MAX_CONFLICT_FILES * MAX_CHARS before JSON overhead.
    const MAX_CHARS: usize = 1_024;
    // Include enough context for typical ours/base/theirs hunks without bloating metadata.
    const PRE_CONTEXT_LINES: usize = 3;
    const MARKER_WINDOW_LINES: usize = 18;

    if !is_safe_conflict_excerpt_path(path) {
        return None;
    }

    // Excerpts are persisted in git operation metadata and can later be surfaced by API clients.
    // Skip common secret-bearing files even when they are plain text conflicts.
    if is_sensitive_conflict_excerpt_path(path) {
        return None;
    }

    let file_path = repo_path.join(path);
    let Ok(metadata) = tokio::fs::symlink_metadata(&file_path).await else {
        return None;
    };
    if !metadata.file_type().is_file() {
        return None;
    }

    let Ok(file) = open_conflict_file_no_follow(&file_path).await else {
        return None;
    };

    let mut reader = file.take(MAX_BYTES as u64);
    let mut bytes = Vec::new();
    if reader.read_to_end(&mut bytes).await.is_err() {
        return None;
    }
    if bytes.contains(&0) {
        return None;
    }

    let text = String::from_utf8_lossy(&bytes);
    let lines = text.lines().collect::<Vec<_>>();
    let marker_index = lines.iter().position(|line| line.starts_with("<<<<<<<"));
    let index = marker_index?;
    let start = index.saturating_sub(PRE_CONTEXT_LINES);
    let end = (index + MARKER_WINDOW_LINES).min(lines.len());
    let selected = lines[start..end].join("\n");

    Some(selected.chars().take(MAX_CHARS).collect())
}

async fn open_conflict_file_no_follow(file_path: &Path) -> std::io::Result<tokio::fs::File> {
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(file_path).await
}

fn is_safe_conflict_excerpt_path(path: &str) -> bool {
    // Git reports paths with slash separators. Reject backslashes as a conservative no-excerpt
    // policy for Windows-style separators or escape-like names before joining into the worktree.
    // On Unix this can skip a legitimate filename containing '\', but only the excerpt is omitted.
    if path.contains('\\') {
        return false;
    }

    let path = Path::new(path);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn is_sensitive_conflict_excerpt_path(path: &str) -> bool {
    // Best-effort denylist for obvious secret-bearing paths. This avoids common leaks while
    // preserving source-code excerpts that operators need to diagnose ordinary conflicts.
    let components = path
        .split(['/', '\\'])
        .filter(|component| !component.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();

    if components.iter().any(|component| {
        matches!(
            component.as_str(),
            ".aws" | ".azure" | ".gcp" | ".ssh" | ".secrets" | "secrets"
        )
    }) {
        return true;
    }

    let sql_dump_directory = components
        .iter()
        .any(|component| matches!(component.as_str(), "backup" | "backups" | "dump" | "dumps"));
    let Some(file_name) = components.last().map(String::as_str) else {
        return false;
    };

    file_name == ".env"
        || file_name.starts_with(".env.")
        || file_name == ".htpasswd"
        || file_name == "htpasswd"
        || file_name.starts_with("htpasswd.")
        || file_name.ends_with(".asc")
        || file_name.ends_with(".cer")
        || file_name.ends_with(".crt")
        || file_name.ends_with(".der")
        || file_name.ends_with(".gpg")
        || file_name.ends_with(".kdbx")
        || file_name.ends_with(".key")
        || file_name.ends_with(".p8")
        || file_name.ends_with(".p7b")
        || file_name.ends_with(".p7c")
        || file_name.ends_with(".pem.bak")
        || file_name.ends_with(".pgp")
        || file_name.ends_with(".pem")
        || file_name.ends_with(".p12")
        || file_name.ends_with(".pfx")
        || file_name.ends_with(".pkcs8")
        || file_name.ends_with(".jks")
        || file_name.ends_with(".keystore")
        || file_name == "dump.sql"
        || file_name.ends_with(".dump.sql")
        || (file_name.starts_with("backup") && file_name.ends_with(".sql"))
        || (sql_dump_directory && file_name.ends_with(".sql"))
        || file_name.ends_with(".tfstate")
        || file_name.ends_with(".tfstate.backup")
        || file_name.ends_with(".tfvars")
        || matches!(
            file_name,
            ".dockerconfigjson"
                | ".envrc"
                | ".git-credentials"
                | ".netrc"
                | ".npmrc"
                | ".pypirc"
                | "app.config"
                | "credentials"
                | "credentials.json"
                | "id_dsa"
                | "id_ecdsa"
                | "id_ed25519"
                | "id_rsa"
                | "kubeconfig"
                | "secret.json"
                | "secret.yaml"
                | "secret.yml"
                | "secrets.json"
                | "secrets.yaml"
                | "secrets.yml"
                | "service-account.json"
                | "service_account.json"
                | "vault.hcl"
                | "web.config"
                | "wp-config.php"
        )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use ingot_test_support::git::unique_temp_path;

    use super::*;

    #[test]
    fn conflict_paths_from_diff_name_only_z_counts_non_utf8_records() {
        let (total_count, paths) =
            conflict_paths_from_diff_name_only_z(b"valid.txt\0conflict-\xff.txt\0");

        assert_eq!(total_count, 2);
        assert_eq!(paths, vec!["valid.txt"]);
        assert!(total_count > paths.len());
    }

    #[tokio::test]
    async fn conflict_file_excerpt_reads_regular_conflict_file() {
        let repo = unique_temp_path("ingot-git-conflict-excerpt-regular");
        fs::create_dir_all(&repo).expect("create temp repo dir");
        fs::write(
            repo.join("conflict.txt"),
            "before\n<<<<<<< ours\nours\n=======\ntheirs\n>>>>>>> theirs\nafter\n",
        )
        .expect("write conflict file");

        let excerpt = conflict_file_excerpt(&repo, "conflict.txt")
            .await
            .expect("regular conflict file should produce excerpt");

        assert!(excerpt.contains("<<<<<<< ours"));
        assert!(excerpt.contains(">>>>>>> theirs"));

        fs::remove_dir_all(repo).expect("remove temp repo dir");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn conflict_file_excerpt_skips_symlink() {
        use std::os::unix::fs::symlink;

        let repo = unique_temp_path("ingot-git-conflict-excerpt-symlink");
        let outside = unique_temp_path("ingot-git-conflict-excerpt-outside");
        fs::create_dir_all(&repo).expect("create temp repo dir");
        fs::write(&outside, "outside contents should not be read\n").expect("write outside file");
        symlink(&outside, repo.join("conflict.txt")).expect("create symlink");

        let excerpt = conflict_file_excerpt(&repo, "conflict.txt").await;

        assert_eq!(excerpt, None);

        fs::remove_dir_all(repo).expect("remove temp repo dir");
        fs::remove_file(outside).expect("remove outside file");
    }

    #[tokio::test]
    async fn conflict_file_excerpt_skips_sensitive_paths() {
        let repo = unique_temp_path("ingot-git-conflict-excerpt-sensitive");
        fs::create_dir_all(&repo).expect("create temp repo dir");
        fs::write(
            repo.join(".env"),
            "<<<<<<< ours\nTOKEN=ours\n=======\nTOKEN=theirs\n>>>>>>> theirs\n",
        )
        .expect("write env file");

        let excerpt = conflict_file_excerpt(&repo, ".env").await;

        assert_eq!(excerpt, None);

        fs::remove_dir_all(repo).expect("remove temp repo dir");
    }

    #[tokio::test]
    async fn conflict_file_excerpt_skips_markerless_text_files() {
        let repo = unique_temp_path("ingot-git-conflict-excerpt-markerless");
        fs::create_dir_all(&repo).expect("create temp repo dir");
        fs::write(
            repo.join("conflict.txt"),
            "plain text without a conflict marker\n",
        )
        .expect("write markerless file");

        let excerpt = conflict_file_excerpt(&repo, "conflict.txt").await;

        assert_eq!(excerpt, None);

        fs::remove_dir_all(repo).expect("remove temp repo dir");
    }

    #[tokio::test]
    async fn conflict_file_excerpt_skips_nul_bytes() {
        let repo = unique_temp_path("ingot-git-conflict-excerpt-nul");
        fs::create_dir_all(&repo).expect("create temp repo dir");
        fs::write(repo.join("conflict.txt"), b"<<<<<<< ours\n\0secret\n").expect("write nul file");

        let excerpt = conflict_file_excerpt(&repo, "conflict.txt").await;

        assert_eq!(excerpt, None);

        fs::remove_dir_all(repo).expect("remove temp repo dir");
    }

    #[tokio::test]
    async fn conflict_file_excerpt_skips_absolute_or_parent_paths() {
        let repo = unique_temp_path("ingot-git-conflict-excerpt-unsafe-path");
        fs::create_dir_all(&repo).expect("create temp repo dir");
        fs::write(repo.join("conflict.txt"), "<<<<<<< ours\n").expect("write conflict file");

        assert_eq!(conflict_file_excerpt(&repo, "/conflict.txt").await, None);
        assert_eq!(conflict_file_excerpt(&repo, "../conflict.txt").await, None);
        assert_eq!(conflict_file_excerpt(&repo, "..\\conflict.txt").await, None);

        fs::remove_dir_all(repo).expect("remove temp repo dir");
    }

    #[test]
    fn sensitive_conflict_excerpt_path_matches_secret_filenames() {
        assert!(is_sensitive_conflict_excerpt_path(".env.local"));
        assert!(is_sensitive_conflict_excerpt_path(
            "config/credentials.json"
        ));
        assert!(is_sensitive_conflict_excerpt_path("terraform.tfvars"));
        assert!(is_sensitive_conflict_excerpt_path("cluster/kubeconfig"));
        assert!(is_sensitive_conflict_excerpt_path("certs/client.key"));
        assert!(is_sensitive_conflict_excerpt_path("certs/client.pem"));
        assert!(is_sensitive_conflict_excerpt_path("certs/client.crt"));
        assert!(is_sensitive_conflict_excerpt_path("certs/client.cer"));
        assert!(is_sensitive_conflict_excerpt_path("certs/client.der"));
        assert!(is_sensitive_conflict_excerpt_path("certs/client.pkcs8"));
        assert!(is_sensitive_conflict_excerpt_path("certs/AuthKey.p8"));
        assert!(is_sensitive_conflict_excerpt_path("certs/client.pem.bak"));
        assert!(is_sensitive_conflict_excerpt_path("certs/client.asc"));
        assert!(is_sensitive_conflict_excerpt_path("certs/client.gpg"));
        assert!(is_sensitive_conflict_excerpt_path("certs/client.pgp"));
        assert!(is_sensitive_conflict_excerpt_path("secrets/client.jks"));
        assert!(is_sensitive_conflict_excerpt_path(
            "secrets/client.keystore"
        ));
        assert!(is_sensitive_conflict_excerpt_path("secrets/client.kdbx"));
        assert!(is_sensitive_conflict_excerpt_path("deploy/secret.yaml"));
        assert!(is_sensitive_conflict_excerpt_path("deploy/secret.yml"));
        assert!(is_sensitive_conflict_excerpt_path("deploy/secrets.yaml"));
        assert!(is_sensitive_conflict_excerpt_path("deploy/secrets.yml"));
        assert!(is_sensitive_conflict_excerpt_path("terraform.tfstate"));
        assert!(is_sensitive_conflict_excerpt_path(
            "terraform.tfstate.backup"
        ));
        assert!(is_sensitive_conflict_excerpt_path("vault.hcl"));
        assert!(is_sensitive_conflict_excerpt_path("app.config"));
        assert!(is_sensitive_conflict_excerpt_path("web.config"));
        assert!(is_sensitive_conflict_excerpt_path("dump.sql"));
        assert!(is_sensitive_conflict_excerpt_path("prod.dump.sql"));
        assert!(is_sensitive_conflict_excerpt_path("backup-2026.sql"));
        assert!(is_sensitive_conflict_excerpt_path("backups/schema.sql"));
        assert!(!is_sensitive_conflict_excerpt_path(
            "crates/ingot-store-sqlite/migrations/0013_lookup.sql"
        ));
        assert!(is_sensitive_conflict_excerpt_path(".htpasswd"));
        assert!(is_sensitive_conflict_excerpt_path("htpasswd.local"));
        assert!(is_sensitive_conflict_excerpt_path(".envrc"));
        assert!(is_sensitive_conflict_excerpt_path(".git-credentials"));
        assert!(is_sensitive_conflict_excerpt_path(".dockerconfigjson"));
        assert!(is_sensitive_conflict_excerpt_path("wp-config.php"));
        assert!(is_sensitive_conflict_excerpt_path(".ssh/config"));
        assert!(!is_sensitive_conflict_excerpt_path("src/credentials.rs"));
    }
}
