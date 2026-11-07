//! Authenticated, bounded-memory restoration of archived media.
//!
//! Format follows Signal Android MediaName, MediaRootBackupKey and
//! AttachmentCipherInputStream (main, 2026-11-07). Neither plaintext nor inner
//! ciphertext is trusted until its enclosing HMAC has been checked.
use super::{cloud::CloudSession, types::MediaReference};
use aes::{
    Aes256,
    cipher::{Array, BlockCipherDecrypt, KeyInit},
};
use anyhow::{Context, Result, bail, ensure};
use hmac::{Hmac, Mac};
use libsignal_message_backup::proto::backup::{
    FilePointer, file_pointer::locator_info::IntegrityCheck,
};
use protobuf::Message;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
};

pub async fn restore_media(
    session: &CloudSession,
    media_root: &[u8; 32],
    reference: &MediaReference,
    work_dir: &Path,
    files_dir: &Path,
) -> Result<crate::signal::Attachment> {
    let pointer =
        FilePointer::parse_from_bytes(&reference.pointer_bytes).context("invalid media pointer")?;
    let locator = pointer
        .locatorInfo
        .as_ref()
        .context("unsupported media pointer without locatorInfo")?;
    if let Some(IntegrityCheck::EncryptedDigest(digest)) = locator.integrityCheck.as_ref() {
        ensure!(digest.len() == 32, "invalid transit attachment digest");
        ensure!(locator.key.len() == 64, "unsupported media key length");
        let cdn = locator
            .transitCdnNumber
            .context("transit attachment has no CDN number")?;
        let key = locator
            .transitCdnKey
            .as_deref()
            .context("transit attachment has no CDN key")?;
        return restore_transit(
            session,
            reference,
            work_dir,
            files_dir,
            cdn,
            key,
            &locator.key,
            digest,
            locator.size,
        )
        .await;
    }
    let hash = match locator.integrityCheck.as_ref() {
        Some(IntegrityCheck::PlaintextHash(hash)) if hash.len() == 32 => hash,
        _ => bail!(
            "unsupported media locator: archive download requires a 32-byte plaintext hash; transit-only encryptedDigest cannot identify archived media"
        ),
    };
    ensure!(locator.key.len() == 64, "unsupported media key length");

    // The root is random key material, not an account entropy pool.
    let root = libsignal_account_keys::BackupKey(*media_root);
    let media_name = hex::encode([hash.as_slice(), locator.key.as_slice()].concat());
    let media_id = root.derive_media_id(&media_name);
    let outer_key = root.derive_media_encryption_key_data(&media_id);
    checked_directory(work_dir)?;
    checked_directory(files_dir)?;
    let id = hex::encode(hash);
    let name = clean_name(reference.file_name.as_deref(), &id);
    let destination = files_dir.join(format!("{id}-{name}"));
    let attachment = crate::signal::Attachment {
        id,
        content_type: reference.content_type.clone(),
        filename: destination.clone(),
        size: locator.size,
    };
    if destination.symlink_metadata().is_ok() {
        verify_plaintext(&destination, u64::from(locator.size), hash)?;
        return Ok(attachment);
    }
    let encrypted = work_dir.join(format!("{}.encrypted", hex::encode(media_id)));
    if encrypted.symlink_metadata().is_ok() {
        ensure!(
            encrypted.symlink_metadata()?.file_type().is_file(),
            "unsafe encrypted media path"
        );
    } else {
        let cdn = session
            .resolve_media_cdn(media_root, &media_id, locator.mediaTierCdnNumber)
            .await?;
        session
            .download_media_blob(media_root, &media_id, cdn, &encrypted)
            .await?;
    }
    // NamedTempFile creates mode 0600 and removes unpublished plaintext on drop.
    let mut inner = tempfile::NamedTempFile::new_in(work_dir)?;
    let padded = padded_size(u64::from(locator.size));
    let inner_length = 48 + (padded / 16 + 1) * 16;
    decrypt_layer(
        &encrypted,
        &mut inner,
        &outer_key[32..],
        &outer_key[..32],
        Some(inner_length),
    )?;
    inner.flush()?;
    let mut plaintext = tempfile::NamedTempFile::new_in(files_dir)?;
    decrypt_layer(
        inner.path(),
        &mut plaintext,
        &locator.key[..32],
        &locator.key[32..],
        Some(u64::from(locator.size)),
    )?;
    plaintext.flush()?;
    verify_plaintext(plaintext.path(), u64::from(locator.size), hash)?;
    plaintext.as_file().sync_all()?;
    match plaintext.persist_noclobber(&destination) {
        Ok(_) => (),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            verify_plaintext(&destination, u64::from(locator.size), hash)?
        }
        Err(error) => return Err(error.error.into()),
    }
    Ok(attachment)
}

