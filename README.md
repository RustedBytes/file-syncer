# file-syncer

Rust CLI that synchronizes a local folder with a git repository in either push or pull mode.

## Features

- Push mode: sync local files to a repository branch
- Pull mode: sync repository files to a local folder
- Private repository support via your existing git/SSH configuration
- Optional custom SSH key via `GIT_SSH_COMMAND` construction
- Size-based log rotation (10MB, keep 3 files) logging to both stdout and `file-syncer.log`
- Skips the `.git` directory during sync
- Generates commit messages based on detected file changes

## Installation

### Prerequisites

- Rust 1.74 or later
- Git

### Build from Source

```bash
git clone https://github.com/rikkicom/file-syncer.git
cd file-syncer
cargo build --release
```

## Usage

```
file-syncer --mode <push|pull> --folder <path> --repo <url> [--branch <branch>] [--ssh-key <path>]
```

Run directly from source:

```bash
cargo run -- --mode push --folder ./myfiles --repo https://github.com/user/repo.git
```

Installed binary:

```bash
cargo install --path .
file-syncer --mode pull --folder ./myfiles --repo https://github.com/user/repo.git --branch develop
```

## Examples

### Example 1: Backing up local files to GitHub

```bash
# First time: push files to a new repository
file-syncer --mode push --folder ~/documents --repo https://github.com/yourusername/my-backup.git

# Later: push updates
file-syncer --mode push --folder ~/documents --repo https://github.com/yourusername/my-backup.git
```

### Example 2: Syncing files between machines

On machine 1:
```bash
file-syncer --mode push --folder ~/projects/shared --repo https://github.com/yourusername/shared-files.git
```

On machine 2:
```bash
file-syncer --mode pull --folder ~/projects/shared --repo https://github.com/yourusername/shared-files.git
```

### Example 3: Using a custom SSH key

```bash
# Push with a specific SSH key (useful for deployment keys or multiple accounts)
file-syncer --mode push --folder ~/backups --repo git@github.com:yourusername/backup-repo.git --ssh-key ~/.ssh/deployment_key

# Pull with a specific SSH key
file-syncer --mode pull --folder ~/restore --repo git@github.com:yourusername/backup-repo.git --ssh-key ~/.ssh/deployment_key
```

## Private Repository Authentication

The application supports both public and private repositories. For private repositories, ensure your system is configured with appropriate git credentials:

### SSH Keys (Recommended)

```bash
# Use SSH URL format with system default SSH key
file-syncer --mode push --folder ./myfiles --repo git@github.com:yourusername/private-repo.git

# Or specify a custom SSH key
file-syncer --mode push --folder ./myfiles --repo git@github.com:yourusername/private-repo.git --ssh-key ~/.ssh/custom_id_rsa
```

### HTTPS with Credential Helper

```bash
# Configure git credential helper (one-time setup)
git config --global credential.helper store

# Or use GitHub CLI for authentication
gh auth login

# Then use HTTPS URL
./file-syncer -mode push -folder ./myfiles -repo https://github.com/yourusername/private-repo.git
```

### Personal Access Token

For HTTPS URLs, you can embed credentials or use a credential helper. The application inherits all git configuration from your system.

## Logging

Logs are emitted to stdout and `file-syncer.log` with size-based rotation (10MB, keep 3 rotated files). The log format is the default provided by `flexi_logger`.

Logs are written to the current working directory.

## Testing

Run the unit tests:

```bash
cargo test
```

Integration tests (use the real `git` binary) are behind a feature flag:

```bash
cargo test --features integration
```
