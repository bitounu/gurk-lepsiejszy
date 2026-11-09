//! Open an isolated, disposable copy of a file in the configured editor.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, bail, ensure};

const MAX_PREVIEW_BYTES: u64 = 100 * 1024 * 1024;

/// Run `$EDITOR` against a private, read-only disposable copy of a file.
///
/// This blocks until the editor exits; callers must release terminal ownership
/// and use a blocking task when called from an asynchronous event loop.
pub fn preview_file(path: PathBuf) -> anyhow::Result<()> {
    let editor = std::env::var("EDITOR")
        .context("Set $EDITOR to an editor command before previewing files")?;
    preview_file_with_editor(&path, &editor)
}

fn preview_file_with_editor(path: &Path, editor: &str) -> anyhow::Result<()> {
    let arguments =
        shell_words::split(editor).context("Cannot parse $EDITOR: check its quoting")?;
    let Some((program, arguments)) = arguments.split_first() else {
        bail!("Set $EDITOR to a nonempty editor command before previewing files");
    };
    ensure!(
        !program.is_empty(),
        "$EDITOR must name an editor executable"
    );

    let source = path
        .canonicalize()
        .context("Cannot resolve preview source file")?;
    let metadata = fs::metadata(&source).context("Cannot inspect preview source file")?;
    ensure!(metadata.is_file(), "Preview source must be a regular file");
    ensure!(
        metadata.len() <= MAX_PREVIEW_BYTES,
        "Preview file exceeds the 100 MiB limit"
    );
    let source_file = File::open(&source).context("Cannot open preview source file")?;
    let metadata = source_file
        .metadata()
        .context("Cannot inspect opened preview source file")?;
    ensure!(metadata.is_file(), "Preview source must be a regular file");
    ensure!(
        metadata.len() <= MAX_PREVIEW_BYTES,
        "Preview file exceeds the 100 MiB limit"
    );
    let name = path
        .file_name()
        .or_else(|| source.file_name())
        .context("Preview source has no file name")?;
    let directory = tempfile::Builder::new()
        .prefix("gurk-file-preview-")
        .tempdir()
        .context("Cannot create private preview directory")?;
    let copy_path = directory.path().join(name);
    let mut copy = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&copy_path)
        .context("Cannot create preview copy")?;
    // Recheck through a bounded read: the source can grow after metadata is read.
    let copied = io::copy(&mut source_file.take(MAX_PREVIEW_BYTES + 1), &mut copy)
        .context("Cannot copy preview source file")?;
    ensure!(
        copied <= MAX_PREVIEW_BYTES,
        "Preview file exceeds the 100 MiB limit"
    );
    copy.sync_all().context("Cannot finish preview copy")?;
    drop(copy);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&copy_path, fs::Permissions::from_mode(0o400))
            .context("Cannot make preview copy read-only")?;
    }
    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(&copy_path)?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&copy_path, permissions)
            .context("Cannot make preview copy read-only")?;
    }

    // No shell, variable expansion, or filename interpolation is involved.
    let status = Command::new(program)
        .args(arguments)
        .arg(&copy_path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("Cannot launch $EDITOR for file preview");
    // Explicit close reports cleanup failures rather than silently relying on Drop.
    // Run cleanup on launch failures and nonzero exit statuses as well.
    let preview_directory = directory.path().to_path_buf();
    directory.close().with_context(|| {
        format!(
            "Cannot remove temporary editor preview {}",
            preview_directory.display()
        )
    })?;
    let status = status?;
    ensure!(status.success(), "$EDITOR exited unsuccessfully: {status}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn rejects_empty_or_unparseable_editor() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("document.pdf");
        fs::write(&source, b"original").unwrap();
        assert!(
            preview_file_with_editor(&source, "  ")
                .unwrap_err()
                .to_string()
                .contains("EDITOR")
        );
        assert!(preview_file_with_editor(&source, "editor 'unterminated").is_err());
    }

    #[test]
    fn rejects_missing_or_nonregular_sources() {
        let directory = tempfile::tempdir().unwrap();
        assert!(preview_file_with_editor(directory.path(), "editor").is_err());
        assert!(preview_file_with_editor(&directory.path().join("missing"), "editor").is_err());
    }

    #[test]
    fn rejects_oversized_source_without_launching_editor() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("large.bin");
        fs::File::create(&source)
            .unwrap()
            .set_len(MAX_PREVIEW_BYTES + 1)
            .unwrap();
        let error =
            preview_file_with_editor(&source, "nonexistent-editor-for-preview-test").unwrap_err();
        assert!(error.to_string().contains("100 MiB"));
    }

    #[cfg(unix)]
    fn fake_editor(directory: &std::path::Path, exit: u8) -> (String, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let script = directory.join("fake editor.sh");
        let record = directory.join("record.args");
        fs::write(
            &script,
            format!(
                r#"#!/bin/sh
record="$1"
shift
printf '%s\n' "$@" > "$record"
shift
copy="$1"
permissions=$(LC_ALL=C ls -ld "$copy")
case "$permissions" in -r--------*) ;; *) exit 18 ;; esac
# A determined editor can replace the read-only preview; the original must survive.
chmod u+w "$copy"
printf 'changed preview' > "$copy"
printf 'swap data' > "$copy.swp"
exit {exit}
"#
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        (
            format!(
                "{} {} 'quoted $HOME argument'",
                shell_words::quote(script.to_str().unwrap()),
                shell_words::quote(record.to_str().unwrap())
            ),
            record,
        )
    }

    #[cfg(unix)]
    #[test]
    fn quoted_args_and_metacharacter_filename_are_literal_original_is_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory
            .path()
            .join("note $(printf injected); '$file'.pdf");
        fs::write(&source, b"original contents").unwrap();
        let (editor, record) = fake_editor(directory.path(), 0);
        preview_file_with_editor(&source, &editor).unwrap();
        let args = fs::read_to_string(record).unwrap();
        let args: Vec<_> = args.lines().collect();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "quoted $HOME argument");
        let copy = std::path::Path::new(args[1]);
        assert_eq!(copy.file_name(), source.file_name());
        assert_ne!(copy, source);
        assert!(!copy.parent().unwrap().exists());
        assert_eq!(fs::read(source).unwrap(), b"original contents");
    }

    #[cfg(unix)]
    #[test]
    fn failed_editor_still_removes_copy_and_sidecars() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("document.bin");
        fs::write(&source, b"original").unwrap();
        let (editor, record) = fake_editor(directory.path(), 7);
        assert!(preview_file_with_editor(&source, &editor).is_err());
        let args = fs::read_to_string(record).unwrap();
        let copy = std::path::Path::new(args.lines().nth(1).unwrap());
        assert!(!copy.parent().unwrap().exists());
        assert_eq!(fs::read(source).unwrap(), b"original");
    }
}
