use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use log::info;
use walkdir::WalkDir;

pub const MODE_PUSH: &str = "push";
pub const MODE_PULL: &str = "pull";
const GZIP_SUFFIX: &str = "-gzipped.txt";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Push,
    Pull,
}

impl std::str::FromStr for Mode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            MODE_PUSH => Ok(Mode::Push),
            MODE_PULL => Ok(Mode::Pull),
            _ => Err(anyhow!("mode must be either 'push' or 'pull'")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub mode: Mode,
    pub folder_path: PathBuf,
    pub repo_url: String,
    pub branch: String,
    pub ssh_key_path: Option<String>,
    pub compress: bool,
}

pub fn validate_config(config: &Config) -> Result<()> {
    match config.mode {
        Mode::Push | Mode::Pull => {}
    }

    if config.folder_path.as_os_str().is_empty() {
        bail!("folder path is required");
    }

    if config.repo_url.trim().is_empty() {
        bail!("repository URL is required");
    }

    Ok(())
}

pub fn run(config: &Config) -> Result<()> {
    validate_config(config)?;

    info!(
        "File Syncer started: mode={}, folder={}, repository={}, branch={}, compress={}",
        match config.mode {
            Mode::Push => MODE_PUSH,
            Mode::Pull => MODE_PULL,
        },
        config.folder_path.display(),
        config.repo_url,
        config.branch,
        config.compress
    );

    match config.mode {
        Mode::Push => push_files(config),
        Mode::Pull => pull_files(config),
    }
}

pub fn init_logger() -> Result<()> {
    use flexi_logger::{Cleanup, Criterion, Duplicate, FileSpec, Logger, Naming};

    Logger::try_with_env_or_str("info")?
        .log_to_file(FileSpec::default().basename("file-syncer").suffix("log"))
        .duplicate_to_stdout(Duplicate::Info)
        .rotate(
            Criterion::Size(10_000_000),
            Naming::Numbers,
            Cleanup::KeepLogFiles(3),
        )
        .start()?;

    Ok(())
}

fn push_files(config: &Config) -> Result<()> {
    info!("Starting push operation");

    let abs_path = fs::canonicalize(&config.folder_path).with_context(|| {
        format!(
            "failed to resolve folder path {}",
            config.folder_path.display()
        )
    })?;

    if !abs_path.exists() {
        bail!("folder does not exist: {}", abs_path.display());
    }

    let temp_dir = tempfile::tempdir().context("failed to create temp directory")?;
    let temp_path = temp_dir.path();

    info!(
        "Cloning repository: url={}, branch={}",
        config.repo_url, config.branch
    );

    if let Err(err) = run_command(
        temp_path,
        config.ssh_key_path.as_deref(),
        "git",
        ["clone", "--branch", &config.branch, &config.repo_url, "."],
    ) {
        info!("Branch not found, cloning default branch: {}", err);
        run_command(
            temp_path,
            config.ssh_key_path.as_deref(),
            "git",
            ["clone", &config.repo_url, "."],
        )
        .context("failed to clone repository")?;

        run_command(
            temp_path,
            config.ssh_key_path.as_deref(),
            "git",
            ["checkout", "-b", &config.branch],
        )
        .context("failed to create branch")?;
    }

    let transform = if config.compress {
        info!("Compression enabled; syncing files with gzip");
        SyncTransform::Compress
    } else {
        SyncTransform::None
    };

    info!(
        "Syncing files from {} to {}",
        abs_path.display(),
        temp_path.display()
    );
    sync_files_with_transform(&abs_path, temp_path, transform).context("failed to sync files")?;

    let status_output = run_command_output(
        temp_path,
        config.ssh_key_path.as_deref(),
        "git",
        ["status", "--porcelain"],
    )
    .context("failed to check git status")?;

    if status_output.trim().is_empty() {
        info!("No changes to push");
        return Ok(());
    }

    info!("Adding changes");
    run_command(
        temp_path,
        config.ssh_key_path.as_deref(),
        "git",
        ["add", "-A"],
    )
    .context("failed to add changes")?;

    let stats = parse_git_status(&status_output);
    let (commit_subject, commit_body) = generate_commit_message(&stats);

    info!("Committing changes: {}", commit_subject);
    let mut commit_args = vec![
        "commit".to_string(),
        "-m".to_string(),
        commit_subject.clone(),
    ];
    if !commit_body.is_empty() {
        commit_args.push("-m".to_string());
        commit_args.push(commit_body.clone());
    }
    run_command(
        temp_path,
        config.ssh_key_path.as_deref(),
        "git",
        commit_args.iter().map(|s| s.as_str()),
    )
    .context("failed to commit changes")?;

    info!("Pushing to remote branch {}", config.branch);
    run_command(
        temp_path,
        config.ssh_key_path.as_deref(),
        "git",
        ["push", "origin", &config.branch],
    )
    .context("failed to push changes")?;

    info!("Push completed successfully");
    Ok(())
}

