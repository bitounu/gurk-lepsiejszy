//! Explicit startup restore mode. Never starts the live receiver or changes registration.
use super::{
    archive::read_archive,
    cloud::CloudSession,
    import::{CloudMedia, merge},
    staging::{Staging, private_directory},
};
use crate::{config::Config, passphrase::Passphrase, storage::SqliteStorage};
use anyhow::{Context, Result};
use std::{fs, path::PathBuf, sync::Arc};
use zeroize::{Zeroize, Zeroizing};

#[derive(Default)]
pub struct RestoreOptions {
    pub work_dir: Option<PathBuf>,
    pub key_file: Option<PathBuf>,
    pub dry_run: bool,
    pub messages_only: bool,
}
fn normalize_key(key: &str) -> Zeroizing<String> {
    Zeroizing::new(
        key.chars()
            .filter(|c| !c.is_ascii_whitespace() && *c != '-')
            .map(|c| match c {
                '#' => 'o',
                '=' => '0',
                _ => c.to_ascii_lowercase(),
            })
            .collect(),
    )
}
fn read_key(path: Option<&std::path::Path>) -> Result<Zeroizing<String>> {
    let raw = if let Some(path) = path {
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        let metadata = file.metadata()?;
        anyhow::ensure!(
            metadata.is_file(),
            "recovery key must be a regular private file"
        );
        anyhow::ensure!(
            metadata.len() <= 4096,
            "recovery key file is unexpectedly large"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            anyhow::ensure!(
                metadata.permissions().mode() & 0o077 == 0,
                "recovery key file must not be readable by other users (chmod 600)"
            );
        }
        use std::io::Read;
        let mut text = Zeroizing::new(String::new());
        file.take(4097).read_to_string(&mut text)?;
        anyhow::ensure!(
            text.len() <= 4096,
            "recovery key file is unexpectedly large"
        );
        text
    } else {
        Zeroizing::new(
            dialoguer::Password::new()
                .with_prompt("Signal Secure Backup recovery key (64 characters)")
                .interact()?,
        )
    };
    Ok(normalize_key(&raw))
}

pub async fn run(config: Config, passphrase: Passphrase, options: RestoreOptions) -> Result<()> {
    private_directory(&config.data_dir)?;
    let lock_path = config.data_dir.join("backup-import.lock");
    let mut opts = fs::OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let lock = opts.open(lock_path)?;
    lock.try_lock()
        .context("another backup import is already running")?;
    let account = crate::signal::backup_account(&config, &passphrase).await?;
    let own = account.aci;
    let work = options
        .work_dir
        .unwrap_or_else(|| config.data_dir.join("backup-import"));
    private_directory(&work)?;
    let mut secret = read_key(options.key_file.as_deref())?;
    let mut session = CloudSession::new(account, &secret)?;
    secret.zeroize();
    let stage = Staging::open(&work.join("staging.sqlite"), &passphrase).await?;
    session.set_secret_cache(Arc::new(stage.clone()));
    eprintln!(
        "Downloading existing Secure Backup; no phone registration or backup changes are made."
    );
    eprintln!(
        "Modern archives may require one bounded SVR-B recovery; cryptographic failures are not retried automatically."
    );
    let downloaded = session.download_archive(&work).await?;
    stage.reset_records().await?;
    let mut sink = stage.clone();
    eprintln!("Authenticating and staging archive...");
    let summary = read_archive(&downloaded.path, &downloaded.key, own, &mut sink).await?;
    stage.finish(&summary).await?;
    println!(
        "Validated archive: {} messages, {} conversations, {} frames",
        summary.messages, summary.channels, summary.frames
    );
    for (reason, count) in &summary.unsupported {
        println!("Preserved raw records ({reason}): {count}");
    }
    if options.dry_run {
        println!(
            "Dry run complete: local chat history was not changed. Encrypted staging is retained for resume."
        );
        return Ok(());
    }
    let metadata = stage.metadata_value().await?;
    let url = config
        .sqlite
        .as_ref()
        .map(|settings| settings.url.clone())
        .unwrap_or(
            url::Url::from_file_path(config.gurk_db_path())
                .map_err(|_| anyhow::anyhow!("invalid gurk database path"))?,
        );
    let mut storage = SqliteStorage::maybe_encrypt_and_open(&url, &passphrase, false).await?;
    let media_work = work.join("media");
    private_directory(&media_work)?;
    let files_dir = config.data_dir.join("files").join("backup");
    private_directory(&files_dir)?;
    let loader = CloudMedia {
        session: &session,
        media_root: metadata.media_root,
        work_dir: &media_work,
        files_dir: &files_dir,
    };
    let report = merge(
        &stage,
        &mut storage,
        (!options.messages_only).then_some(&loader as &dyn super::import::MediaLoader),
    )
    .await?;
    println!(
        "Imported: {} new conversations, {} new messages; {} existing messages preserved",
        report.channels_added, report.messages_added, report.duplicates_preserved
    );
    println!(
        "Media: {} restored/verified, {} failed; {} messages deferred; {} timestamp collisions",
        report.media_restored,
        report.media_failed,
        report.messages_deferred,
        report.timestamp_collisions
    );
    println!("Existing aliases, hidden conversations, drafts and registration were preserved.");
    println!(
        "Expired disappearing messages omitted: {}",
        report.expired_skipped
    );
    println!(
        "Media skipped: {}; oversized long-text records: {}",
        report.media_skipped, report.long_text_deferred
    );
    if report.media_failed > 0
        || report.messages_deferred > 0
        || report.timestamp_collisions > 0
        || report.long_text_deferred > 0
    {
        anyhow::bail!(
            "import incomplete; encrypted report and resume data remain in {}",
            work.display()
        );
    }
    println!(
        "Supported history import finished. Run gurk normally; review the unsupported-record counts above."
    );
    Ok(())
}

/// Reads existing encrypted staging only; does not load account credentials or recovery keys.
pub async fn report(
    config: Config,
    passphrase: Passphrase,
    work_dir: Option<PathBuf>,
) -> Result<()> {
    let work = work_dir.unwrap_or_else(|| config.data_dir.join("backup-import"));
    let stage = Staging::open_report(&work.join("staging.sqlite"), &passphrase).await?;
    super::report::print(&super::report::inspect(&stage).await?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_display_format_is_normalized_without_accepting_extra_data() {
        assert_eq!(&**normalize_key("ABCD #= ef-gh\n"), "abcdo0efgh");
        assert!(
            libsignal_account_keys::AccountEntropyPool::from_str(&normalize_key("short")).is_err()
        );
    }
    use std::str::FromStr;
}