#[allow(clippy::too_many_arguments)]
async fn restore_transit(
    session: &CloudSession,
    reference: &MediaReference,
    work_dir: &Path,
    files_dir: &Path,
    cdn: u32,
    key: &str,
    material: &[u8],
    digest: &[u8],
    size: u32,
) -> Result<crate::signal::Attachment> {
    checked_directory(work_dir)?;
    checked_directory(files_dir)?;
    let encrypted = work_dir.join(format!("transit-{}.encrypted", hex::encode(digest)));
    if !encrypted.try_exists()? {
        session
            .download_transit_blob(cdn, key, &encrypted)
            .await
            .context("transit attachment unavailable (it may have expired)")?;
    }
    ensure!(
        encrypted.symlink_metadata()?.file_type().is_file(),
        "unsafe transit media path"
    );
    // The sender's digest covers the entire inner IV+ciphertext+HMAC.
    let length = encrypted.metadata()?.len();
    verify_plaintext(&encrypted, length, digest).context("transit ciphertext digest mismatch")?;
    let mut plaintext = tempfile::NamedTempFile::new_in(files_dir)?;
    decrypt_layer(
        &encrypted,
        &mut plaintext,
        &material[..32],
        &material[32..],
        Some(u64::from(size)),
    )?;
    plaintext.flush()?;
    let hash = hash_file(plaintext.path())?;
    let id = hex::encode(&hash);
    let destination = files_dir.join(format!(
        "{id}-{}",
        clean_name(reference.file_name.as_deref(), &id)
    ));
    plaintext.as_file().sync_all()?;
    match plaintext.persist_noclobber(&destination) {
        Ok(_) => (),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            verify_plaintext(&destination, u64::from(size), &hash)?
        }
        Err(error) => return Err(error.error.into()),
    }
    Ok(crate::signal::Attachment {
        id,
        content_type: reference.content_type.clone(),
        filename: destination,
        size,
    })
}

fn open_regular(path: &Path) -> Result<File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    ensure!(
        file.metadata()?.file_type().is_file(),
        "unsafe media file type"
    );
    Ok(file)
}

fn hash_file(path: &Path) -> Result<Vec<u8>> {
    let mut file = open_regular(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(digest.finalize().to_vec())
}

fn checked_directory(path: &Path) -> Result<()> {
    // Check existing parents before creation, so a symlink cannot direct creation
    // of missing descendants outside the managed directory.
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) => ensure!(metadata.file_type().is_dir(), "unsafe media directory"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    fs::create_dir_all(path)?;
    // Refuse symlinks in any component, including user-specified parent paths.
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        ensure!(
            fs::symlink_metadata(ancestor)?.file_type().is_dir(),
            "unsafe media directory"
        );
    }
    Ok(())
}

fn clean_name(name: Option<&str>, fallback: &str) -> String {
    let clean: String = name
        .unwrap_or(fallback)
        .chars()
        .map(|c| {
            if c.is_control() || c.is_whitespace() || "<>\"{}^⟨⟩`\\".contains(c) {
                '-'
            } else {
                c
            }
        })
        .collect();
    Path::new(&clean)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .unwrap_or(fallback)
        .chars()
        .scan(0usize, |bytes, c| {
            *bytes += c.len_utf8();
            (*bytes <= 160).then_some(c)
        })
        .collect()
}

