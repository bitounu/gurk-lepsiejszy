//! Download existing Signal Secure Backups without reserving IDs or changing keys.
use anyhow::{Context, Result, bail, ensure};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use libsignal_account_keys::{AccountEntropyPool, BackupKey};
use libsignal_core::Aci;
use libsignal_message_backup::key::MessageBackupKey;
use presage::libsignal_service::configuration::{
    Endpoint, ServiceConfiguration, ServiceCredentials, SignalServers,
};
use rand::{RngCore, TryRngCore};
use reqwest::{
    Client,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
use zkgroup::{
    Timestamp,
    backups::{
        BackupAuthCredentialRequestContext, BackupAuthCredentialResponse, BackupCredentialType,
    },
    generic_server_params::GenericServerPublicParams,
};

#[path = "svrb.rs"]
mod svrb;

// Signal-Android a74583b app/build.gradle.kts BACKUP_SERVER_PUBLIC_PARAMS.
const PROD_PARAMS: &str = "AZwNSU55fsFCbgaxGRD11wO1juAs8Yr5GF8FPlGzzvdJJIKH5/4CC7ZJSOe3yL2vturVaRU2Cx0n751Vt8wkj1Y4pyiScu0/S10n647ipo+iq97JZQv+UOlwH8ThyNlGT5DfxXCwTqivxHuXvZpuezPgHk5Gxl5aC6xuNxOnwmFlmu4CeSgdhW8+Pp0vAJOQ1MsU2D0+/kzI+tU94nB3tybY/Ao1AcGW2q41uKQbnOJUWwmQaFT6s+xTISgzsg7CPox6oORGX8rnyk/9lic3DbGsUHctIVpMAl/ogJBb4aYC";
const STAGING_PARAMS: &str = "AXYrGb9IfugAAJiPKp+mdXUx+OL9zBolPYHYQz6GI1gWjpEu5me3zVNSvmYY4zWboZHif+HG1sDHSuvwFd0QszS6h3nZ6vRdM/IYGK+cLynw3ucWo7idf3zjOG3b6JnGT/z7XYCr6HuOGkWH4DQWCH98hxVZMGOgmT8DCQoqebQb3oK1yrwEglRWmtI01KhRg9RGUKoQiwuej1JZEY8uaG4Uz9n1cVODJ1iuByhNqGHo+KfI4iWhjtx2AnhYqHViQ3CMd4ASGBJtic9UTFVk/4vegVIy0wfYsAmViftzK6t4";

pub struct CloudAccount {
    pub aci: Uuid,
    pub credentials: ServiceCredentials,
    pub servers: SignalServers,
}
pub struct DownloadedArchive {
    pub path: PathBuf,
    pub key: MessageBackupKey,
}
pub trait BackupSecretCache: Send + Sync {
    fn load_token(&self, metadata_hash: [u8; 32]) -> Result<Option<[u8; 32]>>;
    fn store_token(&self, metadata_hash: [u8; 32], token: [u8; 32]) -> Result<()>;
}
type LocationCache = HashMap<([u8; 32], u8, Option<u32>), (std::time::Instant, Info, HeaderMap)>;
type MediaCdnCache = HashMap<([u8; 32], [u8; 15]), u32>;
pub struct CloudSession {
    account: CloudAccount,
    config: ServiceConfiguration,
    client: Client,
    key: BackupKey,
    params: GenericServerPublicParams,
    secret_cache: Option<Arc<dyn BackupSecretCache>>,
    media_cdns: Mutex<MediaCdnCache>,
    locations: Mutex<LocationCache>,
    #[cfg(test)]
    test_origin: Option<url::Url>,
}
#[derive(Deserialize)]
struct Credential {
    credential: String,
    #[serde(rename = "redemptionTime")]
    redemption_time: u64,
}
#[derive(Deserialize)]
struct Credentials {
    credentials: HashMap<String, Vec<Credential>>,
}
impl Credentials {
    fn current(&self, kind: BackupCredentialType, day: u64) -> Result<&Credential> {
        let name = match kind {
            BackupCredentialType::Messages => "messages",
            BackupCredentialType::Media => "media",
        };
        self.credentials
            .get(name)
            .and_then(|items| items.iter().find(|item| item.redemption_time == day))
            .context("No current backup credential; backup IDs will not be changed")
    }
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Info {
    cdn: u32,
    backup_dir: String,
    media_dir: String,
    backup_name: Option<String>,
}
#[derive(Deserialize)]
struct ReadAuth {
    headers: HashMap<String, String>,
}
#[derive(Deserialize)]
struct SvrAuth {
    username: String,
    password: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MediaPage {
    stored_media_objects: Vec<MediaObject>,
    cursor: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MediaObject {
    cdn: u32,
    media_id: String,
}

impl CloudSession {
    pub fn new(account: CloudAccount, recovery: &str) -> Result<Self> {
        let aep = AccountEntropyPool::from_str(recovery).map_err(|_| {
            anyhow::anyhow!("Recovery key must be a valid 64-character Signal recovery key")
        })?;
        ensure!(
            account.credentials.aci == Some(account.aci),
            "Linked account ACI does not match backup account"
        );
        ensure!(
            account.credentials.password.is_some(),
            "Linked account credentials are missing"
        );
        let config = ServiceConfiguration::from(account.servers);
        let root = reqwest::Certificate::from_pem(config.certificate_authority.as_bytes())
            .context("Invalid Signal TLS certificate")?;
        let client = Client::builder()
            .add_root_certificate(root)
            .connect_timeout(std::time::Duration::from_secs(30))
            .read_timeout(std::time::Duration::from_secs(120))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| anyhow::anyhow!("Cannot configure backup transport"))?;
        let encoded = if account.servers == SignalServers::Production {
            PROD_PARAMS
        } else {
            STAGING_PARAMS
        };
        let params = GenericServerPublicParams::try_from(
            STANDARD
                .decode(encoded)
                .context("Invalid backup verification parameters")?
                .as_slice(),
        )
        .map_err(|_| anyhow::anyhow!("Invalid backup verification parameters"))?;
        Ok(Self {
            account,
            config,
            client,
            key: BackupKey::derive_from_account_entropy_pool(&aep),
            params,
            secret_cache: None,
            media_cdns: Mutex::new(HashMap::new()),
            locations: Mutex::new(HashMap::new()),
            #[cfg(test)]
            test_origin: None,
        })
    }

    pub fn set_secret_cache(&mut self, cache: Arc<dyn BackupSecretCache>) {
        self.secret_cache = Some(cache);
    }

    async fn get<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        headers: HeaderMap,
        account_auth: bool,
    ) -> Result<T> {
        // All control requests pass through this closed GET-only allowlist.
        ensure!(
            matches!(
                path.split('?').next(),
                Some(
                    "/v1/archives/auth"
                        | "/v1/archives"
                        | "/v1/archives/auth/read"
                        | "/v1/archives/auth/svrb"
                        | "/v1/archives/media"
                )
            ),
            "Disallowed backup endpoint"
        );
        ensure!(
            !account_auth || path.starts_with("/v1/archives/auth?"),
            "Account authentication is forbidden on anonymous backup requests"
        );
        let url = self.endpoint_url(Endpoint::service(path))?;
        let mut request = self.client.get(url).headers(headers);
        if account_auth {
            request = request.basic_auth(
                self.account.credentials.login(),
                self.account.credentials.password.as_ref(),
            );
        }
        let response = request
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Backup request transport failed"))?;
        ensure!(
            response.status().is_success(),
            "Backup request failed (HTTP {})",
            response.status().as_u16()
        );
        response
            .json()
            .await
            .map_err(|_| anyhow::anyhow!("Invalid backup service response"))
    }

    fn endpoint_url(&self, endpoint: Endpoint<'_>) -> Result<url::Url> {
        let url = endpoint
            .into_url(&self.config)
            .map_err(|_| anyhow::anyhow!("Invalid backup endpoint"))?;
        #[cfg(test)]
        if let Some(origin) = &self.test_origin {
            return origin
                .join(&format!(
                    "{}{}",
                    url.path(),
                    url.query().map(|q| format!("?{q}")).unwrap_or_default()
                ))
                .map_err(Into::into);
        }
        Ok(url)
    }

    async fn auth(&self, key: &BackupKey, kind: BackupCredentialType) -> Result<HeaderMap> {
        let day = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() / 86400 * 86400;
        let reply: Credentials = self
            .get(
                &format!(
                    "/v1/archives/auth?redemptionStartSeconds={day}&redemptionEndSeconds={day}"
                ),
                HeaderMap::new(),
                true,
            )
            .await?;
        let item = reply.current(kind, day)?;
        let response: BackupAuthCredentialResponse = zkgroup::deserialize(
            &STANDARD
                .decode(&item.credential)
                .map_err(|_| anyhow::anyhow!("Invalid backup credential"))?,
        )
        .map_err(|_| anyhow::anyhow!("Invalid backup credential"))?;
        let aci = Aci::from(self.account.aci);
        let credential = BackupAuthCredentialRequestContext::new(key, aci)
            .receive(response, &self.params, Timestamp::from_epoch_seconds(day))
            .map_err(|_| {
                anyhow::anyhow!("Recovery key does not match the existing backup credential")
            })?;
        ensure!(
            credential.credential_type() == kind,
            "Unexpected backup credential type"
        );
        let mut rng = rand::rngs::OsRng.unwrap_err();
        let mut randomness = [0u8; 32];
        rng.fill_bytes(&mut randomness);
        let presentation = zkgroup::serialize(&credential.present(&self.params, randomness));
        let signature = key
            .derive_ec_key(&aci)
            .calculate_signature(&presentation, &mut rng)
            .map_err(|_| anyhow::anyhow!("Cannot sign backup credential"))?;
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-signal-zk-auth",
            HeaderValue::from_str(&STANDARD.encode(&presentation))?,
        );
        headers.insert(
            "x-signal-zk-auth-signature",
            HeaderValue::from_str(&STANDARD.encode(signature))?,
        );
        Ok(headers)
    }

    async fn location(
        &self,
        key: &BackupKey,
        kind: BackupCredentialType,
        requested_cdn: Option<u32>,
    ) -> Result<(Info, HeaderMap)> {
        // Avoid three service requests per attachment in large archives. Keep credentials
        // only in memory, bounded by the two backup keys and the small CDN set.
        let cache_key = (Sha256::digest(key.0).into(), kind as u8, requested_cdn);
        if let Some((created, info, headers)) = self
            .locations
            .lock()
            .map_err(|_| anyhow::anyhow!("Backup location cache unavailable"))?
            .get(&cache_key)
            && created.elapsed() < std::time::Duration::from_secs(300)
        {
            return Ok((info.clone(), headers.clone()));
        }
        let auth = self.auth(key, kind).await?;
        let info: Info = self.get("/v1/archives", auth.clone(), false).await?;
        let cdn = requested_cdn.unwrap_or(info.cdn);
        let read: ReadAuth = self
            .get(&format!("/v1/archives/auth/read?cdn={cdn}"), auth, false)
            .await?;
        let mut headers = HeaderMap::new();
        for (name, value) in read.headers {
            headers.insert(
                HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| anyhow::anyhow!("Invalid CDN header"))?,
                HeaderValue::from_str(&value).map_err(|_| anyhow::anyhow!("Invalid CDN header"))?,
            );
        }
        self.locations
            .lock()
            .map_err(|_| anyhow::anyhow!("Backup location cache unavailable"))?
            .insert(
                cache_key,
                (std::time::Instant::now(), info.clone(), headers.clone()),
            );
        Ok((info, headers))
    }

    pub async fn download_archive(&self, work_dir: &Path) -> Result<DownloadedArchive> {
        fs::create_dir_all(work_dir)?;
        let (info, headers) = self
            .location(&self.key, BackupCredentialType::Messages, None)
            .await?;
        let path = work_dir.join("secure-backup.bin");
        self.download(
            info.cdn,
            &format!(
                "{}/{}",
                segment(&info.backup_dir)?,
                segment(
                    info.backup_name
                        .as_deref()
                        .context("Existing message backup has no archive object")?
                )?
            ),
            headers,
            &path,
        )
        .await?;
        let metadata = read_metadata(&path)?;
        let token = if let Some(metadata) = metadata {
            let backup_id = self.key.derive_backup_id(&Aci::from(self.account.aci));
            let mut hash = Sha256::new();
            hash.update(b"gurk-secure-backup-svrb-token-v1");
            hash.update(backup_id.0);
            hash.update(&metadata);
            let hash: [u8; 32] = hash.finalize().into();
            let cached = self
                .secret_cache
                .as_ref()
                .map(|cache| cache.load_token(hash))
                .transpose()?
                .flatten();
            if let Some(token) = cached {
                Some(libsignal_account_keys::BackupForwardSecrecyToken(token))
            } else {
                let auth = self.auth(&self.key, BackupCredentialType::Messages).await?;
                let svr: SvrAuth = self.get("/v1/archives/auth/svrb", auth, false).await?;
                let token = svrb::recover(
                    &self.key,
                    &metadata,
                    svr.username,
                    svr.password,
                    self.account.servers == SignalServers::Production,
                )
                .await?;
                if let Some(cache) = &self.secret_cache {
                    cache.store_token(hash, token.0)?;
                }
                Some(token)
            }
        } else {
            None
        };
        let key = MessageBackupKey::derive(
            &self.key,
            &self.key.derive_backup_id(&Aci::from(self.account.aci)),
            token.as_ref(),
        );
        Ok(DownloadedArchive { path, key })
    }

    pub async fn download_media_blob(
        &self,
        media_root: &[u8; 32],
        media_id: &[u8; 15],
        cdn: u32,
        target: &Path,
    ) -> Result<()> {
        let key = BackupKey(*media_root);
        let (info, headers) = self
            .location(&key, BackupCredentialType::Media, Some(cdn))
            .await?;
        self.download(
            cdn,
            &format!(
                "{}/{}/{}",
                segment(&info.backup_dir)?,
                segment(&info.media_dir)?,
                URL_SAFE_NO_PAD.encode(media_id)
            ),
            headers,
            target,
        )
        .await
    }

    pub async fn resolve_media_cdn(
        &self,
        media_root: &[u8; 32],
        media_id: &[u8; 15],
        hint: Option<u32>,
    ) -> Result<u32> {
        if let Some(cdn) = hint {
            return Ok(cdn);
        }
        let root_hash: [u8; 32] = Sha256::digest(media_root).into();
        if let Some(cdn) = self
            .media_cdns
            .lock()
            .map_err(|_| anyhow::anyhow!("Media cache unavailable"))?
            .get(&(root_hash, *media_id))
        {
            return Ok(*cdn);
        }
        let key = BackupKey(*media_root);
        let auth = self.auth(&key, BackupCredentialType::Media).await?;
        let mut cursor = None::<String>;
        let mut seen = std::collections::HashSet::new();
        loop {
            let mut url = Endpoint::service("/v1/archives/media")
                .into_url(&self.config)
                .map_err(|_| anyhow::anyhow!("Invalid backup endpoint"))?;
            url.query_pairs_mut().append_pair("limit", "10000");
            if let Some(cursor) = &cursor {
                url.query_pairs_mut().append_pair("cursor", cursor);
            }
            let page: MediaPage = self
                .get(
                    &format!("{}?{}", url.path(), url.query().unwrap_or("")),
                    auth.clone(),
                    false,
                )
                .await?;
            let mut found = None;
            {
                let mut cache = self
                    .media_cdns
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Media cache unavailable"))?;
                for item in page.stored_media_objects {
                    let bytes = URL_SAFE_NO_PAD
                        .decode(item.media_id.trim_end_matches('='))
                        .map_err(|_| anyhow::anyhow!("Invalid backup media identifier"))?;
                    let id: [u8; 15] = bytes
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("Invalid backup media identifier"))?;
                    cache.insert((root_hash, id), item.cdn);
                    if &id == media_id {
                        found = Some(item.cdn);
                    }
                }
            }
            if let Some(cdn) = found {
                return Ok(cdn);
            }
            cursor = page.cursor;
            match &cursor {
                None => bail!("Media object is absent from the existing Secure Backup"),
                Some(next) => ensure!(
                    seen.insert(next.clone()),
                    "Backup media list repeated its cursor"
                ),
            }
        }
    }

    pub async fn download_transit_blob(&self, cdn: u32, key: &str, target: &Path) -> Result<()> {
        self.download_path(
            cdn,
            &format!("attachments/{}", segment(key)?),
            HeaderMap::new(),
            target,
        )
        .await
    }

    async fn download(
        &self,
        cdn: u32,
        object: &str,
        headers: HeaderMap,
        target: &Path,
    ) -> Result<()> {
        self.download_path(cdn, &format!("backups/{object}"), headers, target)
            .await
    }

    async fn download_path(
        &self,
        cdn: u32,
        object: &str,
        headers: HeaderMap,
        target: &Path,
    ) -> Result<()> {
        let url = self.endpoint_url(Endpoint::cdn(cdn, format!("/{object}")))?;
        let partial = target.with_extension("part");
        let state_path = target.with_extension("resume.json");
        let old: Option<Resume> = fs::read(&state_path)
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok());
        let offset = fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
        let can_resume = old.as_ref().filter(|state| {
            state.object == object
                && state.cdn == cdn
                && !state.etag.is_empty()
                && !state.etag.starts_with("W/")
                && offset > 0
                && offset < state.size
        });
        let completed = old.as_ref().filter(|state| {
            state.object == object
                && state.cdn == cdn
                && !state.etag.is_empty()
                && fs::metadata(target)
                    .map(|m| m.len() == state.size)
                    .unwrap_or(false)
        });
        let mut request = self
            .client
            .get(url)
            .headers(headers)
            .header("accept-encoding", "identity");
        if let Some(state) = completed {
            request = request.header("if-none-match", &state.etag);
        }
        if let Some(state) = can_resume {
            request = request
                .header("range", format!("bytes={offset}-"))
                .header("if-range", &state.etag);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Backup download transport failed"))?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            ensure!(completed.is_some(), "Unexpected unchanged CDN response");
            return Ok(());
        }
        let append = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        let length = response
            .content_length()
            .context("Backup CDN did not provide an object length")?;
        let etag = response
            .headers()
            .get("etag")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let size = if append {
            let state = can_resume.context("Unexpected partial CDN response")?;
            ensure!(etag == state.etag, "Backup object changed during download");
            let expected = format!("bytes {}-{}/{}", offset, state.size - 1, state.size);
            ensure!(
                response
                    .headers()
                    .get("content-range")
                    .and_then(|v| v.to_str().ok())
                    == Some(expected.as_str())
                    && length == state.size - offset,
                "Invalid CDN resume range"
            );
            state.size
        } else {
            ensure!(
                response.status() == reqwest::StatusCode::OK,
                "Backup CDN download failed (HTTP {})",
                response.status().as_u16()
            );
            length
        };
        let state = Resume {
            object: object.into(),
            cdn,
            etag,
            size,
        };
        let mut metadata = private_file(&state_path, false)?;
        metadata.write_all(&serde_json::to_vec(&state)?)?;
        let mut file = private_file(&partial, append)?;
        let mut written = if append { offset } else { 0 };
        let mut progress_at = written;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("Backup download interrupted; partial file retained"))?
        {
            written += chunk.len() as u64;
            ensure!(
                written <= size,
                "Backup CDN object exceeded its declared length"
            );
            file.write_all(&chunk)?;
            if written.saturating_sub(progress_at) >= 64 * 1024 * 1024 {
                eprintln!(
                    "Downloading backup data: {} / {} MiB",
                    written / (1024 * 1024),
                    size / (1024 * 1024)
                );
                progress_at = written;
            }
        }
        ensure!(written == size, "Incomplete backup CDN object");
        file.sync_all()?;
        fs::rename(&partial, target)?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct Resume {
    object: String,
    cdn: u32,
    etag: String,
    size: u64,
}
fn private_file(path: &Path, append: bool) -> Result<fs::File> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create(true)
        .append(append)
        .truncate(!append);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}
