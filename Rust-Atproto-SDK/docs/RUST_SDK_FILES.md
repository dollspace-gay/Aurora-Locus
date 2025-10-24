# Rust SDK Files - Rust ATProto SDK

This document lists all non-code-generated files in the Rust SDK implementation.

> **Note:** The `src/client/` directory contains auto-generated type definitions and API methods from lexicon specifications and is excluded from this listing as it's code-generated content.

## Root Directory

### Configuration Files
- `Cargo.toml` - Main package configuration and dependencies
- `.gitignore` - Git ignore rules

### Documentation
- `README.md` - Main project documentation
- `CLAUDE.md` - Development guidelines for Claude (coding standards)
- `TRANSLATION_PLAN.md` - Translation strategy from TypeScript
- `TYPESCRIPT_COMPARISON.md` - Comparison with TypeScript SDK
- `TYPESCRIPT_SDK_FILES.md` - TypeScript SDK file listing
- `AGENT_API.md` - Agent API documentation
- `AGENT_INTEGRATION_COMPLETE.md` - Agent integration notes
- `BLOB_UPLOAD_IMPLEMENTATION.md` - Blob upload implementation details
- `EXTERNAL_EMBEDS.md` - External embeds documentation
- `MODERATION_SYSTEM_COMPLETE.md` - Moderation system documentation
- `NEXT_STEPS.md` - Development roadmap
- `OAUTH_IMPLEMENTATION.md` - OAuth implementation notes
- `REPLY_IMPLEMENTATION_SUMMARY.md` - Reply feature summary
- `REPLY_SUPPORT.md` - Reply support documentation
- `SESSION_PERSISTENCE.md` - Session persistence documentation
- `WEBSOCKET_SUBSCRIPTIONS.md` - WebSocket subscription documentation

### Utility Scripts
- `find_missing_types.py` - Python script to find missing types
- `generate_namespace_impls.py` - Python script to generate namespace implementations
- `namespace_impls.rs` - Generated namespace implementations (standalone)

## Core Source Files (`src/`)

### Main Library Files
- `lib.rs` - Library entry point
- `agent.rs` - Base agent implementation
- `session_manager.rs` - Session management
- `types.rs` - Core type definitions
- `consts.rs` - Constants and configuration
- `util.rs` - Utility functions
- `preferences.rs` - User preferences handling
- `validation.rs` - Input validation
- `syntax.rs` - ATProto syntax validation

### Identity & Handle Management
- `did_doc.rs` - DID document handling
- `handle.rs` - Handle resolution and validation
- `tid.rs` - Timestamp identifier (TID) implementation

### Repository & Data Management
- `repo.rs` - Repository operations
- `mst.rs` - Merkle Search Tree implementation
- `car.rs` - CAR (Content Addressable aRchive) file handling
- `blob.rs` - Blob upload and management

### Network & Authentication
- `server_auth.rs` - Server authentication
- `xrpc/mod.rs` - XRPC client implementation
- `xrpc_subscription.rs` - XRPC subscription support

### Namespace Support
- `namespaces.rs` - Namespace type definitions and helpers

### OAuth Module (`src/oauth/`)
- `oauth/mod.rs` - OAuth module entry point
- `oauth/client.rs` - OAuth client implementation
- `oauth/callback.rs` - OAuth callback handling
- `oauth/dpop.rs` - DPoP (Demonstrating Proof of Possession) implementation
- `oauth/pkce.rs` - PKCE (Proof Key for Code Exchange) implementation
- `oauth/state.rs` - OAuth state management
- `oauth/types.rs` - OAuth type definitions

### Moderation Module (`src/moderation/`)
- `moderation/mod.rs` - Moderation module entry point
- `moderation/decision.rs` - Moderation decision logic
- `moderation/labels.rs` - Label handling and constants
- `moderation/types.rs` - Moderation type definitions
- `moderation/ui.rs` - UI-related moderation utilities

