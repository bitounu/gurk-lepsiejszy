//! Screenshot capture and durable PNG storage, independent of attachment staging.
use anyhow::{Context, Result, anyhow, bail};
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn default_screenshot_dir(home: &Path) -> PathBuf {
    let pictures = home.join("Pictures");
    if pictures.is_dir() {
        pictures
    } else {
        home.to_owned()
    }
}

pub fn resolve_screenshot_dir(configured: Option<&Path>, home: &Path) -> PathBuf {
    match configured {
        None => default_screenshot_dir(home),
        Some(path) if path == Path::new("~") => home.to_owned(),
        Some(path) => path
            .strip_prefix("~/")
            .map(|suffix| home.join(suffix))
            .unwrap_or_else(|_| path.to_owned()),
    }
}

/// Capture interactively, returning `None` when the user cancels. Custom commands
/// receive the output filename as one final argument, without shell expansion.
pub async fn capture_screenshot(
    directory: PathBuf,
    command: Vec<String>,
) -> Result<Option<PathBuf>> {
    if !command.is_empty() {
        return tokio::task::spawn_blocking(move || capture_command(&directory, &command)).await?;
    }
    #[cfg(target_os = "linux")]
    {
        match portal_screenshot().await {
            Ok(None) => Ok(None),
            Ok(Some(source)) => tokio::task::spawn_blocking(move || save_png(&source, &directory))
                .await?
                .map(Some),
            Err(error) => {
                // A denied request is not permission to try a different capture mechanism.
                if !matches!(
                    error,
                    ashpd::Error::Zbus(_)
                        | ashpd::Error::PortalNotFound(_)
                        | ashpd::Error::RequiresVersion(_, _)
                ) {
                    return Err(error.into());
                }
                let portal_error = error.to_string();
                tokio::task::spawn_blocking(move || {
                    let candidates: &[&[&str]] = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                        &[&["grim"]]
                    } else if std::env::var_os("DISPLAY").is_some() {
                        &[&["maim", "-s"], &["scrot", "-s"]]
                    } else { &[] };
                    for candidate in candidates {
                        let command: Vec<_> = candidate.iter().map(|arg| (*arg).to_owned()).collect();
                        match capture_command(&directory, &command) {
                            Err(error) if error.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) => continue,
                            result => return result,
                        }
                    }
                    bail!("Screenshot portal unavailable ({portal_error}); install a screenshot portal or configure screenshot_command")
                }).await?
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    bail!("Configure screenshot_command to capture screenshots on this platform")
}

#[cfg(target_os = "linux")]
async fn portal_screenshot() -> std::result::Result<Option<PathBuf>, ashpd::Error> {
    let response = match ashpd::desktop::screenshot::Screenshot::request()
        .interactive(true)
        .modal(true)
        .send()
        .await
    {
        Ok(request) => request.response(),
        Err(error) => Err(error),
    };
    match response {
        Ok(screenshot) => {
            let uri = url::Url::parse(screenshot.uri().as_str())
                .map_err(|_| ashpd::Error::ParseError("Invalid screenshot URI"))?;
            let path = uri.to_file_path().map_err(|_| {
                ashpd::Error::ParseError("Screenshot portal did not return a local file")
            })?;
            Ok(Some(path))
        }
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled))
        | Err(ashpd::Error::Portal(ashpd::PortalError::Cancelled(_))) => Ok(None),
        Err(error) => Err(error),
    }
}

fn capture_command(directory: &Path, command: &[String]) -> Result<Option<PathBuf>> {
    let (program, args) = command
        .split_first()
        .context("Screenshot command is empty")?;
    fs::create_dir_all(directory).context("Cannot create screenshot directory")?;
    let temporary = tempfile::Builder::new()
        .prefix(".gurk-capture-")
        .tempdir_in(directory)?;
    let target = temporary.path().join("capture.png");
    let status = Command::new(program)
        .args(args)
        .arg(&target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("Cannot run screenshot command {program}"))?;
    // SIGINT and conventional interrupt exit codes indicate interactive cancellation.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if status.signal() == Some(2) {
            return Ok(None);
        }
    }
    if status.code() == Some(130) {
        return Ok(None);
    }
    if !status.success() {
        bail!("Screenshot command {program} exited with {status}");
    }
    if !target.exists() {
        return Ok(None);
    }
    save_png(&target, directory).map(Some)
}

