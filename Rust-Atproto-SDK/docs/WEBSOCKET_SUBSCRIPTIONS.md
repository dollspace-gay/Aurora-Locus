# WebSocket Subscriptions

The ATProto Rust SDK now supports WebSocket subscriptions for real-time event streams!

## Overview

WebSocket subscriptions enable real-time monitoring of ATProto events:

- **Repository Firehose** (`com.atproto.sync.subscribeRepos`) - All repository events across the network
- **Label Stream** (`com.atproto.label.subscribeLabels`) - Real-time moderation label updates

These are essential for building:
- Relays (aggregate data from multiple PDSes)
- Feed generators (index posts in real-time)
- Moderation services (monitor content)
- Analytics tools (track network activity)

## Quick Start

```rust
use atproto::client::com::atproto::sync::subscribe_repos;
use atproto::xrpc_subscription::{SubscriptionClient, SubscriptionEvent};
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create subscription client
    let client = SubscriptionClient::new("wss://bsky.network".to_string());

    // Subscribe to firehose
    let params = subscribe_repos::QueryParams { cursor: None };
    let mut stream = subscribe_repos::subscribe_repos(&client, params).await?;

    // Process events
    while let Some(result) = stream.next().await {
        match result {
            Ok(SubscriptionEvent::Message { message_type, body }) => {
                println!("Event: {} ({} bytes)", message_type, body.len());
            }
            Ok(SubscriptionEvent::Error { error, message }) => {
                eprintln!("Error: {}", error);
            }
            Ok(SubscriptionEvent::Closed) => break,
            Err(e) => eprintln!("Stream error: {}", e),
        }
    }

    Ok(())
}
```

## Subscription Client

### Creating a Client

```rust
use atproto::xrpc_subscription::SubscriptionClient;

// Create client for Bluesky network
let client = SubscriptionClient::new("wss://bsky.network".to_string());

// Works with http:// or https:// - automatically converted to ws:// or wss://
let client = SubscriptionClient::new("https://bsky.network".to_string());
```

### Configuring Reconnection

The subscription client includes automatic reconnection with exponential backoff:

```rust
use atproto::xrpc_subscription::{SubscriptionClient, ReconnectConfig};
use std::time::Duration;

let config = ReconnectConfig {
    max_attempts: 10,  // 0 = infinite retries
    initial_delay: Duration::from_secs(1),
    max_delay: Duration::from_secs(60),
    backoff_multiplier: 2.0,
};

let client = SubscriptionClient::new("wss://bsky.network".to_string())
    .with_reconnect_config(config);
```

## Repository Firehose

The firehose streams all repository events on the network:

```rust
use atproto::client::com::atproto::sync::subscribe_repos;

let params = subscribe_repos::QueryParams {
    cursor: None,  // Start from latest, or provide cursor to resume
};

let mut stream = subscribe_repos::subscribe_repos(&client, params).await?;
```

### Event Types

The firehose emits several event types:

- **`#commit`** - Repository commit (new posts, likes, follows, etc.)
- **`#identity`** - Identity update
- **`#account`** - Account status change
- **`#handle`** - Handle update
- **`#tombstone`** - Deleted repository

### Processing Commit Events

Commit events contain CAR (Content Addressable aRchive) files with CBOR-encoded data:

```rust
match message_type.as_str() {
    "#commit" => {
        // `body` contains CAR file with repository blocks
        // In a real application, parse the CAR file:
        // 1. Extract blocks from CAR format
        // 2. Decode CBOR data
        // 3. Parse repository commits
        //
        // Libraries: libipld, serde_cbor
        println!("Commit event: {} bytes", body.len());
    }
    "#identity" => {
        // Parse as JSON identity update
        let event: serde_json::Value = serde_json::from_slice(&body)?;
        println!("Identity: {:?}", event);
    }
    _ => {}
}
```

## Label Subscription

Monitor real-time moderation label updates:

```rust
use atproto::client::com::atproto::label::subscribe_labels;

let params = subscribe_labels::QueryParams {
    cursor: None,
};

let mut stream = subscribe_labels::subscribe_labels(&client, params).await?;

while let Some(result) = stream.next().await {
    match result {
        Ok(SubscriptionEvent::Message { message_type, body }) => {
            match message_type.as_str() {
                "#labels" => {
                    // Parse labels
                    let labels: serde_json::Value = serde_json::from_slice(&body)?;
                    println!("Labels: {:?}", labels);
                }
                "#info" => {
                    // Info message
                    let info: serde_json::Value = serde_json::from_slice(&body)?;
                    println!("Info: {:?}", info);
                }
                _ => {}
            }
        }
        _ => {}
    }
}
```

## Event Stream

All subscriptions return a `Stream` of `SubscriptionEvent`:

