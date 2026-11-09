//! Open attachments with the operating system's registered file associations.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, LazyLock},
};

use anyhow::{Context, Result, bail};
use tokio::sync::{Semaphore, oneshot};

static OPEN_SLOTS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(2)));

/// Wait for the desktop launcher away from the UI's async executor.
pub(crate) async fn open_file(path: PathBuf) -> Result<()> {
    let permit = OPEN_SLOTS
        .clone()
        .try_acquire_owned()
        .context("file opener is busy; wait for an earlier open request to finish")?;
    let (tx, rx) = oneshot::channel();
    // Some launchers live as long as the GUI application. A detached thread
    // reaps that process without holding up Tokio runtime shutdown.
    std::thread::Builder::new()
        .name("file-opener".into())
        .spawn(move || {
            let _permit = permit;
            let _ = tx.send(open_file_with(&path, desktop_opener()));
        })
        .context("cannot start file opener worker")?;
    rx.await
        .context("file opener worker stopped unexpectedly")?
}

fn desktop_opener() -> &'static OsStr {
    #[cfg(target_os = "macos")]
    let executable = "open";
    #[cfg(target_os = "windows")]
    let executable = "explorer.exe";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let executable = "xdg-open";
    OsStr::new(executable)
}

fn open_file_with(path: &Path, executable: impl AsRef<OsStr>) -> Result<()> {
    let path = path
        .canonicalize()
        .with_context(|| format!("cannot resolve file {}", path.display()))?;
    if !path
        .metadata()
        .with_context(|| format!("cannot inspect file {}", path.display()))?
        .is_file()
    {
        bail!("cannot open {}: not a regular file", path.display());
    }

    // The canonical absolute path is one literal argument, including any spaces
    // or shell metacharacters. Let the desktop resolve its MIME association.
    let status = Command::new(executable.as_ref())
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| {
            format!(
                "cannot launch {} for {}",
                executable.as_ref().to_string_lossy(),
                path.display()
            )
        })?;
    if !status.success() {
        bail!(
            "{} failed to open {} ({status})",
            executable.as_ref().to_string_lossy(),
            path.display()
        );
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use super::*;

    fn fake_opener(directory: &Path, script: &str) -> PathBuf {
        let executable = directory.join("fake opener");
        fs::write(&executable, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        executable
    }

    #[test]
    fn passes_exactly_one_literal_canonical_path() {
        let directory = tempfile::tempdir().unwrap();
        let attachment = directory.path().join("-photo $(touch injected); ' &.odd");
        fs::write(&attachment, b"attachment").unwrap();
        let executable = fake_opener(
            directory.path(),
            "printf '%s\\n' \"$#\" \"$1\" > \"$0.args\"",
        );

        open_file_with(&attachment, &executable).unwrap();

        let captured = fs::read_to_string(directory.path().join("fake opener.args")).unwrap();
        assert_eq!(
            captured,
            format!("1\n{}\n", attachment.canonicalize().unwrap().display())
        );
        assert!(!directory.path().join("injected").exists());
    }

    #[test]
    fn reports_launcher_failure() {
        let directory = tempfile::tempdir().unwrap();
        let attachment = directory.path().join("file");
        fs::write(&attachment, b"attachment").unwrap();
        let executable = fake_opener(directory.path(), "exit 17");

        let error = open_file_with(&attachment, &executable)
            .unwrap_err()
            .to_string();
        assert!(error.contains("failed to open"));
        assert!(error.contains("17"));
    }

    #[test]
    fn rejects_missing_files_and_directories_before_launching() {
        let directory = tempfile::tempdir().unwrap();
        let executable = fake_opener(directory.path(), "touch \"$0.launched\"");

        assert!(open_file_with(&directory.path().join("missing"), &executable).is_err());
        let error = open_file_with(directory.path(), &executable)
            .unwrap_err()
            .to_string();
        assert!(error.contains("not a regular file"));
        assert!(!directory.path().join("fake opener.launched").exists());
    }

    #[test]
    fn reports_missing_launcher() {
        let directory = tempfile::tempdir().unwrap();
        let attachment = directory.path().join("file");
        fs::write(&attachment, b"attachment").unwrap();

        let error = open_file_with(&attachment, directory.path().join("missing opener"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("cannot launch"));
    }

    #[tokio::test]
    async fn rejects_requests_when_both_launcher_slots_are_busy() {
        let _permits = OPEN_SLOTS.clone().try_acquire_many_owned(2).unwrap();

        let error = open_file(PathBuf::from("unused"))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("busy"));
    }
}