fn save_png(source: &Path, directory: &Path) -> Result<PathBuf> {
    // Content detection and bounded decoding prevent corrupt or oversized output
    // from becoming an attachment. Always encode PNG, including JPEG portal output.
    let mut reader = image::ImageReader::open(source)?.with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .context("Screenshot output is not a supported image")?;
    fs::create_dir_all(directory).context("Cannot create screenshot directory")?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = directory.join(format!(
        "gurk-screenshot-{timestamp}-{}.png",
        uuid::Uuid::new_v4()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    if let Err(error) = image
        .write_to(&mut file, image::ImageFormat::Png)
        .map_err(|e| anyhow!(e))
        .and_then(|_| file.sync_all().map_err(Into::into))
    {
        drop(file);
        let _ = fs::remove_file(&path);
        return Err(error).context("Cannot save screenshot PNG");
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_tilde_expansion() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(default_screenshot_dir(home.path()), home.path());
        std::fs::create_dir(home.path().join("Pictures")).unwrap();
        assert_eq!(
            default_screenshot_dir(home.path()),
            home.path().join("Pictures")
        );
        assert_eq!(
            resolve_screenshot_dir(Some(Path::new("~")), home.path()),
            home.path()
        );
        assert_eq!(
            resolve_screenshot_dir(Some(Path::new("~/shots with spaces")), home.path()),
            home.path().join("shots with spaces")
        );
        assert_eq!(
            resolve_screenshot_dir(Some(Path::new("~other/shots")), home.path()),
            Path::new("~other/shots")
        );
    }

    #[test]
    fn non_png_content_is_converted_and_source_preserved() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("portal image.bmp");
        image::RgbImage::new(3, 2).save(&source).unwrap();
        let output = root.path().join("saved");
        let result = save_png(&source, &output).unwrap();
        assert!(source.exists());
        assert_eq!(
            image::guess_format(&std::fs::read(&result).unwrap()).unwrap(),
            image::ImageFormat::Png
        );
        assert_eq!(image::open(result).unwrap().height(), 2);
    }

    #[cfg(unix)]
    fn script(dir: &Path, body: &str) -> Vec<String> {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(format!("capture-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        vec![path.to_str().unwrap().to_owned()]
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_handles_spaces_and_publishes_valid_png() {
        let root = tempfile::tempdir().unwrap();
        let fixture = root.path().join("fixture.png");
        image::RgbImage::new(2, 3).save(&fixture).unwrap();
        let mut command = script(root.path(), "cp -- \"$1\" \"$2\"");
        command.push(fixture.to_str().unwrap().to_owned());
        let dir = root.path().join("screenshots ; $(never-run)");
        let first = capture_screenshot(dir.clone(), command.clone())
            .await
            .unwrap()
            .unwrap();
        let second = capture_screenshot(dir.clone(), command)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(first, second);
        assert_eq!(image::open(first).unwrap().width(), 2);
        assert_eq!(std::fs::read_dir(dir).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_failure_and_invalid_output_leave_no_files() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("shots");
        let cancel = script(root.path(), "exit 130");
        assert!(
            capture_screenshot(dir.clone(), cancel)
                .await
                .unwrap()
                .is_none()
        );
        let no_capture = script(root.path(), "exit 0");
        assert!(
            capture_screenshot(dir.clone(), no_capture)
                .await
                .unwrap()
                .is_none()
        );
        let failure = script(root.path(), "printf partial > \"$1\"; exit 7");
        assert!(capture_screenshot(dir.clone(), failure).await.is_err());
        let invalid = script(root.path(), "printf invalid > \"$1\"");
        assert!(capture_screenshot(dir.clone(), invalid).await.is_err());
        assert_eq!(std::fs::read_dir(dir).unwrap().count(), 0);
    }
}
