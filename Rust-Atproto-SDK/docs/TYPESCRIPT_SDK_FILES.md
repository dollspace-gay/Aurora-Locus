# TypeScript SDK Files - @atproto/api Package

This document lists all non-code-generated files in the TypeScript SDK located at `bluesky-social/atproto/packages/api`.

> **Note:** The `src/client/types/` directory contains auto-generated type definitions from lexicon specifications and is excluded from this listing as requested.

## Root Directory

### Configuration Files
- `package.json` - Package configuration and dependencies
- `tsconfig.json` - TypeScript configuration
- `tsconfig.build.json` - TypeScript build configuration
- `tsconfig.tests.json` - TypeScript test configuration
- `jest.config.js` - Jest testing framework configuration
- `jest.d.ts` - Jest type definitions
- `jest.setup.ts` - Jest test setup

### Documentation
- `README.md` - Package documentation
- `CHANGELOG.md` - Version history and changes
- `OAUTH.md` - OAuth implementation documentation

## Source Files (`src/`)

### Core Files
- `agent.ts` - Base agent implementation
- `atp-agent.ts` - ATProto agent
- `bsky-agent.ts` - Bluesky-specific agent
- `const.ts` - Constants
- `index.ts` - Main entry point
- `mocker.ts` - Testing/mocking utilities
- `predicate.ts` - Type predicates
- `session-manager.ts` - Session management
- `types.ts` - Core type definitions
- `util.ts` - Utility functions

### Client Module (`src/client/`)
- `client/index.ts` - Client module entry point
- `client/lexicons.ts` - Lexicon definitions
- `client/util.ts` - Client utilities

**Note:** `client/types/` directory excluded (contains generated code)

### Moderation Module (`src/moderation/`)

#### Core Files
- `moderation/index.ts` - Moderation module entry point
- `moderation/decision.ts` - Moderation decision logic
- `moderation/mutewords.ts` - Muted words functionality
- `moderation/types.ts` - Moderation type definitions
- `moderation/ui.ts` - UI-related moderation utilities
- `moderation/util.ts` - Moderation utilities

#### Constants (`moderation/const/`)
- `moderation/const/labels.ts` - Label constants

#### Subjects (`moderation/subjects/`)
- `moderation/subjects/account.ts` - Account moderation
- `moderation/subjects/feed-generator.ts` - Feed generator moderation
- `moderation/subjects/notification.ts` - Notification moderation
- `moderation/subjects/post.ts` - Post moderation
- `moderation/subjects/profile.ts` - Profile moderation
- `moderation/subjects/user-list.ts` - User list moderation

### Rich Text Module (`src/rich-text/`)
- `rich-text/detection.ts` - Text detection logic
- `rich-text/rich-text.ts` - Main rich text implementation
- `rich-text/sanitization.ts` - Text sanitization
- `rich-text/unicode.ts` - Unicode handling
- `rich-text/util.ts` - Rich text utilities

## File Count Summary

| Category | Count |
|----------|-------|
| Root config files | 7 |
| Root documentation | 3 |
| Core source files | 10 |
| Client module | 3 |
| Moderation core | 6 |
| Moderation constants | 1 |
| Moderation subjects | 6 |
| Rich text module | 5 |
| **Total** | **41** |

## Directory Structure

```
packages/api/
├── package.json
├── tsconfig.json
├── tsconfig.build.json
├── tsconfig.tests.json
├── jest.config.js
├── jest.d.ts
├── jest.setup.ts
├── README.md
├── CHANGELOG.md
├── OAUTH.md
└── src/
    ├── agent.ts
    ├── atp-agent.ts
    ├── bsky-agent.ts
    ├── const.ts
    ├── index.ts
    ├── mocker.ts
    ├── predicate.ts
    ├── session-manager.ts
    ├── types.ts
    ├── util.ts
    ├── client/
    │   ├── index.ts
    │   ├── lexicons.ts
    │   └── util.ts
    ├── moderation/
    │   ├── index.ts
    │   ├── decision.ts
    │   ├── mutewords.ts
    │   ├── types.ts
    │   ├── ui.ts
    │   ├── util.ts
    │   ├── const/
    │   │   └── labels.ts
    │   └── subjects/
    │       ├── account.ts
    │       ├── feed-generator.ts
    │       ├── notification.ts
    │       ├── post.ts
    │       ├── profile.ts
    │       └── user-list.ts
    └── rich-text/
        ├── detection.ts
        ├── rich-text.ts
        ├── sanitization.ts
        ├── unicode.ts
        └── util.ts
```

## Notes

- **Generated Code Excluded:** The `src/client/types/` directory contains auto-generated TypeScript type definitions from lexicon specifications (organized as `app/bsky/`, `chat/bsky/`, `com/atproto/`, and `tools/ozone/`) and has been excluded from this listing.
- **Test Files:** Test files in the `tests/` directory are not listed here but exist in the repository.
- **Scripts:** Build and development scripts in the `scripts/` directory are not listed here.
- **Definitions:** Lexicon definition files in the `definitions/` directory are not listed here.

---

*Generated on 2025-10-22*
*Source: https://github.com/bluesky-social/atproto/tree/main/packages/api*
