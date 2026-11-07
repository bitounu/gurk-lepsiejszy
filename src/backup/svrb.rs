//! Minimal public-SDK adapter. Recovery consumes SVRB attempts; never store or reset.
use async_trait::async_trait;
use libsignal_account_keys::{BackupForwardSecrecyToken, BackupKey};
use libsignal_net::{
    auth::Auth,
    connect_state::{ConnectState, ConnectionResources, SUGGESTED_CONNECT_CONFIG},
    enclave::{EnclaveEndpoint, PpssSetup, SvrSgx},
    env::SvrBEnv,
    svr::SvrConnection,
    svrb::{self, BackupFileMetadataRef, traits::SvrBConnect},
};
use libsignal_net_infra::{
    EnableDomainFronting, dns::DnsResolver, route::DirectOrProxyProvider,
    utils::no_network_change_events,
};

struct Client<'a> {
    auth: &'a Auth,
    endpoint: &'a EnclaveEndpoint<'static, SvrSgx>,
}
#[async_trait]
impl SvrBConnect for Client<'_> {
    type Env = SvrBEnv<'static>;
    async fn connect(&self) -> <Self::Env as PpssSetup>::ConnectionResults {
        let event = no_network_change_events();
        let state = ConnectState::new(SUGGESTED_CONNECT_CONFIG);
        let resolver = DnsResolver::new(&event);
        let endpoint = self.endpoint;
        let confirmation_header_name = endpoint
            .domain_config
            .connect
            .confirmation_header_name
            .map(http::HeaderName::from_static);
        SvrConnection::connect(
            ConnectionResources {
                connect_state: &state,
                dns_resolver: &resolver,
                network_change_event: &event,
                confirmation_header_name,
            },
            endpoint.domain_config.connect.service,
            DirectOrProxyProvider::direct(
                endpoint.enclave_websocket_provider(EnableDomainFronting::No),
            ),
            endpoint.ws_config,
            &endpoint.params,
            self.auth,
        )
        .await
    }
}

pub(super) async fn recover(
    key: &BackupKey,
    metadata: &[u8],
    username: String,
    password: String,
    production: bool,
) -> anyhow::Result<BackupForwardSecrecyToken> {
    let auth = Auth { username, password };
    let env = if production {
        &libsignal_net::env::PROD.svr_b
    } else {
        &libsignal_net::env::STAGING.svr_b
    };
    let clients = env
        .current_and_previous()
        .map(|endpoint| Client {
            auth: &auth,
            endpoint,
        })
        .collect::<Vec<_>>();
    // The SDK bounds metadata pairs and handles enclave migration. Do not retry this
    // operation at the application layer, nor persist next_backup_data.
    let response = svrb::restore_backup(&clients, key, BackupFileMetadataRef(metadata))
        .await
        .map_err(|_| anyhow::anyhow!("SVRB recovery failed; no automatic retry was attempted"))?;
    Ok(response.forward_secrecy_token)
}