fn padded_size(size: u64) -> u64 {
    (1.05_f64
        .powf(((size as f64).ln() / 1.05_f64.ln()).ceil())
        .floor() as u64)
        .max(541)
}

fn verify_plaintext(path: &Path, size: u64, expected: &[u8]) -> Result<()> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "unsafe restored media path"
    );
    let mut file = open_regular(path)?;
    ensure!(
        file.metadata()?.len() == size,
        "restored media size mismatch"
    );
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    ensure!(
        digest.finalize().as_slice() == expected,
        "restored media plaintext hash mismatch"
    );
    Ok(())
}

/// Authenticate IV+ciphertext first, then stream CBC with validated PKCS#7.
/// Signal adds length-hiding padding inside encryption; truncate only after CBC.
fn decrypt_layer(
    path: &Path,
    output: &mut impl Write,
    aes_key: &[u8],
    mac_key: &[u8],
    limit: Option<u64>,
) -> Result<()> {
    let file = open_regular(path)?;
    let length = file.metadata()?.len();
    let mut input = BufReader::with_capacity(65536, file);
    ensure!(
        length >= 64 && (length - 48) % 16 == 0,
        "truncated encrypted media"
    );
    let mut mac = Hmac::<Sha256>::new_from_slice(mac_key)
        .map_err(|_| anyhow::anyhow!("invalid media MAC key"))?;
    let mut remaining = length - 32;
    let mut buffer = [0; 65536];
    while remaining > 0 {
        let n = remaining.min(buffer.len() as u64) as usize;
        input.read_exact(&mut buffer[..n])?;
        mac.update(&buffer[..n]);
        remaining -= n as u64;
    }
    let mut tag = [0; 32];
    input.read_exact(&mut tag)?;
    mac.verify_slice(&tag)
        .map_err(|_| anyhow::anyhow!("encrypted media authentication failed"))?;
    input.seek(SeekFrom::Start(0))?;
    let cipher =
        Aes256::new_from_slice(aes_key).map_err(|_| anyhow::anyhow!("invalid media AES key"))?;
    let mut previous = [0; 16];
    input.read_exact(&mut previous)?;
    let blocks = (length - 48) / 16;
    let mut output = BufWriter::with_capacity(65536, output);
    let mut written = 0u64;
    for index in 0..blocks {
        let mut encrypted = [0; 16];
        input.read_exact(&mut encrypted)?;
        let mut block = Array::from(encrypted);
        cipher.decrypt_block(&mut block);
        for i in 0..16 {
            block[i] ^= previous[i];
        }
        previous = encrypted;
        let used = if index + 1 == blocks {
            let pad = block[15] as usize;
            ensure!(
                (1..=16).contains(&pad) && block[16 - pad..].iter().all(|b| *b as usize == pad),
                "invalid encrypted media padding"
            );
            16 - pad
        } else {
            16
        };
        let take = limit.map_or(used, |limit| {
            used.min(limit.saturating_sub(written) as usize)
        });
        output.write_all(&block[..take])?;
        written += take as u64;
    }
    if let Some(limit) = limit {
        ensure!(
            written == limit,
            "decrypted media shorter than declared size"
        );
    }
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockCipherEncrypt;
    fn encrypt(data: &[u8], aes: &[u8], mac: &[u8]) -> Vec<u8> {
        let cipher = Aes256::new_from_slice(aes).unwrap();
        let mut result = vec![7; 16];
        let mut previous = [7; 16];
        let pad = 16 - data.len() % 16;
        let mut padded = data.to_vec();
        padded.resize(data.len() + pad, pad as u8);
        for chunk in padded.chunks_exact(16) {
            let mut block = Array::from(previous);
            for i in 0..16 {
                block[i] ^= chunk[i];
            }
            cipher.encrypt_block(&mut block);
            result.extend_from_slice(&block);
            previous.copy_from_slice(&block);
        }
        let mut tag = Hmac::<Sha256>::new_from_slice(mac).unwrap();
        tag.update(&result);
        result.extend_from_slice(&tag.finalize().into_bytes());
        result
    }
    #[test]
    fn double_layer_authenticated_and_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let plain = b"a synthetic archived attachment";
        let inner_aes = [1; 32];
        let inner_mac = [2; 32];
        let outer_aes = [3; 32];
        let outer_mac = [4; 32];
        let mut padded = plain.to_vec();
        padded.resize(541, 0);
        let inner = encrypt(&padded, &inner_aes, &inner_mac);
        let inner_len = inner.len();
        let mut outer_plain = inner;
        outer_plain.extend_from_slice(&[0; 91]);
        let mut encrypted = encrypt(&outer_plain, &outer_aes, &outer_mac);
        let path = dir.path().join("encrypted");
        fs::write(&path, &encrypted).unwrap();
        let mut inner_file = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        decrypt_layer(
            &path,
            &mut inner_file,
            &outer_aes,
            &outer_mac,
            Some(inner_len as u64),
        )
        .unwrap();
        let mut output = vec![];
        decrypt_layer(
            inner_file.path(),
            &mut output,
            &inner_aes,
            &inner_mac,
            Some(plain.len() as u64),
        )
        .unwrap();
        assert_eq!(output, plain);
        encrypted[30] ^= 1;
        fs::write(&path, &encrypted).unwrap();
        let mut unpublished = vec![];
        assert!(decrypt_layer(&path, &mut unpublished, &outer_aes, &outer_mac, None).is_err());
        assert!(unpublished.is_empty());
        fs::write(&path, &encrypted[..50]).unwrap();
        assert!(decrypt_layer(&path, &mut unpublished, &outer_aes, &outer_mac, None).is_err());
    }
    #[test]
    fn authenticated_padding_and_declared_size_are_checked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("encrypted");
        let key = [9; 32];
        let mac = [11; 32];
        // Empty attachments still contain a full PKCS#7 block.
        fs::write(&path, encrypt(b"", &key, &mac)).unwrap();
        let mut output = Vec::new();
        decrypt_layer(&path, &mut output, &key, &mac, Some(0)).unwrap();
        assert!(output.is_empty());
        assert!(decrypt_layer(&path, &mut output, &key, &mac, Some(1)).is_err());
        let mut encrypted = encrypt(&[42; 32], &key, &mac);
        // Flip the preceding CBC block to force the final padding byte to zero,
        // then authenticate the modified ciphertext to exercise padding validation.
        let before_last = encrypted.len() - 32 - 16 - 1;
        encrypted[before_last] ^= 16;
        let end = encrypted.len() - 32;
        let mut tag = Hmac::<Sha256>::new_from_slice(&mac).unwrap();
        tag.update(&encrypted[..end]);
        encrypted[end..].copy_from_slice(&tag.finalize().into_bytes());
        fs::write(&path, encrypted).unwrap();
        assert!(decrypt_layer(&path, &mut output, &key, &mac, None).is_err());
    }
    #[test]
    fn plaintext_resume_requires_size_and_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("restored");
        fs::write(&path, b"verified").unwrap();
        let hash = Sha256::digest(b"verified");
        verify_plaintext(&path, 8, &hash).unwrap();
        verify_plaintext(&path, 8, &hash).unwrap();
        assert!(verify_plaintext(&path, 7, &hash).is_err());
        assert!(verify_plaintext(&path, 8, &[0; 32]).is_err());
    }
    #[test]
    fn names_and_padding() {
        assert_eq!(
            clean_name(Some("../../evil name.sh"), "fallback"),
            "evil-name.sh"
        );
        assert_eq!(clean_name(Some(".."), "fallback"), "fallback");
        assert_eq!(padded_size(0), 541);
        assert_eq!(padded_size(1), 541);
        for size in [540, 541, 1000, 1_000_000, u32::MAX as u64] {
            assert!(padded_size(size) >= size);
        }
    }
    #[cfg(unix)]
    #[test]
    fn refuse_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let actual = dir.path().join("actual");
        fs::write(&actual, b"x").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(actual, &link).unwrap();
        assert!(verify_plaintext(&link, 1, &Sha256::digest(b"x")).is_err());
    }
}
