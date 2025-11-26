use std::fs;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use file_syncer::{Config, Mode, run};
use zstd::stream::read::Decoder as ZstdDecoder;

struct TempRemoteRepo {
    _base_dir: tempfile::TempDir,
    path: PathBuf,
}

impl TempRemoteRepo {
    fn path(&self) -> &Path {
        &self.path
    }
}

#[test]
fn push_integration_pushes_files_to_remote() {
    require_git();
    set_git_identity_env();

    let remote = create_remote_repo_with_content([("seed.txt", "initial content")]);

    let source_dir = tempfile::tempdir().expect("failed to create source dir");
    write_test_file(source_dir.path(), "new-file.txt", "integration content");

    let config = Config {
        mode: Mode::Push,
        folder_path: source_dir.path().to_path_buf(),
        repo_url: remote.path().to_string_lossy().to_string(),
        branch: "main".to_string(),
        ssh_key_path: None,
        compress: false,
        compression_level: file_syncer::CompressionLevel::Default,
        thread_count: None,
        sentry_dsn: None,
    };

    run(&config).expect("run() push failed");

    let verification_dir = tempfile::tempdir().expect("failed to create verification dir");
    run_git(
        verification_dir.path(),
        [
            "clone",
            "--branch",
            "main",
            remote.path().to_str().unwrap(),
            ".",
        ],
    );

    let content =
        fs::read_to_string(verification_dir.path().join("new-file.txt")).expect("read synced file");
    assert_eq!(content, "integration content");
}

#[test]
fn pull_integration_pulls_files_from_remote() {
    require_git();
    set_git_identity_env();

    let remote = create_remote_repo_with_content([("pull-dir/file.txt", "pulled content")]);
    let destination_dir = tempfile::tempdir().expect("failed to create destination dir");

    let config = Config {
        mode: Mode::Pull,
        folder_path: destination_dir.path().to_path_buf(),
        repo_url: remote.path().to_string_lossy().to_string(),
        branch: "main".to_string(),
        ssh_key_path: None,
        compress: false,
        compression_level: file_syncer::CompressionLevel::Default,
        thread_count: None,
        sentry_dsn: None,
    };

    run(&config).expect("run() pull failed");

    let content = fs::read_to_string(destination_dir.path().join("pull-dir/file.txt"))
        .expect("read pulled file");
    assert_eq!(content, "pulled content");

    assert!(
        !destination_dir.path().join(".git").exists(),
        ".git directory should not be present in destination"
    );
}

#[test]
fn compression_round_trip_push_and_pull() {
    require_git();
    set_git_identity_env();

    let remote = create_remote_repo_with_content([("seed.txt", "initial content")]);

    let source_dir = tempfile::tempdir().expect("failed to create source dir");
    write_test_file(source_dir.path(), "reports/data.log", "compressed body");

    let push_config = Config {
        mode: Mode::Push,
        folder_path: source_dir.path().to_path_buf(),
        repo_url: remote.path().to_string_lossy().to_string(),
        branch: "main".to_string(),
        ssh_key_path: None,
        compress: true,
        compression_level: file_syncer::CompressionLevel::Max,
        thread_count: None,
        sentry_dsn: None,
    };

    run(&push_config).expect("run() push with compression failed");

    let verification_dir = tempfile::tempdir().expect("failed to create verification dir");
    run_git(
        verification_dir.path(),
        [
            "clone",
            "--branch",
            "main",
            remote.path().to_str().unwrap(),
            ".",
        ],
    );

    let compressed_path = verification_dir.path().join("reports/data.log-zstd");
    assert!(compressed_path.exists());

    let mut decoded = String::new();
    let file = File::open(&compressed_path).expect("open compressed file");
    let mut decoder = ZstdDecoder::new(file).expect("create decoder");
    decoder
        .read_to_string(&mut decoded)
        .expect("decode zstd contents");
    assert_eq!(decoded, "compressed body");

    let pull_dir = tempfile::tempdir().expect("failed to create pull dir");
    let pull_config = Config {
        mode: Mode::Pull,
        folder_path: pull_dir.path().to_path_buf(),
        repo_url: remote.path().to_string_lossy().to_string(),
        branch: "main".to_string(),
        ssh_key_path: None,
        compress: true,
        compression_level: file_syncer::CompressionLevel::Max,
        thread_count: None,
        sentry_dsn: None,
    };

    run(&pull_config).expect("run() pull with compression failed");

    let pulled =
        fs::read_to_string(pull_dir.path().join("reports/data.log")).expect("read pulled file");
    assert_eq!(pulled, "compressed body");
}

fn create_remote_repo_with_content<const N: usize>(files: [(&str, &str); N]) -> TempRemoteRepo {
    let base_dir = tempfile::tempdir().expect("failed to create base dir");
    let remote_path = base_dir.path().join("remote.git");
    run_git(
        base_dir.path(),
        ["init", "--bare", remote_path.to_str().unwrap()],
    );

    let working_dir = tempfile::tempdir().expect("failed to create working dir");
    run_git(working_dir.path(), ["init"]);
    run_git(
        working_dir.path(),
        ["config", "user.email", "file-syncer@example.com"],
    );
    run_git(working_dir.path(), ["config", "user.name", "file-syncer"]);

    for (path, content) in files {
        write_test_file(working_dir.path(), path, content);
    }

    run_git(working_dir.path(), ["add", "."]);
    run_git(working_dir.path(), ["commit", "-m", "seed"]);
    run_git(working_dir.path(), ["branch", "-M", "main"]);
    run_git(
        working_dir.path(),
        ["remote", "add", "origin", remote_path.to_str().unwrap()],
    );
    run_git(working_dir.path(), ["push", "-u", "origin", "main"]);
    run_git(
        remote_path.as_path(),
        ["symbolic-ref", "HEAD", "refs/heads/main"],
    );

    TempRemoteRepo {
        _base_dir: base_dir,
        path: remote_path,
    }
}

fn run_git<P, I, S>(dir: P, args: I)
where
    P: AsRef<Path>,
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = Command::new("git")
        .args(args)
        .current_dir(dir.as_ref())
        .output()
        .expect("failed to run git");

    if !output.status.success() {
        panic!(
            "git command failed: {}\nstdout: {}\nstderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn write_test_file(base_dir: &Path, relative: &str, content: &str) {
    let full_path = base_dir.join(relative);
    if let Some(parent) = full_path.parent() {
        fs::create_dir_all(parent).expect("failed to create parent dirs");
    }
    fs::write(full_path, content).expect("failed to write file");
}

fn set_git_identity_env() {
    unsafe {
        std::env::set_var("GIT_AUTHOR_NAME", "file-syncer");
        std::env::set_var("GIT_AUTHOR_EMAIL", "file-syncer@example.com");
        std::env::set_var("GIT_COMMITTER_NAME", "file-syncer");
        std::env::set_var("GIT_COMMITTER_EMAIL", "file-syncer@example.com");
    }
}

fn require_git() {
    let Ok(status) = Command::new("git").arg("--version").status() else {
        panic!("git not available in PATH");
    };
    if !status.success() {
        panic!("git not available in PATH");
    }
}