fn pull_files(config: &Config) -> Result<()> {
    info!("Starting pull operation");

    let abs_path = if config.folder_path.is_absolute() {
        config.folder_path.clone()
    } else {
        std::env::current_dir()
            .context("failed to determine current directory")?
            .join(&config.folder_path)
    };

    fs::create_dir_all(&abs_path)
        .with_context(|| format!("failed to create folder {}", abs_path.display()))?;

    let temp_dir = tempfile::tempdir().context("failed to create temp directory")?;
    let temp_path = temp_dir.path();

    info!(
        "Cloning repository: url={}, branch={}",
        config.repo_url, config.branch
    );
    run_command(
        temp_path,
        config.ssh_key_path.as_deref(),
        "git",
        ["clone", "--branch", &config.branch, &config.repo_url, "."],
    )
    .context("failed to clone repository")?;

    let transform = if config.compress {
        info!("Compression enabled; decompressing files after pull");
        SyncTransform::Decompress
    } else {
        SyncTransform::None
    };

    info!(
        "Syncing files from {} to {}",
        temp_path.display(),
        abs_path.display()
    );
    sync_files_with_transform(temp_path, &abs_path, transform).context("failed to sync files")?;

    info!("Pull completed successfully");
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncTransform {
    None,
    Compress,
    Decompress,
}

pub fn sync_files(src_dir: &Path, dst_dir: &Path) -> Result<()> {
    sync_files_with_transform(src_dir, dst_dir, SyncTransform::None)
}

fn sync_files_with_transform(
    src_dir: &Path,
    dst_dir: &Path,
    transform: SyncTransform,
) -> Result<()> {
    let mut entries = WalkDir::new(src_dir).into_iter();
    while let Some(entry) = entries.next() {
        let entry = entry?;
        let rel_path = entry
            .path()
            .strip_prefix(src_dir)
            .context("failed to compute relative path")?;

        if rel_path.as_os_str().is_empty() {
            continue;
        }

        if let Some(first_component) = rel_path.components().next()
            && first_component.as_os_str() == OsStr::new(".git")
        {
            if entry.file_type().is_dir() {
                entries.skip_current_dir();
            }
            continue;
        }

        let metadata = entry.metadata()?;
        if entry.file_type().is_dir() {
            let dst_path = dst_dir.join(rel_path);
            fs::create_dir_all(&dst_path)?;
            fs::set_permissions(&dst_path, metadata.permissions())?;
        } else {
            let target_rel = match transform {
                SyncTransform::Compress => compress_relative_path(rel_path),
                SyncTransform::Decompress => decompress_relative_path(rel_path),
                SyncTransform::None => rel_path.to_path_buf(),
            };
            let dst_path = dst_dir.join(target_rel);
            if matches!(transform, SyncTransform::Compress) {
                compress_file(entry.path(), &dst_path, metadata.permissions())?;
            } else if matches!(transform, SyncTransform::Decompress) && is_gzipped_file(rel_path) {
                decompress_file(entry.path(), &dst_path, metadata.permissions())?;
            } else {
                copy_file(entry.path(), &dst_path, metadata.permissions())?;
            }
        }
    }

    Ok(())
}

fn compress_relative_path(rel_path: &Path) -> PathBuf {
    let mut path = rel_path.to_path_buf();
    if let Some(file_name) = rel_path.file_name().and_then(|name| name.to_str()) {
        path.set_file_name(format!("{file_name}{GZIP_SUFFIX}"));
    }
    path
}

fn decompress_relative_path(rel_path: &Path) -> PathBuf {
    if let Some(original) = original_file_name(rel_path) {
        original
    } else {
        rel_path.to_path_buf()
    }
}

fn original_file_name(rel_path: &Path) -> Option<PathBuf> {
    let file_name = rel_path.file_name()?.to_str()?;
    let stripped = file_name.strip_suffix(GZIP_SUFFIX)?;
    let mut path = rel_path.to_path_buf();
    path.set_file_name(stripped);
    Some(path)
}

fn is_gzipped_file(rel_path: &Path) -> bool {
    rel_path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.ends_with(GZIP_SUFFIX))
        .unwrap_or(false)
}

