//! Provider that takes messages in over NATS and performs invocations of Actors (configured via links)

use wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk;

use wasmcloud_provider_messaging_nats_invoker::MessagingNatsInvoker;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    wasmcloud_provider_sdk::start_provider(
        MessagingNatsInvoker::from_host_data(wasmcloud_provider_sdk::load_host_data()?),
        Some("messaging-nats-invoker".to_string()),
    )?;

    eprintln!("Messaging NATS Invoker provider exiting");
    Ok(())
}
