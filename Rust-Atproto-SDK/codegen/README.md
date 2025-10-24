# ATProto Lexicon Code Generator

Generates Rust code from ATProto Lexicon JSON schema files.

## Overview

This code generator transforms ATProto Lexicon schemas (JSON) into idiomatic Rust code, including:
- Type-safe query endpoint functions
- Type-safe procedure endpoint functions
- Request/response structures with proper Rust types
- Error types for each endpoint
- Full documentation from schema descriptions

## Usage

### 1. Add Lexicon Files

Place your lexicon JSON files in the `lexicons/` directory following the NSID structure:

```
lexicons/
├── com/
│   └── atproto/
│       ├── identity/
│       │   └── resolveHandle.json
│       └── repo/
│           └── createRecord.json
└── app/
    └── bsky/
        └── feed/
            └── getPosts.json
```

### 2. Run the Generator

```bash
cd codegen
cargo run
```

The generator will:
1. Scan all `.json` files in `lexicons/`
2. Parse each lexicon schema
3. Generate Rust code in `../src/client/` following the same structure

### 3. Generated Code

For a lexicon like `com.atproto.identity.resolveHandle`, the generator creates:

```
src/client/com/atproto/identity/resolve_handle.rs
```

With:
- `QueryParams` struct for input parameters
- `Output` struct for response data
- `resolve_handle()` async function
- Error types (if any)
- Full documentation

## Lexicon Schema Format

The generator supports ATProto Lexicon v1 schemas with:

### Query Endpoints (GET)

```json
{
  "lexicon": 1,
  "id": "com.atproto.identity.resolveHandle",
  "defs": {
    "main": {
      "type": "query",
      "description": "Resolves a handle to a DID",
      "parameters": {
        "type": "params",
        "required": ["handle"],
        "properties": {
          "handle": {
            "type": "string",
            "format": "handle",
            "description": "The handle to resolve"
          }
        }
      },
      "output": {
        "encoding": "application/json",
        "schema": {
          "type": "object",
          "required": ["did"],
          "properties": {
            "did": {
              "type": "string",
              "format": "did"
            }
          }
        }
      },
      "errors": [
        {"name": "HandleNotFound"}
      ]
    }
  }
}
```

### Procedure Endpoints (POST)

```json
{
  "lexicon": 1,
  "id": "com.atproto.repo.createRecord",
  "defs": {
    "main": {
      "type": "procedure",
      "description": "Create a repository record",
      "input": {
        "encoding": "application/json",
        "schema": {
          "type": "object",
          "required": ["repo", "collection", "record"],
          "properties": {
            "repo": {
              "type": "string",
              "format": "at-identifier"
            },
            "collection": {
              "type": "string"
            },
            "record": {
              "type": "object"
            }
          }
        }
      },
      "output": {
        "encoding": "application/json",
        "schema": {
          "type": "object",
          "required": ["uri", "cid"],
          "properties": {
            "uri": {"type": "string", "format": "at-uri"},
            "cid": {"type": "string", "format": "cid"}
          }
        }
      }
    }
  }
}
```

## Type Mapping

The generator maps Lexicon types to Rust types:

| Lexicon Type | Format | Rust Type |
|--------------|--------|-----------|
| `string` | - | `String` |
| `string` | `did` | `crate::types::Did` |
| `string` | `handle` | `String` |
| `string` | `at-uri` | `crate::syntax::AtUri` |
| `string` | `datetime` | `String` |
| `string` | `cid` | `String` |
| `integer` | - | `i64` |
| `boolean` | - | `bool` |
| `array` | - | `Vec<T>` |
| `object` | - | `serde_json::Value` |
| `ref` | - | Module path from ref |
| `union` | - | `serde_json::Value` |

## Generated Code Example

From `com.atproto.identity.resolveHandle`:

```rust
//! Generated code for com.atproto.identity.resolveHandle
//!
//! Resolves a handle (domain name) to a DID.

use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};
use serde::{Deserialize, Serialize};

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// The handle to resolve.
    pub handle: String,
}

/// Response output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Output {
    pub did: crate::types::Did,
}

/// Error: HandleNotFound
#[derive(Debug, Clone, thiserror::Error)]
#[error("HandleNotFound")]
pub struct HandleNotFoundError;

/// Resolves a handle (domain name) to a DID.
pub async fn resolve_handle(
    client: &impl XrpcClient,
    params: QueryParams,
) -> Result<XrpcResponse<Output>, XrpcError> {
    let mut req = XrpcRequest::query("com.atproto.identity.resolveHandle");

    // Add query parameters
    let params_json = serde_json::to_value(&params)
        .map_err(|e| XrpcError::Serialization(e))?;

    if let Some(obj) = params_json.as_object() {
        for (key, value) in obj {
            if let Some(s) = value.as_str() {
                req.params.insert(key.clone(), s.to_string());
            } else {
                req.params.insert(key.clone(), value.to_string());
            }
        }
    }

    client.request(req).await
}
```

## Current Support

✅ **Supported:**
- Query endpoints (GET)
- Procedure endpoints (POST)
- Query parameters
- Request input bodies
- Response output bodies
- Error definitions
- Type mapping for primitives, arrays, refs
- Documentation from descriptions
- Proper snake_case/PascalCase conversion

❌ **Not Yet Supported:**
- Record types
- Token types
- Complex union types (uses `serde_json::Value`)
- Namespace/module structure generation
- blob/bytes types
- Advanced validation constraints

## Future Enhancements

1. **Full Lexicon Support:**
   - Record type generation
   - Token types
   - Proper union types as Rust enums
   - Blob handling

2. **Namespace Organization:**
   - Generate `mod.rs` files
   - Namespace structs (ComNS, AppNS, etc.)
   - Integration with Agent

3. **Validation:**
   - String length constraints
   - Integer min/max validation
   - Array length validation
   - Pattern validation

4. **Documentation:**
   - Generate comprehensive docs
   - Examples from lexicon
   - Links to ATProto spec

## Architecture

```
codegen/
├── src/
│   ├── main.rs          # CLI entry point, file scanning
│   ├── lexicon.rs       # Lexicon schema types
│   └── codegen.rs       # Rust code generation
├── lexicons/            # Input: Lexicon JSON files
└── ../src/client/       # Output: Generated Rust code
```

## Dependencies

- `serde` / `serde_json` - JSON parsing
- `walkdir` - Recursive directory walking
- `heck` - Case conversion (snake_case, PascalCase)

## Contributing

To add support for new features:

1. Update `lexicon.rs` with new schema types
2. Add generation logic in `codegen.rs`
3. Test with sample lexicon files
4. Verify generated code compiles and works

## License

Part of the ATProto Rust SDK project.