fn copy_file(src: &Path, dst: &Path, permissions: fs::Permissions) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut src_file = File::open(src)?;
    let mut dst_file = File::create(dst)?;
    io::copy(&mut src_file, &mut dst_file)?;
    fs::set_permissions(dst, permissions)?;
    Ok(())
}

fn compress_file(src: &Path, dst: &Path, permissions: fs::Permissions) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut src_file = File::open(src)?;
    let dst_file = File::create(dst)?;
    let mut encoder = GzEncoder::new(dst_file, Compression::best());
    io::copy(&mut src_file, &mut encoder)?;
    encoder.finish()?;
    fs::set_permissions(dst, permissions)?;
    Ok(())
}

fn decompress_file(src: &Path, dst: &Path, permissions: fs::Permissions) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }

    let src_file = File::open(src)?;
    let mut decoder = GzDecoder::new(src_file);
    let mut dst_file = File::create(dst)?;
    io::copy(&mut decoder, &mut dst_file)?;
    fs::set_permissions(dst, permissions)?;
    Ok(())
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FileChangeStats {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
}

pub fn parse_git_status(status_output: &str) -> FileChangeStats {
    let mut stats = FileChangeStats::default();

    for line in status_output.split('\n') {
        if line.len() < 3 {
            continue;
        }

        let status_code = &line[0..2];
        let mut filename = line[3..].to_string();

        match status_code {
            "A " | "??" => stats.added.push(filename),
            "M " | " M" | "MM" => stats.modified.push(filename),
            "D " | " D" => stats.deleted.push(filename),
            _ => {
                if status_code.starts_with('R') {
                    if let Some(idx) = filename.find(" -> ") {
                        filename = filename[(idx + 4)..].to_string();
                    }
                    stats.modified.push(filename);
                }
            }
        }
    }

    stats
}

pub fn generate_commit_message(stats: &FileChangeStats) -> (String, String) {
    let total_changes = stats.added.len() + stats.modified.len() + stats.deleted.len();

    let mut subject = String::new();
    subject.push_str("Sync ");
    subject.push_str(&format!("{total_changes} file"));
    if total_changes != 1 {
        subject.push('s');
    }

    let mut parts = Vec::new();
    if !stats.added.is_empty() {
        parts.push(format!("{} added", stats.added.len()));
    }
    if !stats.modified.is_empty() {
        parts.push(format!("{} modified", stats.modified.len()));
    }
    if !stats.deleted.is_empty() {
        parts.push(format!("{} deleted", stats.deleted.len()));
    }

    if !parts.is_empty() {
        subject.push(' ');
        subject.push('(');
        subject.push_str(&parts.join(", "));
        subject.push(')');
    }

    let mut body = String::new();
    let mut first_section = true;

    if !stats.added.is_empty() {
        if !first_section {
            body.push('\n');
        }
        body.push_str("Added files:\n");
        for file in &stats.added {
            body.push_str(&format!("  + {file}\n"));
        }
        first_section = false;
    }

    if !stats.modified.is_empty() {
        if !first_section {
            body.push('\n');
        }
        body.push_str("Modified files:\n");
        for file in &stats.modified {
            body.push_str(&format!("  ~ {file}\n"));
        }
        first_section = false;
    }

    if !stats.deleted.is_empty() {
        if !first_section {
            body.push('\n');
        }
        body.push_str("Deleted files:\n");
        for file in &stats.deleted {
            body.push_str(&format!("  - {file}\n"));
        }
    }

    (subject, body.trim().to_string())
}

pub fn escape_shell_arg(input: &str) -> String {
    let needs_escape = " \t\n\r\"'`$\\|&;<>(){}[]!*?";
    let mut result = String::with_capacity(input.len());

    for ch in input.chars() {
        if needs_escape.contains(ch) {
            result.push('\\');
        }
        result.push(ch);
    }

    result
}