```rust
pub enum SubscriptionEvent {
    /// Message with type and binary body
    Message {
        message_type: String,
        body: Vec<u8>
    },

    /// Error from server
    Error {
        error: String,
        message: Option<String>
    },

    /// Connection closed
    Closed,
}
```

### Stream Processing

Use Rust's `Stream` trait for powerful event processing:

```rust
use futures::StreamExt;

// Take first 100 events
let mut stream = subscribe_repos::subscribe_repos(&client, params)
    .await?
    .take(100);

// Filter only commit events
let mut stream = subscribe_repos::subscribe_repos(&client, params)
    .await?
    .filter_map(|result| async move {
        match result {
            Ok(SubscriptionEvent::Message { message_type, body })
                if message_type == "#commit" => Some(body),
            _ => None
        }
    });

// Process in chunks
let mut stream = subscribe_repos::subscribe_repos(&client, params)
    .await?
    .chunks(10);  // Process 10 events at a time
```

## Cursor Management

Subscriptions support cursors for resuming from a specific point:

```rust
// Save cursor periodically
let mut last_cursor: Option<i64> = None;

while let Some(result) = stream.next().await {
    if let Ok(SubscriptionEvent::Message { body, .. }) = result {
        // Extract sequence number from event
        // Store in database or file
        last_cursor = Some(extract_seq(&body));
    }
}

// Resume from saved cursor
let params = subscribe_repos::QueryParams {
    cursor: last_cursor,
};
```

## Error Handling

Handle errors gracefully:

```rust
while let Some(result) = stream.next().await {
    match result {
        Ok(SubscriptionEvent::Message { .. }) => {
            // Process message
        }
        Ok(SubscriptionEvent::Error { error, message }) => {
            eprintln!("Server error: {}", error);
            if let Some(msg) = message {
                eprintln!("Details: {}", msg);
            }
            // Decide whether to continue or break
        }
        Ok(SubscriptionEvent::Closed) => {
            println!("Connection closed");
            break;
        }
        Err(e) => {
            eprintln!("Stream error: {}", e);
            // SubscriptionClient will automatically reconnect
            // Or handle manually
        }
    }
}
```

## Use Cases

### Building a Relay

```rust
// Monitor multiple PDSes and aggregate events
let client1 = SubscriptionClient::new("wss://pds1.example.com".to_string());
let client2 = SubscriptionClient::new("wss://pds2.example.com".to_string());

let stream1 = subscribe_repos::subscribe_repos(&client1, params.clone()).await?;
let stream2 = subscribe_repos::subscribe_repos(&client2, params.clone()).await?;

// Merge streams and process
use futures::stream::select;
let mut combined = select(stream1, stream2);

while let Some(result) = combined.next().await {
    // Aggregate and re-emit events
}
```

### Building a Feed Generator

```rust
// Index posts in real-time
while let Some(result) = stream.next().await {
    if let Ok(SubscriptionEvent::Message { message_type, body }) = result {
        if message_type == "#commit" {
            // Parse CAR file
            // Extract posts
            // Index in database
            // Update feed algorithm
        }
    }
}
```

### Building a Moderation Service

```rust
// Monitor labels in real-time
let mut stream = subscribe_labels::subscribe_labels(&client, params).await?;

while let Some(result) = stream.next().await {
    if let Ok(SubscriptionEvent::Message { body, .. }) = result {
        let labels: LabelsEvent = serde_json::from_slice(&body)?;
        // Apply labels to content
        // Update moderation database
    }
}
```

## Examples

See the `examples/` directory:
- `firehose_monitor.rs` - Monitor and display firehose events

## Performance Tips

1. **Use channels** - Process events in a separate task
   ```rust
   let (tx, mut rx) = tokio::sync::mpsc::channel(100);
   tokio::spawn(async move {
       while let Some(result) = stream.next().await {
           tx.send(result).await.ok();
       }
   });
   ```

2. **Batch processing** - Use `chunks()` to process events in batches
   ```rust
   stream.chunks(100).for_each(|batch| async move {
       // Process 100 events at once
   }).await;
   ```

3. **Cursor persistence** - Save cursors regularly to resume efficiently
   ```rust
   let mut cursor_save_interval = tokio::time::interval(Duration::from_secs(10));
   // Save cursor every 10 seconds
   ```

## Current Limitations

- CAR file parsing not yet implemented (use `libipld` crate)
- CBOR decoding not yet implemented (use `serde_cbor` crate)
- Message union types not strongly typed yet

These will be added in future releases!

## Next Steps

- See [`AGENT_API.md`](AGENT_API.md) for HTTP API usage
- See [`examples/firehose_monitor.rs`](examples/firehose_monitor.rs) for complete working example
- Check out [ATProto specifications](https://atproto.com/specs/event-stream) for event stream details