### Rich Text Module (`src/rich_text/`)
- `rich_text/mod.rs` - Rich text module entry point
- `rich_text/rich_text.rs` - Main rich text implementation
- `rich_text/detection.rs` - Link and mention detection
- `rich_text/sanitization.rs` - Text sanitization
- `rich_text/unicode.rs` - Unicode handling utilities
- `rich_text/util.rs` - Rich text utility functions

### Generated Client Code (`src/client/`)
**Note:** This entire directory is code-generated from lexicon specifications and is excluded from detailed listing. It contains:
- `client/mod.rs` - Client module entry point
- `client/app/` - App-level API implementations (Bluesky)
- `client/chat/` - Chat API implementations
- `client/com/` - Core ATProto API implementations
- `client/tools/` - Ozone moderation tools API

## Test Files (`tests/`)

- `agent_methods_test.rs` - Agent method tests
- `integration_tests.rs` - Integration tests
- `moderation_test.rs` - Moderation system tests
- `notifications_test.rs` - Notification tests
- `preferences_methods_test.rs` - Preferences tests
- `real_world_auth_tests.rs` - Real-world authentication tests
- `simple_integration.rs` - Simple integration tests
- `simple_real_world_test.rs` - Simple real-world tests

## Example Files (`examples/`)

- `agent_demo.rs` - Basic agent usage demo
- `firehose_monitor.rs` - Firehose (repo subscription) monitoring example
- `link_embed_demo.rs` - Link embedding example
- `moderation_demo.rs` - Moderation system demo
- `oauth_demo.rs` - OAuth authentication demo
- `oauth_flow.rs` - Complete OAuth flow example
- `persistent_session.rs` - Session persistence example
- `reply_demo.rs` - Reply functionality demo
- `rich_text_demo.rs` - Rich text formatting example
- `simple_bot.rs` - Simple bot implementation
- `upload_image.rs` - Image upload example

## Code Generation Tool (`codegen/`)

### Configuration
- `codegen/Cargo.toml` - Codegen tool package configuration
- `codegen/README.md` - Codegen documentation

### Source Files
- `codegen/src/main.rs` - Codegen tool entry point
- `codegen/src/codegen.rs` - Code generation logic
- `codegen/src/lexicon.rs` - Lexicon parsing and processing

### Lexicon Definitions
**Note:** The `codegen/lexicons/` directory contains JSON lexicon schema files organized by namespace:
- `codegen/lexicons/app/bsky/` - Bluesky app schemas (actor, feed, graph, etc.)
- `codegen/lexicons/chat/bsky/` - Bluesky chat schemas
- `codegen/lexicons/com/atproto/` - Core ATProto schemas (server, repo, sync, etc.)
- `codegen/lexicons/tools/ozone/` - Ozone moderation tool schemas

## File Count Summary

| Category | Count |
|----------|-------|
| Root documentation | 17 |
| Root config files | 2 |
| Utility scripts | 3 |
| Core source files | 18 |
| OAuth module | 7 |
| Moderation module | 5 |
| Rich text module | 6 |
| Test files | 8 |
| Example files | 11 |
| Codegen tool | 5 |
| **Total (non-generated)** | **82** |

## Directory Structure

