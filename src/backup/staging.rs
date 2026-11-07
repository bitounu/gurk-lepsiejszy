//! Authenticated archive records and recovery tokens stay inside a private SQLCipher database.
use super::{cloud::BackupSecretCache, types::*};
use crate::{
    data::{Channel, ChannelId, GroupData, TypingSet},
    passphrase::Passphrase,
};
use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{
    ConnectOptions, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
};
use std::{future::Future, path::Path};

#[derive(Clone)]
pub struct Staging {
    pool: SqlitePool,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct SavedChannel {
    id: ChannelId,
    name: String,
    group_data: Option<GroupData>,
    expire_timer: Option<u32>,
}

fn block<F: Future>(future: F) -> F::Output {
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(future))
}
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    Ok(postcard::to_allocvec(value)?)
}
fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    Ok(postcard::from_bytes(bytes)?)
}

pub fn private_directory(path: &Path) -> Result<()> {
    let mut current = Some(path);
    while let Some(component) = current {
        if let Ok(metadata) = std::fs::symlink_metadata(component) {
            anyhow::ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "backup directory ancestry must contain only real directories"
            );
        }
        current = component.parent();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(path)?;
    anyhow::ensure!(
        !std::fs::symlink_metadata(path)?.file_type().is_symlink(),
        "backup directory must not be a symlink"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

impl Staging {
    pub async fn open(path: &Path, passphrase: &Passphrase) -> Result<Self> {
        private_directory(path.parent().context("staging database has no parent")?)?;
        if !path.exists() {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
            }
            opts.open(path)?;
        }
        anyhow::ensure!(
            !std::fs::symlink_metadata(path)?.file_type().is_symlink(),
            "staging database must not be a symlink"
        );
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .pragma("key", passphrase.sqlite_pragma_key().to_string())
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .disable_statement_logging();
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        for sql in [
            "CREATE TABLE IF NOT EXISTS secrets (hash BLOB PRIMARY KEY NOT NULL, token BLOB NOT NULL)",
            "CREATE TABLE IF NOT EXISTS metadata (id INTEGER PRIMARY KEY CHECK(id=0), value BLOB NOT NULL)",
            "CREATE TABLE IF NOT EXISTS channels (id BLOB PRIMARY KEY NOT NULL, value BLOB NOT NULL)",
            "CREATE TABLE IF NOT EXISTS records (seq INTEGER PRIMARY KEY NOT NULL, value BLOB NOT NULL)",
            "CREATE TABLE IF NOT EXISTS unsupported (seq INTEGER NOT NULL, reason TEXT NOT NULL, value BLOB NOT NULL, PRIMARY KEY(seq,reason))",
            "CREATE TABLE IF NOT EXISTS status (id INTEGER PRIMARY KEY CHECK(id=0), validated INTEGER NOT NULL, summary BLOB)",
            "CREATE TABLE IF NOT EXISTS media_errors (seq INTEGER NOT NULL, item INTEGER NOT NULL, reason TEXT NOT NULL, PRIMARY KEY(seq,item))",
        ] {
            sqlx::query(sql).execute(&pool).await?;
        }
        Ok(Self { pool })
    }
    /// Open an existing import for offline diagnostics, with SQLite writes disabled.
    pub async fn open_report(path: &Path, passphrase: &Passphrase) -> Result<Self> {
        let metadata = std::fs::symlink_metadata(path)?;
        anyhow::ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "report requires an existing regular staging database"
        );
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .read_only(true)
            .pragma("key", passphrase.sqlite_pragma_key().to_string())
            .pragma("query_only", "ON")
            .disable_statement_logging();
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        Ok(Self { pool })
    }
    pub async fn grouped_media_errors(&self) -> Result<Vec<(String, i64)>> {
        Ok(
            sqlx::query_as("SELECT reason,COUNT(*) FROM media_errors GROUP BY reason")
                .fetch_all(&self.pool)
                .await?,
        )
    }
    pub async fn reset_records(&self) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for sql in [
            "DELETE FROM metadata",
            "DELETE FROM channels",
            "DELETE FROM records",
            "DELETE FROM unsupported",
            "DELETE FROM status",
            "DELETE FROM media_errors",
        ] {
            sqlx::query(sql).execute(&mut *tx).await?;
        }
        sqlx::query("INSERT INTO status(id,validated) VALUES(0,0)")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn finish(&self, summary: &ArchiveSummary) -> Result<()> {
        sqlx::query("UPDATE status SET validated=1,summary=? WHERE id=0")
            .bind(encode(summary)?)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn validated(&self) -> Result<bool> {
        Ok(
            sqlx::query_scalar::<_, bool>("SELECT validated FROM status WHERE id=0")
                .fetch_optional(&self.pool)
                .await?
                .unwrap_or(false),
        )
    }
    pub async fn metadata_value(&self) -> Result<BackupMetadata> {
        decode(
            &sqlx::query_scalar::<_, Vec<u8>>("SELECT value FROM metadata WHERE id=0")
                .fetch_one(&self.pool)
                .await?,
        )
    }
    pub async fn channels(&self) -> Result<Vec<Channel>> {
        let rows = sqlx::query_scalar::<_, Vec<u8>>("SELECT value FROM channels ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|raw| {
                let c: SavedChannel = decode(&raw)?;
                Ok(Channel {
                    id: c.id,
                    name: c.name,
                    group_data: c.group_data,
                    unread_messages: 0,
                    muted: false,
                    typing: TypingSet::new(!c.id.is_user()),
                    expire_timer: c.expire_timer,
                })
            })
            .collect()
    }
    pub async fn records(&self, after: Option<u64>, limit: usize) -> Result<Vec<ImportedMessage>> {
        let rows = if let Some(after) = after {
            sqlx::query_scalar::<_, Vec<u8>>(
                "SELECT value FROM records WHERE seq>? ORDER BY seq LIMIT ?",
            )
            .bind(i64::try_from(after)?)
            .bind(i64::try_from(limit)?)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_scalar::<_, Vec<u8>>("SELECT value FROM records ORDER BY seq LIMIT ?")
                .bind(i64::try_from(limit)?)
                .fetch_all(&self.pool)
                .await?
        };
        rows.into_iter().map(|row| decode(&row)).collect()
    }
    pub async fn media_error(&self, seq: u64, item: usize, reason: &str) -> Result<()> {
        sqlx::query("INSERT INTO media_errors(seq,item,reason) VALUES(?,?,?) ON CONFLICT(seq,item) DO UPDATE SET reason=excluded.reason")
            .bind(i64::try_from(seq)?).bind(i64::try_from(item)?).bind(reason).execute(&self.pool).await?;
        Ok(())
    }
}
impl ArchiveSink for Staging {
    fn metadata(&mut self, metadata: BackupMetadata) -> Result<()> {
        block(
            sqlx::query("INSERT INTO metadata(id,value) VALUES(0,?)")
                .bind(encode(&metadata)?)
                .execute(&self.pool),
        )?;
        Ok(())
    }
    fn channel(&mut self, c: Channel) -> Result<()> {
        let id = encode(&c.id)?;
        let c = SavedChannel {
            id: c.id,
            name: c.name,
            group_data: c.group_data,
            expire_timer: c.expire_timer,
        };
        block(sqlx::query("INSERT INTO channels(id,value) VALUES(?,?) ON CONFLICT(id) DO UPDATE SET value=excluded.value").bind(id).bind(encode(&c)?).execute(&self.pool))?;
        Ok(())
    }
    fn message(&mut self, record: ImportedMessage) -> Result<()> {
        block(
            sqlx::query("INSERT INTO records(seq,value) VALUES(?,?)")
                .bind(i64::try_from(record.render_index)?)
                .bind(encode(&record)?)
                .execute(&self.pool),
        )?;
        Ok(())
    }
    fn unsupported(&mut self, record: UnsupportedRecord) -> Result<()> {
        block(sqlx::query("INSERT INTO unsupported(seq,reason,value) VALUES(?,?,?) ON CONFLICT(seq,reason) DO NOTHING")
            .bind(i64::try_from(record.frame_index)?).bind(&record.reason).bind(encode(&record)?).execute(&self.pool))?;
        Ok(())
    }
}
impl BackupSecretCache for Staging {
    fn load_token(&self, hash: [u8; 32]) -> Result<Option<[u8; 32]>> {
        let bytes = block(
            sqlx::query_scalar::<_, Vec<u8>>("SELECT token FROM secrets WHERE hash=?")
                .bind(hash.as_slice())
                .fetch_optional(&self.pool),
        )?;
        bytes
            .map(|bytes| {
                bytes
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("invalid encrypted recovery token"))
            })
            .transpose()
    }
    fn store_token(&self, hash: [u8; 32], token: [u8; 32]) -> Result<()> {
        block(sqlx::query("INSERT INTO secrets(hash,token) VALUES(?,?) ON CONFLICT(hash) DO UPDATE SET token=excluded.token")
            .bind(hash.as_slice()).bind(token.as_slice()).execute(&self.pool))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(flavor = "multi_thread")]
    async fn recovery_cache_is_encrypted_and_survives_record_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stage.sqlite");
        let pass = Passphrase::new("test-stage-secret").unwrap();
        let stage = Staging::open(&path, &pass).await.unwrap();
        stage.store_token([3; 32], [0x61; 32]).unwrap();
        stage.reset_records().await.unwrap();
        assert_eq!(stage.load_token([3; 32]).unwrap(), Some([0x61; 32]));
        assert!(!stage.validated().await.unwrap());
        stage.pool.close().await;
        let raw = std::fs::read(&path).unwrap();
        assert!(!raw.starts_with(b"SQLite format 3"));
        assert!(!raw.windows(32).any(|window| window == [0x61; 32]));
        let stage = Staging::open(&path, &pass).await.unwrap();
        assert_eq!(stage.load_token([3; 32]).unwrap(), Some([0x61; 32]));
        stage.pool.close().await;
        assert!(
            Staging::open(&path, &Passphrase::new("wrong-secret").unwrap())
                .await
                .is_err()
        );
    }
}