fn segment(value: &str) -> Result<&str> {
    ensure!(
        !value.is_empty()
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "Invalid backup object path"
    );
    Ok(value)
}
fn read_metadata(path: &Path) -> Result<Option<Vec<u8>>> {
    let mut file = fs::File::open(path)?;
    let mut magic = [0; 8];
    file.read_exact(&mut magic)?;
    if &magic != b"SBACKUP\x01" {
        return Ok(None);
    }
    let mut size = 0usize;
    for shift in (0..35).step_by(7) {
        let mut byte = [0];
        file.read_exact(&mut byte)?;
        size |= ((byte[0] & 127) as usize) << shift;
        if byte[0] < 128 {
            ensure!(
                size > 0 && size <= 4096,
                "Invalid backup forward-secrecy metadata size"
            );
            let mut metadata = vec![0; size];
            file.read_exact(&mut metadata)?;
            validate_metadata(&metadata)?;
            return Ok(Some(metadata));
        }
    }
    bail!("Invalid backup metadata length")
}

fn validate_metadata(metadata: &[u8]) -> Result<()> {
    use protobuf::Message;
    let parsed = libsignal_svrb::proto::backup_metadata::MetadataPb::parse_from_bytes(metadata)
        .map_err(|_| anyhow::anyhow!("Invalid backup forward-secrecy metadata"))?;
    ensure!(
        parsed.iv.len() == 12 && (1..=2).contains(&parsed.pair.len()),
        "Invalid backup forward-secrecy metadata"
    );
    ensure!(
        parsed
            .pair
            .iter()
            .all(|pair| pair.pw_salt.len() == 32 && pair.ct.len() == 48),
        "Invalid backup forward-secrecy metadata"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_session(origin: &str) -> CloudSession {
        let aci = Uuid::nil();
        let account = CloudAccount {
            aci,
            servers: SignalServers::Production,
            credentials: ServiceCredentials {
                aci: Some(aci),
                pni: None,
                phonenumber: "+12025550123".parse().unwrap(),
                password: Some("fake-test-password".into()),
                device_id: None,
            },
        };
        let mut session = CloudSession::new(account, &"a".repeat(64)).unwrap();
        session.test_origin = Some(origin.parse().unwrap());
        session
    }
    fn mock_http(replies: Vec<String>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for reply in replies {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                requests.push(String::from_utf8(request).unwrap().to_ascii_lowercase());
                socket.write_all(reply.as_bytes()).unwrap();
            }
            requests
        });
        (origin, task)
    }
    #[tokio::test]
    async fn interrupted_download_resumes_then_reuses_completed_ciphertext() {
        let (origin, task) = mock_http(vec![
            "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nETag: \"one\"\r\nConnection: close\r\n\r\nabc".into(),
            "HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 3-5/6\r\nETag: \"one\"\r\nConnection: close\r\n\r\ndef".into(),
            "HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n".into(),
        ]);
        let session = test_session(&origin);
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("media");
        assert!(
            session
                .download_transit_blob(3, "fake_key", &target)
                .await
                .is_err()
        );
        session
            .download_transit_blob(3, "fake_key", &target)
            .await
            .unwrap();
        session
            .download_transit_blob(3, "fake_key", &target)
            .await
            .unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"abcdef");
        let requests = task.join().unwrap();
        assert!(requests[1].contains("range: bytes=3-"));
        assert!(requests[1].contains("if-range: \"one\""));
        assert!(requests[2].contains("if-none-match: \"one\""));
        assert!(
            requests
                .iter()
                .all(|request| !request.contains("authorization:"))
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(target).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    #[tokio::test]
    async fn rejects_redirects_and_never_attaches_account_auth_to_cdn() {
        let (origin, task) = mock_http(vec!["HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/leak\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into()]);
        let session = test_session(&origin);
        let dir = tempfile::tempdir().unwrap();
        let error = session
            .download_transit_blob(3, "fake_key", &dir.path().join("media"))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("HTTP 302"));
        assert!(!error.contains("127.0.0.1"));
        assert!(!task.join().unwrap()[0].contains("authorization:"));
    }
    #[tokio::test]
    async fn control_endpoint_allowlist_fails_before_network() {
        let session = test_session("http://127.0.0.1:1");
        for path in [
            "/v1/archives/backupid",
            "/v1/archives/upload/form",
            "/v1/archives/keys",
            "/v1/accounts",
        ] {
            assert!(
                session
                    .get::<serde_json::Value>(path, HeaderMap::new(), false)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("Disallowed")
            );
        }
        assert!(
            session
                .get::<serde_json::Value>("/v1/archives/auth/read?cdn=3", HeaderMap::new(), true)
                .await
                .unwrap_err()
                .to_string()
                .contains("forbidden")
        );
    }
    #[tokio::test]
    async fn location_cache_is_scoped_to_key_type_cdn_and_expires() {
        let session = test_session("http://127.0.0.1:1");
        let key = BackupKey([17; 32]);
        let cache_key = (
            Sha256::digest(key.0).into(),
            BackupCredentialType::Media as u8,
            Some(3),
        );
        let info = Info {
            cdn: 3,
            backup_dir: "fixture".into(),
            media_dir: "media".into(),
            backup_name: None,
        };
        session.locations.lock().unwrap().insert(
            cache_key,
            (std::time::Instant::now(), info.clone(), HeaderMap::new()),
        );
        assert_eq!(
            session
                .location(&key, BackupCredentialType::Media, Some(3))
                .await
                .unwrap()
                .0
                .backup_dir,
            "fixture"
        );
        assert!(
            session
                .location(&key, BackupCredentialType::Messages, Some(3))
                .await
                .is_err()
        );
        assert!(
            session
                .location(&key, BackupCredentialType::Media, Some(2))
                .await
                .is_err()
        );
        session.locations.lock().unwrap().insert(
            cache_key,
            (
                std::time::Instant::now() - std::time::Duration::from_secs(301),
                info,
                HeaderMap::new(),
            ),
        );
        assert!(
            session
                .location(&key, BackupCredentialType::Media, Some(3))
                .await
                .is_err()
        );
    }
    #[test]
    fn selects_credentials_from_server_lowercase_json_keys() {
        // Signal-Server ArchiveController.CredentialType.toValue uses lowercase.
        let reply: Credentials = serde_json::from_str(r#"{"credentials":{"messages":[{"credential":"previous","redemptionTime":86400},{"credential":"today-message","redemptionTime":172800}],"media":[{"credential":"today-media","redemptionTime":172800}]}}"#).unwrap();
        assert_eq!(
            reply
                .current(BackupCredentialType::Messages, 172800)
                .unwrap()
                .credential,
            "today-message"
        );
        assert_eq!(
            reply
                .current(BackupCredentialType::Media, 172800)
                .unwrap()
                .credential,
            "today-media"
        );
        assert!(
            reply
                .current(BackupCredentialType::Messages, 259200)
                .is_err()
        );
    }
    #[test]
    fn rejects_paths_that_could_escape_the_cdn_prefix() {
        for path in ["../private", "/absolute", "a?query", "a%2fb", "a\\b", ""] {
            assert!(segment(path).is_err());
        }
        assert!(segment("backup_A-09").is_ok());
    }
    #[test]
    fn official_verification_parameters_deserialize() {
        for encoded in [PROD_PARAMS, STAGING_PARAMS] {
            let _: GenericServerPublicParams =
                GenericServerPublicParams::try_from(STANDARD.decode(encoded).unwrap().as_slice())
                    .unwrap();
        }
    }
    #[test]
    fn metadata_is_checked_before_any_svrb_attempt() {
        use libsignal_svrb::proto::backup_metadata::{MetadataPb, metadata_pb::Pair};
        use protobuf::Message;
        let mut metadata = MetadataPb::new();
        metadata.iv = vec![0; 12];
        let mut pair = Pair::new();
        pair.ct = vec![0; 48];
        pair.pw_salt = vec![0; 32];
        metadata.pair.push(pair);
        assert!(validate_metadata(&metadata.write_to_bytes().unwrap()).is_ok());
        metadata.pair[0].ct.pop();
        assert!(validate_metadata(&metadata.write_to_bytes().unwrap()).is_err());
        assert!(validate_metadata(&[0xff]).is_err());
    }
    #[test]
    fn reads_legacy_and_modern_fixture_headers_without_network() {
        use libsignal_svrb::proto::backup_metadata::{MetadataPb, metadata_pb::Pair};
        use protobuf::Message;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive");
        fs::write(&path, b"legacy-encrypted-data").unwrap();
        assert!(read_metadata(&path).unwrap().is_none());
        let mut metadata = MetadataPb::new();
        metadata.iv = vec![1; 12];
        let mut pair = Pair::new();
        pair.ct = vec![2; 48];
        pair.pw_salt = vec![3; 32];
        metadata.pair.push(pair);
        let bytes = metadata.write_to_bytes().unwrap();
        assert!(bytes.len() < 128);
        let mut archive = b"SBACKUP\x01".to_vec();
        archive.push(bytes.len() as u8);
        archive.extend_from_slice(&bytes);
        fs::write(&path, archive).unwrap();
        assert_eq!(read_metadata(&path).unwrap(), Some(bytes));
        fs::write(&path, b"SBACKUP\x01\xff\xff\xff\xff\xff").unwrap();
        assert!(read_metadata(&path).is_err());
    }
}