```
Rust-Atproto-SDK/
├── Cargo.toml
├── .gitignore
├── README.md
├── CLAUDE.md
├── [16 other .md documentation files]
├── find_missing_types.py
├── generate_namespace_impls.py
├── namespace_impls.rs
│
├── src/
│   ├── lib.rs
│   ├── agent.rs
│   ├── session_manager.rs
│   ├── types.rs
│   ├── consts.rs
│   ├── util.rs
│   ├── preferences.rs
│   ├── validation.rs
│   ├── syntax.rs
│   ├── did_doc.rs
│   ├── handle.rs
│   ├── tid.rs
│   ├── repo.rs
│   ├── mst.rs
│   ├── car.rs
│   ├── blob.rs
│   ├── server_auth.rs
│   ├── xrpc_subscription.rs
│   ├── namespaces.rs
│   │
│   ├── xrpc/
│   │   └── mod.rs
│   │
│   ├── oauth/
│   │   ├── mod.rs
│   │   ├── client.rs
│   │   ├── callback.rs
│   │   ├── dpop.rs
│   │   ├── pkce.rs
│   │   ├── state.rs
│   │   └── types.rs
│   │
│   ├── moderation/
│   │   ├── mod.rs
│   │   ├── decision.rs
│   │   ├── labels.rs
│   │   ├── types.rs
│   │   └── ui.rs
│   │
│   ├── rich_text/
│   │   ├── mod.rs
│   │   ├── rich_text.rs
│   │   ├── detection.rs
│   │   ├── sanitization.rs
│   │   ├── unicode.rs
│   │   └── util.rs
│   │
│   └── client/         [GENERATED - excluded from listing]
│       ├── mod.rs
│       ├── app/        [auto-generated API code]
│       ├── chat/       [auto-generated API code]
│       ├── com/        [auto-generated API code]
│       └── tools/      [auto-generated API code]
│
├── tests/
│   ├── agent_methods_test.rs
│   ├── integration_tests.rs
│   ├── moderation_test.rs
│   ├── notifications_test.rs
│   ├── preferences_methods_test.rs
│   ├── real_world_auth_tests.rs
│   ├── simple_integration.rs
│   └── simple_real_world_test.rs
│
├── examples/
│   ├── agent_demo.rs
│   ├── firehose_monitor.rs
│   ├── link_embed_demo.rs
│   ├── moderation_demo.rs
│   ├── oauth_demo.rs
│   ├── oauth_flow.rs
│   ├── persistent_session.rs
│   ├── reply_demo.rs
│   ├── rich_text_demo.rs
│   ├── simple_bot.rs
│   └── upload_image.rs
│
└── codegen/
    ├── Cargo.toml
    ├── README.md
    ├── src/
    │   ├── main.rs
    │   ├── codegen.rs
    │   └── lexicon.rs
    └── lexicons/       [JSON lexicon schemas]
        ├── app/bsky/
        ├── chat/bsky/
        ├── com/atproto/
        └── tools/ozone/
```

## Key Differences from TypeScript SDK

### Architecture Differences
1. **Code Generation**: Rust SDK uses a separate `codegen` tool to generate all API client code from lexicon JSON files
2. **Module Organization**: Rust follows Rust module conventions with `mod.rs` files
3. **Type System**: Rust's strong type system requires explicit type definitions and error handling
4. **Async Runtime**: Uses Rust's `async`/`await` with tokio runtime

### Additional Features in Rust SDK
1. **MST Implementation**: Native Merkle Search Tree implementation (`mst.rs`)
2. **CAR File Support**: Content Addressable aRchive file handling (`car.rs`)
3. **TID Support**: Timestamp identifier implementation (`tid.rs`)
4. **Validation Module**: Dedicated input validation (`validation.rs`)
5. **Syntax Module**: ATProto syntax validation (`syntax.rs`)

### Missing Features (To Be Implemented)
Based on TypeScript SDK comparison:
- Session management enhancements (partial implementation exists)
- Some moderation subject types (notification, feed-generator, user-list)
- Mocker/testing utilities (TS has `mocker.ts`)
- Some predicate functions (TS has `predicate.ts`)

## Notes

- **Generated Code**: The entire `src/client/` directory is auto-generated from lexicon specifications and contains hundreds of files with type definitions and API method implementations.
- **Lexicon Files**: 300+ JSON lexicon definition files in `codegen/lexicons/` directory define the API surface.
- **Test Coverage**: 8 test files covering various aspects of the SDK functionality.
- **Examples**: 11 comprehensive examples demonstrating key features.
- **Production Ready**: Following the guidelines in `CLAUDE.md`, all non-generated code follows production-quality standards with no stubs or placeholders.

---

*Generated on 2025-10-22*
*Working Directory: c:\Users\admin\RustSDK\Rust-Atproto-SDK*
