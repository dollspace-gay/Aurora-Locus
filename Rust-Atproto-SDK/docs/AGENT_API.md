# Agent API Documentation

The `Agent` is the high-level entry point for interacting with ATProto services. It provides convenient methods for common Bluesky operations.

## Quick Start

```rust
use atproto::agent::Agent;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create an agent
    let agent = Agent::new("https://bsky.social".to_string());

    // Login
    agent.login("alice.bsky.social", "app-password").await?;

    // Create a post
    let uri = agent.post("Hello from Rust! 🦀").await?;
    println!("Posted: {}", uri);

    Ok(())
}
```

## Authentication

### Login

```rust
agent.login("alice.bsky.social", "app-password").await?;
```

Logs in with a handle (or DID) and password. Creates a new session.

### Resume Session

```rust
agent.resume_session(
    access_token,
    refresh_token,
    did,
    handle
).await?;
```

Resumes an existing session with saved tokens.

### Logout

```rust
agent.logout().await?;
```

Ends the current session and clears authentication.

### Check Authentication

```rust
if agent.is_authenticated() {
    println!("Logged in as: {}", agent.did().unwrap());
}
```

## Social Operations

### Create a Post

```rust
let uri = agent.post("Hello world!").await?;
```

Creates a simple text post.

### Follow a User

```rust
let uri = agent.follow("did:plc:z72i7hdynmk6r22z27h6tvur").await?;
```

Follows a user by their DID.

### Like a Post

```rust
let uri = agent.like("at://did:plc:abc123/app.bsky.feed.post/xyz", "cid123").await?;
```

Likes a post. Requires the post's AT-URI and CID.

### Repost

```rust
let uri = agent.repost("at://did:plc:abc123/app.bsky.feed.post/xyz", "cid123").await?;
```

Reposts a post.

### Delete a Record

```rust
agent.delete_record("at://did:plc:abc123/app.bsky.feed.post/xyz").await?;
```

Deletes any record by its AT-URI (works for posts, likes, follows, etc.).

## Data Retrieval

### Get Timeline

```rust
let timeline = agent.get_timeline(Some(50)).await?;
```

Gets the authenticated user's timeline. Returns up to 50 posts (or specify a different limit).

### Get Profile

```rust
let profile = agent.get_profile("bsky.app").await?;
```

Gets a user's profile by handle or DID.

## Configuration

### Labelers

```rust
// Configure instance-specific labelers
agent.configure_labelers(vec!["did:plc:labeler123".to_string()]);

// Configure app-wide labelers
agent.configure_app_labelers(vec!["did:plc:labeler456".to_string()]);
```

Configure content moderation labelers.

### Proxy

```rust
use atproto::types::AtprotoServiceType;

agent.configure_proxy_from_parts(
    AtprotoServiceType::new_unchecked("atproto_labeler"),
    "did:plc:proxy123".to_string()
);
```

Configure a proxy service.

### Custom Headers

```rust
agent.set_header("X-Custom-Header".to_string(), "value".to_string());
agent.clear_header("X-Custom-Header");
agent.clear_all_headers();
```

Add custom HTTP headers to requests.

## Advanced Usage

### Low-Level XRPC Access

For operations not covered by convenience methods, use the generated client code directly:

```rust
use atproto::client::com::atproto::repo::create_record;

let input = create_record::Input {
    repo: agent.assert_did()?,
    collection: "app.bsky.feed.post".to_string(),
    record: serde_json::json!({
        "text": "Custom post",
        "createdAt": chrono::Utc::now().to_rfc3339(),
        "$type": "app.bsky.feed.post"
    }),
    rkey: None,
    validate: None,
    swap_commit: None,
};

let response = create_record::create_record(&*agent.client, input).await?;
```

### Clone Agent

```rust
let agent2 = agent.clone();
```

Creates a new agent with the same configuration but independent state.

### Agent with Proxy

```rust
let proxied_agent = agent.with_proxy(
    AtprotoServiceType::new_unchecked("atproto_labeler"),
    "did:plc:proxy123".to_string()
);
```

Creates a new agent with a proxy configured.

## Error Handling

The Agent API uses the `AgentError` enum for errors:

```rust
match agent.login("alice.bsky.social", "wrong-password").await {
    Ok(_) => println!("Success!"),
    Err(AgentError::NotAuthenticated) => println!("Not authenticated"),
    Err(AgentError::InvalidDid(did)) => println!("Invalid DID: {}", did),
    Err(AgentError::XrpcError(e)) => println!("XRPC error: {}", e),
    Err(AgentError::SessionError(msg)) => println!("Session error: {}", msg),
}
```

## Examples

See the `examples/` directory for complete working examples:

- `simple_bot.rs` - Basic bot that posts and reads timeline
- More examples coming soon!

## Generated Client Code

All 290 generated API endpoints are available under `atproto::client::`:

```rust
use atproto::client::app::bsky::feed::get_posts;
use atproto::client::com::atproto::repo::list_records;
use atproto::client::app::bsky::graph::get_followers;
// ... and 287 more!
```

See the `src/client/` directory for all available endpoints.

## Current Limitations

- Session management is TODO (sessions are not persisted/refreshed yet)
- Blob uploads (images, videos) not yet implemented
- WebSocket subscriptions not yet implemented
- Rich text facets (mentions, links) not yet implemented

These features are planned for future releases!