pub fn build_git_ssh_command(ssh_key_path: &str) -> String {
    format!(
        "ssh -i {} -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new",
        escape_shell_arg(ssh_key_path)
    )
}

fn run_command<I, S>(dir: &Path, ssh_key_path: Option<&str>, program: &str, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    if let Some(key_path) = ssh_key_path {
        command.env("GIT_SSH_COMMAND", build_git_ssh_command(key_path));
    }

    let status = command
        .status()
        .with_context(|| format!("failed to run {program}"))?;
    if status.success() {
        Ok(())
    } else {
        bail!("{program} exited with status {status}");
    }
}

fn run_command_output<I, S>(
    dir: &Path,
    ssh_key_path: Option<&str>,
    program: &str,
    args: I,
) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command.args(args).current_dir(dir);
    if let Some(key_path) = ssh_key_path {
        command.env("GIT_SSH_COMMAND", build_git_ssh_command(key_path));
    }

    let output = command
        .output()
        .with_context(|| format!("failed to run {program}"))?;

    if !output.status.success() {
        bail!(
            "{program} failed with status {} and output {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Read;

    #[test]
    fn validate_config_accepts_valid_modes() {
        let config = Config {
            mode: Mode::Push,
            folder_path: PathBuf::from("/tmp/test"),
            repo_url: "https://github.com/user/repo.git".to_string(),
            branch: "main".to_string(),
            ssh_key_path: None,
            compress: false,
        };

        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn validate_config_requires_folder_path() {
        let config = Config {
            mode: Mode::Push,
            folder_path: PathBuf::new(),
            repo_url: "https://github.com/user/repo.git".to_string(),
            branch: "main".to_string(),
            ssh_key_path: None,
            compress: false,
        };

        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn validate_config_rejects_missing_repo() {
        let config = Config {
            mode: Mode::Push,
            folder_path: PathBuf::from("/tmp/test"),
            repo_url: "".to_string(),
            branch: "main".to_string(),
            ssh_key_path: None,
            compress: false,
        };

        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn sync_files_copies_files_and_dirs() {
        let src_dir = tempfile::tempdir().unwrap();
        let dst_dir = tempfile::tempdir().unwrap();

        let file_one = src_dir.path().join("test1.txt");
        fs::write(&file_one, "test content 1").unwrap();

        let nested_dir = src_dir.path().join("subdir");
        fs::create_dir_all(&nested_dir).unwrap();
        let file_two = nested_dir.join("test2.txt");
        fs::write(&file_two, "test content 2").unwrap();

        sync_files(src_dir.path(), dst_dir.path()).unwrap();

        let copied_one = fs::read_to_string(dst_dir.path().join("test1.txt")).unwrap();
        assert_eq!(copied_one, "test content 1");

        let copied_two = fs::read_to_string(dst_dir.path().join("subdir/test2.txt")).unwrap();
        assert_eq!(copied_two, "test content 2");
    }

    #[test]
    fn sync_files_skips_git_directory() {
        let src_dir = tempfile::tempdir().unwrap();
        let dst_dir = tempfile::tempdir().unwrap();

        let git_dir = src_dir.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(git_dir.join("config"), "config").unwrap();

        fs::write(src_dir.path().join("test.txt"), "content").unwrap();

        sync_files(src_dir.path(), dst_dir.path()).unwrap();

        assert!(!dst_dir.path().join(".git").exists());
        assert!(dst_dir.path().join("test.txt").exists());
    }

    #[test]
    fn copy_file_preserves_content() {
        let temp_dir = tempfile::tempdir().unwrap();
        let src = temp_dir.path().join("source.txt");
        let dst = temp_dir.path().join("dest.txt");
        let content = "hello world";
        fs::write(&src, content).unwrap();

        let permissions = fs::metadata(&src).unwrap().permissions();
        copy_file(&src, &dst, permissions).unwrap();

        let mut buf = String::new();
        File::open(&dst).unwrap().read_to_string(&mut buf).unwrap();
        assert_eq!(buf, content);
    }

    #[test]
    fn compression_relative_path_transforms_file_names() {
        let compressed = compress_relative_path(Path::new("dir/file.txt"));
        assert_eq!(compressed, PathBuf::from("dir/file.txt-gzipped.txt"));

        let decompressed = decompress_relative_path(Path::new("dir/file.txt-gzipped.txt"));
        assert_eq!(decompressed, PathBuf::from("dir/file.txt"));

        let untouched = decompress_relative_path(Path::new("dir/plain.txt"));
        assert_eq!(untouched, PathBuf::from("dir/plain.txt"));
    }

    #[test]
    fn sync_files_can_compress_and_decompress() {
        let source_dir = tempfile::tempdir().unwrap();
        let original_file = source_dir.path().join("notes.md");
        fs::write(&original_file, "compressed content").unwrap();

        let compressed_dir = tempfile::tempdir().unwrap();
        sync_files_with_transform(
            source_dir.path(),
            compressed_dir.path(),
            SyncTransform::Compress,
        )
        .unwrap();

        let compressed_path = compressed_dir.path().join("notes.md-gzipped.txt");
        assert!(compressed_path.exists());

        let restored_dir = tempfile::tempdir().unwrap();
        sync_files_with_transform(
            compressed_dir.path(),
            restored_dir.path(),
            SyncTransform::Decompress,
        )
        .unwrap();

        let restored_content = fs::read_to_string(restored_dir.path().join("notes.md")).unwrap();
        assert_eq!(restored_content, "compressed content");
    }

    #[test]
    fn escape_shell_arg_escapes_special_chars() {
        let cases = vec![
            ("/home/user/.ssh/id_rsa", "/home/user/.ssh/id_rsa"),
            (
                "/home/user/my files/.ssh/id_rsa",
                "/home/user/my\\ files/.ssh/id_rsa",
            ),
            ("/home/user's/.ssh/id_rsa", "/home/user\\'s/.ssh/id_rsa"),
            ("/home/user/.ssh/key$file", "/home/user/.ssh/key\\$file"),
            (
                "/home/user name/.ssh/key file (1).pem",
                "/home/user\\ name/.ssh/key\\ file\\ \\(1\\).pem",
            ),
        ];

        for (input, expected) in cases {
            assert_eq!(escape_shell_arg(input), expected);
        }
    }

    #[test]
    fn build_git_ssh_command_formats_correctly() {
        let cases = vec![
            (
                "/home/user/.ssh/id_rsa",
                "ssh -i /home/user/.ssh/id_rsa -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new",
            ),
            (
                "/home/user/my files/.ssh/id_rsa",
                "ssh -i /home/user/my\\ files/.ssh/id_rsa -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new",
            ),
            (
                "/home/user's key/.ssh/deploy (prod).pem",
                "ssh -i /home/user\\'s\\ key/.ssh/deploy\\ \\(prod\\).pem -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new",
            ),
        ];

        for (input, expected) in cases {
            assert_eq!(build_git_ssh_command(input), expected);
        }
    }

    #[test]
    fn parse_git_status_collects_stats() {
        let stats = parse_git_status("A  newfile.txt");
        assert_eq!(
            stats,
            FileChangeStats {
                added: vec!["newfile.txt".into()],
                modified: vec![],
                deleted: vec![],
            }
        );

        let mixed = parse_git_status("A  added.txt\nM  modified.txt\nD  deleted.txt");
        assert_eq!(mixed.added, vec!["added.txt".to_string()]);
        assert_eq!(mixed.modified, vec!["modified.txt".to_string()]);
        assert_eq!(mixed.deleted, vec!["deleted.txt".to_string()]);

        let renamed = parse_git_status("R  old-name.txt -> new-name.txt");
        assert_eq!(renamed.modified, vec!["new-name.txt".to_string()]);
    }

    #[test]
    fn generate_commit_message_formats_output() {
        let stats = FileChangeStats {
            added: vec!["file.txt".into()],
            modified: vec![],
            deleted: vec![],
        };
        let (subject, body) = generate_commit_message(&stats);
        assert_eq!(subject, "Sync 1 file (1 added)");
        assert_eq!(body, "Added files:\n  + file.txt");

        let stats = FileChangeStats {
            added: vec!["new1.txt".into(), "new2.txt".into()],
            modified: vec!["mod.txt".into()],
            deleted: vec!["old.txt".into()],
        };
        let (subject, body) = generate_commit_message(&stats);
        assert_eq!(subject, "Sync 4 files (2 added, 1 modified, 1 deleted)");
        assert!(body.contains("Added files:\n  + new1.txt\n  + new2.txt"));
        assert!(body.contains("Modified files:\n  ~ mod.txt"));
        assert!(body.contains("Deleted files:\n  - old.txt"));
    }
}
