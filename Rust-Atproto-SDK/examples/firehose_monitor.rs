//! Firehose Monitor Example
//!
//! This example demonstrates how to use WebSocket subscriptions to monitor
//! the ATProto firehose - a real-time stream of all repository events.
//!
//! Run with:
//! ```bash
//! cargo run --example firehose_monitor
//! ```

use atproto::client::com::atproto::sync::subscribe_repos;
use atproto::xrpc_subscription::{SubscriptionClient, SubscriptionEvent};
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔥 ATProto Firehose Monitor\n");
    println!("Connecting to Bluesky firehose...\n");

    // Create subscription client
    // Using bsky.network which provides the public firehose
    let client = SubscriptionClient::new("wss://bsky.network".to_string());

    // Subscribe to the firehose
    let params = subscribe_repos::QueryParams {
        cursor: None, // Start from the latest event
    };

    let mut stream = subscribe_repos::subscribe_repos(&client, params).await?;

    println!("✓ Connected to firehose!");
    println!("📡 Monitoring events... (Ctrl+C to stop)\n");

    let mut event_count = 0;
    let mut message_count = 0;
    let mut error_count = 0;

    // Process events from the stream
    while let Some(result) = stream.next().await {
        match result {
            Ok(SubscriptionEvent::Message { message_type, body }) => {
                message_count += 1;
                event_count += 1;

                // Parse different message types
                match message_type.as_str() {
                    "#commit" => {
                        // Repository commit event
                        // In a real application, you would parse the CAR file in body
                        if message_count <= 10 {
                            println!("📦 Commit event ({} bytes)", body.len());
                        }
                    }
                    "#identity" => {
                        if message_count <= 10 {
                            println!("👤 Identity update event");
                        }
                    }
                    "#account" => {
                        if message_count <= 10 {
                            println!("🔐 Account event");
                        }
                    }
                    "#handle" => {
                        if message_count <= 10 {
                            println!("📛 Handle update event");
                        }
                    }
                    "#tombstone" => {
                        if message_count <= 10 {
                            println!("⚰️  Tombstone event (deleted repo)");
                        }
                    }
                    _ => {
                        if message_count <= 10 {
                            println!("❓ Unknown event type: {}", message_type);
                        }
                    }
                }

                // Print progress every 100 events
                if event_count % 100 == 0 {
                    println!("\n📊 Stats: {} messages, {} errors", message_count, error_count);
                }
            }
            Ok(SubscriptionEvent::Error { error, message }) => {
                error_count += 1;
                eprintln!("❌ Firehose error: {}", error);
                if let Some(msg) = message {
                    eprintln!("   Message: {}", msg);
                }
            }
            Ok(SubscriptionEvent::Closed) => {
                println!("\n🔌 Connection closed by server");
                break;
            }
            Err(e) => {
                error_count += 1;
                eprintln!("❌ Stream error: {}", e);

                // In a real application, you might want to reconnect here
                // The SubscriptionClient has built-in reconnection logic,
                // but you can also handle it manually
            }
        }
    }

    println!("\n✨ Final stats:");
    println!("   Messages: {}", message_count);
    println!("   Errors: {}", error_count);
    println!("   Total events: {}", event_count);

    Ok(())
}
