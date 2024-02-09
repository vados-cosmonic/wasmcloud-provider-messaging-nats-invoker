use std::sync::Arc;

use base64::Engine;
use futures::StreamExt;
use tokio::{sync::RwLock, task::JoinHandle};
use tracing::{error, info, instrument};

use wasmcloud_core::WasmCloudEntity;
use wasmcloud_provider_wit_bindgen::deps::{
    async_trait::async_trait,
    serde::{Deserialize, Serialize},
    serde_json,
    wasmcloud_provider_sdk::Context,
    wasmcloud_provider_sdk::{
        core::{HostData, LinkDefinition},
        provider_main,
    },
};

wasmcloud_provider_wit_bindgen::generate!({
    impl_struct: MessagingNatsInvoker,
    contract: "wasmcloud:messaging",
    wit_bindgen_cfg: "provider-messaging-nats"
});

/// Provider that only *listens* to incoming invocations
#[derive(Default, Clone)]
pub struct MessagingNatsInvoker {
    /// The provider's own host-provided ID
    id: String,
    sub_handles: Arc<RwLock<Vec<JoinHandle<()>>>>,
}

async fn handle_invoke_msg(provider_id: String, body: impl AsRef<[u8]>) {
    let InvokeMessage {
        actor_id,
        link_name,
        operation,
        payload_b64,
        contract_id,
    } = match serde_json::from_slice(body.as_ref()) {
        Ok(v) => v,
        Err(e) => {
            info!(error = ?e, "failed to deserialize body of message");
            return;
        }
    };

    // We need to turn the base64 encoded payload to bytes to send down the wire
    let Ok(payload_bytes) = base64::engine::general_purpose::STANDARD.decode(&payload_b64) else {
        error!("failed to decode input from lattice: [{payload_b64}]");
        return;
    };

    let rpc_client = provider_main::get_connection().get_rpc_client();
    let origin = WasmCloudEntity {
        public_key: provider_id,
        link_name: link_name.clone(),
        contract_id: contract_id.clone(),
    };
    let target = WasmCloudEntity {
        public_key: actor_id.clone(),
        link_name: String::new(),
        contract_id: String::new(),
    };
    match rpc_client
        .send(origin, target, operation, payload_bytes)
        .await
    {
        Ok(resp) => {
            info!(resp = ?resp, "received invocation response");
        }
        Err(e) => {
            error!(error = ?e, "invocation failed");
        }
    }
}

impl MessagingNatsInvoker {
    /// Build a [`MessagingNatsInvoker`] from [`HostData`]
    pub fn from_host_data(host_data: &HostData) -> MessagingNatsInvoker {
        Self {
            id: host_data.provider_key.clone(),
            sub_handles: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Add a regular or queue subscription
    async fn subscribe(
        &self,
        client: &async_nats::Client,
        ld: &LinkDefinition,
        sub: String,
        queue: Option<String>,
    ) -> anyhow::Result<JoinHandle<()>> {
        let mut subscriber = match queue {
            Some(queue) => client.queue_subscribe(sub.clone(), queue).await,
            None => client.subscribe(sub.clone()).await,
        }?;

        let link_def = ld.to_owned();
        info!(?link_def, "spawning listener for link def");

        // Spawn a thread that listens for messages coming from NATS
        // this thread is expected to run the full duration that the provider is available
        let join_handle = tokio::spawn(async move {
            // Listen for NATS message(s)
            while let Some(msg) = subscriber.next().await {
                info!(?msg, actor_id = ?link_def.actor_id, "received messsage");
                let provider_id = link_def.provider_id.clone();
                tokio::spawn(handle_invoke_msg(provider_id, msg.payload));
            }
        });

        Ok(join_handle)
    }
}

/// Handle provider control commands
/// put_link (new actor link command), del_link (remove link command), and shutdown
#[async_trait]
impl WasmcloudCapabilityProvider for MessagingNatsInvoker {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    #[instrument(level = "debug", skip(self, ld), fields(actor_id = %ld.actor_id))]
    async fn put_link(&self, ld: &LinkDefinition) -> bool {
        // When any link is put, start listening on a known channel
        let rpc_client = provider_main::get_connection().get_rpc_client();

        // Only make *one* listening handle since we're hacking this to listen on one subject 'testing'
        if !self.sub_handles.read().await.is_empty() {
            return true;
        }

        let handle = match self
            .subscribe(&rpc_client.client(), ld, "testing".into(), None)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                error!("failed to subscribe: {e}");
                return false;
            }
        };

        // Add handles to list
        self.sub_handles.write().await.push(handle);

        // TODO: when a link is put, we set up a NATS connection ..?
        true
    }

    /// Handle notification that a link is dropped: close the connection
    #[instrument(level = "info", skip(self))]
    async fn delete_link(&self, actor_id: &str) {}

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) {
        self.sub_handles.write().await.clear();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct InvokeMessage {
    actor_id: String,
    link_name: String,
    contract_id: String,
    operation: String,
    payload_b64: String,
}

#[async_trait]
impl WasmcloudMessagingMessageSubscriber for MessagingNatsInvoker {
    async fn handle_message(&self, _ctx: Context, msg: Message) {
        handle_invoke_msg(self.id.clone(), &msg.body).await;
    }
}
