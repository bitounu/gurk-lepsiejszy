//! Offline diagnostics: only fixed categories and counts may leave encrypted staging.
use super::staging::Staging;
use crate::data::ChannelId;
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Default, Debug, serde::Serialize)]
pub struct Report {
    pub validated: bool,
    pub staged_messages: u64,
    pub media_errors: BTreeMap<String, u64>,
    pub duplicate_identical: u64,
    pub duplicate_different_sender: u64,
    pub duplicate_different_payload: u64,
}

fn category(reason: &str) -> String {
    let scope = if reason.contains("transit attachment unavailable") {
        "transit"
    } else {
        "backup"
    };
    if let Some((_, rest)) = reason.split_once("(HTTP ") {
        let code = rest.as_bytes();
        if code.len() >= 4 && code[..3].iter().all(u8::is_ascii_digit) && code[3] == b')' {
            return format!("{scope}_http_{}", &rest[..3]);
        }
    }
    for (needle, label) in [
        (
            "absent from the existing Secure Backup",
            "media_absent_from_archive",
        ),
        ("transport failed", "network_transport"),
        ("download interrupted", "network_interrupted"),
        ("authentication failed", "media_hmac_failed"),
        ("digest mismatch", "ciphertext_digest_mismatch"),
        ("plaintext hash mismatch", "plaintext_hash_mismatch"),
        ("padding", "invalid_padding"),
        ("size mismatch", "size_mismatch"),
        ("shorter than declared size", "declared_size_mismatch"),
        ("truncated encrypted media", "truncated_ciphertext"),
        ("unsupported media", "unsupported_media_locator"),
        ("invalid media", "invalid_media_locator"),
        ("transit attachment has no", "incomplete_transit_locator"),
        ("No space left", "disk_full"),
        ("Permission denied", "filesystem_permission"),
        ("No such file", "filesystem_missing_file"),
        ("unsafe", "unsafe_filesystem_path"),
        ("object changed", "download_object_changed"),
        ("resume range", "invalid_resume_range"),
        ("object length", "missing_download_length"),
    ] {
        if reason.contains(needle) {
            return label.into();
        }
    }
    "other_error".into()
}

pub async fn inspect(stage: &Staging) -> Result<Report> {
    let mut report = Report {
        validated: stage.validated().await?,
        ..Default::default()
    };
    for (reason, count) in stage.grouped_media_errors().await? {
        *report.media_errors.entry(category(&reason)).or_default() += u64::try_from(count)?;
    }
    // Retain only identities and digests in RAM, never the whole message history.
    let mut seen = BTreeMap::<(ChannelId, u64), (uuid::Uuid, [u8; 32])>::new();
    let mut after = None;
    loop {
        let batch = stage.records(after, 1).await?;
        if batch.is_empty() {
            break;
        }
        for record in batch {
            after = Some(record.render_index);
            report.staged_messages += 1;
            let key = (record.channel_id, record.message.arrived_at);
            let digest: [u8; 32] =
                Sha256::digest(postcard::to_allocvec(&(&record.message, &record.media))?).into();
            if let Some((sender, old_digest)) = seen.get(&key) {
                if sender != &record.message.from_id {
                    report.duplicate_different_sender += 1;
                } else if old_digest == &digest {
                    report.duplicate_identical += 1;
                } else {
                    report.duplicate_different_payload += 1;
                }
            } else {
                seen.insert(key, (record.message.from_id, digest));
            }
        }
    }
    Ok(report)
}

pub fn print(report: &Report) {
    println!("Offline backup report (no Signal connection)");
    println!(
        "Authenticated staging: {}; staged messages: {}",
        report.validated, report.staged_messages
    );
    println!(
        "Recorded media errors: {}",
        report.media_errors.values().sum::<u64>()
    );
    for (reason, count) in &report.media_errors {
        println!("  {reason}: {count}");
    }
    println!(
        "Repeated archive timestamps: {} identical, {} different sender, {} different payload",
        report.duplicate_identical,
        report.duplicate_different_sender,
        report.duplicate_different_payload
    );
    println!(
        "Different senders are distinct messages, now supported with separate local IDs. Counts describe the source archive, not unresolved import conflicts."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostic_labels_never_echo_sensitive_error_details() {
        assert_eq!(
            category(
                "transit attachment unavailable (it may have expired): Backup CDN download failed (HTTP 404)"
            ),
            "transit_http_404"
        );
        assert_eq!(
            category("Backup request failed (HTTP 403)"),
            "backup_http_403"
        );
        assert_eq!(
            category("encrypted media authentication failed"),
            "media_hmac_failed"
        );
        for secret in [
            "/home/private/secret.jpg",
            "https://example.org/?token=SECRET",
            "(HTTP SECRET)",
            "message text SECRET",
        ] {
            assert_eq!(category(secret), "other_error");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn offline_report_reads_existing_errors_and_collisions_without_writes() {
        use crate::{
            backup::types::{ArchiveSink, ArchiveSummary, ImportedMessage},
            data::Message,
            passphrase::Passphrase,
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stage.sqlite");
        let pass = Passphrase::new("test-report-secret").unwrap();
        let mut stage = Staging::open(&path, &pass).await.unwrap();
        stage.reset_records().await.unwrap();
        let sender = uuid::Uuid::new_v4();
        let channel = ChannelId::User(sender);
        for (index, from, at, text) in [
            (1, sender, 10, "first"),
            (2, sender, 10, "first"),
            (3, sender, 10, "different"),
            (4, uuid::Uuid::new_v4(), 10, "first"),
            (5, sender, 11, "unique"),
        ] {
            stage
                .message(ImportedMessage {
                    channel_id: channel,
                    message: Message::text(from, at, text.into()),
                    render_index: index,
                    media: vec![],
                })
                .unwrap();
        }
        stage
            .media_error(1, 0, "Backup CDN download failed (HTTP 404)")
            .await
            .unwrap();
        stage
            .media_error(2, 0, "Backup CDN download failed (HTTP 404)")
            .await
            .unwrap();
        stage
            .media_error(3, 0, "secret file path and key")
            .await
            .unwrap();
        stage.finish(&ArchiveSummary::default()).await.unwrap();
        let readonly = Staging::open_report(&path, &pass).await.unwrap();
        let report = inspect(&readonly).await.unwrap();
        assert!(report.validated);
        assert_eq!(report.staged_messages, 5);
        assert_eq!(report.duplicate_identical, 1);
        assert_eq!(report.duplicate_different_payload, 1);
        assert_eq!(report.duplicate_different_sender, 1);
        assert_eq!(report.media_errors.get("backup_http_404"), Some(&2));
        assert_eq!(report.media_errors.get("other_error"), Some(&1));
        let output = serde_json::to_string(&report).unwrap();
        assert!(!output.contains("secret"));
        assert!(!output.contains("first"));
        assert!(!output.contains(&sender.to_string()));
        assert!(readonly.reset_records().await.is_err());
        assert!(stage.validated().await.unwrap());
        assert_eq!(
            stage
                .grouped_media_errors()
                .await
                .unwrap()
                .iter()
                .map(|(_, n)| n)
                .sum::<i64>(),
            3
        );
        let missing = dir.path().join("absent.sqlite");
        assert!(Staging::open_report(&missing, &pass).await.is_err());
        assert!(!missing.exists());
    }
}
