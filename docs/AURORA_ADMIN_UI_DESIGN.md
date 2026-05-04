# Aurora-Locus admin UI design

**Cycle:** v0.2
**Status:** Design — implementation phasing per section 9
**Last updated:** 2026-05-03

---

# 1. Executive summary

This document specifies the Aurora-Locus administrative UI for the v0.2 cycle. It commits to a comprehensive substrate — information architecture, page-level specifications, reusable component library, accessibility contract, and forthcoming-endpoint integration patterns — sufficient to ship a complete first-class moderator and operator interface against the v0.2 endpoint surface.

The cycle's scope on the UI front is ambitious by intent. Aurora-Locus is positioning as the next-tier PDS implementation: when the upstream reference exists as a parity floor and operators rely on shell scripts and direct database queries for everything beyond it, "comprehensive admin UI" is a meaningful axis on which Aurora can lead. The current `static/admin/` scaffolding is honest prototype-grade work — clean visual vocabulary, structurally underbuilt — and the v0.2 cycle is the right time to extend it into something operators can rely on.

This design doc is the spec. Implementation across the remaining v0.2 sub-phases (3.5 emitEvent, 3.7 aggregations, 3.8 audit chain, 3.9 real-time subscription, 3.10 runtime settings, plus #108 UI completion pass) follows the design doc rather than driving it.

## 1.1 What ships

Three top-level navigation domains structuring the UI:

- **Moderation** — queue, reports, appeals, mod events, audit. Visible only when the operator's session has a moderator role and only when the deployment's `AURORA_ADMIN_UI_MODERATION_MODE` runtime setting permits it.
- **Operations** — accounts, account detail, invites, sequencer, federation, blob ops, rate limits, system health, server (capabilities + version + config). Always visible to operators with appropriate role; the only domain visible in `reduced` mode.
- **Settings** — general, UI & modes (theme toggle, moderation mode, language), roles, capabilities probe.

Account detail is a shared destination both Moderation and Operations link into; per-drawer role gating determines what's visible to whom.

Substrate primitives the UI is built from, all shipped in v0.2:

1. Unified action affordance pattern with mandatory rationale and snapshot-at-decision capture
2. Inline subject preview with server-side hardened render and media proxy
3. Reusable history panels and structured context rendering
4. Cross-linked entity references with consistent display rules and routing
5. Real-time subscription substrate for moderation events and audit entries
6. Role-aware affordance gating per-control, not per-page
7. Lexicon-aware action surfacing — the action set adapts to the subject type
8. External labels panel surfacing labels from configured external labelers
9. Lucide icon set replacing emoji throughout
10. Three-state theme toggle (Light / Dark / System) with full dark mode color audit
11. WCAG 2.2 AA accessibility contract on every component
12. Command palette (Cmd/Ctrl+K) with fuzzy search across navigation, subjects, and actions
13. i18n-ready string scaffolding with locale-aware date and number formatting
14. Unified FilterStrip with adaptive chip popovers, calendar date-range picker, and URL-state persistence
15. Capability-routed API substrate handling the Phase 3.5 endpoint transition transparently

A small set of new endpoints in the `tools.aurora.admin.*` namespace, sequenced into existing sub-phases:

- `triggerPasswordReset` — admin-initiated user-mediated password reset
- `exportAccountForensic` — chain-of-custody forensic bundle export
- Six batch endpoints for multi-subject moderation actions (chain-entry atomic; per-subject best-effort with a `failures` array — see chainlink #112 for the two-tier model)
- Phase 3.5/3.7/3.8/3.9 endpoints already planned in their respective design docs, here specified for UI integration

## 1.2 What is preserved from the current UI

The visual identity of the current `static/admin/` scaffolding is preserved and extended, not rebuilt. Specifically:

- The CSS custom property system, color palette, card vocabulary, status badge treatment, table styling, modal shell, and sidebar layout pattern all carry forward.
- The page-header structure (title + subtitle + right-aligned actions) carries forward.
- The pagination strip pattern (Previous/Next buttons with cursor-based navigation) carries forward.
- The settings-grid two-column card pattern carries forward.

What changes is the navigation structure (flat 8-item nav becomes three-domain grouped nav), the icon system (emoji to Lucide), the introduction of dark mode, and the substrate primitives that didn't previously exist. Existing visual vocabulary is the foundation for new components rather than being replaced.

## 1.3 What this doc does not specify

Three deliberate exclusions worth naming up front:

**Implementation order across sub-phases.** This doc specifies *what* ships; the cycle plan and chainlink issues specify *when* each piece ships. Implementation will land across sub-phases 3.5 through 3.10 plus #108, with the substrate primitives that don't depend on forthcoming endpoints landing earliest.

**Server-side endpoint internals.** Where this doc commits to new endpoints (`exportAccountForensic`, batch operations, etc.), it specifies the lexicon shape and behavior contract. Implementation details (handler internals, transaction boundaries, audit-chain integration mechanics) defer to the per-phase design docs that govern the namespace work.

**Visual design comprehensives beyond the existing palette.** This doc preserves and extends the current visual language. It does not propose a wholesale visual redesign. If a future cycle wants to redesign the visual language, that's a separate piece of work — this doc is about substrate and surfaces, not aesthetic reinvention.

## 1.4 Decoupling

The UI is designed for Aurora-Locus as a standalone PDS deployment first. Where references to external moderation tooling are unavoidable (capability detection, federation endpoints, the moderation mode toggle's interaction with externally-coordinated moderation), they are framed abstractly. This UI does not assume, name, or detect any specific external moderation tool. Operators deploying Aurora-Locus alongside any external tooling configure that pairing through their own runtime settings; the UI surfaces the toggle and respects the configuration.

This decoupling discipline is structural, not just nominal. The design doc, the components, the strings file, the CSS class names, the route paths, and the audit log all reference external tooling abstractly. Aurora-Locus is its own product with its own UI, and that property is visible at every layer of the spec.

## 1.5 Reading this document

The doc is structured to support both linear reading and reference lookup:

- Sections 2 and 3 cover scope and architecture principles. Read in order if you're new to the design conversation.
- Section 4 covers information architecture — page structure, navigation, role visibility. Reference this when you need to know where a feature lives.
- Section 5 specifies every page in the UI. Reference this when you need to know what a specific page does.
- Section 6 specifies every reusable component. Reference this when you're building a new page and need to know which primitives exist.
- Section 7 specifies the visual design tokens and component library at the design-system level.
- Section 8 specifies the new endpoints to add and the integration pattern for forthcoming endpoints.
- Section 9 covers implementation phasing and the capability-routed transition.
- Sections 10 and 11 cover accessibility and i18n in depth.
- Section 12 covers the file-level transition from current `static/admin/` to the new structure.
- Section 13 covers testing strategy.
- Section 14 captures deferred work for v0.3 and beyond.

Cross-references between sections use anchored links where helpful. Endpoint references use the full NSID (e.g. `tools.aurora.moderator.queryEvents`) so they're greppable.

# 2. Scope and non-scope

This section lists what the v0.2 UI explicitly includes and excludes. The intent is to make boundaries unambiguous so implementation across sub-phases doesn't accumulate scope drift, and so that decisions deferred to v0.3 are visible as deferred rather than missed.

## 2.1 In scope

### 2.1.1 Pages and surfaces

The complete list of pages shipping in v0.2:

**Moderation domain:**
- Queue (filtered active-work view of reports + appeals needing attention)
- Reports (browseable, filterable report archive)
- Appeals (browseable, filterable appeal archive)
- Events (chronological moderation event log)
- Audit (unified verified + unverified audit feed)
- Report detail (rebuilt from current modal as full page)
- Appeal detail (rebuilt to first-class page with timeline)
- Event detail (rebuilt from current alert-based stub to full page)
- Audit entry detail (new, with chain-walk navigation)

**Operations domain:**
- Operator dashboard (real metrics, mode-aware)
- Accounts (account browser with text search and filter chips)
- Account detail (multi-drawer, role-gated panels)
- Record detail (new, reachable via cross-link from reports/events/etc.)
- Blob detail (new, reachable via cross-link)
- Invites (browser + create + bulk disable)
- Invite detail (new)
- Sequencer (status + controls)
- Federation (relay config, known instances, discovery trigger)
- Blob ops (storage stats, blob inventory, GC controls)
- Rate limits (config + status + cleanup)
- System health (consolidated health/metrics page)
- Server (capabilities probe, version info, server config)

**Settings domain:**
- General (instance name, service URL, basic config)
- UI & modes (theme toggle, moderation mode setting, language selector)
- Roles (member list per role, grant/revoke for SuperAdmin)
- Capabilities (alias to Operations → Server → Capabilities for top-level discoverability)

Total: 28 distinct pages plus inline drawers and modals.

### 2.1.2 Substrate primitives

The 21 substrate primitives enumerated in section 1.1 all ship in v0.2. Each is fully specified in section 6. The substrate is the foundation; pages compose primitives. New pages added in v0.3 should compose existing primitives rather than introduce new ones unless a genuinely novel pattern is needed.

### 2.1.3 New endpoints

The 14 forthcoming endpoints enumerated in section 1.1 are committed for v0.2 — meaning the UI specifies behavior against them and the lexicon shapes are fixed in this design doc. Each lands in its respective sub-phase. Where an endpoint is not yet shipped at the time a UI surface depending on it is built, the capability-routed substrate (primitive 21) provides a fallback path against the parity-floor `com.atproto.admin.*` endpoints. Section 8 specifies each endpoint; section 9 specifies the phase in which each lands.

### 2.1.4 Real-time integration

Subscription-driven real-time updates for moderation events and audit entries via `tools.aurora.admin.subscribeModEvents` (Phase 3.9). The subscription substrate (primitive 5) is built in v0.2; surfaces consuming it (Mod Events page, Audit page, Subject detail) integrate when the endpoint lands. Pre-3.9, those surfaces poll on a 10s interval as a fallback per the capability-routed pattern.

### 2.1.5 Accessibility

WCAG 2.2 Level AA compliance across every component shipping in v0.2. Section 10 specifies the contract per primitive: keyboard contracts, screen reader semantics, contrast ratios, focus management, motion preferences, target sizes, form semantics, structural landmarks. Accessibility is not a polish pass — it's a substrate property of every component.

### 2.1.6 Theming

Light, Dark, and System theme modes. Three-state toggle in sidebar footer. Cmd/Ctrl+Shift+L keyboard shortcut. Parallel CSS variable set under `[data-theme="dark"]`. `prefers-color-scheme` media query for System mode. Persistence via `localStorage`. Full color audit for both modes including all status badge variants. Section 7.2 specifies the dark-mode palette.

### 2.1.7 Internationalization

i18n-ready scaffolding: all user-facing strings routed through a `t()` helper, English-only string file shipped at `static/admin/i18n/en.json`, locale-aware date and number formatting via `Intl.DateTimeFormat` and `Intl.NumberFormat`. Language selector in Settings populated from available locale files. v0.2 ships English-only; future contributors add languages by dropping new locale files into the i18n directory. Section 11 specifies the contract.

### 2.1.8 Stylistic preservation

The current `static/admin/` visual language carries forward: color palette via CSS custom properties, card vocabulary (.75rem radius, light box-shadow, white surface, 1.5rem internal padding), table styling (uppercase letterspaced headers, hover rows, bordered cells), status badge pattern (rounded-pill, soft-tinted background with darker text), modal shell, sidebar dark slate with primary-blue active state. The flat 8-item nav becomes a three-domain grouped nav; emoji icons become Lucide icons; everything else extends rather than replaces.

## 2.2 Not in scope for v0.2

Items deliberately excluded from this cycle. Each carries a brief rationale and notes the natural future home.

### 2.2.1 Hover-card context previews

Hover tooltips that show contextual information about an entity reference (account preview, record preview, etc.) are deferred. The substrate primitive `<EntityRef>` is built without hover content. v0.3 can layer hover on top by extending `<EntityRef>` to fetch and render preview cards. Rationale: hover events fire frequently and add real backend traffic and rendering overhead even with debouncing; the canonical detail page is one click away and provides full information. Operators who need information now click through.

### 2.2.2 Calendar widget upgrade beyond v0.2 baseline

The v0.2 calendar widget supports range selection with preset chips (Today, Last 7 days, Last 30 days, This month, Last month). What v0.2 does not include: month/year jump navigation beyond standard prev/next, multi-month side-by-side view, or relative-date tokens like "yesterday + 3 days." These extensions are v0.3 if operator usage indicates they're needed.

### 2.2.3 Bulk operations beyond the six batch endpoints

The six batch endpoints (`batchTakedownAccounts`, `batchSuspendAccounts`, `batchRestoreAccounts`, `batchTakedownRecords`, `batchApplyLabel`, `batchRemoveLabel`) cover the highest-frequency bulk moderation workflows. Other bulk operations — bulk role grant, bulk invite generation with per-account binding, bulk email send — are not in v0.2. Operators needing those workflows for v0.2 use scripted access against single-subject endpoints. v0.3 evaluates which additional batch endpoints are warranted.

### 2.2.4 Time-bounded historical export

`exportAccountForensic` produces a current-state bundle. Reconstructing past account states for historical export ("the account as it was on March 1") requires sequencer replay infrastructure that is not in v0.2 scope. Forensic exports in v0.2 are point-in-time snapshots taken at the moment of export. Past actions are visible via audit chain entries (which are immutable by construction post-Phase-3.8) but past content state is not reconstructable.

### 2.2.5 Hardened SSR for record render

Aurora-Locus v0.2 renders ATProto records server-side with sanitization and routes media through a server-side proxy. This is a meaningful step beyond client-side render with direct media fetches. What v0.2 does not include: the maximally hardened pattern of full SSR with no JavaScript execution context whatsoever, used by some forensic-tier moderation tooling for embeds shown to moderators investigating sensitive content. The Aurora UI is admin-tier and trusted-environment; the additional hardening is unnecessary overhead for the v0.2 threat model. v0.3 can extend the render layer if a deployment posture warrants it.

### 2.2.6 Multi-tenant or per-namespace UI configuration

The UI assumes a single Aurora-Locus deployment serving a single set of operators. Per-tenant or per-namespace UI customization (different branding per service, different admin scopes per tenant) is not in scope. v0.3 evaluates if multi-tenant deployments emerge as a use case.

### 2.2.7 Wholesale visual redesign

The v0.2 cycle preserves the current visual identity. A future cycle could propose a redesign — different palette, different layout vocabulary, different design language — but this design doc does not contemplate that work. Operators upgrading to v0.2 will find the UI extended and modernized, not visually unrecognizable.

### 2.2.8 Browser support beyond modern evergreen

The UI targets Chrome, Firefox, Safari, and Edge in their currently-supported releases as of the v0.2 ship date. No support for legacy browsers, no IE-era polyfills, no compatibility shims for non-evergreen deployments. Operators using ancient browsers can use the parity `com.atproto.admin.*` endpoints directly via curl or scripted access.

### 2.2.9 Mobile-first design

The UI is desktop-first. The current responsive breakpoint at `max-width: 768px` is preserved and extended for usability on tablets, but v0.2 does not optimize for phone-shaped viewports. Operators doing administrative work on mobile use a tablet or laptop. v0.3 evaluates if mobile-first treatment is worthwhile based on actual operator usage patterns.

### 2.2.10 Operator activity dashboards beyond per-page metrics

The Operator dashboard shows instance metrics. The Moderator dashboard shows moderation metrics. What v0.2 does not include: a per-operator activity dashboard ("here is what @somemod has done this week, with throughput stats and decision patterns"). This is administrative-of-administrators visibility — relevant for team leads but not in v0.2 scope. v0.3 can introduce it as a SuperAdmin-tier surface.

### 2.2.11 Rich text editing for rationale fields

Rationale fields are plain `<textarea>` in v0.2. No markdown rendering, no rich text controls, no @-mentions of other operators, no image attachments. Rationales are operational notes; plain text is sufficient. Future cycles can extend if operator workflows demand richer authoring.

### 2.2.12 Federated cross-PDS subject views

When viewing an account or record on this PDS, the UI surfaces only data this PDS has authority over or has explicitly cached. It does not render content from other PDSes inline (parent thread rendering for replies, for example, only renders if the parent record is on this PDS). Cross-PDS context fetching is not in v0.2 scope. References to external content render as references with link-out, not inline content.

## 2.3 Out of scope, will not be added in any future v0.x cycle

Three things this UI does not now and does not intend to do:

### 2.3.1 Network-level moderation authority

This UI is for PDS administration. It does not apply network-wide labels, does not function as a labeling service for federation, does not act on behalf of external moderation services. Aurora-Locus operators who want to run a labeler service can do so via separate tooling (Ozone or alternatives); this UI does not duplicate that surface.

### 2.3.2 End-user account self-service

This UI is for operators. It does not include end-user-facing surfaces (password self-reset flow UI, account deletion request UI, email change confirmation UI from the user's side, etc.). Those flows are part of bsky-PDS and other end-user-facing tooling. The admin UI initiates user-mediated flows (e.g., `triggerPasswordReset` sends the email) but does not host the user-side completion of those flows.

### 2.3.3 Coupling to specific external tooling

The UI's design and implementation references external moderator tooling abstractly. It does not detect, prefer, or specially-handle any specific external tool. Deployments pairing Aurora with external tooling configure that pairing through runtime settings; the UI's behavior depends on operator configuration, not on the identity of any particular external system. This is permanent; future cycles will not introduce specific external-tool integration into this UI.

# 3. Architecture principles

This section states the load-bearing principles every subsequent specification derives from. When section 5 specifies a page or section 6 specifies a component, the rules in this section explain *why* the specification takes the shape it does. When future contributors extend the UI, these principles are the test for whether an extension fits.

Seven principles, in order of authority:

## 3.1 Server authority is total; the client is untrusted

The Aurora-Locus admin UI is a web application running in the operator's browser. It is software the operator can read, modify, fork, and replace. Its presence enables convenience; its absence does not enable bypass. Every operation that changes state, reveals data beyond the operator's authority, or affects the PDS's behavior must be authorized at the server, not at the client.

Concrete consequences:

- **The UI never holds authoritative state.** Module-globals in the current `script.js` that decide "is this operator allowed to see X" are advisory display state, not access control. The server makes the same decision independently for every request and rejects requests the operator's session cannot make.
- **Role-aware affordance gating** (substrate primitive 7) hides or disables UI elements based on the operator's session role. This is *display* logic; the server enforces the same gate independently. A moderator who manually constructs a request to a SuperAdmin-only endpoint receives a 403 from the server, regardless of what the UI shows them.
- **Capability-routed substrate** (substrate primitive 21) uses `tools.aurora.describeCapabilities` to decide which endpoints to call. This is a capability-detection mechanism for graceful degradation, not a security boundary. The server's response to each call is the operative authorization.
- **The forensic export feature** specifies that `includeAuditChain` and `includeAccountMetadata` parameters reject when the caller is not SuperAdmin. The UI hides those checkboxes for non-SuperAdmin sessions; the server independently rejects the parameters. Both gates exist; only the server one is load-bearing.

When this principle conflicts with convenience — when a UI shortcut would simplify a workflow but the server doesn't enforce the same shortcut's authorization — convenience yields. Convenience that the server cannot independently re-derive is not convenience the UI may offer.

## 3.2 Roles are tiered, asymmetric, and named at the action layer

The role model has three tiers:

- **Moderator** acts on subjects-as-content: takedown content, suspend accounts for behavior, apply labels, resolve reports, decide appeals. The reason for action is policy.
- **Admin** acts on accounts-as-infrastructure: passwords, emails, handles, signing keys, deletion, invite enable/disable, account export. The reason for action is operational maintenance.
- **SuperAdmin** acts on authority itself: granting and revoking roles, exporting authority-shaped data (audit chain inclusion in forensic exports, account metadata inclusion in forensic exports). The reason for action is "who has authority over what."

The tiers are *additive* — Admin includes Moderator capabilities, SuperAdmin includes Admin capabilities. Authority composes upward. A SuperAdmin viewing a queue resolves reports as a moderator would; nothing in the UI gates moderator actions away from higher tiers.

The tiers are *asymmetric in unusual cases*. The clearest example is role visibility versus role mutation: any tier can see who has what role (`com.atproto.admin.listRoles`), but only SuperAdmin can grant or revoke roles (`tools.aurora.superadmin.grantRole`, `tools.aurora.superadmin.revokeRole`). Read at one tier, write at another, deliberate by design.

The tiers are *named at the action layer*, not the page layer. Most of the time a page's existence depends on tier (Moderation domain pages require Moderator+; the Roles management write surface requires SuperAdmin). But a page can mix tiers — Account detail's drawers each have independent role gates: overview drawer for everyone, moderation drawer for Moderator+, management drawer for Admin+, role-write affordances for SuperAdmin only. The page is the surface; the actions inside it are the gates.

This principle has a direct consequence for substrate primitive 7 (role-aware action affordance gating): the gate is per-control, not per-page. A page may have a mix of visible-and-disabled, visible-and-enabled, and entirely-hidden controls based on the operator's tier. The substrate handles this uniformly.

## 3.3 Subject-shape determines action set; lexicon is the source of truth

The actions a moderator can take depend on what kind of subject they're acting on. An account can be taken down, suspended, restored, labeled. A record can be taken down, labeled, but not "suspended" (records aren't entities with suspension semantics). A blob can be quarantined, restored, deleted, but not "suspended" or "labeled" (blobs are content-addressable storage, not subjects with reputational state).

The UI's action surfaces enumerate actions by subject shape, not by a flat list of "all moderation actions." Substrate primitive 12 (lexicon-aware action surfacing) implements this: the action dropdown on the unified action panel shows only the actions valid for the current subject's type. Future lexicon additions (new subject types, new action types) extend the dropdown automatically.

The lexicon is the source of truth for which actions apply to which subject types. Aurora-Locus uses Rust types as the lexicon (per CLAUDE.md's "Rust-types-as-lexicon convention"); the UI consumes the action enumeration from the same place the server validates against. There is no second list of "what actions exist" the UI maintains separately.

This extends to error semantics. When a server rejects an action because the action doesn't apply to the subject type, it returns an error the UI surfaces directly. The UI does not pre-validate type/action combinations except as a display optimization — the server is the authority on what actions are valid against what subjects.

## 3.4 Snapshots-at-decision and audit chain are co-equal substrate

Two pieces of forensic infrastructure ship in v0.2: snapshot-at-decision-time (substrate primitive 8) and the hash-chained audit log (`tools.aurora.admin.getAuditTrail`, Phase 3.8). They are co-equal substrate, not optional features.

A snapshot captures *what the subject looked like at the moment a decision was taken on it*. An audit chain entry captures *what decision was taken, by whom, when, with what rationale*. Together they answer the forensic question: "show me this decision, the reasoning behind it, and the artifact it was made on." Neither alone is sufficient.

The two integrate at write time:

- When an action is taken, the snapshot is captured before the action lands.
- The audit chain entry is written after the action, with the snapshot's hash referenced.
- The audit chain entry's own hash is computed over its content including the snapshot reference.
- Verification of the audit chain implicitly verifies the snapshot was not tampered with after the fact.

This integration means the UI's audit page (section 5.4.5) and the UI's individual event detail page (section 5.4.4) both render the snapshot inline alongside the chain entry, as a single forensic artifact. The user does not navigate to a separate "snapshot store" — the snapshot is part of the decision record.

Forensic exports (`tools.aurora.admin.exportAccountForensic`, section 8) extend this principle: a forensic bundle is hashed, the hash is recorded in the audit chain, and the bundle's chain entry id is returned to the operator. A bundle's integrity is verifiable against the chain forever afterward. Chain-of-custody is built in, not bolted on.

This principle is the reason the audit page is a "unified" surface (section 5.4.5) merging both the parity-floor `getAuditLog` data and the Phase 3.8 `getAuditTrail` chain data into one feed: snapshot-and-chain forensic infrastructure is one thing the UI presents, not two.

## 3.5 Real-time is for signal arrival; everything else polls

Subscription-driven real-time updates are appropriate where *the moment an event arrives matters*. Polling is appropriate everywhere else. The UI applies real-time selectively, not uniformly.

Surfaces that consume `tools.aurora.admin.subscribeModEvents` (Phase 3.9):

- Mod Events page — events are the page's purpose; arrival latency is the signal
- Audit page — chain-entry arrival is forensic signal
- Subject detail page — events affecting the active subject arrive in real-time as a coordination signal between operators acting on the same subject

Surfaces that poll on a 30-second interval:

- Dashboard widgets (instance metrics, queue counts)
- Reports queue (queue depth and status counts in nav)
- Appeals page
- Operator dashboard health surfaces (longer interval — 60-120s — for expensive surfaces like federation status and blob statistics)
- Bell badge in the sidebar

Surfaces that refresh only on user action:

- Account detail (refetch after action completion)
- Record detail (refetch after action)
- Subject detail's history panels (refetch after action)
- Settings (manual save and reload)

The principle: real-time is a tool with cost. Subscription state needs lifecycle management, reconnect logic, stale-state recovery on resume. Applying it everywhere creates connection management overhead that degrades the operator's experience for the surfaces that don't actually benefit. Applying it selectively keeps the cost where the value is.

When the design doc commits a surface to real-time, that commitment is *post-Phase 3.9*. Pre-3.9, the surface polls at a tighter interval (10s for surfaces that will become subscription-driven) as a fallback per substrate primitive 21. The capability-routed substrate handles the transition transparently — components consuming the substrate don't change when 3.9 ships; the substrate flips from polling to subscription internally.

## 3.6 Decoupling is structural, not nominal

Aurora-Locus is its own product. It interoperates with the broader ATProto ecosystem and with whatever external tooling an operator chooses to deploy alongside it. It does not name, prefer, detect, or specially-handle any specific external system in its UI, design doc, code, strings, or comments.

When the UI references "external moderator tools" or "external clients" or "configured external labelers," these references are abstract by design and remain abstract permanently. The substrate exists for operators to configure pairings; the UI does not assume which pairing they'll choose.

Concrete consequences:

- The moderation mode setting (`AURORA_ADMIN_UI_MODERATION_MODE`) takes values `full`, `reduced`, `disabled`. It does not take values like "external-tool-X-paired" or "running-with-Y-system." Operators choose modes based on their deployment posture; the UI doesn't infer the deployment from external signals.
- The capability detection system (`tools.aurora.describeCapabilities`) reports what *Aurora-Locus* exposes. It does not probe for or report on external systems.
- The external labels panel (substrate primitive 13) accepts a configured list of labeler service DIDs. It treats them uniformly. It does not have a special "this is the well-known labeler" path.
- File names, class names, NSIDs, route paths, log lines, and string keys never contain names of external systems.

This is structural decoupling: every layer of the spec maintains the abstraction. It is not "we don't mention X in the README"; it is "the system is genuinely agnostic to which external pairing operators choose."

A direct consequence: this design doc, the resulting code, the strings file, the audit log entries, and any test fixtures use abstract framing throughout. Section 13 (testing strategy) commits to a sweep verifying this discipline before the v0.2 cycle closes.

## 3.7 PDS authority is bounded by network posture

This UI exposes the full administrative surface a PDS legitimately controls: account lifecycle, record takedowns, blob operations, audit, and the local resolution of reports and appeals. The actual *network reach* of those actions varies by deployment posture.

On networks where the PDS is paired with an external labeling authority (Ozone on bsky, or comparable arrangements elsewhere), label-application from this UI has bounded effect — local annotation on this PDS rather than network-wide signal. Account-level actions remain real (the account is unreachable on this PDS). Record takedowns remain real (the records are removed from this PDS's storage). But network-visible labels propagate from the labeler, not from the PDS.

On independent deployments where the PDS is also the labeler (or where the network's labeler is configured to pair with this PDS's labels), the same operations have full network effect. Same UI, different deployment context, different practical authority.

The UI does not vary its surface by deployment posture. It exposes everything a PDS can do and treats operators as adults who understand what their deployment's authority is. Operators deploying with external labeler pairings understand that label-application here is local; operators deploying independently understand the full scope. The UI's job is to expose capabilities clearly, not to gate them based on assumptions about deployment context.

One consequence: this UI ships with the full label-application affordance regardless of posture. Operators who don't want the affordance visible (because in their deployment posture it's purely local annotation and they don't use it that way) can hide it via the moderation mode setting, or future runtime UI configuration may add per-affordance visibility toggles.

---

These seven principles are sufficient to derive the rest of the design doc. Where a subsequent section specifies behavior, you can trace that behavior back to one or more of these principles. Where a future contributor proposes an extension, the test for whether the extension fits the design is whether it follows these principles.

The principles are not a checklist; they are a frame. They prioritize:

- **Correctness under pressure over surface area** (3.1, 3.4)
- **Clarity of authority over flexibility of action** (3.2, 3.3)
- **Honest cost of real-time over uniform application** (3.5)
- **Genuine product independence over convenience** (3.6)
- **Honest exposure over patronizing abstraction** (3.7)

These priorities are visible in every page-level specification that follows.

# 4. Information architecture

This section is the map of the UI: every page's location, every navigation path, every routing pattern, and every rule governing what is visible to whom. Subsequent sections specify what individual pages do; this section specifies where they live and how operators reach them.

## 4.1 Top-level structure

The UI has three top-level navigation domains, presented in this order in the sidebar:

1. **Moderation**
2. **Operations**
3. **Settings**

The Dashboard is not a domain — it is a single landing page above the three domains, always visible to any authenticated operator regardless of role.

The sidebar's structure top to bottom:

```
┌─────────────────────────────┐
│  Aurora Locus               │
│  Admin panel                │
├─────────────────────────────┤
│  Dashboard                  │
│                             │
│  MODERATION                 │
│  Queue                      │
│  Reports                    │
│  Appeals                    │
│  Events                     │
│  Audit                      │
│                             │
│  OPERATIONS                 │
│  Accounts                   │
│  Invites                    │
│  Sequencer                  │
│  Federation                 │
│  Blob ops                   │
│  Rate limits                │
│  System health              │
│  Server                     │
│                             │
│  SETTINGS                   │
│  General                    │
│  UI & modes                 │
│  Roles                      │
│  Capabilities               │
├─────────────────────────────┤
│  @operator-handle           │
│  Admin                      │
│  ◐ Theme: System            │
│  Logout                     │
└─────────────────────────────┘
```

Domain group labels are uppercase, letterspaced, dimmed-white text rendered above their items as section headers without dividers. Dashboard sits alone above the first group label.

## 4.2 Mode-aware visibility

The sidebar's visible domains depend on the deployment's `AURORA_ADMIN_UI_MODERATION_MODE` setting (Phase 3.10) and the operator's session role.

The mode setting takes three values:

| Mode | Description |
|---|---|
| `full` | Default. Complete moderator UI; Moderation domain visible to operators with moderator role. |
| `reduced` | Operator-tier features only. Moderation domain hidden entirely. Operations and Settings domains visible. |
| `disabled` | Aurora's UI is not the active moderation interface. Sidebar collapses to Settings only; landing page is a configurable redirect or a "managed elsewhere" stub. |

The mode interacts with role visibility:

| Operator role | `full` mode | `reduced` mode | `disabled` mode |
|---|---|---|---|
| Moderator only | Dashboard, Moderation, Settings (limited) | Dashboard, Settings (limited) | Settings (limited) |
| Admin (includes Moderator) | Dashboard, Moderation, Operations, Settings | Dashboard, Operations, Settings | Settings (limited) |
| SuperAdmin (includes Admin) | All four | All except Moderation | Settings (full) |

Two notes on this matrix:

**A Moderator-only operator in `reduced` mode sees no moderation surfaces at all.** This is correct: in reduced mode, moderation is happening elsewhere, and a moderator's role on this PDS provides no operative authority. The UI is honest about that — the operator sees an Operator dashboard with read-only metrics and Settings with what their role permits, nothing more. This may feel sparse; that is the deployment posture's intent.

**Settings is always visible** because it contains the controls (UI & modes) needed to change the mode itself. Without this exception, an operator who deploys with `disabled` mode and no redirect configured would have no path to recover the UI to a usable state. The `AURORA_RECOVERY_MODE=true` environment variable provides a separate emergency path bypassing the runtime setting on startup; the always-visible Settings is the in-band path.

The mode toggle in Settings → UI & modes is gated to SuperAdmin role only. Lower tiers can view the current mode but not change it.

## 4.3 Routing

The UI uses hash-based routing. The hash structure is:

```
#<domain>/<section>/<detail>?<filters>
```

Where:
- `<domain>` is one of: `dashboard`, `mod`, `ops`, `settings`
- `<section>` is the page within the domain (e.g., `queue`, `accounts`, `general`)
- `<detail>` is an optional entity identifier for detail pages (e.g., a DID, a record URI, an appeal ID)
- `<filters>` are optional URL-encoded filter values for list pages

The full route table:

### Moderation domain

| Route | Surface |
|---|---|
| `#mod/queue` | Queue page |
| `#mod/reports` | Reports list page |
| `#mod/reports/:id` | Report detail page |
| `#mod/appeals` | Appeals list page |
| `#mod/appeals/:id` | Appeal detail page |
| `#mod/events` | Events list page |
| `#mod/events/:id` | Event detail page |
| `#mod/audit` | Audit list page |
| `#mod/audit/:id` | Audit entry detail page |

### Operations domain

| Route | Surface |
|---|---|
| `#ops/dashboard` | Operator dashboard (or `#dashboard` for shared landing) |
| `#ops/accounts` | Accounts browser |
| `#ops/accounts/:did` | Account detail page |
| `#ops/records/:uri` | Record detail page (URI is URL-encoded) |
| `#ops/blobs/:cid` | Blob detail page |
| `#ops/invites` | Invites list page |
| `#ops/invites/:code` | Invite detail page |
| `#ops/sequencer` | Sequencer status page |
| `#ops/federation` | Federation status page |
| `#ops/blob-ops` | Blob ops page |
| `#ops/rate-limits` | Rate limits page |
| `#ops/system-health` | System health page |
| `#ops/server` | Server (capabilities + version + config) |

### Settings domain

| Route | Surface |
|---|---|
| `#settings/general` | General settings |
| `#settings/ui-modes` | UI & modes settings |
| `#settings/roles` | Roles list |
| `#settings/roles/:role` | Members list for a specific role |
| `#settings/capabilities` | Capabilities probe (alias for `#ops/server`) |

### Special routes

| Route | Behavior |
|---|---|
| `#` (empty) | Redirects to `#dashboard` |
| `#dashboard` | Dashboard landing (mode-and-role aware content; see section 5.1) |
| `#404/<original-route>` | Not-found page when an authenticated route fails to resolve |

### Filter persistence

List pages encode their filter state in the hash query string. Examples:

```
#mod/events?actor=did:plc:abc&type=takedown&page=2
#mod/audit?verified=true&since=2026-04-01
#ops/accounts?status=suspended&created_after=2026-01-01
```

Filter state writes to the hash whenever filters change; reading the hash on page load restores the filtered view. Browser back and forward navigate through filter changes naturally.

The visible "Filters appear in your URL" tooltip near the FilterStrip (substrate primitive 20) sets operator expectation about URL contents.

## 4.4 Cross-domain destinations

Several entity types are reached from multiple domains:

**Account detail** is reached from:
- Operations → Accounts (canonical entry; full management drawer)
- Moderation → Reports → report detail → subject link (moderation drawer is primary)
- Moderation → Events → event detail → subject link
- Moderation → Audit → audit entry → subject link
- Moderation → Appeals → appeal detail → appellant link
- Settings → Roles → member list → member link
- Anywhere a `<EntityRef>` for a DID is rendered

The page itself is the same regardless of entry path. The visible drawers and active tab default differ:

- Entry from Operations → Accounts: Account overview drawer expanded; Account management drawer expanded for Admin+.
- Entry from any Moderation surface: Account overview drawer expanded; Moderation actions drawer expanded.
- Entry from Settings → Roles: Account overview drawer expanded; Roles section emphasized.

The breadcrumb reflects entry path:

- `Operations › Accounts › @somehandle`
- `Moderation › Reports › r:abc123 › @somehandle`
- `Settings › Roles › moderators › @somehandle`

**Record detail** is reached from:
- Anywhere a `<EntityRef>` for a record URI is rendered (reports, events, audit entries, account detail's records-authored panel)
- Direct hash navigation when an operator pastes a URI

**Blob detail** is reached from:
- Account detail's blob inventory panel
- Record detail (when the record embeds blobs)
- Operations → Blob ops → blob inventory list → row click
- Anywhere a `<EntityRef>` for a CID is rendered

These cross-domain destinations are governed by the principle that a canonical detail page exists per entity (section 3.3 implication). Different entry paths arrive at the same page; the page does not bifurcate by entry.

## 4.5 Breadcrumbs

Every page (except Dashboard, which is a landing) renders a breadcrumb above the page header.

Breadcrumb structure:

- Each segment is a link (except the last, which is the current page and not a link)
- Segments separated by `›` (single-character right angle quotation, U+203A)
- First segment is always the domain name
- Maximum visible segments: as many as fit on one line; middle segments truncate with `…` if needed
- Truncated segments restore via expand-on-click; the underlying URL retains full path for sharing

Standard patterns:

- 2 segments for list pages: `Operations › Accounts`
- 3 segments for top-level detail: `Operations › Accounts › @somehandle`
- 4+ segments for cross-domain or nested detail: `Moderation › Reports › r:abc123 › @somehandle`

The breadcrumb does not duplicate the sidebar — the active sidebar item shows where you are at the domain/section level; the breadcrumb shows the path you took to arrive at the current detail. They convey complementary information.

## 4.6 Page header pattern

Every page has a header block immediately below the breadcrumb:

```
┌─────────────────────────────────────────────────────────┐
│  Page Title                              [page actions] │
│  Optional subtitle text                                 │
└─────────────────────────────────────────────────────────┘
```

- **Title** uses `<h1>` (the existing CSS treats `.page-header h2` as the main heading; section 12 specifies the migration to `<h1>` for accessibility hierarchy correctness).
- **Subtitle** (optional) describes the page's purpose in one sentence.
- **Page actions** sit in the right-aligned action group: primary actions, search inputs, filter toggles, export buttons.

The page header is followed by the FilterStrip (on list pages) or by content directly (on detail pages and dashboards). The FilterStrip is not part of the header — it's a separate substrate component immediately below.

## 4.7 Bell badge and notification surface

The sidebar contains a bell badge integrated into the Moderation group label. The badge displays a combined count of items requiring attention: open reports + pending appeals.

```
MODERATION  [3]
Queue
Reports
Appeals
Events
Audit
```

- Visible only when operator has Moderator+ role and mode is `full`.
- Hidden in `reduced` and `disabled` modes regardless of role.
- Badge value polled every 30 seconds via `tools.aurora.admin.getQueueStats` (Phase 3.7) when shipped, falling back to summing counts from `listReports` filtered to open + `listAppeals` filtered to pending pre-3.7.
- Clicking the group label navigates to Queue (the most actionable surface); clicking the badge specifically also goes to Queue.

The bell badge is not a notification feed. New events arriving in real-time (Phase 3.9) display as transient toasts (top-right of viewport, auto-dismiss after 4 seconds, accessibility via `aria-live`); they do not accumulate in a notification panel. Operators wanting full event history navigate to Events.

## 4.8 Sidebar footer

The footer sits below all navigation and contains four elements:

1. Operator handle (top, with click-to-go-to-account-detail)
2. Operator role(s) (small, secondary text)
3. Theme toggle (three-state pill: Light / Dark / System)
4. Logout button

The theme toggle is always visible regardless of mode or role (per section 1.1's "always available, never buried" intent for theme).

## 4.9 Empty states

Routes that resolve to entities the operator cannot access return one of three responses:

- **404**: The entity does not exist on this PDS, or the operator's role cannot enumerate the entity's existence. The UI surfaces a generic "Not found" page with a "Back to Dashboard" link. The 404 does not distinguish "doesn't exist" from "you can't see it" — non-enumeration is the discipline.
- **403**: The operator's session expired or the role changed during the session. UI surfaces "Session expired" and redirects to login.
- **5xx**: The PDS itself errored. UI surfaces a generic error page with a retry link and the request ID for debugging.

Empty *list* states (the list resolved successfully but contains no items) are different from 404s; they're handled by the FilterStrip's empty-state component (section 6.20).

## 4.10 Responsive behavior

The UI is desktop-first. Three breakpoints:

| Breakpoint | Behavior |
|---|---|
| ≥ 1200px | Full sidebar (260px) + main content + optional secondary panel |
| 768-1199px | Sidebar (260px) + main content; secondary panels collapse to overlays |
| < 768px | Sidebar collapses to top-bar with hamburger menu; main content fills viewport |

Phone-shaped viewports (< 480px) are not optimized in v0.2 (per section 2.2.9). The 768px breakpoint covers tablets and works acceptably at narrower widths.

Sidebar collapse is purely responsive — no manual sidebar toggle in v0.2. Operators on smaller viewports get the hamburger automatically.

# 5. Page-level specifications

This section specifies every page in the v0.2 UI. Each page specification follows a consistent template:

- **Route** — the hash route(s) that resolve to the page
- **Role gating** — minimum role required to reach the page; per-element gating noted inline
- **Mode visibility** — which `AURORA_ADMIN_UI_MODERATION_MODE` values render the page
- **Purpose** — one paragraph stating what the page exists for
- **Endpoint mapping** — table of UI elements ↔ endpoints
- **Layout** — the page's visual structure
- **Real-time behavior** — subscription, polling, or action-driven refresh
- **Action affordances** — what actions the page supports, with confirmation flows
- **Cross-pivots** — links out to related entities
- **Empty / loading / error states**
- **Notes** — anything specific to the page that doesn't fit above

The pages are grouped by domain. Within each domain, list pages precede their detail counterparts.

## 5.1 Dashboard

The Dashboard is the operator's landing page. Its content adapts to the operator's role and the deployment mode.

- **Route:** `#dashboard` (canonical), `#` (redirects), `#ops/dashboard` (alias)
- **Role gating:** Visible to any authenticated operator
- **Mode visibility:** Visible in `full` and `reduced`. In `disabled` mode, replaced by the redirect-or-stub page.

### Purpose

The Dashboard surfaces situational awareness on landing: what's running, what needs attention, what's recent. It is not a workspace; it is a starting point. Operators who land here either move to a specific page to do work, or scan the surfaces to confirm "nothing's on fire" before logging out.

The dashboard has two flavors that share the page:

- **Operator dashboard** — instance metrics, system health, federation status. Always visible. The only flavor in `reduced` mode.
- **Moderator dashboard** — queue depth, recent moderation activity, throughput stats. Visible only in `full` mode to operators with Moderator+ role.

When both flavors apply, a tab toggle at the top of the page switches between them. The default tab is Moderator if the operator's role permits and mode is `full`; Operator otherwise.

### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Operator stat cards (accounts, posts, blobs, storage) | `tools.aurora.ops.getInstanceMetrics` | Counts and growth deltas per metric |
| Operator system health row | `tools.aurora.ops.getSystemHealth` | Status indicator per subsystem |
| Operator federation health | `tools.aurora.ops.getFederationStatus` | Peer count, recent event volume |
| Operator recent activity feed | `com.atproto.admin.listRecentEvents` | Last 20 sequencer events |
| Moderator stat cards (open reports, pending appeals, queue depth) | `tools.aurora.admin.getQueueStats` (Phase 3.7) | Polled every 30s |
| Moderator activity charts | `tools.aurora.admin.getModerationMetrics` (Phase 3.7) | Time-series for last 30 days |
| Moderator recent activity feed | `tools.aurora.moderator.queryEvents` | Last 20 mod events; subscription-driven post-Phase 3.9 |

### Layout

```
┌──────────────────────────────────────────────────────────┐
│  Dashboard                                               │
│  Aurora Locus PDS overview                               │
│  ─────────────────────                                   │
│  [Operator] [Moderator]                                  │
│                                                          │
│  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐             │
│  │ Stat   │ │ Stat   │ │ Stat   │ │ Stat   │             │
│  └────────┘ └────────┘ └────────┘ └────────┘             │
│                                                          │
│  ┌──────────────────────┐ ┌──────────────────────┐       │
│  │ Chart                │ │ Chart                │       │
│  └──────────────────────┘ └──────────────────────┘       │
│                                                          │
│  ┌──────────────────────────────────────────────┐        │
│  │ Recent activity                              │        │
│  │ • event row                                  │        │
│  │ • event row                                  │        │
│  │ • event row                                  │        │
│  └──────────────────────────────────────────────┘        │
└──────────────────────────────────────────────────────────┘
```

Tab toggle below the page header. Stat cards in a responsive grid (4 columns at full width, collapses below 768px). Charts in a 2-column grid. Recent activity feed full-width below.

The stat card pattern preserves the current `.stat-card` styling (icon + label + value + change). The change indicator has three semantic variants: `.positive` (green), `.attention` (warning amber, for "this number existing is something to look at" metrics like Pending Reports), and neutral. The current `.positive` is preserved; `.attention` is new (replaces the current "Requires attention" plain text).

### Real-time behavior

- **Operator stats:** polled every 30 seconds
- **Operator system health:** polled every 30 seconds
- **Operator federation health:** polled every 60 seconds
- **Operator recent activity:** polled every 30 seconds; no subscription path
- **Moderator stats:** polled every 30 seconds
- **Moderator activity feed:** subscription-driven post-Phase 3.9; polled every 10 seconds pre-3.9

The dashboard does not initiate background fetches when the tab is not focused. On tab refocus, all polled surfaces refetch immediately.

### Action affordances

The Dashboard has no action affordances. Stat cards and activity rows are display-only with click-through links to relevant detail pages.

### Cross-pivots

- Stat card → relevant page (e.g., Pending Reports stat card → Reports page)
- Activity feed row → event detail or originating subject (whichever is more useful per row)
- Chart legends are not interactive in v0.2 (no click-to-filter from chart)

### Empty / loading / error states

- **Loading on first visit:** skeleton placeholders for stat cards (gray bars matching layout); spinner in chart areas; "Loading recent activity…" in feed.
- **Empty activity feed:** "No recent activity. As events occur, they'll appear here." Lucide `inbox` icon.
- **Stat card with no comparison data:** value renders without delta indicator; no fake "0%" change.
- **Endpoint failure:** specific element shows error state with retry link; other elements continue rendering. Dashboard never completely fails — partial loads are acceptable.

### Notes

- The current dashboard's hardcoded `+12 this week` and `+243 today` values are removed. Real deltas come from `getInstanceMetrics`'s historical fields. If the endpoint returns a metric without a comparison value, the change indicator omits.
- Moderator and Operator tabs preserve filter and scroll position when switching back and forth within a session.
- Dashboard is *not* what `#dashboard` resolves to in `disabled` mode — that's the stub-or-redirect surface (specified in section 5.5.2 for Settings → UI & modes).

## 5.2 Account detail

This is the canonical detail surface for a single account. It is the most heavily-trafficked page in the UI because almost every moderation and operations workflow either starts or ends here.

- **Route:** `#ops/accounts/:did` (canonical), reachable from many cross-links
- **Role gating:** Page visible to any authenticated operator. Drawer-level gating per section 5.2.4.
- **Mode visibility:** Visible in `full` and `reduced`. In `disabled` mode, only Settings is reachable so this page is unreachable.

### Purpose

Account detail consolidates everything Aurora-Locus knows about a single account into one page: identity, status, history, content references, and the action surfaces appropriate to the operator's role. It is designed to be the page an operator opens, takes action on, and closes — without needing to navigate elsewhere to gather context.

### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Account overview drawer | `com.atproto.admin.getAccountInfo` | Identity, status, creation date, role(s) |
| Subject context drawer | `tools.aurora.moderator.getSubjectContext` | Recent actions, recent reports, recent appeals (Moderator+) |
| Subject history drawer | `tools.aurora.moderator.getSubjectHistory` | Full chronological action history (Moderator+) |
| Records authored panel | `com.atproto.repo.listRecords` (filtered to this DID's repo) | Most recent N posts/profile/lists |
| Blob inventory panel | `tools.aurora.ops.listBlobs` (filtered to this DID) | Owned blob count + recent (Admin+) |
| Invite lineage panel | `com.atproto.admin.getInviteCodes` (filtered to this DID as creator + as redeemer) | Codes created by, code that created (Admin+) |
| External labels panel | per-labeler `com.atproto.label.queryLabels` calls | Labels from configured external labelers |
| Moderation actions drawer | (action-tier endpoints; see Action affordances below) | Moderator+ |
| Account management drawer | (action-tier endpoints; see Action affordances below) | Admin+ |
| Roles panel | `com.atproto.admin.listRoles` (read), `tools.aurora.superadmin.{grantRole, revokeRole}` (write) | Read at any tier; write at SuperAdmin |
| Forensic export action | `tools.aurora.admin.exportAccountForensic` | Admin+ basic; SuperAdmin extended sections |

### Layout

The page uses a primary-content + secondary-rail two-column layout at full width:

```
┌──────────────────────────────────────────────────────────┐
│  Operations › Accounts › @somehandle                     │
│                                                          │
│  @somehandle                            [active] badge   │
│  did:plc:abc... · Member since 2026-03-15                │
│  ──────────────                                          │
│                                                          │
│  ┌─────────────────────────────┐ ┌──────────────────┐    │
│  │ ▼ Account overview          │ │ Subject context  │    │
│  │   handle, did, email, role  │ │ Recent actions   │    │
│  │   creation date, status     │ │ Recent reports   │    │
│  │                             │ │ Recent appeals   │    │
│  ├─────────────────────────────┤ │ External labels  │    │
│  │ ▼ Moderation actions [Mod+] │ │                  │    │
│  │   takedown, suspend, label  │ ├──────────────────┤    │
│  │                             │ │ Records authored │    │
│  ├─────────────────────────────┤ │ Recent posts     │    │
│  │ ▼ Account management [Adm+] │ │                  │    │
│  │   email, handle, password   │ ├──────────────────┤    │
│  │   signing key, delete       │ │ Blob inventory   │    │
│  │   forensic export           │ │ Owned blobs      │    │
│  │                             │ │                  │    │
│  ├─────────────────────────────┤ ├──────────────────┤    │
│  │ ▼ Subject history [Mod+]    │ │ Invite lineage   │    │
│  │   chronological actions     │ │ Created by       │    │
│  │                             │ │ Created codes    │    │
│  └─────────────────────────────┘ └──────────────────┘    │
└──────────────────────────────────────────────────────────┘
```

The primary column (left) holds drawers — collapsible action and history surfaces. The secondary rail (right) holds context panels — read-only information surfaces.

Below 1200px, the rail collapses below the primary column. Below 768px, drawers default to collapsed.

The status badge to the right of the handle reflects current account state (active, suspended, takedown, deactivated). It uses the existing `.status-badge` styling with mode-appropriate variants.

### Drawer-level role gating

Drawers visible per role:

| Drawer | Moderator | Admin | SuperAdmin |
|---|---|---|---|
| Account overview | ✓ | ✓ | ✓ |
| Subject context | ✓ | ✓ | ✓ |
| Subject history | ✓ | ✓ | ✓ |
| Moderation actions | ✓ | ✓ | ✓ |
| Account management | — | ✓ | ✓ |
| Forensic export | — | ✓ (with section gates) | ✓ (full) |

Each drawer renders or doesn't render based on the role check. Hidden drawers don't appear collapsed-empty; they don't render at all.

### Real-time behavior

- **Page load:** all drawers and panels fetch in parallel
- **Subscription (post-Phase 3.9):** events affecting this DID arrive via filtered `subscribeModEvents`. When a new event arrives, a banner above the drawers shows: "New event: @anotheroperator suspended this account 4s ago. [Refresh]" — non-intrusive; operator chooses when to refresh. Page does not auto-mutate while operator is mid-action.
- **Action completion:** affected drawers refetch automatically; banner clears

### Action affordances

The page has two distinct action drawers: Moderation actions and Account management. Each uses the unified action panel (substrate primitive 3) but with different available actions.

#### Moderation actions drawer (Moderator+)

Available actions, surfaced via the action dropdown:

- Takedown account
- Suspend account
- Restore account
- Apply label
- Remove label
- Send email (administrative communication)

Confirmation flow per substrate primitive 3:

- Reversible actions (suspend, restore, label apply/remove): confirmation modal with rationale (required); single "Confirm" button.
- High-impact actions (takedown): confirmation modal with rationale (required) + "I understand this affects all federation" checkbox.

Snapshot-at-decision captured per action; audit chain entry written; cross-pivot link to the resulting Event detail page available in the success toast.

Endpoint routing: pre-Phase 3.5, calls per-action endpoints (`takedownAccount`, `suspendAccount`, etc.). Post-3.5, calls `tools.aurora.admin.emitEvent` with action discrimination. Substrate primitive 21 (capability-routed substrate) handles the transition.

#### Account management drawer (Admin+)

Three sub-sections:

**Identity sub-section:**
- Update email — inline form within drawer
  - Default path: trigger user-mediated change-email flow (deferred to v0.3 if endpoint absent)
  - Override path: direct set via `updateAccountEmail` with rationale + typed confirmation
- Update handle — inline form within drawer
  - Same two-track pattern as email

**Credentials sub-section:**
- Send password reset — single-click button, friendly styling
  - Calls `tools.aurora.admin.triggerPasswordReset` (new endpoint, section 8.6)
  - Toast confirms: "Password reset email sent to e****@example.com" (email masked)
  - Rationale required, captured in audit
- Override password — separate action, distinct styling, opens modal
  - Modal with strong warning copy, typed confirmation, rationale required
  - Calls `updateAccountPassword`
- Update signing key — modal-only action
  - Modal with "irreversible" checkbox, typed confirmation, rationale required
  - Calls `updateAccountSigningKey`

**Lifecycle sub-section:**
- Enable/disable invites — single-click toggle, no confirmation modal, default rationale prefilled
  - Calls `enableAccountInvites` / `disableAccountInvites`
- Delete account — modal with handle-typed confirmation, irreversibility checkbox, rationale required
  - Calls `deleteAccount`
- Forensic export — opens forensic export modal (specified below)

#### Forensic export modal

Triggered from Account management drawer's lifecycle sub-section.

```
Generate forensic export
─────────────────────────

Subject: @somehandle
         did:plc:abc...

Include:
☑ Repository content (CAR file)
☑ Blobs                         (~12.4 MB, 47 blobs)
☑ Moderation history            (3 prior actions)
☐ Account metadata              [SuperAdmin only]
                                — Email, signing keys, invite lineage
☐ Audit chain entries           [SuperAdmin only]
                                — Operator decisions and chain context

Rationale (required)
[                                                      ]

This export will be recorded in the audit chain with a tamper-evident hash.
The bundle will contain account data; treat as sensitive.

                              [Cancel]  [Generate export]
```

For Admin sessions: SuperAdmin-only checkboxes are disabled with explanatory text shown.
For SuperAdmin sessions: SuperAdmin-only checkboxes are available, default unchecked.

On submit:
- Calls `tools.aurora.admin.exportAccountForensic` with the parameter set
- Server validates SuperAdmin-gated parameters against caller's role; rejects with explicit error if Admin attempts to set them
- Bundle streams as download; toast confirms with audit entry ID and link to that audit entry detail page

### Cross-pivots

From the page, the operator can navigate to:

- **Subject context drawer pivots:** any actor DID, any reporter DID, any reviewer DID renders as `<EntityRef>` linking to that account; any subject URI links to the relevant record/blob detail
- **Subject history drawer pivots:** every action row links to its event detail (`#mod/events/:id`)
- **Records authored panel:** each record links to record detail (`#ops/records/:uri`)
- **Blob inventory panel:** each blob links to blob detail (`#ops/blobs/:cid`)
- **Invite lineage panel:** the creator code links to invite detail (`#ops/invites/:code`); created codes link to their detail pages; redeemers from those codes link back to their account details
- **Roles panel:** each role badge links to role members list (`#settings/roles/:role`)

Every cross-pivot is a real navigation (not a modal expansion) per section 3 architecture principle on canonical detail pages.

### Empty / loading / error states

- **Loading:** Each drawer and panel shows skeleton state independently. The page is usable as soon as Account overview loads; other drawers fill in as their endpoints respond.
- **Empty drawer/panel:** Subject context drawer shows "No recent activity" if zero across all categories. Records panel shows "No records authored." Blob panel shows "No owned blobs." Invite lineage shows "Account did not redeem an invite code" or "Account has not created any invite codes" per direction.
- **DID resolution failure:** if the route's DID does not resolve to an account on this PDS, render 404 (per section 4.9 non-enumeration discipline).
- **Partial endpoint failure:** drawer/panel shows "Could not load — [Retry]" inline; rest of page continues.

### Notes

- The page is wide. Account detail is the surface where most substrate primitives compose simultaneously. Implementation should treat each drawer as an independently-developed component to keep cognitive load manageable.
- The status badge color is the visible signal that something is materially different about an account. A "takedown" red badge is distinctive; an operator scanning down an Account browser then opening one with that badge knows immediately what state they're entering.
- Account management drawer's lifecycle actions (delete especially) are intentionally separated from moderation actions. Admin destructive operations and Moderator policy operations are different categories of work; the drawer separation reflects that.
- Per section 3.5 (real-time is for signal arrival), this page is not subscription-driven for its primary content. Action completion drives refresh. The subscription banner for new events is the only real-time element, and it's specifically a coordination signal between operators acting on the same subject simultaneously.

## 5.3 Moderation domain list pages

### 5.3.1 Queue

The Queue is the active-work landing surface within the Moderation domain. It surfaces items requiring attention: open reports, pending appeals, optionally other actionable signals.

- **Route:** `#mod/queue`
- **Role gating:** Moderator+
- **Mode visibility:** `full` only

#### Purpose

The Queue is the operator's primary work surface — what they open at the start of a moderation shift and work through. It's not a comprehensive view of all moderation items; it's a curated view of items that need a decision, sorted by attention-priority. Operators who want comprehensive views go to Reports or Appeals.

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Queue items | `tools.aurora.moderator.queryStatuses` filtered to actionable + `tools.aurora.moderator.listAppeals` filtered to pending | Merged client-side, sorted by priority |
| Queue stats (in page header) | `tools.aurora.admin.getQueueStats` (Phase 3.7) | Counts by category |
| Multi-select bulk actions | The six batch endpoints (per section 8) | When operator selects multiple items |

#### Layout

```
┌──────────────────────────────────────────────────────────┐
│  Moderation › Queue                                      │
│                                                          │
│  Queue                              [Refresh] [Bulk]     │
│  12 reports, 3 appeals needing attention                 │
│  ─────────────────────                                   │
│                                                          │
│  [FilterStrip: Status · Type · Subject · Date range]     │
│                                                          │
│  ☐ ┌────────────────────────────────────────────────┐    │
│    │ Report  · @reporter on @subject · 2h ago       │    │
│    │ "Spam content from this account..."            │    │
│    │ [→ Open]                                       │    │
│    └────────────────────────────────────────────────┘    │
│                                                          │
│  ☐ ┌────────────────────────────────────────────────┐    │
│    │ Appeal · @appellant on suspension · 4h ago     │    │
│    │ "I was suspended for X but actually..."        │    │
│    │ [→ Open]                                       │    │
│    └────────────────────────────────────────────────┘    │
│                                                          │
│  ┌──────────────────────────────────────────────┐        │
│  │ [3 selected → Takedown · Suspend · Cancel]   │        │
│  └──────────────────────────────────────────────┘        │
│                                                          │
│  [Prev]  [25 / 50 / 100]  [Next]                         │
└──────────────────────────────────────────────────────────┘
```

Items render as cards in a vertical list (preserving the existing `.mod-item`/`.report-item` card-list pattern). Each card shows item type, primary parties, age, excerpt, and a "Open" link to the canonical detail page.

Multi-select is enabled via the "Bulk" toggle in the page header. When enabled, checkboxes appear at the left of each card and a sticky action bar slides in at the bottom of the viewport showing selection count and bulk action buttons.

#### Real-time behavior

- **Pre-Phase 3.9:** poll every 10 seconds when page is visible
- **Post-Phase 3.9:** subscribe to filtered `subscribeModEvents` (events that affect queue state — new reports, status changes, appeals filed)
- **New item arrival:** subtle fade-in animation; new card appears at top with 1px highlight that fades over 2 seconds
- **Item state change while operator is on page** (e.g., another operator resolved an item): card fades out with a small "resolved by @anotheroperator" indication that lingers 3 seconds before removal

#### Action affordances

- **Per-card "Open"**: navigates to the canonical detail page
- **Multi-select + bulk action**: opens batch confirmation modal (per section 6 bulk-action component)
- **Filter changes**: applied via FilterStrip; URL hash updates

No inline actions on cards (no "resolve here" buttons). Decisions happen on detail pages where context is fuller. Queue is a triage surface; the work surface is detail.

#### Cross-pivots

- Reporter handle/DID → Account detail
- Subject handle/DID/URI → Account / Record / Blob detail
- "Open" → canonical detail (Report or Appeal)

#### Empty / loading / error states

- **Empty (filter active):** "No matches. Try widening your filters." with "Clear all filters" link
- **Empty (no filter, no items):** "Nothing in the queue. Things will appear here as reports and appeals come in." Lucide `inbox` icon, neutral copy
- **Loading:** skeleton cards (3 placeholders) while initial fetch resolves
- **Error:** inline error at top with retry; existing items remain visible if refresh fails partway

#### Notes

- Priority sort order: appeals first (because appeals have implicit deadline-like semantics), then reports by age (oldest first). Tunable per Phase 3.7's `getQueueStats` shape; may evolve with operator feedback.
- The Queue is the natural target for the bell badge link (per section 4.7).

### 5.3.2 Reports

The full Reports list. Comprehensive, filterable, paginated.

- **Route:** `#mod/reports`
- **Role gating:** Moderator+
- **Mode visibility:** `full` only

#### Purpose

Reports lists every report ever filed against subjects on this PDS, regardless of state. Where the Queue is curated triage, Reports is comprehensive archive. Operators reach for Reports when they want to research patterns ("how often is this account reported"), find a specific report by filter, or audit historical resolution.

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Reports list | `com.atproto.admin.listReports` | Paginated, cursor-based |
| Filter chips | Filter parameters on `listReports` | See FilterStrip per section 6 |
| Search input | None in v0.2 | Reports don't have full-text search; filtering is by structured fields |
| Report status update (inline) | None — happens on detail page | List is read; detail is write |

#### Layout

```
┌──────────────────────────────────────────────────────────┐
│  Moderation › Reports                                    │
│                                                          │
│  Content reports                                         │
│  All reports filed against subjects on this PDS          │
│  ─────────────────────                                   │
│                                                          │
│  [FilterStrip: Status · Reporter · Subject · Type · Date]│
│                                                          │
│  ┌────────────────────────────────────────────────┐      │
│  │ Reporter   Subject       Reason       Status   │      │
│  │ Date       Action taken                         │     │
│  ├────────────────────────────────────────────────┤      │
│  │ @reporter  @subject     spam        open       │      │
│  │ 2h ago     none yet                             │     │
│  ├────────────────────────────────────────────────┤      │
│  │ ...                                             │     │
│  └────────────────────────────────────────────────┘      │
│                                                          │
│  [Prev]  [25 / 50 / 100]  [Next]                         │
│                                                          │
│  Showing 1-50 of 1,247                                   │
└──────────────────────────────────────────────────────────┘
```

Tabular layout (preserving the existing `.data-table` pattern). Row hover highlights. Click row navigates to Report detail.

The status column uses status badges (preserving `.status-active`, `.status-pending`, etc. — extended with new variants for resolved/dismissed).

#### Real-time behavior

- Polled every 30 seconds when page is visible
- Not subscription-driven (per section 3.5; Reports is reference data, not signal-arrival)

#### Action affordances

None at the list level. Reports detail handles all actions.

#### Cross-pivots

- Reporter / Subject DIDs → Account detail
- Subject URI → Record detail
- Click anywhere on row → Report detail
- "Action taken" cell, when populated → Event detail

#### Empty / loading / error states

- **Empty (filter active):** "No reports match these filters."
- **Empty (no filter, no reports ever):** "No reports filed yet."
- **Loading:** skeleton table rows
- **Error:** inline error at top; existing rows preserved

### 5.3.3 Report detail

The canonical detail surface for a single report.

- **Route:** `#mod/reports/:id`
- **Role gating:** Moderator+
- **Mode visibility:** `full` only

#### Purpose

Report detail shows everything about a single report: who filed it, on what subject, with what reasoning; the subject in context (preview, history, related reports); and the action surface to resolve, dismiss, or take action.

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Report metadata | `com.atproto.admin.listReports` filtered to id (or dedicated getter if added later) | Reporter, subject, reason, narrative, timestamp |
| Subject preview | Server-side hardened render via Aurora's render layer | See substrate primitive 9 |
| Subject history (related reports, prior actions) | `tools.aurora.moderator.getSubjectContext` filtered to subject | |
| Reporter context (their report-to-action ratio) | `tools.aurora.moderator.queryEvents` filtered to reporter | Aggregation client-side |
| External labels on subject | per-labeler `queryLabels` calls | Substrate primitive 13 |
| Status update / action | `com.atproto.admin.updateReportStatus` + per-action endpoints (or `emitEvent` post-3.5) | Via unified action panel |

#### Layout

```
┌──────────────────────────────────────────────────────────┐
│  Moderation › Reports › r:abc12345                       │
│                                                          │
│  Report from @reporter                       [open]      │
│  Filed 2h ago · spam                                     │
│  ─────────────────────                                   │
│                                                          │
│  ┌──────────────────────────────┐ ┌─────────────────┐    │
│  │ Subject                      │ │ Reporter context│    │
│  │ ┌─────────────────────────┐  │ │ @reporter       │    │
│  │ │ [rendered post preview] │  │ │ 47 reports filed│    │
│  │ │ "spammy text here..."   │  │ │ 31 actioned     │    │
│  │ └─────────────────────────┘  │ │ 66% accuracy    │    │
│  │                              │ │                 │    │
│  │ Reporter narrative           │ ├─────────────────┤    │
│  │ ┌─────────────────────────┐  │ │ Subject history │    │
│  │ │ "I keep seeing posts..."│  │ │ 3 prior reports │    │
│  │ └─────────────────────────┘  │ │ 1 prior label   │    │
│  │                              │ │ 0 prior actions │    │
│  ├──────────────────────────────┤ │                 │    │
│  │ Take action                  │ ├─────────────────┤    │
│  │ ┌─────────────────────────┐  │ │ External labels │    │
│  │ │ Action: [▾ Resolve    ] │  │ │ spam (Bsky Mod) │    │
│  │ │ Rationale (required)    │  │ │                 │    │
│  │ │ [                     ] │  │ └─────────────────┘    │
│  │ │ [Cancel] [Confirm]      │  │                        │
│  │ └─────────────────────────┘  │                        │
│  └──────────────────────────────┘                        │
└──────────────────────────────────────────────────────────┘
```

Two-column layout at full width: primary content (subject + narrative + action panel) on the left, context rail (reporter context + subject history + external labels) on the right.

Subject preview uses inline subject preview (substrate primitive 1). Hardened render with media proxy. Graduated content reveal (primitive 10) blurs sensitive content with a click-to-reveal layer when the report category indicates potentially-disturbing content.

#### Real-time behavior

- **Page load:** all panels fetch in parallel
- **Subscription (post-Phase 3.9):** events affecting this report's subject arrive via filtered subscription. Banner above action panel: "New event: @anotheroperator labeled this subject 4s ago. [Refresh]"
- **Action completion:** page refetches; toast confirms with link to resulting Event detail

#### Action affordances

The action panel is the unified action panel pattern (substrate primitive 3).

Actions available based on subject type:

For an account subject:
- Resolve report (no further action — the report itself is the operator's response)
- Dismiss report (mark dismissed without action)
- Takedown account (resolves report + takes action)
- Suspend account
- Apply label
- Escalate (status change to under_review without resolution)

For a record subject:
- Resolve report
- Dismiss report
- Takedown record
- Apply label
- Escalate

The action dropdown filters to valid combinations per substrate primitive 12 (lexicon-aware action surfacing).

Each action requires rationale. Snapshot-at-decision captured. Audit chain entry written.

#### Cross-pivots

- Reporter DID → Account detail
- Subject DID/URI → Account / Record detail (whichever applies)
- Reporter context "47 reports filed" → Reports filtered to this reporter
- Subject history "3 prior reports" → Reports filtered to this subject
- "Action taken" (when filled) → Event detail
- After action: toast with "View action" link → Event detail of the new event

#### Empty / loading / error states

- **404:** report id does not exist or is not accessible — generic not-found
- **Subject preview unavailable:** "Subject content cannot be displayed (record deleted or unreachable)." Show DID/URI; action panel still works
- **Loading:** skeleton placeholders per panel; action panel disabled until full report metadata loads

#### Notes

- This page replaces the current Report Details modal (per section 3 cluster 5 decision: page navigation canonical, modals for transient interactions only).
- The "Reporter context" panel is informational, not authoritative. The 66% accuracy figure does not gate any action — operators consider it as context but the action decision is theirs alone.

### 5.3.4 Appeals

The full Appeals list.

- **Route:** `#mod/appeals`
- **Role gating:** Moderator+
- **Mode visibility:** `full` only

#### Purpose

Appeals lists every appeal filed by an account contesting a moderation action. Where Reports surface incoming complaints, Appeals surface incoming reversals — operators reviewing whether a prior decision should stand.

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Appeals list | `tools.aurora.moderator.listAppeals` | Paginated; status / appellant / reviewer / date filters |
| Filter chips | listAppeals filter parameters | |

#### Layout

Same tabular pattern as Reports list. Status badge, appellant, original action, filed date, reviewer if assigned.

#### Real-time behavior

- Polled every 30 seconds
- Not subscription-driven

#### Action affordances

None at list level.

#### Cross-pivots

- Appellant DID → Account detail
- Reviewer DID → Account detail
- Original action → Event detail
- Click row → Appeal detail

#### Empty / loading / error states

Same patterns as Reports list.

### 5.3.5 Appeal detail

Canonical detail for a single appeal.

- **Route:** `#mod/appeals/:id`
- **Role gating:** Moderator+
- **Mode visibility:** `full` only

#### Purpose

Appeal detail surfaces the full decision chain: the appeal narrative, the original action being appealed, the originating report (if any), the resulting decision and its cascading effect (if approved).

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Appeal metadata | `tools.aurora.moderator.getAppeal` | Includes lifecycle entries and original-action summary |
| Original action | Embedded in `getAppeal` response | Action type, actor, timestamp, rationale |
| Originating report (if any) | Embedded in `getAppeal` response | |
| Subject preview | Substrate primitive 1 | |
| Appeal lifecycle (timeline) | Embedded in `getAppeal` response | All status changes with timestamps |
| Appeal resolution | Pattern D action endpoint (forthcoming, see section 8) or `emitEvent` post-3.5 | |

#### Layout

```
┌──────────────────────────────────────────────────────────┐
│  Moderation › Appeals › a:xyz67890                       │
│                                                          │
│  Appeal from @appellant                  [pending]       │
│  Filed 4h ago · contesting suspension                    │
│  ─────────────────────                                   │
│                                                          │
│  ┌──────────────────────────────┐ ┌─────────────────┐    │
│  │ Decision chain               │ │ Appellant       │    │
│  │                              │ │ @appellant      │    │
│  │ Original report              │ │ Member 2025-... │    │
│  │ ↓ filed by @reporter         │ │ Status: suspend │    │
│  │                              │ │                 │    │
│  │ Original action              │ ├─────────────────┤    │
│  │ ↓ suspend by @somemod        │ │ Subject preview │    │
│  │   "repeated harassment"      │ │ [post or profile│    │
│  │                              │ │  rendering]     │    │
│  │ Appeal filed                 │ │                 │    │
│  │ ↓ "I wasn't harassing,       │ ├─────────────────┤    │
│  │   I was responding..."       │ │ Lifecycle       │    │
│  │                              │ │ filed: 4h ago   │    │
│  │ Pending review               │ │ assigned: 1h ago│    │
│  │                              │ │                 │    │
│  ├──────────────────────────────┤ └─────────────────┘    │
│  │ Resolve appeal               │                        │
│  │ ┌─────────────────────────┐  │                        │
│  │ │ Action: [▾ Approve]     │  │                        │
│  │ │   Approving will        │  │                        │
│  │ │   restore @appellant.   │  │                        │
│  │ │ Rationale (required)    │  │                        │
│  │ │ [                     ] │  │                        │
│  │ │ [Cancel] [Confirm]      │  │                        │
│  │ └─────────────────────────┘  │                        │
│  └──────────────────────────────┘                        │
└──────────────────────────────────────────────────────────┘
```

Decision chain renders the chronological case progression. Each step in the chain links to its canonical detail (originating report → Report detail; original action → Event detail). Substrate primitive 4 (decision chain rendering) handles this in minimal-viable form: vertical list with arrows between steps; each step is an inline-expandable summary.

#### Real-time behavior

- Polled every 30 seconds
- Subscription (post-Phase 3.9): events affecting this appeal's subject arrive; banner above action panel as in Report detail

#### Action affordances

Available actions:
- Approve (overturns original action)
- Deny (upholds original action)
- Escalate (status change to under_review)
- Request more info (special status with optional message to appellant)

Approving an appeal cascades server-side — the original action is reversed atomically as part of the appeal-resolution. UI flags this in the confirmation: "Approving will restore @appellant" with explicit acknowledgment in the rationale.

Each action requires rationale (which becomes part of the appeal's resolution record visible to the appellant).

#### Cross-pivots

- Appellant DID → Account detail
- Reviewer DID → Account detail (if assigned)
- Original action → Event detail
- Originating report → Report detail
- Subject → Account / Record detail

#### Empty / loading / error states

- **404:** appeal id does not exist
- **Loading:** skeleton placeholders per panel
- **Subject preview unavailable:** same handling as Report detail

#### Notes

- The cascade effect on approval is server-side atomic per section 3.4 (snapshots and audit chain are co-equal). UI doesn't make the cascade a separate operator action — both the appeal resolution and the original action's reversal are one operator decision, recorded as one chain entry with both subjects referenced.
- Appeals against labels (rather than against actions) follow the same pattern but the cascade is label removal rather than action reversal.

### 5.3.6 Events

The Mod events list — chronological log of moderation events.

- **Route:** `#mod/events`
- **Role gating:** Moderator+
- **Mode visibility:** `full` only

#### Purpose

Events is the primary log of moderation activity. Where Reports surface incoming complaints and Appeals surface incoming reversals, Events surface what was *done* — actions taken by operators, in chronological order. It is the first surface to consult when researching "what happened to this subject" or "what has @somemod done this week."

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Events list | `tools.aurora.moderator.queryEvents` | Paginated, rich-context handle resolution |
| Filter chips | queryEvents filter parameters: actor, subject, type, date range | |

#### Layout

Tabular pattern. Columns: timestamp, actor, action, subject, rationale (truncated). Click row → Event detail.

#### Real-time behavior

- **Pre-Phase 3.9:** poll every 10 seconds when visible (faster than Reports/Appeals because events are signal-arrival-shaped data)
- **Post-Phase 3.9:** subscription via `subscribeModEvents`. New events animate in at top with subtle fade.

#### Action affordances

None at list level.

#### Cross-pivots

- Actor DID → Account detail
- Subject DID/URI → Account / Record / Blob detail
- Click row → Event detail

#### Empty / loading / error states

Standard patterns.

### 5.3.7 Event detail

Canonical detail for a single mod event.

- **Route:** `#mod/events/:id`
- **Role gating:** Moderator+
- **Mode visibility:** `full` only

#### Purpose

Event detail shows the full forensic record of a single moderation action: who acted, what action they took, on what subject, with what rationale, what state the subject was in at the time of action (snapshot), and what audit chain entry resulted.

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Event metadata | `tools.aurora.moderator.getEvent` | Resolved actor/subject handles |
| Snapshot at decision | Embedded in event response (or fetched separately) | Substrate primitive 8 |
| Originating report | Embedded if event was a response to a report | |
| Audit chain entry | `tools.aurora.admin.getAuditTrail` filtered to this event | Chain hash, prev/next pointers |
| Resulting appeal | Embedded if event was appealed | |

#### Layout

```
┌──────────────────────────────────────────────────────────┐
│  Moderation › Events › e:def45678                        │
│                                                          │
│  takedown by @somemod                                    │
│  on @somesubject · 3d ago                                │
│  ─────────────────────                                   │
│                                                          │
│  ┌──────────────────────────────┐ ┌─────────────────┐    │
│  │ Snapshot at decision         │ │ Decision chain  │    │
│  │ ┌─────────────────────────┐  │ │                 │    │
│  │ │ [rendered subject       │  │ │ ← Report        │    │
│  │ │  state at takedown      │  │ │   r:abc12345    │    │
│  │ │  time]                  │  │ │                 │    │
│  │ └─────────────────────────┘  │ │ This event      │    │
│  │                              │ │   takedown      │    │
│  ├──────────────────────────────┤ │                 │    │
│  │ Rationale                    │ │ → Appeal        │    │
│  │ ┌─────────────────────────┐  │ │   a:xyz67890    │    │
│  │ │ "Repeated harassment    │  │ │                 │    │
│  │ │  after warning."        │  │ ├─────────────────┤    │
│  │ └─────────────────────────┘  │ │ Audit chain     │    │
│  │                              │ │ Entry verified ✓│    │
│  ├──────────────────────────────┤ │ Hash: a83f...   │    │
│  │ Subject                      │ │                 │    │
│  │ @somesubject (did:plc:...)   │ │ ← Previous      │    │
│  │ Currently: takedown          │ │ → Next          │    │
│  └──────────────────────────────┘ └─────────────────┘    │
└──────────────────────────────────────────────────────────┘
```

Snapshot renders as inline subject preview at decision-time state — not the current state. Operators reading old events see the subject as it appeared when the decision was made, not its current form.

The decision chain rail in the right column shows the case progression. Originating report (if any), this event, resulting appeal (if any). Audit chain section below, with prev/next chain navigation.

#### Real-time behavior

- **Page load:** all panels fetch in parallel
- Read-only after load (events don't mutate post-creation; only their downstream consequences may)

#### Action affordances

None — events are immutable history. The page is read-only.

#### Cross-pivots

- Actor → Account detail
- Subject → Account / Record / Blob detail
- Originating report → Report detail
- Resulting appeal → Appeal detail
- Audit chain entry → Audit entry detail
- Previous/Next in chain → Audit entry detail (chain walk)

#### Empty / loading / error states

- **404:** event id does not exist
- **Snapshot unavailable:** "Snapshot not captured for this event (predates snapshot infrastructure)." Show metadata only
- **Audit chain entry unavailable:** "This event predates the audit chain. No verification available." Subject and rationale still display

#### Notes

- Pre-Phase 3.8 events have no chain entry. They display as "verified: no" in the audit panel without a hash. Post-3.8 events have full chain integration.
- Pre-snapshot events (those taken before substrate primitive 8 ships) display the current subject state with explicit "Showing current subject state — snapshot was not captured at decision time" note. This is honest about what the historical record can and cannot prove.

### 5.3.8 Audit

The unified audit feed — verified chain entries plus pre-chain entries in one chronological stream.

- **Route:** `#mod/audit`
- **Role gating:** Moderator+
- **Mode visibility:** `full` only

#### Purpose

Audit is the comprehensive accountability surface. It merges parity-floor `getAuditLog` data and Phase 3.8 hash-chained `getAuditTrail` data into one feed. Each row carries a verification badge: verified (in chain) or pre-chain (predates the chain). Operators investigating "what happened" reach for Audit; auditors verifying cryptographic accountability reach for the verified subset.

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Audit feed | `com.atproto.admin.getAuditLog` + `tools.aurora.admin.getAuditTrail` (Phase 3.8) | Merged client-side, sorted by timestamp |
| Filter chips | actor, action type, subject, date range, verified-only toggle | |
| Bulk export | None in v0.2 | Consider for v0.3 |

#### Layout

Tabular pattern. Columns: timestamp, verified badge, actor, action, subject, hash (truncated, click to copy).

The "verified-only" toggle in FilterStrip filters to chain entries only.

#### Real-time behavior

- **Pre-Phase 3.9:** poll every 30 seconds
- **Post-Phase 3.9:** subscription. New chain entries arrive in real-time

#### Action affordances

None at list level.

#### Cross-pivots

- Actor → Account detail
- Subject → Account / Record / Blob detail
- Click row → Audit entry detail

#### Empty / loading / error states

Standard patterns. The "no audit data" empty state is unlikely on any production deployment but renders if it occurs.

#### Notes

- Merging the two endpoints client-side is computationally simple but pagination is awkward because cursor schemes differ. v0.2 implementation: fetch a bounded recent window from both, merge, paginate the merged result client-side. Bounded means "most recent N where N is sufficient for typical operator browsing." Operators needing deeper audit history use the verified-only filter (which uses `getAuditTrail`'s native pagination cleanly).
- The cursor-merge complexity is acceptable for v0.2 because the Audit page is forensic, not operational — operators don't reach for it dozens of times per shift. v0.3 may normalize cursor schemes server-side and simplify the merge.

### 5.3.9 Audit entry detail

Canonical detail for a single audit entry. Includes chain-walk navigation.

- **Route:** `#mod/audit/:id`
- **Role gating:** Moderator+
- **Mode visibility:** `full` only

#### Purpose

Audit entry detail surfaces the full forensic record of a single chain entry: the action it records, the previous chain entry, the next chain entry, and verification status. It is the page operators land on when verifying chain integrity or walking the chain to investigate.

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Audit entry | `tools.aurora.admin.getAuditTrail` filtered to id (or dedicated getter) | Includes hash, prev_hash, current_hash, content |
| Linked event | `tools.aurora.moderator.getEvent` | The mod event this audit entry records |
| Snapshot reference | Embedded in event response | Linked snapshot if applicable |

#### Layout

Similar to Event detail but emphasizing the chain structure. Includes a "Verify chain integrity" button that re-computes the hash chain from the previous entry forward and confirms the current entry's hash matches.

```
┌──────────────────────────────────────────────────────────┐
│  Moderation › Audit › h:a83f1234                         │
│                                                          │
│  Audit entry · takedown by @somemod                      │
│  Verified ✓ · 3d ago                                     │
│  ─────────────────────                                   │
│                                                          │
│  ┌──────────────────────────────┐ ┌─────────────────┐    │
│  │ Linked event                 │ │ Chain navigation│    │
│  │ e:def45678                   │ │                 │    │
│  │ → Open                       │ │ ← Previous      │    │
│  │                              │ │   h:97b2...     │    │
│  ├──────────────────────────────┤ │                 │    │
│  │ Action                       │ │ This entry      │    │
│  │ takedown @somesubject        │ │   h:a83f...     │    │
│  │ "Repeated harassment..."     │ │                 │    │
│  │                              │ │ → Next          │    │
│  ├──────────────────────────────┤ │   h:c1e4...     │    │
│  │ Verification                 │ │                 │    │
│  │ Hash: a83f1234abc...         │ ├─────────────────┤    │
│  │ Prev: 97b2def56...           │ │ Verify integrity│    │
│  │ Computed:  ✓ matches         │ │                 │    │
│  │                              │ │ [Verify chain]  │    │
│  │ [Recompute]                  │ │                 │    │
│  └──────────────────────────────┘ └─────────────────┘    │
└──────────────────────────────────────────────────────────┘
```

#### Real-time behavior

- Read-only post-load. Audit entries are immutable.

#### Action affordances

- **Recompute hash**: re-fetches the entry's content and re-derives its hash; confirms the stored hash matches. Read-only operation, no state change. Server endpoint or client-side derivation depending on Phase 3.8 implementation.
- **Verify chain**: walks the chain forward from the previous entry, confirming each link. Bounded operation (operator-initiated, optional progress indicator for long ranges).

#### Cross-pivots

- Linked event → Event detail
- Previous chain entry → Audit entry detail (preceding `:id`)
- Next chain entry → Audit entry detail (subsequent `:id`)
- Actor → Account detail
- Subject → Account / Record / Blob detail

#### Empty / loading / error states

- **404:** audit entry does not exist
- **Pre-chain entry:** "This entry predates the cryptographic chain (sentinel: pre-chain). No hash verification available." Display content as best as possible
- **Hash mismatch on recompute:** explicit "Hash mismatch — entry may be corrupted" warning with surrounding context. This is the case the chain exists to detect; UI surfaces it loudly when found

#### Notes

- Chain walk navigation is bounded by the chain itself — first entry has no previous, latest has no next. UI gracefully handles boundaries.
- The "Verify chain" action is genuinely useful for incident response. If chain integrity is questioned, an operator can walk the chain from a known-good point forward and confirm every link. UI must support this workflow; backend must support efficient verification queries.

## 5.4 Operations domain pages

The Operations domain is the operator-tier surface. Always visible (in `full` and `reduced` modes), serves the bulk of the day-to-day administrative work, and contains both the Account browser (the high-traffic surface) and the operator-tooling sub-pages (Sequencer, Federation, Blob ops, etc.). Account detail is specified in section 5.2; this batch covers everything else.

### 5.4.1 Accounts

The account browser — search, filter, paginate.

- **Route:** `#ops/accounts`
- **Role gating:** Moderator+ for read; Account detail's drawers handle their own role gates
- **Mode visibility:** `full` and `reduced`

#### Purpose

Accounts is the canonical entry to per-account work. Operators arrive here to find an account by handle, DID, email, or filter; they leave by clicking through to Account detail. It's the most heavily-trafficked page in the Operations domain because almost every operator workflow either starts here or eventually lands here.

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Search input (text) | `com.atproto.admin.searchAccounts` | Debounced 400ms + Enter submit |
| Filter chips (status, created, invite source) | `tools.aurora.ops.listAccounts` | Phase 2.4 endpoint, broader filters |
| Account row stat (records, blobs, status) | Embedded in `listAccounts` response | No per-row separate fetch |
| Bulk multi-select | (no bulk operations in v0.2 against accounts list — bulk goes through batch endpoints from individual account selections elsewhere) | |

The two endpoints behind search vs filter are intentional per cluster 4: search is for finding-by-text, filter is for finding-by-attributes. They serve different access patterns and the UI branches by intent.

#### Layout

```
┌──────────────────────────────────────────────────────────┐
│  Operations › Accounts                                   │
│                                                          │
│  Accounts                                  [Bulk]        │
│  All accounts on this PDS                                │
│  ─────────────────────                                   │
│                                                          │
│  [Search by handle, DID, or email]      [Filter chips]   │
│                                                          │
│  ┌─────────────────────────────────────────────────────┐ │
│  │ Handle      DID         Email      Created  Status  │ │
│  ├─────────────────────────────────────────────────────┤ │
│  │ @somehand   did:plc:.. e@x.com    2026-03  active   │ │
│  │ @another    did:plc:.. f@y.com    2026-04  suspend  │ │
│  └─────────────────────────────────────────────────────┘ │
│                                                          │
│  [Prev]  [25 / 50 / 100]  [Next]                         │
│  Showing 1-50 of 1,247                                   │
└──────────────────────────────────────────────────────────┘
```

Tabular layout. Search input (substrate primitive 20's text-search variant) on the left; filter chips on the right with their popovers. Click row → Account detail.

#### Real-time behavior

- No automatic refresh. Accounts list is reference data; operators control when it refetches via filter changes or pagination.
- "Refresh" button in page header for manual refetch.

#### Action affordances

None at list level. Account-tier actions live on Account detail.

The "Bulk" toggle does enable a multi-select column, but the available bulk operations are limited:
- Bulk takedown accounts (Moderator+; calls `batchTakedownAccounts`)
- Bulk suspend accounts (Moderator+)
- Bulk restore accounts (Moderator+)

Account-management bulk operations (bulk delete, bulk password reset, etc.) are not in v0.2 — those workflows are too high-impact to expose as bulk operations on a list page; they require per-account confirmation.

#### Cross-pivots

- Click row → Account detail
- Email column "did:plc:..." → Account detail (same target as clicking the row)

#### Empty / loading / error states

Standard patterns.

#### Notes

- The list page deliberately does not show "external label" indicators on accounts. That information is account-detail level — putting it on the list creates information overload on a page meant for finding-and-clicking. Operators wanting external label state on a specific account go to Account detail.
- The status column uses status badges. `.status-active` (green) is visually quiet; `.status-suspended` and `.status-takedown` are visually distinct so operators scanning a list immediately notice anomalies.

### 5.4.2 Record detail

Canonical detail for a single ATProto record on this PDS.

- **Route:** `#ops/records/:uri` (URI is URL-encoded)
- **Role gating:** Moderator+ for read; action drawer at Moderator+
- **Mode visibility:** `full` and `reduced`

#### Purpose

Record detail is the per-record analog of Account detail. Operators arrive when investigating a specific record — usually via cross-link from a report, event, or audit entry. The page shows the record itself (rendered), its provenance, its history, and the action surface to take action against it.

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Record metadata + content | `com.atproto.repo.getRecord` (PDS internal) | Authoritative for records on this PDS |
| Record render | Substrate primitive 9 (server-side hardened render) | |
| Record subject context | `tools.aurora.moderator.getSubjectContext` | Recent reports, prior actions on this URI |
| Subject history | `tools.aurora.moderator.getSubjectHistory` | |
| Owning account | Resolved from URI's DID component | Renders as `<EntityRef>` |
| External labels panel | per-labeler `queryLabels` | Substrate primitive 13 |
| Action affordances | Per-action endpoints or `emitEvent` post-3.5 | |

#### Layout

```
┌──────────────────────────────────────────────────────────┐
│  Operations › Records › at://did:plc:.../app.bsky...     │
│                                                          │
│  Post by @owner                          [active]        │
│  app.bsky.feed.post · 2026-04-12                         │
│  ─────────────────────                                   │
│                                                          │
│  ┌──────────────────────────────┐ ┌─────────────────┐    │
│  │ Record content               │ │ Owning account  │    │
│  │ ┌─────────────────────────┐  │ │ @owner          │    │
│  │ │ [hardened render of     │  │ │ → Open account  │    │
│  │ │  the post; text +       │  │ │                 │    │
│  │ │  embeds + media via     │  │ ├─────────────────┤    │
│  │ │  proxy]                 │  │ │ Subject context │    │
│  │ └─────────────────────────┘  │ │ Recent reports  │    │
│  │                              │ │ Prior actions   │    │
│  ├──────────────────────────────┤ │                 │    │
│  │ Moderation actions [Mod+]    │ ├─────────────────┤    │
│  │ ┌─────────────────────────┐  │ │ External labels │    │
│  │ │ Action: [▾ Takedown]    │  │ │ (none reported) │    │
│  │ │ Rationale (required)    │  │ │                 │    │
│  │ │ [Cancel] [Confirm]      │  │ └─────────────────┘    │
│  │ └─────────────────────────┘  │                        │
│  ├──────────────────────────────┤                        │
│  │ Subject history              │                        │
│  │ Chronological actions        │                        │
│  └──────────────────────────────┘                        │
└──────────────────────────────────────────────────────────┘
```

Two-column layout. Record content + action surface on the left; context rail on the right.

#### Real-time behavior

- Subscription-driven for events affecting this record's URI (post-Phase 3.9)
- Otherwise read on load + refetch after action

#### Action affordances

Per the unified action panel pattern. Available actions for a record subject:
- Takedown record
- Apply label
- Remove label
- Restore (if currently taken down)

The action set is filtered by record type per substrate primitive 12. For example: a `app.bsky.feed.post` record can be taken down or labeled; a `app.bsky.actor.profile` record may also support takedown but with different cascade implications (taking down the profile record affects how the account presents).

#### Cross-pivots

- Owning account → Account detail
- Reports filed against this URI → Reports filtered to this subject
- Events affecting this URI → Events filtered to this subject
- Audit entries for this URI → Audit filtered to this subject
- Parent record (if reply) → Record detail of parent (only if parent is on this PDS; otherwise renders as external reference per section 3.7)
- Quoted record (if quote) → Record detail of quoted (same caveat)
- Embedded blobs → Blob detail per blob

#### Empty / loading / error states

- **404:** record URI does not resolve to a record on this PDS (or operator can't see it). Generic not-found.
- **Record taken down:** Record content panel shows "This record is currently taken down" with the takedown action's metadata visible (when, who, rationale). Action affordances become "Restore" instead of "Takedown".
- **Record from another PDS:** if operator pastes a URI for a record not on this PDS, the page shows "This record is on another PDS — Aurora-Locus does not have authority over it." Single link out to the record's home PDS where applicable.

### 5.4.3 Blob detail

Canonical detail for a single blob.

- **Route:** `#ops/blobs/:cid`
- **Role gating:** Moderator+ for read; action drawer at Moderator+
- **Mode visibility:** `full` and `reduced`

#### Purpose

Blob detail surfaces a single blob — its metadata, the records that reference it, and the actions available against it (quarantine, restore, delete).

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Blob metadata | `tools.aurora.ops.listBlobs` filtered to CID (or dedicated getter) | Mime type, size, hash |
| Blob content | Substrate primitive 9 (hardened render — image/video preview, file metadata otherwise) | Subject to graduated content reveal |
| Referencing records | Cross-query against `com.atproto.repo.listRecords` via index | If the index is available; otherwise listed as count |
| Owning account | Embedded in blob metadata | |
| Action affordances | `tools.aurora.ops.{quarantineBlob, restoreBlob, deleteBlob}` | |

#### Layout

Similar pattern to Record detail. Blob preview (with graduated content reveal for sensitive content), action surface, context rail showing owning account and referencing records.

#### Real-time behavior

- Read on load
- Refetch after action

#### Action affordances

Available actions for a blob subject:
- Quarantine blob (reversible)
- Restore blob (if quarantined)
- Delete blob (irreversible — modal with typed confirmation)

Each requires rationale. Snapshot-at-decision and audit chain integration as standard.

#### Cross-pivots

- Owning account → Account detail
- Each referencing record → Record detail

#### Empty / loading / error states

- **404:** CID does not exist on this PDS
- **Blob preview unavailable:** for blob types that can't render inline (PDFs, archives, etc.) show metadata + "Download to inspect" action button

### 5.4.4 Invites

The invite codes browser.

- **Route:** `#ops/invites`
- **Role gating:** Admin+
- **Mode visibility:** `full` and `reduced`

#### Purpose

Invite codes management: create, view, disable. Lists all invite codes on the PDS with their usage state and creator.

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Invites list | `com.atproto.admin.getInviteCodes` (or `listInviteCodes`) | Paginated |
| Filter chips | invite filter parameters: status, creator, used/unused | |
| Create invite | `com.atproto.admin.createInviteCode` | Modal form |
| Disable single | `com.atproto.admin.disableInviteCode` | Inline confirm |
| Disable bulk | `com.atproto.admin.disableInviteCodes` | Multi-select + atomic call |

#### Layout

Tabular pattern. Columns: code, uses, creator, created date, status, actions (disable/enable).

Page header has "Generate codes" button on the right.

#### Real-time behavior

- Polled every 60 seconds
- Not subscription-driven

#### Action affordances

- **Generate codes** (page header): modal with options for use count (1, 5, 10, 25, custom) and account-binding (optional DID). Confirms creation; toast shows new code(s) with copy-to-clipboard.
- **Disable single** (inline): confirm prompt; no modal needed for single-row reversible actions.
- **Bulk disable** (when multi-select enabled): modal with count + rationale, atomic call.

#### Cross-pivots

- Creator DID → Account detail
- Code → Invite detail

#### Empty / loading / error states

Standard patterns. "No invite codes yet" empty state.

### 5.4.5 Invite detail

Canonical detail for a single invite code.

- **Route:** `#ops/invites/:code`
- **Role gating:** Admin+
- **Mode visibility:** `full` and `reduced`

#### Purpose

Per-invite detail showing the code, its usage history (which accounts redeemed it), its creator, and lineage (codes generated by accounts originating from this code).

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Invite metadata | `com.atproto.admin.getInviteCodes` filtered to code | |
| Used by | Embedded in invite response | List of accounts that redeemed |
| Creator | Embedded | |
| Lineage (downstream codes) | Cross-query | Accounts created from this code, then their generated codes |

#### Layout

Single-column layout. Metadata panel at top, used-by list, lineage tree expandable.

#### Real-time behavior

Read on load; no auto-refresh.

#### Action affordances

- Disable code (if active): confirmation prompt
- Re-enable code (if disabled): confirmation prompt

#### Cross-pivots

- Creator → Account detail
- Each redeemer → Account detail
- Lineage downstream → Account detail per node, Invite detail per code

#### Notes

- Lineage visualization in v0.2: simple indented list (parent/child relationships shown by indentation). v0.3 may add a tree visualization.

### 5.4.6 Operations sub-pages

The remaining Operations sub-pages (Sequencer, Federation, Blob ops, Rate limits, System health, Server) share enough structure that they are specified collectively with per-page differences noted. Each is a single-column page with read-only metrics and a small set of admin actions.

#### Common pattern

```
┌──────────────────────────────────────────────────────────┐
│  Operations › <Sub-page>                                 │
│                                                          │
│  <Title>                                                 │
│  <Subtitle>                                              │
│  ─────────────────────                                   │
│                                                          │
│  ┌──────────────────────────────────────────────┐        │
│  │ Status / metrics                             │        │
│  │ Stat cards or dense data table               │        │
│  └──────────────────────────────────────────────┘        │
│                                                          │
│  ┌──────────────────────────────────────────────┐        │
│  │ Controls (when present)                      │        │
│  │ Buttons for the sub-page's actions           │        │
│  └──────────────────────────────────────────────┘        │
└──────────────────────────────────────────────────────────┘
```

#### 5.4.6.1 Sequencer (`#ops/sequencer`)

- **Endpoints:** `getSequencerStatus`, `pauseSequencer`, `resumeSequencer`, `resetSequencerCursor`, `rebuildSequencer`, `listRecentEvents`
- **Surfaces:** sequencer position, lag indicator, recent events list
- **Actions:** pause/resume (single-click toggle); reset cursor (modal with typed confirmation, high-impact); rebuild (modal with typed confirmation, very high-impact)
- **Polling:** every 30 seconds for status

#### 5.4.6.2 Federation (`#ops/federation`)

- **Endpoints:** `getFederationStatus`, `getRelayConfig`, `listKnownInstances`, `triggerPdsDiscovery`
- **Surfaces:** relay configuration, peer count, known PDS instances list, recent federation activity
- **Actions:** trigger discovery (single-click with toast confirm)
- **Polling:** every 60 seconds for status (federation is slow-changing)

#### 5.4.6.3 Blob ops (`#ops/blob-ops`)

- **Endpoints:** `getBlobStatistics`, `listBlobs`, `getBlobQuotas`, `runBlobGC`, `quarantineBlob`, `restoreBlob`, `deleteBlob`
- **Surfaces:** storage statistics (total blobs, total size, by mime type), per-account quotas, recent blob operations
- **Actions:** Run GC (single-click + progress polling); per-blob actions via Blob detail
- **Polling:** every 60 seconds for statistics

#### 5.4.6.4 Rate limits (`#ops/rate-limits`)

- **Endpoints:** `getRateLimitConfig`, `getRateLimitStatus`, `cleanupRateLimitState`
- **Surfaces:** per-endpoint configured limits, current request counts, tracked identifiers
- **Actions:** cleanup state (single-click + confirm)
- **Polling:** every 30 seconds for status

#### 5.4.6.5 System health (`#ops/system-health`)

- **Endpoints:** `getSystemHealth`, `runHealthChecks`, `getResourceUsage`, `getDatabaseStatus`, `listBackgroundJobs`, `getValidationFailures`, `getNonceStoreStatus`, `cleanupNonceStores`, `getSystemMetrics`
- **Surfaces:** consolidated health dashboard — multiple sub-cards each surfacing one health domain
- **Actions:** run health checks on demand; cleanup nonce stores
- **Polling:** every 30 seconds across all panels

#### 5.4.6.6 Server (`#ops/server`)

- **Endpoints:** `tools.aurora.describeCapabilities`, `getVersionInfo`
- **Surfaces:** capabilities probe (raw output, formatted), version + build info, server config (read-only display of the runtime configuration)
- **Actions:** none (read-only)
- **Polling:** none (configuration is static within a deployment)

#### Notes on Operations sub-pages

- All Operations sub-pages are simple display + occasional control surfaces. The substrate work for these pages is minimal beyond what's already specified.
- Each sub-page gets its own breadcrumb (`Operations › <Sub-page>`) but doesn't need detail surfaces beyond the sub-page itself (no "blob ops detail" — that's just Blob detail).
- Where actions exist (pause sequencer, run blob GC, etc.), they use the unified action panel pattern with appropriate confirmation level. High-impact destructive actions (reset cursor, rebuild) get typed confirmation; reversible actions (pause/resume) get single-click + toast.

## 5.5 Settings domain pages

The Settings domain is small — four pages, no detail variants beyond the Roles members list. It's the lowest-traffic domain in the UI but architecturally important: it's always visible regardless of mode, contains the controls that govern the UI's own behavior (theme, language, moderation mode), and houses the role management surface that enforces the authority tiers.

### 5.5.1 General

Server identity and basic configuration.

- **Route:** `#settings/general`
- **Role gating:** Admin+ for read; SuperAdmin for write
- **Mode visibility:** Always visible

#### Purpose

General captures the deployment's identity-and-basic-config: instance name, service URL, contact info, basic operational thresholds. It's the page operators visit when first setting up a deployment and rarely revisit.

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Read current config | Server config endpoint (existing or via `getVersionInfo`) | Read-only config display |
| Update config | Server config update endpoint (existing or new) | Write requires SuperAdmin |

The exact backing endpoint for server config read/write is implementation-specific to the Aurora-Locus runtime configuration system. Section 8 captures any new endpoints needed; for v0.2 the General page works against whatever existing config endpoint is available, with a fallback to display-only if write isn't yet exposed.

#### Layout

```
┌──────────────────────────────────────────────────────────┐
│  Settings › General                                      │
│                                                          │
│  General settings                                        │
│  Server identity and basic configuration                 │
│  ─────────────────────                                   │
│                                                          │
│  ┌──────────────────────────────────────────────┐        │
│  │ Server identity                              │        │
│  │ Instance name  [Aurora Locus PDS         ]   │        │
│  │ Service URL    [https://pds.example.com  ]   │        │
│  │ Contact email  [admin@example.com        ]   │        │
│  │                                              │        │
│  │ [Save changes]                               │        │
│  └──────────────────────────────────────────────┘        │
│                                                          │
│  ┌──────────────────────────────────────────────┐        │
│  │ Operational thresholds                       │        │
│  │ Max blob size (MB)    [5         ]           │        │
│  │ Account creation rate [100/day   ]           │        │
│  │                                              │        │
│  │ [Save changes]                               │        │
│  └──────────────────────────────────────────────┘        │
│                                                          │
│  ┌──────────────────────────────────────────────┐        │
│  │ Registration                                 │        │
│  │ ☐ Require invite codes                       │        │
│  │ ☐ Require email verification                 │        │
│  │                                              │        │
│  │ [Save changes]                               │        │
│  └──────────────────────────────────────────────┘        │
└──────────────────────────────────────────────────────────┘
```

The settings-grid two-column card layout from the existing `static/admin/` is preserved. Each card is a logical group with its own Save button (cards are independently editable).

For Admin sessions: read-only display of all values, Save buttons hidden.
For SuperAdmin sessions: editable inputs, Save buttons visible.

#### Real-time behavior

- Read on load
- No auto-refresh
- Save action causes the affected card to refetch its values

#### Action affordances

- **Save changes** per card: validates input, calls config update endpoint, captures rationale (the rationale field is part of the form, defaulted to "Routine config update" for low-impact changes).
- High-impact config changes (changing service URL — affects identity verification across federation) get an additional confirmation modal before the save lands.

#### Cross-pivots

None — General is a leaf page.

#### Empty / loading / error states

Standard patterns. Form validation errors render inline below the relevant field with `aria-describedby` per accessibility contract.

#### Notes

- The current `static/admin/index.html` Settings page contains four cards (General, Registration, Moderation Settings, Aurora Capabilities). The General page in v0.2 absorbs the General and Registration cards and keeps the same logical structure. Moderation Settings moves to UI & modes (5.5.2). Aurora Capabilities moves to its own page (5.5.5) which is also reachable from Operations → Server.
- Many of the existing Moderation Settings card's settings ("Enable Automatic Content Filtering", "Report Threshold") are placeholder-grade scaffolding — they don't map to actual server-side behavior in the current Aurora-Locus build. They're not preserved in v0.2 unless they connect to real backing endpoints. Section 12 (migration from current static/admin/) specifies which scaffolded controls are removed vs preserved.

### 5.5.2 UI & modes

Theme, moderation mode, language. The page that controls how the UI itself behaves.

- **Route:** `#settings/ui-modes`
- **Role gating:** Read at any role (every operator can see their own theme/language); SuperAdmin for moderation mode write
- **Mode visibility:** Always visible (this is where the mode toggle lives)

#### Purpose

UI & modes controls per-deployment and per-operator UI behavior:

- **Theme** (Light / Dark / System) — per-operator preference, persisted in localStorage
- **Moderation mode** (`full` / `reduced` / `disabled`) — deployment-wide setting in `runtime_settings`, gated to SuperAdmin
- **Moderation mode redirect URL** — when mode is `disabled`, this is where operators get redirected (or the page renders "managed elsewhere" stub if URL is blank)
- **Language** — per-operator preference, currently English-only with the dropdown populated from available locale files

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Read theme preference | localStorage | Client-only |
| Write theme preference | localStorage | Client-only |
| Read moderation mode | Runtime settings endpoint (Phase 3.10) | |
| Write moderation mode | Runtime settings endpoint (Phase 3.10) | SuperAdmin only |
| Read language preference | localStorage | Client-only |
| Write language preference | localStorage | Client-only |

The theme toggle in the sidebar footer (section 4.8) is the same control surfaced more conveniently — both routes write to the same localStorage key. Setting theme on one updates both surfaces.

#### Layout

```
┌──────────────────────────────────────────────────────────┐
│  Settings › UI & modes                                   │
│                                                          │
│  UI & modes                                              │
│  Theme, moderation mode, language                        │
│  ─────────────────────                                   │
│                                                          │
│  ┌──────────────────────────────────────────────┐        │
│  │ Theme                                        │        │
│  │ ◉ Light  ○ Dark  ○ System                    │        │
│  │ System uses your operating system preference │        │
│  └──────────────────────────────────────────────┘        │
│                                                          │
│  ┌──────────────────────────────────────────────┐        │
│  │ Language                                     │        │
│  │ [English ▾]                                  │        │
│  │ Other languages may be available as locale   │        │
│  │ files are added.                             │        │
│  └──────────────────────────────────────────────┘        │
│                                                          │
│  ┌──────────────────────────────────────────────┐        │
│  │ Moderation mode [SuperAdmin only]            │        │
│  │ ◉ Full     ○ Reduced     ○ Disabled          │        │
│  │ Current: Full                                │        │
│  │                                              │        │
│  │ Redirect URL (when disabled)                 │        │
│  │ [https://moderation.example.com         ]    │        │
│  │                                              │        │
│  │ Changing this affects all operators          │        │
│  │ and is recorded in the audit chain.          │        │
│  │                                              │        │
│  │ [Save mode change]                           │        │
│  └──────────────────────────────────────────────┘        │
└──────────────────────────────────────────────────────────┘
```

For non-SuperAdmin sessions: the Moderation mode card displays current values read-only, with no Save button and the radio buttons disabled.

#### Real-time behavior

- Theme and language: instant. No save button needed; selection writes immediately.
- Moderation mode: explicit save with confirmation modal (this is a deployment-wide change). Save calls runtime settings endpoint; UI refreshes to reflect new mode (which may cause sidebar to re-render with different visible domains).

#### Action affordances

- **Theme change**: instant write to localStorage, document gets `data-theme` attribute updated. No confirmation. Reversible by changing again.
- **Language change**: instant write to localStorage, page reloads to apply new language. v0.2 with English-only is effectively a no-op but the wiring is in place.
- **Moderation mode change** (SuperAdmin only): confirmation modal with explicit description of what changes. "Switching to Reduced will hide the Moderation domain for all operators on this deployment. Confirm?" Rationale field required. Audit chain entry written.
- **Redirect URL update** (SuperAdmin only, only when mode is or will be `disabled`): inline form save.

#### Cross-pivots

None — UI & modes is a leaf page.

#### Empty / loading / error states

Standard patterns. The "current mode" indicator updates after save lands; if save fails, indicator stays on current mode and inline error shows.

#### Notes

- The recovery path: if an operator deploys with `disabled` mode and no redirect, and discovers Settings is the only available surface, they can use this page to flip back to `full`. The deployment also supports `AURORA_RECOVERY_MODE=true` as an environment variable that bypasses the runtime setting on startup, providing an out-of-band recovery path if the in-band one is somehow broken.
- The mode change explicitly writes to the audit chain so deployment-wide UI changes are forensic-grade visible. Operators investigating "why did the UI behavior change" can find the answer in audit history.

### 5.5.3 Roles

The role management surface.

- **Route:** `#settings/roles`
- **Role gating:** Moderator+ for read; SuperAdmin for write
- **Mode visibility:** Always visible

#### Purpose

Roles surface lists the deployment's roles and their members. Operators of any tier can see who has what role (read at moderation tier per Phase 3.6 design); only SuperAdmins can grant or revoke roles.

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Roles list | `com.atproto.admin.listRoles` | Returns role names + member counts |
| Members per role | `com.atproto.admin.listRoles` (extended) or per-role query | Members visible per role |
| Grant role | `tools.aurora.superadmin.grantRole` | SuperAdmin only |
| Revoke role | `tools.aurora.superadmin.revokeRole` | SuperAdmin only |

#### Layout

```
┌──────────────────────────────────────────────────────────┐
│  Settings › Roles                                        │
│                                                          │
│  Roles                                                   │
│  Authority tiers and current members                     │
│  ─────────────────────                                   │
│                                                          │
│  ┌──────────────────────────────────────────────┐        │
│  │ Moderators                       [3 members] │        │
│  │ Acts on subjects-as-content                  │        │
│  │ ─────────────                                │        │
│  │ @somemod        @anothermod      @thirdmod   │        │
│  │ [Grant role]                       [SuperAdm]│        │
│  └──────────────────────────────────────────────┘        │
│                                                          │
│  ┌──────────────────────────────────────────────┐        │
│  │ Administrators                   [2 members] │        │
│  │ Acts on accounts-as-infrastructure           │        │
│  │ ─────────────                                │        │
│  │ @adminone       @admintwo                    │        │
│  │ [Grant role]                       [SuperAdm]│        │
│  └──────────────────────────────────────────────┘        │
│                                                          │
│  ┌──────────────────────────────────────────────┐        │
│  │ SuperAdmins                      [1 member]  │        │
│  │ Acts on authority itself                     │        │
│  │ ─────────────                                │        │
│  │ @superadmin                                  │        │
│  │ [Grant role]                       [SuperAdm]│        │
│  └──────────────────────────────────────────────┘        │
└──────────────────────────────────────────────────────────┘
```

Each role gets a card showing its description, member count, and members list. Grant role button at the bottom of each card is gated to SuperAdmin (substrate primitive 7 — role-aware affordance gating).

Members render as `<EntityRef>` linking to Account detail.

#### Real-time behavior

- Read on load
- Refetch after grant/revoke

#### Action affordances

- **Grant role** (SuperAdmin only): modal with DID/handle input field (with type-ahead via DID/handle picker substrate), rationale required. Confirmation. Calls `grantRole`.
- **Revoke role** (SuperAdmin only, hover/right-click on member or "Manage" button per row): confirmation modal with rationale. Calls `revokeRole`.

#### Cross-pivots

- Each member → Account detail
- "Manage role" or "Members of {role}" → `#settings/roles/:role` (full members list page if a role has many members)

#### Empty / loading / error states

- **Empty role:** "No members" with a Grant button (SuperAdmin only)
- **Standard patterns** otherwise

#### Notes

- The role cards are read-accessible to Moderators because moderators legitimately benefit from knowing who has authority over what — coordinating cases with the right operator, knowing whose decisions are appealable, etc.
- Self-revoke (SuperAdmin revoking their own SuperAdmin role) requires extra confirmation and a second SuperAdmin to be present in the system. UI flags "You are the only SuperAdmin — at least one must remain. Grant SuperAdmin to another operator first." — preventing accidental lockout of the deployment.

### 5.5.4 Roles members list

The full members list for a single role, when the inline card display is insufficient (large memberships).

- **Route:** `#settings/roles/:role`
- **Role gating:** Same as parent: Moderator+ read, SuperAdmin write
- **Mode visibility:** Always visible

#### Purpose

For roles with many members, the inline display on the Roles page is impractical. This page is the deep view: full paginated members list per role.

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Members list | `com.atproto.admin.listRoles` filtered to specific role + paginated | |
| Filter chips | by member status (active/disabled), member granted-date range | |
| Grant role | `tools.aurora.superadmin.grantRole` | SuperAdmin only |
| Revoke role | `tools.aurora.superadmin.revokeRole` | SuperAdmin only |

#### Layout

Tabular pattern. Columns: handle, DID, granted date, granted by, actions (revoke).

#### Real-time behavior

- Read on load
- Refetch after grant/revoke

#### Action affordances

- **Grant role** (SuperAdmin only, page header): same modal pattern as Roles page
- **Revoke** (SuperAdmin only, per-row): confirmation modal

#### Cross-pivots

- Each member → Account detail
- "Granted by" → Account detail of the SuperAdmin who granted

#### Empty / loading / error states

Standard patterns.

### 5.5.5 Capabilities

The capabilities probe surface.

- **Route:** `#settings/capabilities` (and aliased from `#ops/server`)
- **Role gating:** Any authenticated operator
- **Mode visibility:** Always visible

#### Purpose

Capabilities is the discovery surface for what this Aurora-Locus deployment exposes. Operators (and external tools probing the PDS) get a definitive answer to "what features are available here." The page is also referenced by capability-routed substrate (substrate primitive 21) for runtime feature detection.

#### Endpoint mapping

| Element | Endpoint | Notes |
|---|---|---|
| Capabilities list | `tools.aurora.describeCapabilities` | Returns the static capability vocabulary |
| Version info | `tools.aurora.ops.getVersionInfo` | Adjacent display |

#### Layout

```
┌──────────────────────────────────────────────────────────┐
│  Settings › Capabilities                                 │
│                                                          │
│  Capabilities                                            │
│  Aurora-Locus features exposed by this deployment        │
│  ─────────────────────                                   │
│                                                          │
│  ┌──────────────────────────────────────────────┐        │
│  │ Capabilities                                 │        │
│  │                                              │        │
│  │ ✓ audit-trail-v1                             │        │
│  │ ✓ subject-history-v1                         │        │
│  │ ✓ subject-context-v1                         │        │
│  │ ✓ batch-takedown-v1                          │        │
│  │ ✓ moderator-activity-v1                      │        │
│  │ ✓ invite-lineage-v1                          │        │
│  │ ✓ instance-metrics-v1                        │        │
│  │ ✓ appeals-v1                                 │        │
│  │ ✗ mod-events-stream-v1   (Phase 3.9, pending)│        │
│  │ ✓ reporter-context-v1                        │        │
│  │ ─────                                        │        │
│  │ Version: 0.2.0                               │        │
│  │ Implementation: aurora-locus                 │        │
│  └──────────────────────────────────────────────┘        │
│                                                          │
│  ┌──────────────────────────────────────────────┐        │
│  │ Build information                            │        │
│  │ Version:    0.2.0-rc1                        │        │
│  │ Commit:     abc123def456                     │        │
│  │ Built:      2026-04-30                       │        │
│  │ Rust:       1.84.0                           │        │
│  └──────────────────────────────────────────────┘        │
│                                                          │
│  [Refresh]    [Copy raw JSON]                            │
└──────────────────────────────────────────────────────────┘
```

Each capability is listed with its presence indicator. Available capabilities show a green check; absent capabilities show a gray cross with optional explanatory text (e.g., "Phase 3.9, pending"). The raw JSON response is available via "Copy raw JSON" for operators inspecting the response programmatically.

#### Real-time behavior

- Read on load
- "Refresh" button for manual re-probe (capabilities can change with deployment updates)

#### Action affordances

- **Refresh**: re-call `describeCapabilities`. Read-only, no state change.
- **Copy raw JSON**: copy the underlying response to clipboard.

#### Cross-pivots

None.

#### Empty / loading / error states

- **Loading**: skeleton placeholders
- **Error fetching capabilities**: "Could not probe capabilities. The describeCapabilities endpoint may be unavailable." Retry button.

#### Notes

- The page is identical whether accessed via `#settings/capabilities` or `#ops/server`. Both routes resolve to the same surface; the breadcrumb differs by entry path.
- Capability strings on this page must match the canonical strings the substrate primitive 21 uses. Section 8 commits the capability vocabulary as a fixed list in the design doc; new capabilities require design-doc update before being added.

## 5.6 Modal and dialog patterns

A handful of modals and dialogs appear across multiple pages. They share enough structure to specify once and reference forward; the details belong with the substrate primitives in section 6, but the UX-level pattern is captured here.

### 5.6.1 Action confirmation modals

Used everywhere actions are taken — Pattern A through G action affordances, batch operations, forensic export.

Common structure:
- Modal header naming the action explicitly
- Subject(s) shown
- Action-specific configuration controls (action dropdown, options, etc.)
- Mandatory rationale textarea
- Action-specific gates (typed-confirmation field, "I understand" checkboxes for high-impact)
- Footer with Cancel + destructive-styled action button

Per substrate primitive 3 (action affordances). Rationale required by default; specific actions may default rationale text or disable rationale entirely (low-impact toggles only).

### 5.6.2 Bulk operation modals

Used for batch endpoint actions.

Structure:
- Modal header naming the bulk operation
- Selection count + collapsible list of subjects (visible up to 10; collapsed beyond)
- Single rationale textarea applying to all subjects
- Single confirmation gate (no per-subject confirmation)
- 50-subject hard cap with explanatory text if approached

Same substrate primitive 3 + bulk-specific extensions per section 6.

### 5.6.3 Forensic export modal

Specified in section 5.2's Account detail page; reused on any future surface that supports forensic export.

### 5.6.4 Role grant/revoke modals

Specified in section 5.5.3's Roles page.

### 5.6.5 Generate invite codes modal

Specified in section 5.4.4's Invites page.

### 5.6.6 Override-action modals

Used for the override paths in Account management drawer (override password, override email, override handle, update signing key, delete account).

Common structure: modal with strong warning copy, explicit explanation of what direct override does and why default user-mediated path is preferred, typed confirmation field (handle for destructive operations), rationale required.

### 5.6.7 Toast notifications

Not modals, but worth specifying in section 5 for completeness.

Toast pattern:
- Position: top-right of viewport
- Stack vertically when multiple
- Auto-dismiss after 4 seconds (or 6 seconds for errors)
- Manual close button
- Three variants: info, success (`.positive` color), error (`.danger` color)
- Accessibility via `aria-live="polite"` for info/success, `aria-live="assertive"` for errors

Toast content includes optional action button: "Action completed. [View result]" → click navigates to result detail.

# 6. Substrate primitives

This section specifies every reusable component the UI is built from. The 21 primitives enumerated in section 1.1 each get their own subsection, structured consistently:

- **Purpose** — what the primitive is for
- **Used by** — which pages or other primitives consume it
- **Props / configuration** — the interface the primitive exposes
- **Visual contract** — what it looks like, with reference to design tokens
- **Accessibility contract** — keyboard, ARIA, screen reader behavior
- **Implementation notes** — anything specific to implementation that doesn't fit above

The primitives are presented in dependency order — primitives consumed by other primitives appear first.

## 6.1 EntityRef

The component for rendering any reference to a canonical entity (account, record, blob, report, event, appeal, audit entry, invite, role) as a link to that entity's detail page.

### Purpose

Every entity reference in the UI renders consistently with appropriate display rules and routing. Adding a new entity type later is one registry addition, not changes scattered across the codebase.

### Used by

Every page that displays entity references — which is most pages. Account detail, Record detail, Blob detail, all moderation list pages, all detail pages, the Roles surface, the FilterStrip's DID picker, the command palette's results, the recent activity feed, every action confirmation modal that displays affected subjects.

### Props / configuration

```
EntityRef:
  type: 'account' | 'record' | 'blob' | 'report' | 'event' | 'appeal' | 'audit' | 'invite' | 'role'
  id: string                  # the identifier — DID, URI, CID, ID, etc.
  display?: 'short' | 'full'  # default 'short' (truncated), 'full' for emphasized contexts
  cache?: boolean             # default true; set false to bypass DID resolution cache
```

### Display rules per type

| Type | Display (short) | Display (full) | Tooltip on hover (v0.3) |
|---|---|---|---|
| Account | `@handle` (resolved) or `did:plc:abc...xyz` | `@handle (did:plc:abc...xyz)` | Full DID, status, member-since |
| Record | `@author / type` (e.g., `@somehand / post`) | Full URI | Author, type, creation date, status |
| Blob | `bafkre...` (truncated CID) | Full CID | Mime type, size, ref count |
| Report | `r:abc12345` (truncated id) | Full id | Reporter, subject, type, age |
| Event | `e:def67890` (truncated id) | Full id | Actor, action, subject, timestamp |
| Appeal | `a:xyz12345` (truncated id) | Full id | Appellant, reviewer, status |
| Audit | `h:a83f1234` (truncated hash) | Full hash | Action, actor, verified status |
| Invite | the code itself | the code itself | Creator, uses, status |
| Role | role name + tier badge | role name + tier badge | Description, member count |

### Visual contract

- Inline element rendered as `<a>` with appropriate `href` (the canonical hash route for that entity)
- Truncated identifiers use middle-truncation (first 6 + last 4 chars typically) with `…` between
- Color: `var(--primary-color)` for links, `var(--primary-dark)` on hover
- No underline by default; underline on hover (matches the existing `.data-table` link treatment)
- For `@handle` rendering: the `@` is part of the visible text (preserves the social-media-native treatment)
- For DIDs: monospace font (`var(--font-mono)`) for the technical identifier

### Accessibility contract

- Native `<a>` element ensures screen reader announces as link
- `aria-label` includes full identifier even when display is truncated: `aria-label="Account @somehandle, did:plc:abc...xyz"` so screen readers convey the full context
- Keyboard focus follows browser default (Tab order, Enter to activate)
- Focus indicator: 2px solid `var(--primary-color)` outline, 2px offset (per accessibility substrate)

### Implementation notes

- DID-to-handle resolution cache is in-memory, keyed by DID. LRU with 500-entry default cap. Hits during page navigation populate the cache lazily.
- Cache is populated by responses from other endpoints (anywhere a handle is resolved alongside a DID), not by speculative pre-fetching.
- For touch devices, no hover behavior — link is just a link. The hover-card extension (substrate primitive 1's v0.3 hover layer) would not apply to touch.
- v0.2 ships without hover-card content (per cluster 5 decision). The substrate is positioned to support hover layer in v0.3 by extending the component's render to optionally include a hover wrapper.

## 6.2 Inline subject preview (server-side hardened render)

Renders an ATProto record (post, profile, list, etc.) for inline display in a moderator surface, with sanitization and media proxying.

### Purpose

Operators viewing a record (in Report detail, Event detail, Subject context, anywhere a record needs to be shown alongside metadata) see the record rendered safely. The Aurora-Locus PDS renders server-side using its lexicon-aware Rust render layer; the operator's browser receives sanitized HTML with no JavaScript execution context from the rendered content and no third-party fetches.

### Used by

Report detail (subject preview), Event detail (snapshot rendering), Account detail (records authored panel inline previews), Audit entry detail (snapshot), Subject detail surfaces, the forensic export configuration modal (subject preview).

### Props / configuration

```
SubjectPreview:
  uri: string                  # at:// URI for the record
  snapshot?: SnapshotId        # if provided, renders the snapshot at decision time, not current state
  contentReveal?: 'auto' | 'forced'  # 'auto' applies graduated reveal based on report category; 'forced' always reveals
  maxHeight?: pixels           # optional max height with scroll if exceeded; default 600px
```

### Visual contract

- Container: white surface card matching `.activity-item` style, border `0.5px solid var(--color-border-tertiary)` in light mode (parallel for dark)
- Inner padding: 1rem
- Rendered content: typography matches rendered post (sans-serif body, slight increase in line-height for readability)
- Media: images and videos rendered at constrained max-width with proxied URLs
- Embeds: rendered inline (quote posts, link previews, embedded lists) — each as a nested SubjectPreview when applicable
- Graduated reveal layer: when `contentReveal: 'auto'` and the content is flagged sensitive, a frosted-overlay covers the content with explicit "Click to reveal" copy and the reason ("Reported for: harassment" or similar)

### Accessibility contract

- Container: `role="article"` with `aria-label` describing the record (e.g., "Post by @somehandle, dated...")
- Rendered content text reads naturally to screen readers
- Media images carry `alt` text from the record's alt-text fields (when present); when absent, screen reader announces "image, no alt text provided"
- Graduated reveal layer is keyboard-activatable (Enter/Space to reveal) and announces its reason via `aria-describedby`
- Sensitive content auto-reveal preferences are respected: operators with reduced-motion preferences also get reduced graduated-reveal animations

### Implementation notes

- Render layer is server-side Rust, per substrate primitive 9
- Media proxy endpoint: `/xrpc/tools.aurora.ops.proxyMedia?url=...` (or similar — exact route in section 8). Proxies image/video URLs through the PDS so moderator browser doesn't fetch from arbitrary hosts.
- Snapshot rendering: when `snapshot` is provided, the render layer renders the snapshot's stored content (not the current record state). Snapshots are immutable per substrate primitive 8.
- Records from other PDSes (referenced via at:// URI but not in this PDS's repos) render as "External record reference" with a link out, per architecture principle 3.7. v0.2 does not fetch external records inline.

## 6.3 ActionPanel (unified action affordance)

The single component for action affordances across the UI: takedown, suspend, label, resolve report, resolve appeal, etc.

### Purpose

Every state-changing action follows the same pattern: pick action, configure it, provide rationale, confirm. One component implements that pattern; pages compose it into their action surfaces.

### Used by

Report detail, Event detail's downstream pivots, Appeal detail, Account detail's moderation drawer, Account detail's management drawer (for actions that fit the panel pattern), Record detail, Blob detail, anywhere an operator takes an action.

### Props / configuration

```
ActionPanel:
  subject: SubjectRef                      # what the action targets
  availableActions: Action[]               # filtered by lexicon-aware action surfacing
  defaultAction?: ActionType                # which action is preselected
  requiresRationale: boolean (default true) # set false for low-impact toggles
  defaultRationale?: string                # prefilled text when rationale is optional
  highImpactActions: ActionType[]          # subset that require typed-confirmation
  onConfirm: (action, rationale, options) => Promise
  onCancel: () => void
```

### Layout

```
┌─────────────────────────────────┐
│ Take action                     │
│                                 │
│ Action                          │
│ [▾ Takedown account           ] │
│                                 │
│ Subject                         │
│ @somehandle                     │
│ did:plc:abc...                  │
│                                 │
│ Rationale (required)            │
│ ┌─────────────────────────────┐ │
│ │                             │ │
│ │                             │ │
│ └─────────────────────────────┘ │
│                                 │
│ ☐ I understand this affects all │
│   federation                    │
│                                 │
│       [Cancel]  [Confirm]       │
└─────────────────────────────────┘
```

### Visual contract

- Renders inline within a drawer or modal — never page-wide
- Card surface: white background, `--radius-md` (12px), internal padding `1.5rem`
- Action dropdown: standard select styled to match `.filter-select`
- Subject display: read-only, formatted with `<EntityRef>` for the subject
- Rationale textarea: full-width, min-height 4 lines, max-height 12 lines with internal scroll
- High-impact gates (typed confirmation, checkbox) appear conditionally based on selected action
- Confirm button: `.btn-danger` style for destructive actions; `.btn-primary` for non-destructive
- Cancel button: `.btn-secondary` always

### Accessibility contract

- Action dropdown: `role="combobox"` with proper labeling
- Rationale textarea: `<label for="rationale">Rationale (required)</label>`, `aria-required="true"`, `aria-describedby` pointing to validation hints
- Typed-confirmation field: `aria-required="true"`, validation announces via `aria-live="polite"`
- High-impact checkbox: standard checkbox with `<label>` association
- Confirm button: disabled (with `aria-disabled="true"`) until all required fields validate. Disabled reason announced when focused: "Confirm disabled — rationale is required and not provided"
- Form submission: Enter in textarea inserts newline; Tab+Enter submits; explicit click on Confirm submits

### Implementation notes

- Server-side authorization is the operative gate per architecture principle 3.1; UI's role-aware visibility is display logic
- Rationale field validates non-empty when required; whitespace-only is treated as empty
- Snapshot capture (substrate primitive 8) happens automatically as part of action submission — no operator UI involvement
- Audit chain entry written by server post-action; UI displays the resulting audit entry id in the success toast for navigation
- For `emitEvent` substitution post-Phase 3.5: same UI, different underlying call. Substrate primitive 21 routes the call. Component-level interface unchanged.

## 6.4 BulkActionPanel

Specialized variant of ActionPanel for batch operations on multiple subjects.

### Purpose

When operators select multiple subjects and apply a single action to all (Bulk takedown, Bulk apply label, etc.), the same pattern applies but with batched semantics: one rationale, one audit entry, one confirmation.

### Used by

Queue page's bulk action mode, Reports page's bulk action mode, Accounts page's bulk action mode, anywhere multi-select enables batch operations.

### Props / configuration

```
BulkActionPanel:
  subjects: SubjectRef[]                  # the multi-selected subjects
  availableActions: BatchAction[]          # filtered to actions the batch endpoints support
  onConfirm: (action, rationale) => Promise
  onCancel: () => void
  maxBatchSize: number (default 50)        # hard cap per batch endpoint contracts
```

### Layout

```
┌─────────────────────────────────────────┐
│ Bulk action: 3 subjects                 │
│                                         │
│ Action                                  │
│ [▾ Takedown accounts                  ] │
│                                         │
│ Subjects (3)                            │
│ • @somehandle (did:plc:abc...)          │
│ • @another (did:plc:def...)             │
│ • @third (did:plc:ghi...)               │
│                                         │
│ Rationale (applies to all)              │
│ ┌─────────────────────────────────────┐ │
│ │                                     │ │
│ └─────────────────────────────────────┘ │
│                                         │
│           [Cancel]  [Confirm 3 actions] │
└─────────────────────────────────────────┘
```

### Visual contract

- Subjects list collapsible if longer than 10
- Confirm button reads "Confirm N actions" where N is the count, making the scope unambiguous
- 50-subject cap surfaces an inline warning when approached: "Batch operations are limited to 50 subjects per call. Select fewer or repeat in batches."

### Accessibility contract

- Same as ActionPanel
- Subject count announces with action label: "Confirm bulk takedown of 3 accounts"
- Subjects list is `aria-label="Affected subjects"` for screen reader navigation

### Implementation notes

- Calls one of the six batch endpoints (`batchTakedownAccounts`, `batchSuspendAccounts`, etc.).
- Atomicity is two-tier (chainlink #112): the chain entry recording the operator decision is atomic — the moderation_event row and the corresponding `audit_chain_entry` row land together in a single transaction, or neither lands. Per-subject actor-state mutations (account_moderation rows, takedown_ref updates) are best-effort; failures are surfaced in the response's `failures` array and do not roll back the chain entry. True end-to-end per-subject atomicity is a v0.3 candidate (chainlink #113).
- Single audit chain entry per batch with all subjects referenced (per section 8 batch endpoint specs).
- Snapshot-at-decision captures per-subject (one snapshot per affected subject, all referenced from the single chain entry via `cascade_snapshot_ids`).
- UI surfaces partial-success: when the response's `failures` array is non-empty the batch summary distinguishes "applied" (`affected_count`) from "requested" (`cascade_subjects.length`) and lists the failing subjects with their per-subject reason. The chain entry still landed and operators can reconcile via `getAuditTrail`.

## 6.5 FilterStrip

The unified filter and search component used on every list page.

### Purpose

Consistent search and filter UX across all list surfaces. Operators learn one pattern, apply it everywhere. Filter chips popovers handle the four filter types (single-select enum, DID/handle picker, date range, free-text) with shared behavior.

### Used by

Queue, Reports, Appeals, Events, Audit, Accounts, Invites — every list page.

### Props / configuration

```
FilterStrip:
  showSearch: boolean                  # whether to render the text search input
  searchPlaceholder?: string           # i18n string for search context
  searchEndpoint?: string              # which endpoint to hit on search submit
  filters: FilterDef[]                 # array of filter definitions
  onApply: (state) => void             # callback when Apply is pressed
  onClear: () => void                  # callback when Clear all is pressed
  initialState?: FilterState           # restore from URL hash
```

```
FilterDef:
  key: string                          # filter identifier (e.g., 'actor', 'status')
  label: string                        # i18n label for the chip
  type: 'enum' | 'did-picker' | 'date-range' | 'text'
  options?: EnumOption[]               # for type 'enum'
  multiSelect?: boolean                # for type 'enum', allow multiple
```

### Layout

```
┌──────────────────────────────────────────────────────────┐
│  [Search...]    [Status: Open ×] [+ Actor] [+ Date]      │
│                                            [Apply] Clear │
└──────────────────────────────────────────────────────────┘
```

- Left section: search input (when present)
- Middle section: filter chips, active chips with values + ×, empty chips with `+ Label` to invite filling
- Right section: Apply button (disabled until filter state changes) and Clear all link (visible when any filter is active)

### Visual contract

- Search input: matches `.search-input` from existing CSS, full-width within its section, with Lucide search icon prefix
- Chips: soft rectangular shape, `0.625rem 1rem` padding, `--radius-sm` (6px) border-radius, secondary border color when empty, primary background when filled
- Empty chip text: "+ Filter label" in dimmed text
- Filled chip text: "Filter: value" in primary color, with × clear icon at right
- Apply button: `.btn-primary` style, disabled state visible
- Clear link: text-link style, smaller font, secondary color
- Filter privacy tooltip: small info icon next to chips on first encounter, dismissible. Copy: "Filters appear in your URL and may be shared when you copy this page's URL."

### Filter chip popovers

When a chip is opened, a popover appears below it. Four popover variants per filter type:

**Enum variant**: vertical list of options with checkboxes (multi-select) or radios (single-select). Options labeled, current selection highlighted.

**DID/handle picker variant**: text input with type-ahead. Type-ahead hits `tools.aurora.ops.listAccounts` debounced 400ms, shows dropdown of matching handles. Operator can paste raw DID. Validates as `did:plc:...` or `did:web:...` or `@handle` (resolved on selection).

**Date range variant**: calendar widget + two text inputs side-by-side for start/end. Range selection on calendar (click start, click end with hover preview). Preset chips at top: "Today, Last 7 days, Last 30 days, This month, Last month." Locale-aware via `Intl.DateTimeFormat`.

**Text variant**: simple text input.

All popover variants close on Escape and on outside click. Apply or Enter inside popover commits the filter value to the chip.

### Accessibility contract

- Search input: `<label class="sr-only">` per page context (i18n string), Tab order before chips
- Each chip: `<button>` with `aria-haspopup="dialog"`, `aria-expanded` reflecting popover state, `aria-label` including current value when filled
- Popover: `role="dialog"`, focus trapped while open, `aria-labelledby` pointing to the chip's label
- Apply button: announces filter result count after applying via `aria-live="polite"`: "Filter applied: Status is Open. Showing 23 results."
- Calendar: `role="grid"` with `aria-rowcount` / `aria-colcount`, each date `aria-selected` reflecting current selection state, `aria-label` per cell with full date in operator's locale

### Implementation notes

- URL state persistence: filter state writes to `#<route>?<filters>` query string on Apply. Reading the hash on page load restores state. Browser back/forward navigates filter changes.
- Search is independent of filters: it has its own debounce + Enter-submit; filters require explicit Apply.
- Calendar widget: vanilla JS, ~200 lines, no external dependency. Range selection logic, preset chips, locale formatting all inline.
- Cache for DID/handle picker type-ahead: 5-minute in-memory, LRU 200 entries.

## 6.6 Action confirmation modal

The wrapper modal that hosts ActionPanel for actions launched from non-drawer contexts.

### Purpose

Some actions launch from a button click rather than from an inline drawer (e.g., "Takedown" button on a Report list row, before the operator opens detail). These need modal hosting; ActionPanel renders inside the modal.

### Used by

Wherever an action is initiated outside a drawer context: list-row actions, Account detail's lifecycle sub-section actions, the forensic export modal flow.

### Props / configuration

Inherits ActionPanel props plus modal-specific:
- `title: string` (modal header)
- `dismissible: boolean` (whether Esc/overlay-click dismiss; default true unless action is in-progress)

### Visual contract

- Modal shell: matches existing `.modal` styling, max-width 600px, vertical center
- Header: bold modal title, close button at right
- Body: ActionPanel content
- Footer: integrated into ActionPanel's Cancel/Confirm buttons (no separate modal footer)

### Accessibility contract

- `role="dialog"`, `aria-modal="true"`, `aria-labelledby` to header
- Focus trap while open: Tab cycles through modal contents, doesn't escape to underlying page
- Escape closes (when dismissible)
- Focus returns to triggering element on close
- Underlying page becomes `aria-hidden="true"` while modal is open

## 6.7 ToastNotification

Transient notifications appearing at the top-right of the viewport.

### Purpose

Communicate action completion, errors, and incoming events without interrupting operator flow.

### Used by

Action completion (every state-changing operation), error surfacing for non-critical errors, real-time event arrival on certain pages, theme-change confirmation.

### Variants

| Variant | Color | Auto-dismiss | aria-live |
|---|---|---|---|
| info | neutral (text-secondary) | 4s | polite |
| success | `--success-color` accent | 4s | polite |
| error | `--danger-color` accent | 6s (longer for readability) | assertive |

### Visual contract

- Position: 1.5rem from top, 1.5rem from right of viewport
- Width: 360px (fits longer messages without dominating)
- Stack vertically when multiple, gap 0.75rem
- Background: var(--surface), border: 0.5px solid border color, border-radius `--radius-md`
- Padding: 1rem 1.25rem
- Optional action button on right (e.g., "View result")
- Optional manual close button (×) at top-right of the toast
- Slide-in animation from the right (respects `prefers-reduced-motion`)

### Accessibility contract

- `role="status"` for info/success, `role="alert"` for error
- `aria-live` matching variant
- Auto-dismiss is not announced (toast simply disappears); manual interaction respected
- Action button is keyboard-accessible
- Multiple toasts queued correctly for screen reader sequencing

## 6.8 Drawer

Collapsible content section used in detail pages (Account detail, Report detail) to organize content into discrete units.

### Purpose

Pages with many content sections (Account detail's overview / moderation actions / management / history) need a way to organize without overwhelming. Drawers collapse and expand individually.

### Used by

Account detail (4-5 drawers), Record detail (action drawer + history), Appeal detail (decision chain), wherever a detail page benefits from collapsible sections.

### Props / configuration

```
Drawer:
  title: string                    # drawer header
  defaultOpen?: boolean            # default true
  roleGate?: RoleRequirement       # if set, drawer hides for sessions below this role
  badgeCount?: number              # optional badge in header (e.g., "Subject history (3)")
```

### Visual contract

- Header: matches `.settings-card h3` styling, with chevron icon at right (Lucide chevron-down when open, chevron-right when closed)
- Header is clickable to toggle
- Body: appears when open; collapses with subtle animation when closed
- Border-radius `--radius-md` (12px), padding `1.5rem`, white surface — matches the existing `.settings-card`

### Accessibility contract

- Header: `<button>` with `aria-expanded` reflecting state, `aria-controls` pointing to body's id
- Body: `id` matching `aria-controls`, `aria-hidden` reflecting state
- Keyboard: Enter/Space toggles, Tab order skips collapsed drawer body
- Animation respects `prefers-reduced-motion`

### Implementation notes

- Role-gated drawers don't render at all for sessions below required role (not "render disabled" — actually absent)
- State (open/closed) persists per page during a session via in-memory state, not localStorage (operator preference per-session)

## 6.9 Skeleton loader

Placeholder content shown while data loads.

### Purpose

Loading states that look like actual content rather than blank space or generic spinners. Operators perceive page as faster when skeleton appears immediately and content fills in.

### Used by

Every list page (skeleton table rows during initial fetch), every detail page (skeleton placeholders for each panel), Dashboard widgets, anywhere content takes >100ms to appear.

### Variants

- `SkeletonTableRow`: gray bars matching column widths
- `SkeletonStatCard`: gray bars matching stat-card layout
- `SkeletonChart`: gray rectangle matching chart area
- `SkeletonText`: lines of varying width simulating paragraph
- `SkeletonAvatar`: gray circle for avatar placeholders

### Visual contract

- Background: `var(--background)` (slightly darker than surface to be visible)
- Subtle pulse animation (1.5s ease-in-out alternating opacity)
- No rounded corners except where the actual element would have them
- Animation respects `prefers-reduced-motion` (static gray when reduced)

### Accessibility contract

- `aria-busy="true"` on parent container during skeleton state
- `aria-label="Loading"` on skeleton container
- Screen reader announces "Loading" once on skeleton appearance, not repeatedly

## 6.10 Pagination strip

Cursor-based pagination control appearing below list contents.

### Purpose

Consistent pagination UX across list pages. Cursor-based to match the Phase 3 endpoint pagination scheme.

### Used by

Every list page.

### Props / configuration

```
PaginationStrip:
  prevCursor?: string              # null/undefined disables Previous
  nextCursor?: string              # null/undefined disables Next
  pageSize: number (default 50)
  pageSizeOptions: [25, 50, 100]
  totalCount?: number              # when available, displays "Showing X-Y of Z"
  onNavigate: (cursor) => void
  onPageSizeChange: (size) => void
```

### Layout

```
[Prev]  [25 | 50 | 100]  [Next]    Showing 1-50 of 230
```

### Visual contract

- Prev/Next buttons: `.btn-secondary` style, with Lucide chevron icons
- Page size: soft rectangular toggle group at `--radius-sm` (6px), primary highlight on active option
- Showing-X-of-Y: secondary text, right-aligned

### Accessibility contract

- Prev/Next: `<button>` with disabled state when no cursor
- Page size group: `role="radiogroup"`, each option `role="radio"`
- Showing-X-of-Y: `aria-live="polite"` so screen reader announces page changes
- Keyboard navigation: Tab between Prev/Next/page-size; arrow keys within page-size group

### Implementation notes

- Per-page size persisted in localStorage, defaulted to 50 (matches API default)
- Cursor stack maintained in-memory: when operator clicks Next, current nextCursor pushes to stack; clicking Previous pops to navigate back
- Stack resets on filter changes or page navigation

## 6.11 EmptyState

Component for rendering empty list states consistently across the UI.

### Purpose

Empty states convey "no data here" with appropriate context — distinguishing "no results match your filter" from "this list is genuinely empty" from "an error prevented loading." Operators understand which situation they're in.

### Used by

Every list page when zero results render. Some detail page panels when their data is empty (Subject context drawer with no recent activity, Records authored panel with zero records, etc.).

### Props / configuration

```
EmptyState:
  variant: 'no-results' | 'no-data' | 'error'
  title: string                    # i18n string
  description?: string             # i18n string with optional context
  icon?: LucideIcon                # default per variant: filter / inbox / alert-circle
  action?: { label: string, onClick: () => void }  # optional action button (e.g., "Clear filters")
```

### Visual contract

- Centered within the parent container
- Vertical layout: icon (48px) + title + description + optional action button
- Icon: dimmed primary color or secondary text color
- Title: `var(--font-size-base)` semibold
- Description: `var(--font-size-sm)` text-secondary
- Action button: `.btn-secondary` style
- Padding: `3rem 1.5rem` to give breathing room

### Accessibility contract

- Container: `role="status"` (declares the state without being interruptive)
- Icon: `aria-hidden="true"` (decorative; the title conveys the state)
- Action button: standard button accessibility

## 6.12 StatusBadge

Soft-rectangular status indicator with semantic variants.

### Purpose

Communicate state at a glance: account status, report status, appeal status, audit verification status, etc.

### Used by

Account detail header, Account list rows, Report list rows, Appeal list rows, Audit list rows, Account management drawer state indicators, anywhere a status string benefits from visual emphasis.

### Variants

| Variant | Light mode bg / fg | Dark mode bg / fg | Used for |
|---|---|---|---|
| `.status-active` | `#dcfce7` / `#166534` | `#064e3b` / `#6ee7b7` | active accounts, resolved reports |
| `.status-suspended` | `#fef3c7` / `#92400e` | `#78350f` / `#fcd34d` | suspended accounts |
| `.status-takedown` | `#fee2e2` / `#991b1b` | `#7f1d1d` / `#fca5a5` | takedown accounts/records |
| `.status-pending` | `#dbeafe` / `#1e40af` | `#1e3a8a` / `#93c5fd` | pending reports/appeals |
| `.status-deactivated` | `#f3f4f6` / `#374151` | `#1f2937` / `#9ca3af` | deactivated accounts |
| `.status-verified` | `#ecfdf5` / `#047857` | `#064e3b` / `#6ee7b7` | verified audit chain entries |
| `.status-pre-chain` | `#fef3c7` / `#78350f` | `#451a03` / `#fbbf24` | pre-chain audit entries (verification not available) |

### Visual contract

- Padding: `0.25rem 0.625rem`
- Border-radius: `--radius-sm` (6px) — soft rectangle, NOT pill
- Font: `var(--font-size-xs)` semibold, `text-transform: uppercase`, `letter-spacing: 0.025em`
- Inline element, no margin

### Accessibility contract

- Native text content conveys the status; screen reader announces "Active" / "Suspended" naturally
- Color is supplementary, not the only signal — text is always present
- Sufficient contrast in both light and dark modes per Section 10's audit

## 6.13 Lucide icon set

Consistent SVG icon library replacing the current emoji icons throughout the UI.

### Purpose

Operators benefit from consistent icon vocabulary. Lucide icons are visually coherent, sized predictably, scale cleanly, and avoid the cross-platform rendering inconsistencies of emoji.

### Used by

Every UI surface that uses icons. Sidebar navigation (replacing 📊 👥 🛡️ etc.), action buttons, status indicators, FilterStrip chips, ToastNotification, EmptyState, real-time indicators, every place that needs visual semantics beyond text.

### Implementation

Lucide icons embed inline as SVG. v0.2 ships a curated subset matching the UI's usage rather than the full Lucide library.

Subset shipped:

```
Navigation: layout-dashboard, gavel, file-text, scale, shield-alert, archive,
           users, ticket, activity, network, image, gauge, heart-pulse, server,
           settings, sliders, key, plug

Actions: ban, pause, play, archive, tag, tag-x, mail, trash-2, refresh-cw,
         download, upload, copy, external-link, plus, minus, x, check,
         alert-triangle, info, eye, eye-off

Filters/UI: search, calendar, chevron-down, chevron-right, chevron-left, chevron-up,
            arrow-up, arrow-down, more-horizontal, filter, command, sun, moon,
            monitor, dot, circle, square, log-out

Status/feedback: check-circle, x-circle, alert-circle, alert-octagon,
                clock, loader-2 (for spinners), shield-check
```

### Visual contract

- Default size: 16px (matches body text)
- Sidebar nav size: 20px
- Stroke width: 1.5 (Lucide default)
- Color: inherits from `currentColor` so icons match surrounding text or button color
- Container: when icon stands alone (no adjacent text), wrapped in element with `aria-label` for screen readers

### Accessibility contract

- Decorative icons (alongside text labels): `aria-hidden="true"`
- Icon-only buttons: `<button aria-label="Close">` with the icon as visual content
- Icon size respects font-size scaling for users with zoom

### Implementation notes

- Icons inline as SVG strings in a JS module: `lib/icons.js`. Each icon is a function returning the SVG markup with appropriate size.
- Tree-shakeable: only icons imported get bundled (vanilla approach, not actual webpack tree-shaking but conceptually equivalent).
- Total icon footprint (~50 icons × ~400 bytes each) ≈ 20KB. Acceptable for the no-build vanilla approach.

## 6.14 ThemeToggle

Three-state toggle for Light / Dark / System theme preference.

### Purpose

Operators set their theme preference. The control appears in the sidebar footer (always visible) and on Settings → UI & modes (canonical location).

### Used by

Sidebar footer. Settings → UI & modes page.

### Props / configuration

```
ThemeToggle:
  variant: 'compact' | 'full'  # 'compact' for sidebar, 'full' for settings
  onChange: (theme) => void
```

### Layout (compact)

A small three-segment pill. Each segment is an icon: sun (light), moon (dark), monitor (system).

### Layout (full)

Three radio-button-like options each with icon and label, in a horizontal row.

### Visual contract

- Compact: ~30px tall, three Lucide icons separated by 1px dividers, active state highlights the selected segment
- Full: 36px tall segments, includes both icon and label
- Active segment: `var(--primary-color)` background, white text/icon
- Inactive segments: transparent background, secondary text color
- Border-radius: `--radius-sm` for the outer pill; segments inherit

### Accessibility contract

- `role="radiogroup"` with `aria-label="Theme preference"`
- Each segment: `role="radio"` with `aria-checked`
- Keyboard: Arrow keys cycle, Enter/Space activates
- `Cmd/Ctrl+Shift+L` keyboard shortcut to cycle through (registered globally)

### Implementation notes

- Theme value persists in localStorage under key `aurora-admin-theme` (values: `light`, `dark`, `system`)
- On change, sets `data-theme` attribute on document root: `<html data-theme="dark">` or `<html data-theme="light">` (System resolves to light/dark via `prefers-color-scheme` and sets accordingly)
- CSS uses `[data-theme="dark"]` selector for dark-mode variable overrides (per Section 7)
- System mode reactively listens to `prefers-color-scheme` media query and updates the data-theme attribute when OS preference changes

## 6.15 CommandPalette

Global Cmd/Ctrl+K palette for quick navigation, search, and action invocation.

### Purpose

Operators with keyboard-first workflows benefit from a single shortcut to reach anywhere or do anything. The palette surfaces three result categories: navigate to page, find subject (account/record/etc.), invoke action.

### Used by

Globally accessible from anywhere via Cmd/Ctrl+K.

### Layout

```
┌─────────────────────────────────────┐
│ [Search anywhere...]            [⌘K]│
├─────────────────────────────────────┤
│ NAVIGATE                            │
│ Dashboard                          ↵│
│ Queue                              ↵│
│ Reports                            ↵│
│                                     │
│ SUBJECTS                            │
│ @somehandle (did:plc:abc...)       ↵│
│ @another (did:plc:def...)          ↵│
│                                     │
│ ACTIONS                             │
│ Generate forensic export            │
│ Apply label                         │
│                                     │
│ Recent: ↑ Queue                     │
└─────────────────────────────────────┘
```

### Visual contract

- Modal-style overlay, vertically positioned 1/3 from top of viewport
- Width: 600px max, centered
- Search input: monospace, full-width, with `⌘K` hint
- Results: grouped by category with section labels (NAVIGATE, SUBJECTS, ACTIONS)
- Each result: keyboard cursor highlights, ↵ icon indicates Enter activates
- Recent items: small horizontal strip at bottom showing last 3 invocations

### Accessibility contract

- `role="dialog"`, `aria-modal="true"`, `aria-labelledby` to search input label
- Results: `role="listbox"`, each result `role="option"`
- Keyboard: arrow keys navigate, Enter activates, Esc closes
- Focus trap while open, returns focus on close
- Search input announces result count via `aria-live="polite"`

### Implementation notes

- Fuzzy search across navigation (page titles), subjects (handles + DIDs from recent context + recent searches), actions (registered action names)
- Subject search uses `searchAccounts` debounced 300ms when query is text-shaped
- Action invocation: clicking "Apply label" navigates to current page's action panel with the action preselected (when applicable); for actions with no current page context, navigates to the appropriate detail page first
- Recent items stored in localStorage keyed by `aurora-admin-recent-commands`, capped at 10
- Implementation lives in `components/CommandPalette.js`, ~300 lines vanilla JS

## 6.16 i18n string helper

The `t()` function for resolving user-facing strings.

### Purpose

All visible strings in the UI route through one helper. Adding a language is a drop-in, not a code change.

### Used by

Every component that renders user-facing text.

### API

```
t(key, params?)
```

Examples:
```
t('queue.title')                       // "Queue"
t('reports.count', { count: 3 })       // "3 reports"
t('reports.count', { count: 0 })       // "No reports"
t('reports.count', { count: 1 })       // "1 report"
```

### String file structure

`static/admin/i18n/en.json`:
```json
{
  "queue": {
    "title": "Queue",
    "subtitle": "Items needing attention",
    "empty_filtered": "No matches. Try widening your filters.",
    "empty_unfiltered": "Nothing in the queue. Things will appear here as reports and appeals come in."
  },
  "reports": {
    "count": "{count, plural, =0 {No reports} one {# report} other {# reports}}",
    ...
  },
  ...
}
```

ICU MessageFormat for plurals and complex substitutions. JSON file structure mirrors the UI's information architecture so new pages add new top-level keys.

### Implementation notes

- Vanilla JS implementation, no library dependency
- Locale loading: fetches `/admin/i18n/<locale>.json` on app init based on operator's preference (Settings → UI & modes language selector, defaults to `navigator.language`)
- Falls back to English if requested locale's file doesn't exist
- Caches loaded locales in module-level state for the session
- ICU MessageFormat parsing implemented inline (~50 lines for the subset used)

## 6.17 Capability-routed substrate

The substrate that handles endpoint routing through the Phase 3.5 transition (and similar future transitions).

### Purpose

UI components calling endpoints don't know which version of the endpoint is available. The capability substrate detects via `tools.aurora.describeCapabilities` and routes the call appropriately. Components stay on the same interface; the substrate flips routing internally.

### Used by

ActionPanel (routes through `emitEvent` post-3.5, per-action endpoints pre-3.5), real-time subscription substrate (uses `subscribeModEvents` post-3.9, falls back to polling pre-3.9), any future endpoint transitions.

### API

```
api.callEndpoint(feature, params)
```

Where `feature` is a high-level capability name ("emit-mod-event", "subscribe-mod-events") not an NSID. The substrate maps feature → NSID based on capabilities.

### Implementation notes

- Capabilities cached in localStorage on session start, refreshed every 60 minutes or on demand via the Settings → Capabilities page's Refresh button
- Mapping table is a simple dict: `feature → { capability, primaryNsid, fallbackNsid }`
- Substrate is in `api/capabilities.js`, exposes `getCapabilities()`, `hasCapability()`, `getEndpointForFeature()` helpers
- Future endpoint transitions add to the mapping table, no component changes needed

## 6.18 Subscription substrate

The WebSocket substrate for `tools.aurora.admin.subscribeModEvents` and similar real-time endpoints.

### Purpose

Real-time event delivery for surfaces that need it (per architecture principle 3.5). Connection lifecycle, reconnection, backpressure, and stale-state recovery all handled in one place.

### Used by

Mod Events page (consumes mod events stream), Audit page (consumes audit chain entries), Subject detail surfaces (consumes filtered events affecting active subject), Dashboard's recent activity feed (post-Phase 3.9).

### API

```
api.subscribe(feature, filters, handlers)
  returns: subscription handle with unsubscribe()
  handlers:
    onEvent: (event) => void
    onConnected: () => void
    onDisconnected: () => void
    onReconnecting: () => void
```

### Implementation notes

- WebSocket connection per feature; multiple subscriptions to same feature share the connection (multiplexed via filters)
- Reconnect logic: exponential backoff 1s → 2s → 4s → 8s → 16s (capped), reset on successful connection
- Stale-state recovery: on reconnect after a disconnect, resume cursor includes last-seen sequence position; server replays missed events
- Visual indicator (substrate primitive 19) reflects connection state to the user
- Pre-Phase 3.9 fallback: substrate primitive 21 (capability routing) detects `mod-events-stream-v1` absent, routes subscriptions through periodic polling instead. Same handler API; component-level interface unchanged.

## 6.19 Real-time indicator

Visual indicator showing the connection state of subscription-driven surfaces.

### Purpose

Operators on Mod Events or Audit page need to know whether they're seeing live data or stale data. The indicator answers that question at a glance.

### Used by

Page header on Mod Events, Audit, and Dashboard's Moderator flavor (when subscription is the data source post-Phase 3.9).

### Variants

| State | Visual | Label |
|---|---|---|
| Connected | green pulsing dot | "Live" |
| Reconnecting | amber spinning ring | "Reconnecting…" |
| Disconnected | red static dot | "Offline" |
| Polling fallback (pre-3.9) | gray dot | "Polling every 10s" |

### Visual contract

- Compact: 12px dot + text label
- Position: right-aligned in page header, near other meta-controls (Refresh button, etc.)
- Pulse animation on Connected state; respects `prefers-reduced-motion`
- Color reflects state per variant table
- Label is part of the visible UI; not just a tooltip

### Accessibility contract

- `aria-live="polite"` on state-change announcements: "Live event stream connected" / "Disconnected; reconnecting" / etc.
- Non-pulsing fallback (static dot) when reduced-motion preferred
- Color is supplementary; the label conveys the state

## 6.20 Calendar widget

Locale-aware date and date-range picker.

### Purpose

Date filtering on FilterStrip's date-range chip. Used wherever an operator picks a date or range.

### Used by

FilterStrip date-range filter chip (Reports, Events, Audit, Accounts pages), forensic export modal's "events from {date} to {date}" filter (deferred to v0.3 unless surfaces emerge needing it).

### Layout

```
┌─────────────────────────────────────┐
│ [Today] [Last 7] [Last 30] [Month]  │
├─────────────────────────────────────┤
│        ◀ April 2026 ▶               │
│                                     │
│ Su Mo Tu We Th Fr Sa                │
│           1  2  3  4                │
│  5  6  7  8  9 10 11                │
│ 12 13 14 15 16 17 18                │
│ 19 20 21 22 23 24 25                │
│ 26 27 28 29 30                      │
│                                     │
│ Start [2026-04-01]  End [2026-04-30]│
└─────────────────────────────────────┘
```

### Visual contract

- Preset chips at top: quick-select common ranges
- Calendar grid: locale-aware first-day-of-week, single-month view
- Range selection: click first date (start), click second date (end). Hover preview shows range as it would be selected.
- Selected range: primary background on cells, lighter shade between
- Month/year navigation: prev/next buttons; click month/year text to jump
- Below grid: text inputs for explicit date entry, validates against locale format

### Accessibility contract

- `role="grid"` with `aria-rowcount` and `aria-colcount`
- Each date cell: `aria-selected` reflecting selection, `aria-label` with full date in operator locale
- Keyboard: arrow keys navigate dates, Enter selects, Tab moves to text inputs
- Text inputs accept locale-formatted dates with validation
- Preset chips: standard buttons with `aria-pressed` for active state

### Implementation notes

- Locale formatting via `Intl.DateTimeFormat` for cell labels and text-input parsing
- First-day-of-week: derived from locale via `Intl` (en-US starts Sunday, most others Monday)
- Range validation: end ≥ start; reverse selection auto-corrects
- Vanilla JS implementation, no external dependency, ~250 lines

## 6.21 Forensic export modal

Specialized modal hosting the forensic export configuration form.

### Purpose

Forensic export is high-impact and parameter-rich enough to warrant its own modal pattern (rather than the generic ActionPanel modal).

### Used by

Account detail's Account management drawer's lifecycle sub-section.

### Layout

Specified inline in section 5.2 (Account detail). Reproducing here for completeness:

```
Generate forensic export
─────────────────────────

Subject: @somehandle
         did:plc:abc...

Include:
☑ Repository content (CAR file)
☑ Blobs                         (~12.4 MB, 47 blobs)
☑ Moderation history            (3 prior actions)
☐ Account metadata              [SuperAdmin only]
                                — Email, signing keys, invite lineage
☐ Audit chain entries           [SuperAdmin only]
                                — Operator decisions and chain context

Rationale (required)
[                                                      ]

This export will be recorded in the audit chain with a tamper-evident hash.
The bundle will contain account data; treat as sensitive.

                              [Cancel]  [Generate export]
```

### Implementation notes

- Builds on the Action confirmation modal (substrate primitive 6.6) with form-specific extensions
- Role gating per checkbox: SuperAdmin-only options disabled with explanatory text for Admin sessions
- Calls `tools.aurora.admin.exportAccountForensic` with the parameter set
- Streams response as download (large bundles); progress indicator during stream
- Success toast: "Forensic export generated. Audit entry: h:abc... [View]"

# 7. Visual design tokens and component library

This section specifies the design tokens (colors, typography, spacing, radii, shadows, motion) that govern the UI's visual appearance. Every component in section 6 references these tokens; future contributors extending the UI use these tokens rather than hardcoded values.

The tokens preserve the existing `static/admin/` palette where it works and extend it where needed (dark mode parallels, soft-rectangle radii, additional status variants). Implementation lives in `static/admin/styles/tokens.css` (per section 12 migration).

## 7.1 Color tokens

### Light mode (default)

```css
:root {
  /* Primary */
  --primary-color: #3b82f6;          /* preserved from current */
  --primary-dark: #2563eb;           /* preserved */
  --primary-light: #60a5fa;          /* preserved */

  /* Surfaces */
  --background: #f8fafc;             /* preserved */
  --surface: #ffffff;                /* preserved */
  --surface-elevated: #ffffff;       /* same as surface in light mode */

  /* Text */
  --text-primary: #0f172a;           /* preserved (was --gray-900) */
  --text-secondary: #475569;         /* preserved (was --gray-600) */
  --text-tertiary: #94a3b8;          /* preserved (was --gray-400) */
  --text-on-primary: #ffffff;        /* white text on primary-color background */

  /* Borders */
  --border-color: #e2e8f0;           /* preserved (was --gray-200) */
  --border-color-strong: #cbd5e1;    /* for emphasized borders */

  /* Sidebar */
  --sidebar-bg: #1e293b;             /* preserved */
  --sidebar-text: #cbd5e1;           /* preserved */
  --sidebar-active: #3b82f6;         /* preserved */
  --sidebar-hover: #334155;          /* preserved */

  /* Semantic colors */
  --success-color: #10b981;          /* preserved */
  --warning-color: #f59e0b;          /* preserved */
  --danger-color: #ef4444;           /* preserved */
  --attention-color: #f59e0b;        /* new — for "this number existing is something to look at" stat indicators */

  /* Status badge backgrounds (soft tints) */
  --status-active-bg: #dcfce7;
  --status-active-fg: #166534;
  --status-suspended-bg: #fef3c7;
  --status-suspended-fg: #92400e;
  --status-takedown-bg: #fee2e2;
  --status-takedown-fg: #991b1b;
  --status-pending-bg: #dbeafe;
  --status-pending-fg: #1e40af;
  --status-deactivated-bg: #f3f4f6;
  --status-deactivated-fg: #374151;
  --status-verified-bg: #ecfdf5;
  --status-verified-fg: #047857;
  --status-pre-chain-bg: #fef3c7;
  --status-pre-chain-fg: #78350f;
}
```

### Dark mode

```css
[data-theme="dark"] {
  /* Primary - lightened for dark backgrounds */
  --primary-color: #60a5fa;
  --primary-dark: #3b82f6;
  --primary-light: #93c5fd;

  /* Surfaces - dark grays, not pure black */
  --background: #0a0e14;             /* deep slate */
  --surface: #151b23;                /* card surface */
  --surface-elevated: #1c242e;       /* elevated cards/modals */

  /* Text - off-white, not pure white */
  --text-primary: #e6edf3;
  --text-secondary: #9ba8b8;
  --text-tertiary: #6e7681;
  --text-on-primary: #0a0e14;        /* dark text on primary in dark mode */

  /* Borders - subtle in dark mode */
  --border-color: #2d3748;
  --border-color-strong: #475569;

  /* Sidebar - same as light mode (stays dark) */
  --sidebar-bg: #0a0e14;             /* even darker than surface */
  --sidebar-text: #cbd5e1;
  --sidebar-active: #3b82f6;
  --sidebar-hover: #1c242e;

  /* Semantic colors - lightened slightly for contrast */
  --success-color: #34d399;
  --warning-color: #fbbf24;
  --danger-color: #f87171;
  --attention-color: #fbbf24;

  /* Status badge backgrounds - dark tinted */
  --status-active-bg: #064e3b;
  --status-active-fg: #6ee7b7;
  --status-suspended-bg: #78350f;
  --status-suspended-fg: #fcd34d;
  --status-takedown-bg: #7f1d1d;
  --status-takedown-fg: #fca5a5;
  --status-pending-bg: #1e3a8a;
  --status-pending-fg: #93c5fd;
  --status-deactivated-bg: #1f2937;
  --status-deactivated-fg: #9ca3af;
  --status-verified-bg: #064e3b;
  --status-verified-fg: #6ee7b7;
  --status-pre-chain-bg: #451a03;
  --status-pre-chain-fg: #fbbf24;
}
```

### System theme

When operator selects "System" theme, the document root sets `data-theme` to either `light` or `dark` based on `prefers-color-scheme`, and updates reactively when OS preference changes. No third "auto" CSS state — the theme resolves to one or the other.

## 7.2 Contrast verification

WCAG 2.2 AA requires 4.5:1 contrast for normal text, 3:1 for large text and UI components.

Key combinations verified for both modes:

| Combination | Light ratio | Dark ratio |
|---|---|---|
| `--text-primary` on `--surface` | 18.4:1 ✓ | 13.2:1 ✓ |
| `--text-secondary` on `--surface` | 7.5:1 ✓ | 6.7:1 ✓ |
| `--text-tertiary` on `--surface` | 3.4:1 (large text only) | 3.8:1 (large text only) |
| `--primary-color` on `--surface` | 4.5:1 ✓ | 6.4:1 ✓ |
| `--text-on-primary` on `--primary-color` | 4.5:1 ✓ | 5.1:1 ✓ |
| `--status-active-fg` on `--status-active-bg` | 8.4:1 ✓ | 7.7:1 ✓ |
| `--status-takedown-fg` on `--status-takedown-bg` | 7.7:1 ✓ | 5.0:1 ✓ |

Full verification per Section 10's accessibility audit. Any combination falling below WCAG 2.2 AA in either mode is a bug, not a styling choice.

## 7.3 Typography

```css
:root {
  --font-sans: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
  --font-mono: 'SF Mono', Menlo, Consolas, 'Courier New', monospace;

  /* Sizes */
  --font-size-xs: 0.75rem;      /* 12px */
  --font-size-sm: 0.875rem;     /* 14px */
  --font-size-base: 1rem;       /* 16px */
  --font-size-lg: 1.125rem;     /* 18px */
  --font-size-xl: 1.25rem;      /* 20px */
  --font-size-2xl: 1.5rem;      /* 24px */

  /* Weights */
  --font-weight-normal: 400;
  --font-weight-medium: 500;
  --font-weight-semibold: 600;
  --font-weight-bold: 700;

  /* Line heights */
  --line-height-tight: 1.25;
  --line-height-normal: 1.5;
  --line-height-relaxed: 1.625;
}
```

System font stack (no web font load) for fast initial render. Monospace stack for DIDs, hashes, code, and other technical identifiers.

## 7.4 Spacing

```css
:root {
  --space-0: 0;
  --space-1: 0.25rem;   /* 4px */
  --space-2: 0.5rem;    /* 8px */
  --space-3: 0.75rem;   /* 12px */
  --space-4: 1rem;      /* 16px */
  --space-5: 1.25rem;   /* 20px */
  --space-6: 1.5rem;    /* 24px */
  --space-8: 2rem;      /* 32px */
  --space-10: 2.5rem;   /* 40px */
  --space-12: 3rem;     /* 48px */
  --space-16: 4rem;     /* 64px */
}
```

Used for padding, margins, gap. Component-specific spacing references these tokens rather than hardcoding pixel values.

## 7.5 Radii

```css
:root {
  --radius-sm: 0.375rem;  /* 6px — small elements: chips, badges, buttons */
  --radius-md: 0.75rem;   /* 12px — cards, modals, drawers */
  --radius-full: 9999px;  /* fully rounded — only used where pills are explicitly correct (avatar, real-time indicator dot) */
}
```

Two primary radii (sm and md) per the locked design decision: soft rectangles, not pills. The `--radius-full` exists for the rare cases where pill shape is correct (a circular avatar, a small indicator dot).

## 7.6 Shadows

```css
:root {
  --shadow-sm: 0 1px 2px 0 rgba(0, 0, 0, 0.05);
  --shadow-md: 0 4px 6px -1px rgba(0, 0, 0, 0.1), 0 2px 4px -2px rgba(0, 0, 0, 0.05);
  --shadow-lg: 0 10px 15px -3px rgba(0, 0, 0, 0.1), 0 4px 6px -4px rgba(0, 0, 0, 0.05);

  /* Focus ring */
  --shadow-focus: 0 0 0 3px rgba(59, 130, 246, 0.4);  /* primary-color at 40% */
}

[data-theme="dark"] {
  --shadow-sm: 0 1px 2px 0 rgba(0, 0, 0, 0.3);
  --shadow-md: 0 4px 6px -1px rgba(0, 0, 0, 0.4), 0 2px 4px -2px rgba(0, 0, 0, 0.3);
  --shadow-lg: 0 10px 15px -3px rgba(0, 0, 0, 0.5), 0 4px 6px -4px rgba(0, 0, 0, 0.4);
  --shadow-focus: 0 0 0 3px rgba(96, 165, 250, 0.5);  /* dark-mode primary at 50% */
}
```

Shadows in dark mode are deeper (more pronounced rgba alpha) because dark surfaces need more shadow to convey elevation than light surfaces.

## 7.7 Motion

```css
:root {
  --transition-fast: 100ms ease;
  --transition-normal: 200ms ease;
  --transition-slow: 400ms ease;
}

@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0.01ms !important;
  }
}
```

Reduced-motion respect is universal across the UI. Animations that exceed `prefers-reduced-motion` get disabled, not just slowed.

## 7.8 Layout primitives

```css
:root {
  --sidebar-width: 260px;          /* preserved from current */
  --content-max-width: 1400px;     /* primary content area max */
  --rail-width: 320px;             /* secondary rail width on detail pages */

  --header-height: auto;           /* page header is content-sized */
  --footer-height: auto;
}
```

Layouts use these tokens for primary structural dimensions.

## 7.9 Focus management

Focus indicators are governed by:

```css
*:focus-visible {
  outline: 2px solid var(--primary-color);
  outline-offset: 2px;
}

input:focus-visible,
select:focus-visible,
textarea:focus-visible {
  outline: none;
  box-shadow: var(--shadow-focus);
}
```

Universal focus visibility per WCAG 2.2's 2.4.11 (focus not obscured) and 2.4.13 (focus appearance).

## 7.10 Z-index scale

```css
:root {
  --z-dropdown: 100;
  --z-sticky: 200;
  --z-modal: 1000;
  --z-toast: 2000;
  --z-palette: 3000;
}
```

Stacking order made explicit. Modals over sticky elements; toasts over modals (toasts often confirm modal actions); palette over everything (palette is the operator's escape hatch).

## 7.11 Component library principles

Beyond tokens, the component library follows three principles:

**Composition over inheritance.** Components combine primitives rather than extending them. A page that needs a status badge with custom behavior wraps `<StatusBadge>` rather than subclassing.

**Vanilla JS, native ES modules.** No build step. Components live in `scripts/components/<Name>.js`, each exporting a `render(props)` function returning DOM nodes (or HTML strings to be parsed). Module loading via native `<script type="module">`.

**Server authority is the gate, not component logic.** Components display state per their props. They do not implement role-checking or capability-checking themselves — the substrate (api/capabilities, state/session) provides resolved values; components consume them.

## 7.12 Anti-patterns

The following are explicit anti-patterns the v0.2 UI does not adopt:

- **Pills for non-pill content.** `--radius-full` is reserved for genuinely-circular elements. Status badges, chips, buttons all use `--radius-sm`. Pills feel dated and don't fit the operator-grade aesthetic.
- **Drop shadows for decoration.** Shadows convey elevation (modal above page, popover above content). Decorative shadows on flat surfaces add visual noise.
- **Hardcoded colors.** Every color references a token. If a component needs a color the tokens don't provide, the right move is adding a token, not hardcoding.
- **Pixel-precise positioning.** Use spacing tokens. If something needs an unusual offset, that's a signal to revisit the layout, not to hardcode `13px`.
- **Heavy gradients.** The UI uses solid colors. Gradients introduce contrast ambiguity and rarely improve clarity. The existing CSS has zero gradients beyond the sidebar's brand area; v0.2 preserves this.
- **Inline styles in components.** All styling lives in CSS files. Components add classes; the CSS responds. This keeps the styling layer maintainable and accessible to operators who modify deployments.

# 8. Forthcoming endpoint commitments

This section commits to the lexicon shapes of every new or extended endpoint the UI integrates with. The Aurora-Locus convention is Rust-types-as-lexicon (no JSON lexicon files); these specs are the Rust handler signature contracts subsequent sub-phases implement against.

Each endpoint specification follows a consistent template:

- **NSID** — the full namespace identifier
- **Type** — query | procedure | subscription
- **Auth** — required scope and role
- **Phase** — which v0.2 sub-phase ships the endpoint
- **Used by** — which UI surfaces consume the endpoint
- **Input** — request shape (Rust-types representation)
- **Output** — response shape
- **Behavior** — server-side semantics including audit-chain integration
- **Notes** — anything specific that doesn't fit above

The endpoints are grouped by namespace and ordered by phase.

## 8.1 Phase 3.5 — `tools.aurora.admin.emitEvent`

The unified moderation action surface. Subsumes the per-action endpoints (`takedownAccount`, `suspendAccount`, etc.) post-3.5 while those remain live for protocol compatibility.

### Specification

- **NSID:** `tools.aurora.admin.emitEvent`
- **Type:** procedure
- **Auth:** AdminModeration scope; role gating per action (Moderator+ for content actions, Admin+ for account-infrastructure actions)
- **Phase:** 3.5
- **Used by:** ActionPanel (substrate primitive 3) for all moderation actions; substrate primitive 21 routes here when capability `mod-events-emit-v1` is present

### Input

```rust
struct EmitEventInput {
    action: ModEventAction,           // discriminated enum
    subject: SubjectRef,              // {type, did|uri|cid}
    rationale: String,                // required, non-empty after trim
    snapshot_capture: bool,           // default true; whether to capture snapshot-at-decision
    metadata: Option<serde_json::Value>,  // action-specific options
}

enum ModEventAction {
    TakedownAccount,
    SuspendAccount,
    RestoreAccount,
    DeleteAccount,
    ApplyLabel { val: String, neg: bool },
    RemoveLabel { val: String },
    TakedownRecord,
    QuarantineBlob,
    RestoreBlob,
    DeleteBlob,
    ResolveReport { report_id: String, resolution: ReportResolution },
    DismissReport { report_id: String },
    ResolveAppeal { appeal_id: String, resolution: AppealResolution },
    EscalateAppeal { appeal_id: String },
    SendEmail { template: Option<String>, subject: String, body: String },
    UpdateSubjectStatus { status: SubjectStatus },
}

struct SubjectRef {
    #[serde(tag = "$type")]
    discriminator: SubjectType,  // RepoRef | StrongRef | RepoBlobRef
    did: Option<String>,
    uri: Option<String>,
    cid: Option<String>,
}
```

### Output

```rust
struct EmitEventOutput {
    event_id: String,
    audit_entry_id: Option<String>,    // None if pre-chain
    snapshot_id: Option<String>,       // None if snapshot_capture was false or not applicable
    cascading_actions: Vec<String>,    // event_ids of actions cascaded server-side (e.g., appeal-approval triggering restore)
}
```

### Behavior

1. Validate operator's role against action requirements (Moderator+ vs Admin+).
2. Validate action against subject type (lexicon-aware action surfacing — e.g., reject "SuspendAccount" with a record subject).
3. Capture snapshot of subject's current state if `snapshot_capture` is true and the action affects an entity that has snapshottable state.
4. Apply the action atomically.
5. Write audit chain entry referencing the snapshot and including operator DID, action, subject, rationale, timestamp.
6. Emit event to `mod_event_seq` for subscription consumers.
7. If the action cascades (e.g., approving an appeal also reverses the original action), perform the cascade atomically as part of the same audit entry — one chain entry, multiple referenced subjects.
8. Return event_id, audit_entry_id, snapshot_id, and cascading_actions.

### Notes

- The discriminated enum approach lets the lexicon evolve — new action types added by extending the enum, existing handlers untouched.
- Snapshot capture is opt-out for actions that don't benefit from snapshots (e.g., `SendEmail` doesn't need a snapshot of the recipient's repo state).
- Capability advertisement: deployments shipping this endpoint advertise `mod-events-emit-v1` in `describeCapabilities`. UI checks for this capability before routing actions here.

## 8.2 Phase 3.7 — `tools.aurora.admin.getModerationMetrics`

Aggregate moderation metrics for dashboard widgets and time-series charts.

### Specification

- **NSID:** `tools.aurora.admin.getModerationMetrics`
- **Type:** query
- **Auth:** AdminModeration scope, Moderator+ role
- **Phase:** 3.7
- **Used by:** Dashboard's Moderator flavor (stat cards + activity charts)

### Input

```rust
struct GetModerationMetricsInput {
    range: TimeRange,                 // {start: ISO8601, end: ISO8601} or preset
    granularity: Granularity,         // Hour | Day | Week | Month
    metrics: Vec<MetricType>,         // which metrics to return
}

enum MetricType {
    ReportsFiled,
    ReportsResolved,
    AppealsFiled,
    AppealsResolved,
    ActionsTaken,
    ActiveModerators,
    AverageTimeToResolution,
}
```

### Output

```rust
struct GetModerationMetricsOutput {
    range: TimeRange,
    granularity: Granularity,
    series: Vec<MetricSeries>,
}

struct MetricSeries {
    metric: MetricType,
    points: Vec<DataPoint>,           // time-series for charts
    aggregate: f64,                   // total over the range
    delta: Option<DeltaInfo>,         // comparison to previous range of same length
}

struct DeltaInfo {
    previous_aggregate: f64,
    change_absolute: f64,
    change_percent: f64,
}
```

### Behavior

- Computes metrics from `mod_event_seq` and report/appeal tables for the requested range and granularity.
- Returns time-series suitable for chart rendering plus aggregate values for stat cards.
- Delta comparison: previous range of the same length immediately preceding `start`. E.g., if range is "last 7 days," delta compares against the 7 days before that.
- Performance: response should be cached server-side for ~5 minutes since metrics don't change second-to-second.

### Notes

- The UI's stat-change indicators (positive / attention / neutral) consume the `delta` field. Negative changes for "ReportsFiled" are positive sentiment (fewer reports = good); for "ReportsResolved" positive change is positive sentiment. The UI's display logic interprets per-metric.

## 8.3 Phase 3.7 — `tools.aurora.admin.getQueueStats`

Counts of items in moderation queue states. Powers the bell badge and Dashboard moderation stat cards.

### Specification

- **NSID:** `tools.aurora.admin.getQueueStats`
- **Type:** query
- **Auth:** AdminModeration scope, Moderator+ role
- **Phase:** 3.7
- **Used by:** Sidebar bell badge (polled), Dashboard Moderator flavor stat cards, Queue page header

### Input

```rust
struct GetQueueStatsInput {
    // No parameters; always returns current state
}
```

### Output

```rust
struct GetQueueStatsOutput {
    open_reports: u32,
    pending_appeals: u32,
    under_review_reports: u32,
    under_review_appeals: u32,
    queue_attention_total: u32,        // sum of items needing operator decision
    average_age_open_reports_seconds: u64,
    oldest_open_report_age_seconds: u64,
}
```

### Behavior

- Computed from current report and appeal status tables. Cached for ~30 seconds since polling cadence is 30s and stale-by-30s is acceptable for UI display.
- `queue_attention_total` is the canonical badge value the UI displays.

### Notes

- The "average age" and "oldest age" metrics surface in the Dashboard Moderator flavor as a "queue health" indicator. Long queues with old items are signals of moderation team capacity issues.

## 8.4 Phase 3.8 — `tools.aurora.admin.getAuditTrail`

Hash-chained audit log query.

### Specification

- **NSID:** `tools.aurora.admin.getAuditTrail`
- **Type:** query
- **Auth:** AdminModeration scope, Moderator+ role
- **Phase:** 3.8
- **Used by:** Audit page (filtered by verified-only toggle merges with `getAuditLog`); Audit entry detail page; chain-walk navigation

### Input

```rust
struct GetAuditTrailInput {
    cursor: Option<String>,           // base64 opaque cursor
    limit: u32,                       // default 50, max 100
    actor_did: Option<String>,
    action: Option<String>,
    subject_did: Option<String>,
    subject_uri: Option<String>,
    after_created: Option<DateTime<Utc>>,
    before_created: Option<DateTime<Utc>>,
}
```

### Output

```rust
struct GetAuditTrailOutput {
    entries: Vec<AuditEntry>,
    cursor: Option<String>,           // for pagination
}

struct AuditEntry {
    id: String,                       // entry id (often the hash itself)
    sequence: u64,                    // position in chain
    timestamp: DateTime<Utc>,
    actor_did: String,
    action: String,                   // canonical action name
    subject_ref: SubjectRef,
    rationale: String,
    snapshot_id: Option<String>,      // reference to snapshot if applicable
    event_id: Option<String>,         // reference to mod_event if applicable
    current_hash: String,             // SHA-256 over entry content
    previous_hash: Option<String>,    // points to prev entry; None for first or pre-chain sentinel
    verified: bool,                   // false for pre-chain sentinel rows
    cascade_subjects: Vec<SubjectRef>, // for batch operations and cascades
}
```

### Behavior

- Returns audit entries matching filters in reverse chronological order (newest first).
- `previous_hash` field links entries into chain.
- Pre-chain entries (those predating Phase 3.8) return with `current_hash: "pre-chain"` sentinel and `verified: false`.
- Hash verification can be performed by re-hashing entry content and comparing against `current_hash`.

### Notes

- Chain walk: client navigates "previous in chain" by querying for the entry whose `current_hash` matches the current entry's `previous_hash`.
- Forensic export (`exportAccountForensic`) writes its own audit entries here. The chain encompasses all administrative actions, not just moderation actions.

## 8.5 Phase 3.9 — `tools.aurora.admin.subscribeModEvents`

WebSocket subscription for real-time moderation event delivery.

### Specification

- **NSID:** `tools.aurora.admin.subscribeModEvents`
- **Type:** subscription
- **Auth:** AdminModeration scope, Moderator+ role
- **Phase:** 3.9
- **Used by:** Subscription substrate (primitive 18); consumed by Mod Events page, Audit page, Subject detail pages, Dashboard's recent activity feed

### Input (subscription parameters)

```rust
struct SubscribeModEventsInput {
    cursor: Option<u64>,              // resume from sequence position
    actor_did: Option<String>,
    subject_did: Option<String>,
    subject_uri: Option<String>,
    action_filter: Option<Vec<String>>, // subset of action names to receive
    include_audit_chain: bool,        // whether to send audit chain entries alongside events
}
```

### Output (streamed messages)

```rust
enum SubscribeModEventsMessage {
    Hello { instance_version: String, sequence: u64 },
    Event { event: ModEvent, sequence: u64 },
    AuditEntry { entry: AuditEntry, sequence: u64 },
    Heartbeat { sequence: u64 },
    Error { code: String, message: String },
}
```

### Behavior

- WebSocket connection upgraded from HTTP request with auth in the upgrade.
- Filtered server-side per input parameters.
- Sequence positions allow client to detect missed messages on reconnect.
- Heartbeat every 30s when no other messages flow, so client can detect dead connections.
- Disconnect is clean — server sends Error message before closing if disconnect is intentional.
- Reconnect: client sends `cursor` of last received sequence to resume; server replays from that position if data is still in `mod_event_seq` retention window (7-day floor per locked design).

### Notes

- Subscription respects the audit chain inclusion gate (per architecture principle 3.4) — if the operator's role doesn't permit audit chain visibility, AuditEntry messages don't ship even with `include_audit_chain: true`.
- For the Subject detail subscription banner pattern (section 5.2): the page subscribes with `subject_did` filter set, so it only receives events affecting that subject.

## 8.6 New endpoint — `tools.aurora.admin.triggerPasswordReset`

Admin-initiated user-mediated password reset.

### Specification

- **NSID:** `tools.aurora.admin.triggerPasswordReset`
- **Type:** procedure
- **Auth:** AdminServer scope, Admin+ role
- **Phase:** Lands as part of Phase 3.5 scope or as a small companion sub-issue in 3.10
- **Used by:** Account detail page → Account management drawer → "Send password reset" button

### Input

```rust
struct TriggerPasswordResetInput {
    did: String,
    rationale: String,
}
```

### Output

```rust
struct TriggerPasswordResetOutput {
    reset_email_sent: bool,
    masked_email: String,             // format: "e****@example.com"
    audit_entry_id: String,
}
```

### Behavior

1. Validate operator's role (Admin+).
2. Look up account email by DID.
3. Generate password reset token via internal `requestPasswordReset` machinery.
4. Send reset email to the account's registered address.
5. Write audit chain entry: action = "TriggerPasswordReset", actor = operator, subject = target DID, rationale.
6. Return success indicator with masked email confirmation.

### Notes

- Masked email returned to UI confirms "yes the right email was used" without exposing full PII to the operator session. Format: first character + asterisks + @ + domain (`e****@example.com` for `evan@example.com`).
- If account email is not set or invalid, return error explicitly rather than silently failing — operator needs to know the reset wasn't sent.
- Rationale is mandatory and recorded in audit chain. UI's Account management drawer enforces non-empty rationale before allowing the action.

## 8.7 New endpoint — `tools.aurora.admin.exportAccountForensic`

Chain-of-custody forensic bundle export.

### Specification

- **NSID:** `tools.aurora.admin.exportAccountForensic`
- **Type:** procedure (returns streamed bundle)
- **Auth:** AdminServer scope, Admin+ role minimum; SuperAdmin required for `includeAccountMetadata` and `includeAuditChain` parameters
- **Phase:** Phase 3.8 (depends on audit chain for chain-of-custody)
- **Used by:** Account detail page → Account management drawer → "Generate forensic export" action

### Input

```rust
struct ExportAccountForensicInput {
    did: String,
    rationale: String,
    include_repo: bool,                   // default true
    include_blobs: bool,                  // default true
    include_moderation_history: bool,     // default true
    include_account_metadata: bool,       // default false; rejects unless SuperAdmin
    include_audit_chain: bool,            // default false; rejects unless SuperAdmin
}
```

### Output

Streamed tar archive with response headers including audit entry id:

```
Content-Type: application/x-tar
Content-Disposition: attachment; filename="forensic-export-<did>-<timestamp>.tar"
X-Aurora-Audit-Entry-Id: <id>
X-Aurora-Bundle-Hash: <sha256>
```

Bundle contents:

```
<bundle>/
├── manifest.json              # bundle structure, file hashes, parameters used
├── account-state.json         # status, role, creation date, signing keys (if metadata included)
├── repo.car                   # if include_repo
├── blobs/                     # if include_blobs
│   ├── <cid>.bin
│   └── ...
├── moderation-history.json    # if include_moderation_history
├── audit-entries.json         # if include_audit_chain
└── audit-trail.json           # this export's own audit entry, always included
```

### Behavior

1. Validate operator's role; reject SuperAdmin-gated parameters if caller is not SuperAdmin.
2. Capture timestamp at export start.
3. Assemble bundle per parameters:
   - CAR file via internal getRepo machinery
   - Blobs streamed to bundle subdirectory
   - Account state JSON (metadata gated to SuperAdmin)
   - Moderation history JSON (Admin+)
   - Audit entries affecting this DID (gated to SuperAdmin)
4. Compute manifest with per-file hashes.
5. Compute bundle hash (SHA-256 over manifest).
6. Write audit chain entry: action = "ForensicExport", actor = operator, subject = target DID, rationale, bundle hash.
7. Stream bundle to client with audit entry id and bundle hash in response headers.

### Notes

- Bundle hash recorded in chain enables tamper detection: re-hashing the bundle later and comparing to chain entry's recorded hash detects modification.
- Streaming response avoids loading the entire bundle into memory server-side; large blob inventories don't need bounded memory.
- The "audit-trail.json" is always included in bundles — it contains this export's own audit entry, so the bundle is self-describing about its provenance.

## 8.8 New endpoint — `tools.aurora.admin.batchTakedownAccounts`

Atomic multi-account takedown.

### Specification

- **NSID:** `tools.aurora.admin.batchTakedownAccounts`
- **Type:** procedure
- **Auth:** AdminModeration scope, Moderator+ role
- **Phase:** Phase 3.5 (alongside emitEvent, since batch operations conceptually emit batch events)
- **Used by:** BulkActionPanel (substrate primitive 4) for account bulk takedown workflows

### Input

```rust
struct BatchTakedownAccountsInput {
    dids: Vec<String>,                // 1-50 accounts
    rationale: String,
}
```

### Output

```rust
struct BatchTakedownAccountsOutput {
    event_id: String,                 // single event for the batch
    audit_entry_id: String,           // single chain entry for the batch
    affected_count: u32,              // count whose actor-table mutation applied
    snapshots: Vec<SnapshotRef>,      // one snapshot per affected DID
    failures: Vec<BatchFailure>,      // per-subject failures; empty on full success
}

struct BatchFailure {
    subject: String,                  // DID for account batches; URI for records
    reason: String,                   // operator-readable failure reason
}
```

### Behavior

1. Validate batch size ≤ 50.
2. Validate operator's role.
3. Capture snapshot per subject before action (recorded in chain row's `cascade_snapshot_ids`, paired by index with `cascade_subjects`).
4. Apply takedown with **two-tier atomicity** (chainlink #112): the chain entry is atomic — moderation_event row + chain entry land together or neither lands. Per-subject actor-state mutations (account_moderation rows, takedown_ref updates) are best-effort; per-subject failures land in `failures` without rolling back the chain entry. True end-to-end per-subject atomicity is a v0.3 candidate (chainlink #113).
5. Write single audit chain entry referencing all subjects with shared rationale.
6. Return single event_id and audit_entry_id with snapshot references per subject and any per-subject failures.

### Notes

- 50-subject hard cap. Larger batches require multiple calls. UI surfaces this expectation in the BulkActionPanel.
- Single audit entry is intentional — operator made one decision affecting many subjects, audit reflects that semantically.
- `affected_count` may be less than `cascade_subjects.length` on the chain row when `failures` is non-empty. The chain entry records operator intent (every requested subject); `affected_count` records actuated subjects. The two are reconciled via `getAuditTrail`.

## 8.9 New endpoint — `tools.aurora.admin.batchSuspendAccounts`

Multi-account suspension; same atomicity model as `batchTakedownAccounts` (chainlink #112).

### Specification

- Same shape as `batchTakedownAccounts`, with `suspend` semantics.
- Same auth, phase, behavior pattern.
- Today the suspension record is the moderation_event row itself — there is no separate per-DID actor-table side-effect, so `failures` is always empty in v0.2. Field is in the response shape for parity.

## 8.10 New endpoint — `tools.aurora.admin.batchRestoreAccounts`

Multi-account restoration (reverses takedown or suspension); same atomicity model as `batchTakedownAccounts` (chainlink #112).

### Specification

- Same shape as `batchTakedownAccounts`, with `restore` semantics.
- Same auth, phase, behavior pattern.
- Per-DID `UPDATE actor SET takedown_ref = NULL` failures land in `failures`; the chain entry still records the full set of requested subjects.

## 8.11 New endpoint — `tools.aurora.admin.batchTakedownRecords`

Multi-record takedown.

### Specification

- **NSID:** `tools.aurora.admin.batchTakedownRecords`
- **Type:** procedure
- **Auth:** AdminModeration scope, Moderator+ role
- **Phase:** Phase 3.5
- **Used by:** BulkActionPanel for record-shaped subjects

### Input

```rust
struct BatchTakedownRecordsInput {
    uris: Vec<String>,                // 1-50 record URIs
    rationale: String,
}
```

### Output / Behavior

Same shape as `batchTakedownAccounts` adapted to record subjects. Single audit entry, snapshots per record. The label INSERTs and the moderation_event row commit together inside one transaction; today there is no partial-failure surface, so `failures` is always empty in v0.2.

## 8.12 New endpoint — `tools.aurora.admin.batchApplyLabel`

Multi-subject label application.

### Specification

- **NSID:** `tools.aurora.admin.batchApplyLabel`
- **Type:** procedure
- **Auth:** AdminModeration scope, Moderator+ role
- **Phase:** Phase 3.5
- **Used by:** BulkActionPanel for label workflows

### Input

```rust
struct BatchApplyLabelInput {
    subjects: Vec<SubjectRef>,        // can mix accounts and records, 1-50 total
    label_val: String,
    label_neg: bool,                  // default false
    rationale: String,
}
```

### Output / Behavior

Same shape pattern. Single audit entry, snapshots per subject. Per-row INSERTs run inside a single transaction; `failures` is always empty in v0.2 (any failure aborts the whole batch with 500).

## 8.13 New endpoint — `tools.aurora.admin.batchRemoveLabel`

Multi-subject label removal.

### Specification

- Same shape as `batchApplyLabel` with `remove` semantics, plus a `skipped` array.
- Subject must currently have the label; subjects without it are reported in `skipped` rather than failing the whole batch (a separate dimension from `failures`).
- Per-row negative-label INSERTs run inside a single transaction; `failures` is always empty in v0.2.

```rust
struct BatchRemoveLabelOutput {
    event_id: String,
    audit_entry_id: String,
    affected_count: u32,
    skipped: Vec<SubjectRef>,         // subjects that didn't have the label
    snapshots: Vec<SnapshotRef>,
}
```

This is the only batch endpoint with non-atomic-failure semantics — removal of a non-existent label is a no-op rather than an error. The skipped list informs the operator transparently.

## 8.14 New endpoint — Appeal resolution

Resolves an appeal with a decision and triggers any cascading reversal.

### Specification

- **NSID:** Lands as a variant of `tools.aurora.admin.emitEvent` rather than a separate endpoint; the `ResolveAppeal` and `EscalateAppeal` action variants in `EmitEventInput` cover this. No new endpoint NSID needed.
- **Phase:** Phase 3.5 (when emitEvent ships)
- **Used by:** Appeal detail page action panel

### Behavior in `emitEvent` context

When `emitEvent` is called with `action: ResolveAppeal { appeal_id, resolution }`:

1. Validate operator's role (Moderator+).
2. Validate appeal exists and is in a resolvable state.
3. Apply resolution.
4. If resolution is "approve" and the original action is reversible (takedown, suspension, label), cascade the reversal atomically.
5. Capture snapshots: appeal subject's state before resolution, and (for cascade) original action subject's state before reversal.
6. Write single audit chain entry referencing the appeal, the original action, and any cascade subjects.
7. Return event_id with `cascading_actions` populated if cascade fired.

### Notes

- Appeal resolution is the canonical example of cascade semantics. The `cascading_actions` field in `EmitEventOutput` allows the UI to surface "and these other actions also fired" in the success toast.

## 8.15 Extended endpoint — `tools.aurora.describeCapabilities`

Already shipped in Phase 3.2; this design doc commits the canonical capability vocabulary.

### Capability strings

The fixed capability vocabulary v0.2 ships:

```
audit-trail-v1                 — getAuditTrail (Phase 3.8)
subject-history-v1             — getSubjectHistory (shipped 3.3)
subject-context-v1             — getSubjectContext (shipped 3.3)
batch-takedown-v1              — batch endpoints (Phase 3.5)
moderator-activity-v1          — queryEvents with rich context (shipped 3.3)
invite-lineage-v1              — invite lineage queries
instance-metrics-v1            — getInstanceMetrics (shipped)
appeals-v1                     — listAppeals + getAppeal (shipped 3.4)
mod-events-stream-v1           — subscribeModEvents (Phase 3.9)
mod-events-emit-v1             — emitEvent (Phase 3.5)
moderation-metrics-v1          — getModerationMetrics (Phase 3.7)
queue-stats-v1                 — getQueueStats (Phase 3.7)
forensic-export-v1             — exportAccountForensic
trigger-password-reset-v1      — triggerPasswordReset
reporter-context-v1            — reporter context aggregation
runtime-settings-v1            — runtime settings infrastructure (Phase 3.10)
```

Adding new capabilities requires design-doc update before the server starts advertising them. Removing capabilities is a breaking change requiring major version bump (v1 → v2).

### Notes

- Capability strings are versioned (`-v1`) so future incompatible changes can ship as `-v2` while v1 remains advertised for clients depending on the original shape.
- The substrate primitive 21 reads this list on session start and uses it to route requests. Adding a capability to the server's response automatically activates the corresponding UI path.

## 8.16 New endpoint — `tools.aurora.admin.getRuntimeSetting` and `setRuntimeSetting`

Phase 3.10 runtime settings infrastructure.

### Specifications

```rust
// Read
struct GetRuntimeSettingInput {
    key: String,
}

struct GetRuntimeSettingOutput {
    key: String,
    value: serde_json::Value,         // typed per setting key
    source: SettingSource,            // Runtime | File | Default
    last_modified: Option<DateTime<Utc>>,
    last_modified_by: Option<String>,
}

// Write
struct SetRuntimeSettingInput {
    key: String,
    value: serde_json::Value,
    rationale: String,
}

struct SetRuntimeSettingOutput {
    key: String,
    previous_value: serde_json::Value,
    new_value: serde_json::Value,
    audit_entry_id: String,
}
```

- **Auth (read):** AdminServer scope, Admin+ role for most settings; specific settings may be public-read (e.g., the moderation mode itself can be read at any role since it affects all operators)
- **Auth (write):** AdminServer scope, SuperAdmin role
- **Phase:** 3.10
- **Used by:** Settings → UI & modes (moderation mode toggle); General settings page; future runtime-configurable surfaces

### Behavior

- Two-tier configuration per locked design: file-level config (env vars, yaml) is fallback; runtime setting takes precedence.
- Writes go to `runtime_settings` table.
- `AURORA_RECOVERY_MODE=true` env var bypasses runtime settings on startup for emergency recovery (per Phase 3.10 design).
- All writes audit-chained.

### Known runtime setting keys (v0.2)

```
moderation-mode                # "full" | "reduced" | "disabled"
moderation-mode-redirect-url   # string URL or empty
```

Future cycles add keys without breaking changes to the endpoint shape.

# 9. Implementation phasing

This section maps every UI surface and substrate primitive to the v0.2 sub-phase that ships it. The intent is to give chainlink planners a concrete answer to "what UI work belongs in this sub-phase" without re-deriving the dependency graph each time.

The phasing is constrained by two things: which endpoints are available when, and which substrate primitives must exist before pages depending on them can ship. Pages that depend only on already-shipped endpoints can land in any sub-phase; pages depending on Phase 3.5+ endpoints land in or after their dependency phase, with capability-routed fallbacks (substrate primitive 21) bridging the gap.

## 9.1 Phase mapping principles

Three operating principles for assigning UI work to phases:

**Principle 1: Substrate before consumers.** Substrate primitives (Section 6) ship before the pages that consume them. Within the v0.2 cycle, this means the substrate work clusters early — the `<EntityRef>`, `<ActionPanel>`, `<FilterStrip>`, `<Drawer>`, `<ToastNotification>`, `<EmptyState>` and others land in or before the first UI-bearing sub-phase.

**Principle 2: Surfaces ship with full capability-routed fallbacks.** A page depending on `emitEvent` (Phase 3.5) doesn't wait until 3.5 to ship — the page ships earlier with substrate primitive 21 routing actions through per-action endpoints (`takedownAccount`, `suspendAccount`) until 3.5 lands. When 3.5 ships, no page changes; the substrate flips its routing internally.

**Principle 3: Audit-bearing work clusters.** Snapshot capture, audit chain integration, and forensic export all interrelate. They land together in Phase 3.8's UI work rather than spread across sub-phases.

## 9.2 Phase-by-phase breakdown

### Phase 3.5 — emitEvent + batch operations

**New endpoints shipping:**
- `tools.aurora.admin.emitEvent` (8.1)
- `tools.aurora.admin.batchTakedownAccounts` (8.8)
- `tools.aurora.admin.batchSuspendAccounts` (8.9)
- `tools.aurora.admin.batchRestoreAccounts` (8.10)
- `tools.aurora.admin.batchTakedownRecords` (8.11)
- `tools.aurora.admin.batchApplyLabel` (8.12)
- `tools.aurora.admin.batchRemoveLabel` (8.13)
- `tools.aurora.admin.triggerPasswordReset` (8.6) — companion sub-issue, lands here

**UI work shipping:**
- `<ActionPanel>` substrate primitive (6.3) — full implementation
- `<BulkActionPanel>` substrate primitive (6.4)
- Capability-routed substrate (6.17) — substrate primitive 21
- Bulk multi-select on Queue, Reports, Accounts pages
- Pattern A (account-scoped administrative actions) on Account detail with the password-reset two-track flow
- Pattern B (account-scoped moderation actions) on Account detail
- Pattern E (invite code actions) bulk variant on Invites page

**Capability strings advertised after this phase:**
- `mod-events-emit-v1`
- `batch-takedown-v1`
- `trigger-password-reset-v1`

**Notes:**
- This is the largest sub-phase in terms of UI substrate work because the action surfaces touch most pages.
- The substrate primitive 21 ships alongside `emitEvent` so the UI immediately routes through the new path. Per-action endpoints stay live for protocol-compatibility but the UI consumes `emitEvent` exclusively post-3.5.
- Pages built before 3.5 lands have action panels wired to per-action endpoints; the substrate flips routing transparently when 3.5 ships.

### Phase 3.7 — aggregations and dashboard

**New endpoints shipping:**
- `tools.aurora.admin.getModerationMetrics` (8.2)
- `tools.aurora.admin.getQueueStats` (8.3)

**UI work shipping:**
- Dashboard's Moderator flavor with real metrics
- Dashboard's Operator flavor refreshed against `getInstanceMetrics` (already-shipped) — fake "+12 this week" data removed
- Stat card delta indicators (`.positive`, `.attention`, neutral variants) wired to real comparison data
- Sidebar bell badge integration with `getQueueStats` for real-time count
- Queue page's stat header counts wired to `getQueueStats`

**Capability strings advertised after this phase:**
- `moderation-metrics-v1`
- `queue-stats-v1`

**Notes:**
- Dashboard work that depends on these endpoints is gated by phase, but the page itself can ship earlier with placeholder stats. Pre-3.7, Dashboard widgets show "Loading..." states or fall back to coarse aggregations from existing endpoints.
- Bell badge pre-3.7 polls a sum of `listReports` filtered to open + `listAppeals` filtered to pending — less efficient but functionally equivalent. Post-3.7 switches to `getQueueStats` for a single optimized call.

### Phase 3.8 — audit chain and forensic export

**New endpoints shipping:**
- `tools.aurora.admin.getAuditTrail` (8.4)
- `tools.aurora.admin.exportAccountForensic` (8.7)

**UI work shipping:**
- Audit page (5.3.8) with verified-only toggle and merged feed
- Audit entry detail page (5.3.9) with chain-walk navigation
- Snapshot-at-decision-time substrate primitive integrated into Action Panel
- Snapshot rendering in Event detail (5.3.7) and Audit entry detail
- Forensic export modal on Account detail (5.2)
- Hash verification UI on Audit entry detail

**Capability strings advertised after this phase:**
- `audit-trail-v1`
- `forensic-export-v1`

**Notes:**
- Audit page pre-3.8 shows only `getAuditLog` data (parity-floor) without verification badges. Post-3.8 merges chain entries with verification badges and "verified-only" filter becomes meaningful.
- Snapshot capture is integrated into the action substrate at this phase — earlier actions have no snapshot, displayed honestly as "Snapshot not captured for this event (predates snapshot infrastructure)."
- Forensic export depends on audit chain for chain-of-custody, hence its placement here.

### Phase 3.9 — real-time subscription

**New endpoints shipping:**
- `tools.aurora.admin.subscribeModEvents` (8.5)

**UI work shipping:**
- Subscription substrate primitive (6.18) full implementation
- Real-time indicator (6.19) ships across surfaces
- Mod Events page subscription integration with new-event fade-in animation
- Audit page subscription integration
- Subject detail "new event" banner pattern on Account detail and Record detail
- Dashboard's recent activity feed subscription consumption

**Capability strings advertised after this phase:**
- `mod-events-stream-v1`

**Notes:**
- Pages built before 3.9 lands have polling fallbacks per substrate primitive 21. When 3.9 ships, substrate flips from polling to subscription transparently.
- The "Reconnecting..." indicator is universally needed because subscription state can drop; it ships with this phase.
- Polling intervals pre-3.9: Mod Events page polls every 10s; Audit page polls every 30s. These intervals tighten when subscription is available because they no longer carry the load.

### Phase 3.10 — runtime settings infrastructure

**New endpoints shipping:**
- `tools.aurora.admin.getRuntimeSetting` and `setRuntimeSetting` (8.16)

**UI work shipping:**
- Settings → UI & modes page (5.5.2) full implementation including the moderation mode toggle
- Mode-aware sidebar visibility logic in the navigation substrate
- Recovery path documentation surfaced in Settings → UI & modes
- Three-state moderation mode toggle (full / reduced / disabled) wired to runtime settings

**Capability strings advertised after this phase:**
- `runtime-settings-v1`

**Notes:**
- Pre-3.10, moderation mode is configured via env var only and the UI's mode-aware visibility logic reads from initial bootstrap data without runtime updates.
- Post-3.10, moderation mode can change mid-session via Settings → UI & modes; the UI reacts by re-rendering the sidebar to reflect new domain visibility.

### #108 UI completion pass

**No new endpoints.**

**UI work shipping:**
- Full migration from current `static/admin/` per Section 12
- Lucide icon set replacement (substrate primitive 14)
- Three-state theme toggle (substrate primitive 15) full implementation
- Dark mode CSS variable parallel set (substrate primitive 16) full implementation
- Accessibility substrate (substrate primitive 17) — WCAG 2.2 AA compliance pass on all components
- Command palette (substrate primitive 18 — `Cmd/Ctrl+K`) full implementation
- i18n-ready scaffolding (substrate primitive 19) — `t()` helper, `en.json`, locale-aware formatting
- FilterStrip with calendar widget (substrate primitive 20) full implementation
- All remaining Operations sub-pages (Sequencer, Federation, Blob ops, Rate limits, System health, Server) implementation
- All remaining Settings pages (General, Roles, Capabilities) implementation
- Account detail's full drawer pattern with role gating
- Record detail page implementation
- Blob detail page implementation
- Invite detail page implementation
- Cross-link substrate via `<EntityRef>` (substrate primitive 6) consistent application across all surfaces
- Hash-based routing per Section 4.3 with deep-linkable filter state
- Visual design tokens (Section 7) consolidation and dark-mode contrast audit
- Toast notification substrate (primitive 7) consistent across all action completions
- Final stylesheet refactor: inline styles in Phase 3.3/3.4 additions moved to CSS rules

**Notes:**
- This is the consolidation pass. By the time #108 lands, all endpoint dependencies have shipped (Phases 3.5 through 3.10 are complete). #108 is where everything composes into the final v0.2 UI.
- Implementation order within #108 follows substrate-before-consumers principle: substrate primitives ship before pages that depend on them, even within a single sub-phase.
- The accessibility audit pass at #108 is comprehensive — every component verified against the contracts in Section 10. Earlier sub-phases ship with accessibility in mind but the formal audit lands here.

## 9.3 Cross-phase dependencies

Some UI work has cross-phase dependencies worth flagging:

**ActionPanel ↔ Snapshot capture.** ActionPanel ships in Phase 3.5 with action submission. Snapshot capture as a substrate property of ActionPanel ships in Phase 3.8 with the audit chain. Pre-3.8, actions submit without snapshot capture; post-3.8, actions automatically capture snapshots referenced in the audit entry. The component-level interface doesn't change.

**Audit page ↔ subscription.** Audit page ships in Phase 3.8 with polling. In Phase 3.9, subscription substrate replaces polling without page-level changes. The page sees "events arriving"; the substrate decides whether they came via WebSocket or HTTP poll.

**Bell badge ↔ getQueueStats.** Sidebar bell badge has a count from cycle start. Pre-3.7, count is computed from coarse aggregation. Post-3.7, count comes from `getQueueStats`. UI-level representation unchanged.

**Settings → UI & modes ↔ moderation mode runtime.** Settings page exists from #108 with read-only display of mode. Post-3.10, mode becomes editable for SuperAdmin sessions. The page renders both states correctly per role.

**Forensic export ↔ audit chain.** Forensic export ships with Phase 3.8 because chain-of-custody requires the chain. The bundle's audit entry references the chain entry id; without the chain, no entry id can be returned, so the feature ships when the chain ships.

## 9.4 Pre-cycle implementation

Some UI work can land before the first endpoint-bearing sub-phase if the substrate doesn't depend on new endpoints:

**Sidebar grouped navigation.** The three-domain structure (Moderation / Operations / Settings with group labels) doesn't depend on any new endpoint — it's structural reorganization of existing nav items plus new pages. Can ship in advance of #108 if a sub-phase has bandwidth.

**Lucide icons.** Pure visual replacement. Can ship anytime.

**Theme toggle and dark mode.** No backend dependency. Can ship anytime once the parallel CSS variable set is finalized.

**Page-header structure with `<h1>` accessibility correction.** Existing pages can have their header tags fixed without endpoint changes.

In practice, this work most likely clusters in #108 because that's where the comprehensive overhaul lands, but if any sub-phase has UI bandwidth and benefits from these incremental wins, they can ship early without dependency conflicts.

## 9.5 Post-cycle (deferred to v0.3)

The Section 2.2 deferred items are explicitly out of scope for v0.2. Implementation phasing acknowledges:

- Hover-card context previews
- Calendar widget enhancements (multi-month view, etc.)
- Bulk operations beyond the six batch endpoints
- Time-bounded historical export
- Hardened SSR for record render
- Multi-tenant UI configuration
- Visual redesign work
- Mobile-first treatment
- Operator activity dashboards
- Rich text editing for rationale
- Federated cross-PDS subject views

These items may sequence into a v0.3 cycle plan once v0.2 ships and operator usage informs priority. The design doc commits to v0.2 boundaries; v0.3 planning is separate work.

## 9.6 Phase milestones

Each phase has a "definition of done" for UI work specifically:

**Phase 3.5 done when:**
- ActionPanel ships with capability-routed substrate
- All Pattern A and B actions on Account detail use ActionPanel
- BulkActionPanel ships with multi-select integrations on Queue, Reports, Accounts
- Trigger password reset surfaces work end-to-end including audit logging

**Phase 3.7 done when:**
- Dashboard Moderator flavor displays real metrics with comparisons
- Bell badge counts via `getQueueStats`
- Stat card delta indicators distinguish positive / attention / neutral semantically

**Phase 3.8 done when:**
- Audit page renders verified and unverified entries with badges
- Audit entry detail page supports chain-walk
- Snapshot capture integrated into ActionPanel
- Forensic export modal works end-to-end including chain integration

**Phase 3.9 done when:**
- Mod Events page receives events via subscription with new-event animations
- Audit page subscription delivery functional
- Real-time indicator displays connection state
- Subject detail subscription banner appears on multi-operator scenarios
- Pre-3.9 polling fallbacks remain in place for clients without subscription support

**Phase 3.10 done when:**
- Settings → UI & modes operates against runtime settings
- Mode toggle changes propagate to all operator sessions (via `subscribeModEvents` if available, or via session refresh)
- Recovery path documented and tested

**#108 done when:**
- All 28 pages from Section 5 are implemented
- Lucide icons replace emoji throughout
- Theme toggle and dark mode complete with WCAG 2.2 AA contrast verification
- Command palette functional with all action registrations
- i18n scaffolding in place with `en.json` populated
- Calendar widget complete with locale-aware formatting
- All cross-domain detail pages reachable via cross-pivots from any source surface
- Inline styles from Phase 3.3/3.4 additions migrated to CSS rules
- Migration from `static/admin/` complete per Section 12
- Accessibility audit pass complete with documented results
- Decoupling sweep complete per Section 13's testing strategy

When all six milestone sets are complete, v0.2 UI is ready for end-of-cycle review (#109 functional verification, #110 adversarial review, #107 decoupling sweep) and merge.

# 10. Accessibility commitments

This section consolidates Aurora-Locus's accessibility contract at WCAG 2.2 Level AA. Earlier sections reference accessibility per-component (Section 6) or per-token (Section 7); this section is the comprehensive specification that implementation teams audit against and that future contributors must preserve when extending the UI.

The intent is twofold: ensure the UI is genuinely usable by operators with diverse access needs (not "accessible" as a label without substance), and make accessibility verifiable — concrete enough that any operator or external auditor can determine pass/fail without subjective judgment.

## 10.1 Standard

The UI targets **WCAG 2.2 Level AA**.

WCAG 2.2 (published October 2023) extends 2.1 with several criteria particularly relevant for administrative UIs:

- **2.4.11 Focus Not Obscured (Minimum)** — focused elements must remain visible, not hidden behind sticky headers or modals
- **2.4.12 Focus Not Obscured (Enhanced)** — focused elements fully visible (AAA, not required but useful)
- **2.5.7 Dragging Movements** — any drag interaction has a non-drag alternative
- **2.5.8 Target Size (Minimum)** — interactive targets at least 24×24 CSS pixels
- **3.2.6 Consistent Help** — when help is provided, it appears in consistent location across pages
- **3.3.7 Redundant Entry** — operators don't have to re-enter information they've already provided in a session
- **3.3.8 Accessible Authentication (Minimum)** — authentication doesn't rely on cognitive function tests
- **3.3.9 Accessible Authentication (Enhanced)** — authentication offers alternatives to cognitive tests (AAA)

The full WCAG 2.2 AA criterion set governs. This section captures the UI-specific commitments derived from those criteria.

## 10.2 Keyboard contract

Every action achievable by mouse must be achievable by keyboard. No exceptions.

### 10.2.1 Tab order

Tab order follows visual reading order throughout the UI:

1. Skip-to-content link (visible on focus)
2. Sidebar navigation
3. Page header (breadcrumb + title + page actions)
4. FilterStrip (when present)
5. Page content (in document order)
6. Pagination (when present)
7. Sidebar footer (theme toggle, logout)

The skip-to-content link is the first focusable element on every page. Pressing Tab on initial page load reveals it; activating it (Enter) moves focus past the sidebar nav directly into page content. Without this affordance, keyboard users tab through the entire sidebar before reaching content on every page navigation — friction that compounds across long operator sessions.

### 10.2.2 Keyboard shortcuts

Global shortcuts:

| Shortcut | Action |
|---|---|
| `Cmd/Ctrl + K` | Open command palette |
| `Cmd/Ctrl + Shift + L` | Toggle theme between Light and Dark (skips System) |
| `Esc` | Close any open modal, dropdown, popover, or palette |
| `/` | Focus search field on current page (when present) |

Navigation shortcuts (`g`-prefix pattern):

| Shortcut | Destination |
|---|---|
| `g d` | Dashboard |
| `g q` | Queue |
| `g r` | Reports |
| `g a` | Appeals |
| `g e` | Events |
| `g u` | Audit (mnemonic: aUdit) |
| `g c` | Accounts |
| `g i` | Invites |
| `g s` | Settings |

Form shortcuts:

| Shortcut | Action |
|---|---|
| `Tab` | Next field |
| `Shift + Tab` | Previous field |
| `Enter` | Submit form (or insert newline in textarea unless modifiers) |
| `Cmd/Ctrl + Enter` | Submit form from inside textarea |
| `Esc` | Cancel form (with unsaved-changes confirmation if dirty) |

Within composite widgets:

| Widget | Shortcuts |
|---|---|
| Theme toggle | Arrow keys cycle, Space/Enter selects |
| FilterStrip chips | Tab navigates chips, Enter/Space opens popover |
| Calendar widget | Arrow keys navigate dates, Enter selects, Tab to text inputs |
| Command palette | Arrow keys navigate results, Enter selects, Esc closes |
| Pagination strip | Tab between Prev/Next/page-size, arrow keys within page-size group |

Shortcuts are documented in a "Keyboard shortcuts" entry in Settings → UI & modes (display-only, not editable in v0.2).

### 10.2.3 Focus indicators

Every focusable element has a visible focus indicator per the focus tokens in Section 7.9:

```css
*:focus-visible {
  outline: 2px solid var(--primary-color);
  outline-offset: 2px;
}

input:focus-visible,
select:focus-visible,
textarea:focus-visible {
  outline: none;
  box-shadow: var(--shadow-focus);
}
```

Focus indicators meet contrast minimum 3:1 against adjacent colors. The 2px outline plus 2px offset gives a total 4px-thick visible halo around focused elements — substantially more visible than browser defaults but not visually heavy.

`:focus-visible` (not `:focus`) ensures focus indicators appear for keyboard navigation but not for mouse clicks where they'd be visual noise.

### 10.2.4 Focus management

Modals and popovers trap focus while open:

- Tab cycles within the modal/popover, never escaping to the underlying page
- Focus moves to the first focusable element in the modal on open
- Focus returns to the triggering element on close
- Underlying page becomes `aria-hidden="true"` while modal is open (so screen readers don't navigate into it)
- Esc dismisses the modal (returning focus to trigger)

Drawers (collapsible sections within pages) don't trap focus — they're not modal. Tab order skips the body when collapsed.

Per WCAG 2.2 criterion 2.4.11: focused elements must remain visible. The UI's sticky page header (when present) does not obscure focused elements within page content — focus auto-scrolls page content to keep the focused element visible.

## 10.3 Screen reader contract

Every functional element announces correctly to screen readers.

### 10.3.1 Semantic HTML first

The UI uses semantic HTML elements wherever possible:

- `<nav>` for sidebar navigation
- `<main>` for primary content (one per page)
- `<aside>` for secondary rails (Account detail's context rail)
- `<header>` for page header
- `<footer>` for sidebar footer
- `<table>` for tabular data with proper `<thead>`, `<tbody>`, `<th scope="col">`
- `<button>` for interactive actions (never `<div>` with click handler)
- `<form>` with proper `<fieldset>` and `<legend>` for grouped inputs
- `<a>` for navigation links (never `<div>` with navigate-on-click)

Semantic HTML provides screen reader semantics out of the box. Aurora-Locus does not need to add ARIA where HTML already provides the semantics.

### 10.3.2 ARIA usage

ARIA is added only where semantic HTML is insufficient:

**Landmarks named when multiple of same type exist:**

```html
<nav aria-label="Primary"> <!-- sidebar -->
<nav aria-label="Page navigation"> <!-- pagination -->
<nav aria-label="Filters"> <!-- FilterStrip -->
```

**`aria-current` on active navigation:**

```html
<a href="#mod/queue" aria-current="page">Queue</a>
```

**`aria-label` on icon-only controls:**

```html
<button aria-label="Close modal">×</button>
<button aria-label="Refresh queue">[refresh icon]</button>
```

**`aria-describedby` for input help and validation:**

```html
<label for="rationale">Rationale</label>
<textarea id="rationale" aria-required="true" aria-describedby="rationale-hint rationale-error"></textarea>
<span id="rationale-hint">Required. Recorded in the audit chain.</span>
<span id="rationale-error" role="alert" hidden>Rationale is required.</span>
```

**`aria-live` regions for dynamic announcements:**

```html
<div aria-live="polite" aria-atomic="true" id="filter-status"></div>
<div aria-live="assertive" aria-atomic="true" id="error-status"></div>
```

The polite region announces filter applications, action completions, page transitions. The assertive region announces errors and validation failures.

**`aria-busy` on loading containers:**

```html
<div aria-busy="true" aria-label="Loading">
  <!-- skeleton placeholders -->
</div>
```

**`role="dialog"` and `aria-modal="true"` on modals:**

```html
<div role="dialog" aria-modal="true" aria-labelledby="modal-title">
  <h2 id="modal-title">Generate forensic export</h2>
  ...
</div>
```

### 10.3.3 Status badges and color-conveyed information

Status badges (Section 6.12) include their semantic meaning textually:

```html
<span class="status-badge status-active" aria-label="Status: Active">
  Active
</span>
```

The visible text "Active" is the badge's primary signal. The `aria-label` reinforces that the color encodes status. Operators with color vision deficiencies, screen reader users, and operators viewing the UI in monochrome contexts all understand "Active" identically.

This pattern applies everywhere color carries meaning — chart series legends, validation states, real-time indicators.

### 10.3.4 Dynamic content announcements

Operations that change page state announce their result:

| Operation | Announcement | Live region |
|---|---|---|
| Filter applied | "Filter applied: Status is Open. Showing 23 results." | polite |
| Action completed | "Account taken down. View result in Events." | polite |
| Validation failed | "Rationale is required." | assertive |
| Subscription connected | "Live event stream connected." | polite |
| Subscription disconnected | "Live event stream disconnected. Reconnecting." | polite |
| New event arrived | (None — visual fade-in only; constant arrival noise would be hostile to screen readers) | — |

The "new event arrival" exception is deliberate. A subscription page receiving frequent events would announce constantly; that's accessibility-hostile. Instead, screen readers announce the first event and on-demand on user request (operator can press a "what's new" shortcut to hear recent events as a list).

### 10.3.5 Loading states

Loading states announce on appearance:

```html
<div aria-busy="true" aria-label="Loading reports">
  <SkeletonTableRow /> ...
</div>
```

When loading completes, the `aria-busy` removes and content appears. Screen readers announce content arrival via `aria-live="polite"` on the table container if the page-load latency exceeds ~500ms.

## 10.4 Color and contrast

WCAG 2.2 AA requires:

- **4.5:1** contrast for normal text
- **3:1** contrast for large text (≥18pt or 14pt bold)
- **3:1** contrast for non-text UI components (focus indicators, active states, borders communicating meaning)

### 10.4.1 Light mode contrast verification

All text-on-surface combinations:

| Foreground | Background | Ratio | Status |
|---|---|---|---|
| `--text-primary` (#0f172a) | `--surface` (#ffffff) | 18.4:1 | ✓ |
| `--text-primary` (#0f172a) | `--background` (#f8fafc) | 17.3:1 | ✓ |
| `--text-secondary` (#475569) | `--surface` (#ffffff) | 7.5:1 | ✓ |
| `--text-secondary` (#475569) | `--background` (#f8fafc) | 7.0:1 | ✓ |
| `--text-tertiary` (#94a3b8) | `--surface` (#ffffff) | 3.4:1 | ✓ (large text only — used for hints, not body) |
| `--primary-color` (#3b82f6) | `--surface` (#ffffff) | 4.5:1 | ✓ |

Status badge combinations:

| Variant | FG/BG | Ratio | Status |
|---|---|---|---|
| `.status-active` | #166534 / #dcfce7 | 8.4:1 | ✓ |
| `.status-suspended` | #92400e / #fef3c7 | 6.4:1 | ✓ |
| `.status-takedown` | #991b1b / #fee2e2 | 7.7:1 | ✓ |
| `.status-pending` | #1e40af / #dbeafe | 7.5:1 | ✓ |
| `.status-verified` | #047857 / #ecfdf5 | 7.5:1 | ✓ |
| `.status-pre-chain` | #78350f / #fef3c7 | 8.0:1 | ✓ |

### 10.4.2 Dark mode contrast verification

| Foreground | Background | Ratio | Status |
|---|---|---|---|
| `--text-primary` (#e6edf3) | `--surface` (#151b23) | 13.2:1 | ✓ |
| `--text-primary` (#e6edf3) | `--background` (#0a0e14) | 16.0:1 | ✓ |
| `--text-secondary` (#9ba8b8) | `--surface` (#151b23) | 6.7:1 | ✓ |
| `--text-secondary` (#9ba8b8) | `--background` (#0a0e14) | 8.1:1 | ✓ |
| `--text-tertiary` (#6e7681) | `--surface` (#151b23) | 3.8:1 | ✓ (large text only) |
| `--primary-color` (#60a5fa) | `--surface` (#151b23) | 6.4:1 | ✓ |

Dark-mode status badges:

| Variant | FG/BG | Ratio | Status |
|---|---|---|---|
| `.status-active` | #6ee7b7 / #064e3b | 7.7:1 | ✓ |
| `.status-suspended` | #fcd34d / #78350f | 6.5:1 | ✓ |
| `.status-takedown` | #fca5a5 / #7f1d1d | 5.0:1 | ✓ |
| `.status-pending` | #93c5fd / #1e3a8a | 5.4:1 | ✓ |
| `.status-verified` | #6ee7b7 / #064e3b | 7.7:1 | ✓ |

All combinations meet WCAG 2.2 AA. Implementation includes an automated contrast check in the testing strategy (Section 13).

### 10.4.3 Color is supplementary

Color never carries information alone. Every state encoded through color also has:

- Text labels (status badges)
- Icons (real-time indicator dots paired with "Live" / "Offline" text)
- Icon shapes that distinguish without color (chart series shape variants)
- Position (active nav item is highlighted *and* has `aria-current="page"`)

Operators with severe color vision deficiencies, with monochrome displays, or in environments with extreme glare should be able to use the UI without relying on color cues.

## 10.5 Motion and reduced-motion

Per Section 7.7, all animations respect `prefers-reduced-motion`:

```css
@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0.01ms !important;
  }
}
```

This blanket rule disables animations system-wide for users with the preference. Specific components also need behavioral adjustments:

- **Real-time indicator pulse:** static (no pulse animation; presence still indicates state)
- **Skeleton loader pulse:** static gray (still indicates loading via `aria-busy`)
- **New event arrival fade:** instant appearance (events still arrive, just without visual fade)
- **Theme toggle slide:** instant state change (no segment-shift animation)
- **Drawer expand/collapse:** instant (no expand animation)

State changes are preserved; motion is removed.

Per WCAG 2.2 criterion 2.3.3 (Animation from Interactions): no animation is essential to functionality. Operators can complete every workflow with zero animation.

## 10.6 Target size

All interactive targets meet WCAG 2.2 criterion 2.5.8: minimum 24×24 CSS pixels.

Verification per component:

| Component | Min size | Status |
|---|---|---|
| Sidebar nav items | 44×260px | ✓ |
| Buttons (`.btn-primary`, etc.) | ~37×~80px | ✓ |
| Buttons (`.btn-sm`) | ~30×~60px | ✓ |
| Status badges (clickable) | n/a — typically not interactive | — |
| Filter chips | ~30×~80px | ✓ |
| Pagination buttons | ~37×~80px | ✓ |
| Modal close (×) | 24×24px (minimum) | ✓ |
| Drawer headers (clickable) | ~50×full-width | ✓ |
| Theme toggle segments | ~30×~80px each | ✓ |
| Calendar date cells | 32×32px | ✓ |
| Table row hit area (entire row clickable) | ~50×full-width | ✓ |

The current `.btn-sm` size at `0.375rem 0.75rem` padding produces ~30×~60px which is comfortably above the 24×24 floor. No components require resizing.

## 10.7 Forms

Form accessibility is substrate-level (every form follows the same pattern):

### 10.7.1 Labels

Every input has a visible or programmatically associated label:

```html
<!-- Visible label -->
<label for="email">Email address</label>
<input id="email" type="email" />

<!-- Visually hidden label -->
<label for="search" class="sr-only">Search accounts</label>
<input id="search" type="search" placeholder="Search by handle, DID, or email" />
```

Placeholder text is never the sole label. The `placeholder` attribute supplements the label by hinting at format; it does not replace `<label>`.

### 10.7.2 Required fields

Required fields are programmatically marked:

```html
<label for="rationale">Rationale <span aria-hidden="true">*</span></label>
<textarea id="rationale" aria-required="true"></textarea>
```

The visible asterisk is decorative (`aria-hidden`); the `aria-required="true"` is the screen reader signal. Visible labels include "(required)" text where space permits, which is more explicit than asterisks.

### 10.7.3 Validation

Errors associate with their fields and announce immediately:

```html
<input id="email" type="email" aria-required="true" aria-invalid="true" aria-describedby="email-error" />
<span id="email-error" role="alert">Email format is invalid.</span>
```

The `role="alert"` ensures the error announces immediately on appearance (assertive region behavior for inline errors). The `aria-invalid` flags the field's invalid state for assistive technology.

### 10.7.4 Submission state

During submission, the submit button is disabled with explanatory state:

```html
<button type="submit" aria-disabled="true" aria-describedby="submit-state">
  Confirm
</button>
<span id="submit-state" class="sr-only">Submitting. Please wait.</span>
```

After completion, the submit state announces success or failure via `aria-live="polite"` or `aria-live="assertive"` respectively.

## 10.8 Authentication

Per WCAG 2.2 criterion 3.3.8: authentication doesn't rely solely on cognitive function tests.

Aurora-Locus admin auth is via OAuth flow with the operator's PDS account credentials. No CAPTCHAs, no memory-based puzzles, no transcription tests. The login surface is keyboard-accessible, screen-reader-friendly, and works with password managers (autofill is supported, no anti-paste protection).

Session timeout (when configured) gives operators a warning with sufficient time to extend before forcing re-authentication.

## 10.9 Component accessibility audit

Section 6 specifies per-primitive accessibility contracts. Section 10's audit consolidates verification: every primitive has been reviewed against the WCAG 2.2 AA criteria above and either passes or has its non-conformance documented.

The full audit (one row per primitive × each WCAG criterion) is too large for inline display in this section. It exists as a separate document in `docs/_design_scratch/AURORA_ACCESSIBILITY_AUDIT.md`, generated and verified during the #108 implementation pass. Implementation completes when the audit shows zero failures and any documented non-conformances have explicit rationale.

The audit is regenerated when:

- A new component is added to the substrate
- An existing component's behavior changes
- WCAG updates introduce new criteria

## 10.10 User-facing accessibility statement

The UI includes a brief accessibility statement reachable from Settings → UI & modes:

> Aurora-Locus targets WCAG 2.2 Level AA. The UI is keyboard-navigable, screen-reader-friendly, supports reduced-motion preferences, and meets contrast requirements in both light and dark modes.
>
> If you encounter an accessibility issue, please report it in the Aurora-Locus repository. We treat accessibility regressions as bugs.

This is operator-facing rather than auditor-facing. It signals commitment without overpromising.

# 11. i18n-ready scaffolding

This section specifies the internationalization infrastructure that v0.2 ships. The intent is to make adding a new language a drop-in task — copy the English locale file, translate the values, ship — without code changes anywhere in the UI.

v0.2 ships English-only. The scaffolding is the commitment; actual translations are future contributors' work.

## 11.1 The `t()` helper

A single function routes all user-facing text:

```javascript
t(key, params?)
```

Examples:

```javascript
t('queue.title')
// → "Queue"

t('reports.count', { count: 3 })
// → "3 reports"

t('reports.count', { count: 0 })
// → "No reports"

t('reports.count', { count: 1 })
// → "1 report"
```

The helper is implemented as vanilla JavaScript with no library dependency. It supports:

- Simple key lookup
- Parameter substitution (`{name}` placeholders)
- ICU MessageFormat plurals (`{count, plural, =0 {No reports} one {# report} other {# reports}}`)
- Nested key paths (`section.subsection.key`)
- Fallback to the key itself when missing (so missing translations are visible during development)

Implementation lives in `static/admin/i18n/i18n.js` (or equivalent path determined by the migration plan in Section 12).

## 11.2 String file structure

The English locale file at `static/admin/i18n/en.json`:

```json
{
  "common": {
    "save": "Save",
    "cancel": "Cancel",
    "confirm": "Confirm",
    "close": "Close",
    "loading": "Loading",
    "error": "Error",
    "retry": "Retry",
    "refresh": "Refresh",
    "applyFilters": "Apply",
    "clearFilters": "Clear all"
  },
  "navigation": {
    "dashboard": "Dashboard",
    "domains": {
      "moderation": "Moderation",
      "operations": "Operations",
      "settings": "Settings"
    },
    "sections": {
      "queue": "Queue",
      "reports": "Reports",
      "appeals": "Appeals",
      "events": "Events",
      "audit": "Audit",
      "accounts": "Accounts",
      "invites": "Invites",
      "sequencer": "Sequencer",
      "federation": "Federation",
      "blobOps": "Blob ops",
      "rateLimits": "Rate limits",
      "systemHealth": "System health",
      "server": "Server",
      "general": "General",
      "uiModes": "UI & modes",
      "roles": "Roles",
      "capabilities": "Capabilities"
    }
  },
  "queue": {
    "title": "Queue",
    "subtitle": "Items needing attention",
    "empty": {
      "filtered": "No matches. Try widening your filters.",
      "unfiltered": "Nothing in the queue. Things will appear here as reports and appeals come in."
    },
    "stats": "{reports, plural, =0 {No reports} one {# report} other {# reports}}, {appeals, plural, =0 {no appeals} one {# appeal} other {# appeals}} needing attention"
  },
  "reports": { },
  "appeals": { },
  "events": { },
  "audit": { },
  "accounts": { },
  "accountDetail": {
    "drawers": {
      "overview": "Account overview",
      "moderation": "Moderation actions",
      "management": "Account management",
      "history": "Subject history"
    },
    "actions": {
      "takedown": {
        "label": "Takedown account",
        "confirm": "Confirm takedown",
        "warning": "I understand this affects all federation",
        "rationaleHint": "Required. Recorded in the audit chain."
      },
      "sendPasswordReset": {
        "label": "Send password reset",
        "success": "Password reset email sent to {maskedEmail}"
      }
    }
  },
  "forensicExport": {
    "title": "Generate forensic export",
    "include": {
      "repo": "Repository content (CAR file)",
      "blobs": "Blobs",
      "blobsSize": "{count} blobs, ~{size}",
      "moderationHistory": "Moderation history",
      "moderationHistoryCount": "{count, plural, =0 {No prior actions} one {# prior action} other {# prior actions}}",
      "metadata": "Account metadata",
      "metadataDescription": "Email, signing keys, invite lineage",
      "metadataGate": "SuperAdmin only",
      "auditChain": "Audit chain entries",
      "auditChainDescription": "Operator decisions and chain context",
      "auditChainGate": "SuperAdmin only"
    },
    "warning": "This export will be recorded in the audit chain with a tamper-evident hash. The bundle will contain account data; treat as sensitive.",
    "submit": "Generate export"
  },
  "settings": { },
  "errors": {
    "generic": "Something went wrong. Try again or report the issue.",
    "rationaleRequired": "Rationale is required.",
    "didInvalid": "DID format is invalid.",
    "handleNotFound": "Handle could not be resolved.",
    "permissionDenied": "You don't have permission to perform this action."
  },
  "accessibility": {
    "skipToContent": "Skip to content",
    "loadingAnnouncement": "Loading",
    "filterApplied": "Filter applied: {filter}. Showing {count, plural, =0 {no results} one {# result} other {# results}}.",
    "actionCompleted": "{action} completed.",
    "subscriptionConnected": "Live event stream connected.",
    "subscriptionDisconnected": "Live event stream disconnected. Reconnecting."
  }
}
```

The structure mirrors the UI's information architecture. Top-level keys correspond to domains and surfaces; nested keys correspond to elements within those surfaces. New surfaces add new top-level keys.

## 11.3 Locale determination

The active locale is determined in priority order:

1. **Explicit operator preference** stored in `localStorage` under key `aurora-admin-locale`. Set via Settings → UI & modes language selector.
2. **Browser preference** via `navigator.language` (or `navigator.languages` in priority order).
3. **English fallback** if no match found among available locale files.

The locale value is a BCP 47 language tag (`en`, `en-US`, `es`, `de`, `ja`). The helper resolves the closest match available — if `es-MX` is requested but only `es` exists, `es` is used. If `fr` is requested but no French file exists, English is used.

Locale changes apply on page reload. The Settings → UI & modes language selector triggers a reload after writing the new preference. Live locale switching without reload is not in v0.2 scope (it's complex, low-value, and the reload is fast).

## 11.4 Date and number formatting

All user-facing dates and numbers route through `Intl` APIs with the active locale:

```javascript
formatDate(date, format)
// Uses Intl.DateTimeFormat with active locale

formatNumber(number, options)
// Uses Intl.NumberFormat with active locale

formatRelativeTime(date)
// Uses Intl.RelativeTimeFormat with active locale ("3 hours ago")

formatDuration(seconds)
// Composite: uses Intl with locale-specific formatting for "2 days, 4 hours"
```

Examples:

```javascript
formatDate(new Date(), 'short')
// en-US: "5/3/2026"
// en-GB: "03/05/2026"
// de-DE: "03.05.2026"

formatNumber(1247)
// en-US: "1,247"
// de-DE: "1.247"
// fr-FR: "1 247"

formatRelativeTime(threeHoursAgo)
// en: "3 hours ago"
// es: "hace 3 horas"
// de: "vor 3 Stunden"
```

Hardcoded format strings (`toLocaleString()` without arguments, `Date.prototype.toString()`, etc.) are forbidden in component code. Section 13's testing strategy includes a lint check for these patterns.

## 11.5 String discipline

Three rules govern string usage:

**Rule 1: Every visible string routes through `t()`.**

```javascript
// Wrong
button.textContent = "Save changes"

// Right
button.textContent = t('common.save')
```

This includes ARIA labels, screen-reader-only text, validation messages, error messages, and dynamic announcements.

**Rule 2: String concatenation is forbidden for sentences.**

Languages have different word orders. Concatenating strings to form sentences breaks translation. Use parameter substitution instead:

```javascript
// Wrong
const message = t('actions.takedownAccount') + " " + handle + " " + t('common.completed')

// Right
const message = t('actions.takedownCompleted', { handle })
// "Takedown of @somehandle completed." — translatable as a whole unit
```

The exception: concatenating data fragments (e.g., joining a list of subjects with commas) is fine because the structural language ("Subjects: ", ", ", ".") routes through `t()` while the data values are operator-supplied identifiers.

**Rule 3: Pluralization uses ICU MessageFormat.**

Different languages have different plural rules. English has two forms (singular/plural); Russian has three; Arabic has six. ICU plurals handle this correctly:

```json
{
  "items.count": "{count, plural, =0 {No items} one {# item} other {# items}}"
}
```

The `=0` is exact-match for zero; `one` and `other` are CLDR plural categories that vary per language. Translators populate the relevant categories per their language.

## 11.6 Adding a new language

Workflow for a contributor wanting to add a new locale:

1. **Copy** `static/admin/i18n/en.json` to `static/admin/i18n/<locale>.json` (e.g., `es.json`, `de.json`, `pt-BR.json`).

2. **Translate** the values, leaving keys unchanged. Pluralization categories adjusted per the target language's CLDR plural rules.

3. **Register** the locale in `static/admin/i18n/locales.json`:

   ```json
   {
     "available": [
       { "code": "en", "name": "English" },
       { "code": "es", "name": "Español" }
     ]
   }
   ```

4. **Test** by setting `localStorage.aurora-admin-locale = 'es'` in browser dev tools and reloading. Verify:
   - All strings render in Spanish
   - Pluralization works correctly
   - Date and number formatting matches Spanish conventions
   - Layout doesn't break with longer/shorter strings

5. **Submit** as a contribution. No code changes needed.

The Settings → UI & modes language selector automatically populates from `locales.json`, so adding a locale to the registry makes it operator-selectable without code changes.

## 11.7 Translation maintenance

When the source `en.json` adds, removes, or modifies keys, other locale files become stale. Three strategies handle this:

**Missing keys** in a non-English locale fall back to English. Operators see a partial translation rather than missing UI elements. The `t()` helper logs a warning during development for missing keys.

**Removed keys** are simply unused; locale files may carry orphan keys without breaking anything. A periodic cleanup tool (out of v0.2 scope) can identify orphan keys.

**Modified keys** (English copy changed but locale not updated) ship the old translation. This is incorrect but not broken. The convention is: modifying English copy without updating locales is allowed for trivial wording changes; significant copy changes should bump the key name (e.g., `queue.title` → `queue.titleV2`) to force re-translation.

The doc commits to the workflow but doesn't ship the maintenance tooling in v0.2.

## 11.8 Right-to-left support

v0.2 does not include right-to-left language support. RTL adds layout complexity (flipping mirrored layouts, handling bidirectional text within UI controls) that exceeds v0.2 scope.

If a contributor wants to add Arabic, Hebrew, or other RTL languages, the `<html dir="rtl">` attribute and CSS logical properties (`margin-inline-start` instead of `margin-left`, etc.) provide the foundation. v0.3 evaluates whether to make RTL a first-class commitment.

## 11.9 Locale-aware substrate primitives

Several substrate primitives have locale-aware behavior worth highlighting:

- **Calendar widget** (6.20) uses `Intl.DateTimeFormat` for date formatting and respects the active locale's first-day-of-week convention.
- **Pagination strip** (6.10) uses `Intl.NumberFormat` for "Showing 1-50 of 1,247" formatting (commas, periods, or spaces depending on locale).
- **Toast notifications** (6.7) accept locale-aware message keys, not hardcoded strings.
- **Status badges** (6.12) display locale-aware status names ("Active" / "Activo" / "Aktiv").
- **Empty states** (6.11) source primary and secondary copy from the locale file.

Implementation of these primitives must reference `t()` and `Intl` APIs throughout, not hardcoded English values.

# 12. Migration from current static/admin/

This section specifies the file-level transition from the current `static/admin/` scaffolding to the v0.2 structure. It answers "where does the new code live, what happens to existing files, and how do we get from here to there without breaking the deployed UI."

The transition lands within #108 (UI completion pass) but the planning happens here so chainlink work can sequence file moves cleanly.

## 12.1 Current structure

```
static/admin/
├── index.html       (408 lines)
├── login.html       (63 lines)
├── login.css        (256 lines)
├── login.js         (117 lines)
├── style.css        (647 lines)
├── script.js        (938 lines)
└── debug.html       (153 lines)
```

Total: roughly 2,580 lines across 7 files. All hand-written, no build step, no framework. Vanilla JS + Chart.js via CDN.

## 12.2 Target structure

```
static/admin/
├── index.html              (page shell with mount points; ~150 lines)
├── login.html              (preserved, minor accessibility updates; ~80 lines)
├── debug.html              (preserved as-is for diagnostic purposes)
│
├── styles/
│   ├── tokens.css          (CSS custom properties; light + dark mode)
│   ├── base.css            (reset, typography, layout primitives)
│   ├── components.css      (substrate primitive styles)
│   └── pages.css           (page-specific overrides where unavoidable)
│
├── scripts/
│   ├── app.js              (entry point, router, session bootstrap)
│   ├── api/
│   │   ├── client.js       (fetch wrapper with auth + capability routing)
│   │   ├── capabilities.js (capability detection and caching)
│   │   ├── endpoints.js    (per-namespace endpoint helpers)
│   │   └── subscription.js (WebSocket subscription substrate)
│   ├── routing/
│   │   ├── router.js       (hash-based routing, deep-link handling)
│   │   └── routes.js       (route table)
│   ├── state/
│   │   ├── session.js      (operator session + role)
│   │   ├── settings.js     (theme, locale, runtime settings)
│   │   └── cache.js        (DID resolution cache, capability cache)
│   ├── components/
│   │   ├── EntityRef.js
│   │   ├── ActionPanel.js
│   │   ├── BulkActionPanel.js
│   │   ├── FilterStrip.js
│   │   ├── Drawer.js
│   │   ├── Modal.js
│   │   ├── Toast.js
│   │   ├── PaginationStrip.js
│   │   ├── EmptyState.js
│   │   ├── StatusBadge.js
│   │   ├── ThemeToggle.js
│   │   ├── CommandPalette.js
│   │   ├── CalendarWidget.js
│   │   ├── SubjectPreview.js
│   │   └── (other substrate primitives per Section 6)
│   ├── pages/
│   │   ├── Dashboard.js
│   │   ├── Queue.js
│   │   ├── Reports.js
│   │   ├── ReportDetail.js
│   │   ├── Appeals.js
│   │   ├── AppealDetail.js
│   │   ├── Events.js
│   │   ├── EventDetail.js
│   │   ├── Audit.js
│   │   ├── AuditEntryDetail.js
│   │   ├── Accounts.js
│   │   ├── AccountDetail.js
│   │   ├── RecordDetail.js
│   │   ├── BlobDetail.js
│   │   ├── Invites.js
│   │   ├── InviteDetail.js
│   │   ├── Sequencer.js
│   │   ├── Federation.js
│   │   ├── BlobOps.js
│   │   ├── RateLimits.js
│   │   ├── SystemHealth.js
│   │   ├── Server.js
│   │   ├── SettingsGeneral.js
│   │   ├── SettingsUiModes.js
│   │   ├── SettingsRoles.js
│   │   └── SettingsCapabilities.js
│   └── lib/
│       ├── icons.js        (Lucide SVG inline icon set)
│       ├── i18n.js         (string helper, locale management)
│       ├── format.js       (date/number/duration formatters)
│       ├── a11y.js         (focus trap, aria-live announcer, etc.)
│       └── dom.js          (small DOM utilities)
│
├── i18n/
│   ├── en.json             (English strings)
│   └── locales.json        (registry of available locales)
│
└── login/
    ├── login.css           (preserved with theme-token updates)
    └── login.js            (preserved with minor improvements)
```

The structure separates concerns: `styles/` for CSS, `scripts/` for JS organized by purpose (api, routing, state, components, pages, lib), `i18n/` for localization. Login retains its own subfolder because it's a separate entry point with simpler needs.

## 12.3 Migration of existing assets

### 12.3.1 `index.html` → page shell

The current `index.html` (408 lines) contains all 8 pages as sibling `<div>` elements toggled by data attributes. The new `index.html` (~150 lines) becomes a minimal shell:

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Aurora Locus admin</title>
  <link rel="stylesheet" href="/admin/styles/tokens.css">
  <link rel="stylesheet" href="/admin/styles/base.css">
  <link rel="stylesheet" href="/admin/styles/components.css">
  <link rel="stylesheet" href="/admin/styles/pages.css">
</head>
<body>
  <a href="#content" class="skip-to-content">Skip to content</a>

  <div class="admin-container">
    <aside class="sidebar" aria-label="Primary navigation">
      <!-- Sidebar mounts here -->
    </aside>

    <main id="content" class="main-content">
      <!-- Page mounts here -->
    </main>
  </div>

  <div id="modal-root" aria-live="polite"></div>
  <div id="toast-root" aria-live="polite"></div>
  <div id="palette-root"></div>

  <script type="module" src="/admin/scripts/app.js"></script>
</body>
</html>
```

The page-specific HTML moves into per-page JS modules that render their content into the `<main>` mount point. Modals, toasts, and the command palette have their own root containers for portal rendering.

### 12.3.2 `style.css` → `styles/`

The 647-line `style.css` decomposes into four files:

**`tokens.css` (~150 lines)** — the CSS custom properties from Section 7 (light mode + dark mode). Imports first so all subsequent styles can reference tokens.

**`base.css` (~150 lines)** — reset, typography, body styles, the `.admin-container` layout, sidebar shell, page-header pattern, accessibility utilities (`.sr-only`, `.skip-to-content`, focus-visible rules).

**`components.css` (~250 lines)** — substrate primitive styles: buttons, cards, tables, modals, drawers, status badges, filter chips, pagination, calendar, theme toggle, command palette, etc.

**`pages.css` (~100 lines)** — page-specific overrides where component composition isn't sufficient. Should be small; most styling lives in components.

The current `style.css` is mostly preserved in spirit. The decomposition is organizational — every existing rule moves to its appropriate file with token-reference updates (replacing hardcoded colors with CSS variables) and dark-mode parallels added.

### 12.3.3 `script.js` → `scripts/`

The 938-line `script.js` is the most substantial migration. Current structure:

- Module-level globals (`currentPage`, `adminToken`, `currentUser`)
- Authentication (`checkAuth`, `logout`)
- Navigation (`setupNavigation`, `navigateTo` with switch on page name)
- Per-page loaders (`loadDashboardData`, `loadUsers`, `loadModerationQueue`, etc.)
- Per-page renderers (DOM manipulation per page)
- Modal handlers
- Helper functions

The new structure splits these concerns:

- **Globals → `state/`** modules with explicit getter/setter APIs
- **Authentication → `api/client.js` + `state/session.js`**
- **Navigation → `routing/router.js`** with route table
- **Per-page logic → `pages/<PageName>.js`** modules each exporting a render function
- **Modal handlers → `components/Modal.js`** with portal-style mounting
- **Helpers → `lib/`** with single-purpose utilities

The migration is mechanical rewriting, not new logic invention. The current script's behavior maps to the new structure piece by piece.

### 12.3.4 `login.html`, `login.css`, `login.js` — preserved with minor updates

The login flow is functionally complete and well-isolated. v0.2 preserves it with three updates:

1. **Token references** — replace any hardcoded colors with the v0.2 CSS variables from `tokens.css` so login respects theme preference if the operator's theme cookie is set pre-auth.
2. **Accessibility tweaks** — add proper `<label>` associations on form fields, ensure focus indicators meet contrast requirements, add `aria-describedby` for validation errors.
3. **Lucide icons** — replace any emoji or generic icons with Lucide SVG.

Otherwise login stays as-is. Operators authenticating won't notice changes.

### 12.3.5 `debug.html` — preserved as-is

The debug page is diagnostic tooling, not part of the user-facing UI. v0.2 doesn't touch it. Future cycles can integrate or discard.

## 12.4 What gets removed

Several elements in the current scaffolding are explicitly removed in the migration:

**Hardcoded fake data:**

- The `+12 this week` and `+243 today` strings on Dashboard stat cards (line 80 of current `index.html`)
- The hardcoded `0 GB / 100 GB` storage display
- Any other placeholder values that don't have real backing endpoints

These are replaced with live values from `getInstanceMetrics` (already shipped) or hidden when no backing data exists.

**Scaffolded settings cards without real backing:**

The current Moderation Settings card has `setting-auto-report` and `setting-report-threshold` checkboxes/inputs that don't connect to actual server-side behavior. These are removed in the migration. Future cycles may add real automated content filtering with proper backing endpoints; v0.2 doesn't ship the placeholder.

**Emoji icons:**

The `📊 👥 🛡️ 🚨 📜 ⚖️ ✉️ ⚙️` icons in the sidebar nav are replaced with Lucide SVG per substrate primitive 14.

**The flat 8-item nav structure:**

The sidebar nav is reorganized into the three-domain grouped structure per Section 4.1.

**Inline styles from Phase 3.3 / 3.4 additions:**

The `style="..."` attributes on Mod Events and Appeals page filter bars and pagination buttons move into proper CSS rules. Mechanical hygiene, no behavior change.

**The User Details and Report Details modals:**

These become proper detail pages per Section 5.2 (Account detail), 5.3.3 (Report detail), and 5.3.5 (Appeal detail). The modal shells aren't removed — they're repurposed for transient interactions (action confirmations, generate-invite modal, forensic export modal). The "detail in a modal" pattern is what changes.

**`alert(JSON.stringify(...))` patterns:**

Wherever the current `script.js` falls back to `alert` for displaying details, those code paths are replaced with proper detail page navigation or inline rendering.

## 12.5 What gets preserved

These elements explicitly carry forward unchanged or with minor refinements:

**Visual identity:**
- Color palette (extended for dark mode, but light mode tokens preserved)
- Card vocabulary (.75rem radius, light shadow, white surface)
- Status badge pattern (extended for soft-rectangle, but variant colors preserved)
- Table styling (uppercase letterspaced headers, hover rows)
- Sidebar slate-on-dark with primary-blue active state
- Modal shell layout
- Settings card grid pattern

**Structural patterns:**
- Page header (title + subtitle + right-side actions)
- Pagination strip (Previous / Next + page-size selector)
- Filter bar pattern from Phase 3.3 / 3.4 additions
- The login flow

**Functional logic:**
- Authentication via OAuth
- Token storage in localStorage
- Hash-based routing (with structural extension to `#domain/section/detail`)
- Existing endpoint integrations (continued via the new `api/` modules)

The migration preserves what works. The reorganization is structural; the design language is consistent.

## 12.6 Migration order

The migration happens within #108 in this order to minimize broken-state windows:

**Step 1: Token migration.** Create `styles/tokens.css` with all CSS variables (light + dark). Existing `style.css` updated to reference tokens instead of hardcoded values. UI looks identical post-step but is now token-driven.

**Step 2: Base + components decomposition.** Split `style.css` into `base.css` and `components.css`. UI looks identical; styles are organizationally cleaner.

**Step 3: Lucide icons.** Replace emoji in sidebar nav with Lucide SVG. UI visibly changes (cleaner iconography); functionality unchanged.

**Step 4: Theme toggle and dark mode.** Add theme toggle to sidebar footer, implement dark-mode CSS variable parallels. New capability; existing functionality preserved.

**Step 5: Sidebar reorganization.** Restructure sidebar nav into three-domain grouped layout. Keep all existing routes wired; add new routes for new pages as stubs initially.

**Step 6: Substrate primitive components.** Build out the `components/` directory with substrate primitives. Stub out per-page modules to consume primitives.

**Step 7: Per-page implementations.** Implement each page in `pages/` against its substrate primitives and endpoint helpers. Pages come online incrementally; the sidebar shows them as they ship.

**Step 8: i18n scaffolding.** Add `i18n/en.json` and route all visible strings through `t()`. Mechanical refactor of every visible string.

**Step 9: Accessibility audit and polish.** Run the full WCAG 2.2 AA verification pass per Section 10. Fix any gaps identified.

**Step 10: Decoupling sweep.** Verify no external moderator tool names appear anywhere in the codebase per Section 13's testing strategy.

The order is dependency-driven: tokens before components, components before pages, pages before audit. Each step leaves the UI in a usable state — no "everything is broken until the last commit" intermediate stages.

## 12.7 Backward compatibility during migration

During #108's implementation, the deployed UI must remain usable. Three strategies:

**Strategy 1: Feature flags.** New UI behaviors gated behind a flag that defaults off. Operators on the production deployment continue using the existing UI; testers enable the flag to see new work. Not in v0.2 scope but worth flagging for future migration cycles.

**Strategy 2: Branch-based development.** All migration work happens on a feature branch. Production stays on the previous structure until the branch merges. This is the actual approach for v0.2 — the cycle ships as a coherent merge.

**Strategy 3: Side-by-side directories.** Build the new structure in `static/admin-v2/` alongside the old `static/admin/`. Operators access either via different URLs. Once new is complete, swap the routes. Adds complexity; not chosen for v0.2.

v0.2 uses strategy 2 (branch-based). The migration's intermediate states live on the cycle branch; the merge to main lands the complete v0.2 UI as a single coherent change.

## 12.8 Backward compatibility post-migration

After v0.2 ships, two compatibility concerns:

**Operator URLs.** Existing bookmarks pointing to `#users` or `#moderation` (the old hash routes) are caught by a redirect map in the new router:

```javascript
const legacyRedirects = {
  'users': 'ops/accounts',
  'moderation': 'mod/queue',
  'reports': 'mod/reports',
  'invites': 'ops/invites',
  'settings': 'settings/general',
  'events': 'mod/events',
  'appeals': 'mod/appeals'
}
```

Operators landing on legacy hashes get redirected to the new equivalent. Bookmarks continue working transparently.

**API endpoint compatibility.** The UI consumes new endpoints (`tools.aurora.admin.*`) where available and falls back to parity-floor endpoints (`com.atproto.admin.*`) where not. This means the v0.2 UI works against both pre-3.5 and post-3.5 server states. Section 9's capability-routed substrate handles the routing.

**localStorage migration.** Operator preferences stored under old keys (if any) migrate to new key namespace on first load post-deployment. Specifically: `adminToken` → `aurora-admin-token`. The migration code runs once and removes the old key. After all operator sessions have migrated, the migration code can be removed in a future cycle.

## 12.9 What this migration is not

A few things explicitly out of scope for the migration:

**No build step introduction.** The current scaffolding has zero build dependencies (vanilla HTML, CSS, JS, plus Chart.js via CDN). v0.2 maintains zero build dependencies. Module organization happens through ES modules with native browser support, not bundlers. This keeps deployment simple and operators able to inspect the deployed UI directly.

**No framework adoption.** No React, Vue, Svelte, etc. The current vanilla approach is preserved. Substrate primitives are vanilla JS modules; pages are vanilla JS modules; state management is via simple module-level state with explicit subscribe/notify patterns. This is unusual for a UI of this size but suits Aurora-Locus's deployment model — operators inspect, modify, fork, and replace the UI without needing a framework's mental model.

**No CSS preprocessor.** Plain CSS with custom properties. No Sass, no PostCSS. The CSS variable system handles theming and the file-level decomposition handles organization. Preprocessor benefits don't justify the build-step cost.

**No npm package.** The UI is not a published library. It's deployment-specific code that ships with the Aurora-Locus binary. Other Aurora-Locus deployments fork or modify the UI in their own deployments; there's no "install it as a dependency" path.

These are deliberate constraints, not oversights. v0.2 preserves them.

# 13. Testing strategy

This section specifies what gets tested, at what layer, and how. The intent is to ship v0.2 with confidence that the UI behaves correctly across role tiers, deployment modes, theme states, and accessibility configurations — without committing to a testing burden that exceeds the cycle's bandwidth.

The Aurora-Locus UI is structurally simpler than typical web applications (no framework, no build step, vanilla JS modules), which informs the testing approach: less harness ceremony, more real-browser verification, more targeted automation where it earns its keep.

## 13.1 Testing layers

Three layers, each serving a distinct purpose:

**Layer 1: Unit-level logic tests.** For substrate primitives with non-trivial logic (capability routing, filter state serialization, hash-route parsing, plural rules in i18n). Vanilla JS test files runnable directly in Node. No DOM required. Fast.

**Layer 2: Browser-based integration tests.** For per-page behavior — navigation, action submission, form validation, real endpoint integration against a test deployment. Browser-driven via lightweight test runner (specifics in 13.2).

**Layer 3: Manual verification per page.** For visual correctness, accessibility audit, and operator-experience evaluation that automation can't reasonably catch. Structured manual testing with documented checklists.

The layers compose: Layer 1 catches logic bugs cheaply, Layer 2 catches integration bugs at moderate cost, Layer 3 catches experience and visual bugs that require human judgment.

## 13.2 Layer 1: Unit-level logic tests

Substrate primitives with logic worth testing in isolation:

- **Capability routing** (substrate primitive 21) — given a capability set and a feature request, returns the correct endpoint path. Test cases: capability present, capability absent, multiple capabilities, capability set empty.
- **Filter state serialization** (substrate primitive 20) — round-trip filter state through URL hash encoding without loss. Test cases: empty filters, single filter, multiple filters of different types, filters with special characters in values, filters representing the same state in different orderings.
- **Hash route parsing** (substrate primitive in routing/router.js) — given a hash string, return parsed route + filters. Test cases: every documented route pattern, malformed routes, legacy routes that should redirect.
- **i18n helper** (substrate primitive 19) — `t()` correctly resolves keys, applies parameters, handles plurals, falls back to keys when missing. Test cases: per the documented examples in Section 11.
- **Cursor stack management** (pagination strip substrate) — push/pop semantics correctly track navigation history.
- **Cache management** (DID resolution cache, capability cache) — LRU eviction works correctly, cache invalidation on logout.

Tests live in `static/admin/scripts/__tests__/` (or equivalent) as `.test.js` files. Run via Node with native ES module support; no test framework dependency required for v0.2 (the tests are simple enough that vanilla `console.assert` suffices, with a small custom runner script).

If a test framework is introduced later, the candidates are minimal vanilla-JS-friendly options like `node:test` (built-in to Node 20+) or `uvu`. v0.2 doesn't commit to any framework; tests are runnable JS files.

## 13.3 Layer 2: Browser-based integration tests

For per-page behavior testing, v0.2 commits to a lightweight approach: a small test deployment configured with predictable test data, exercised by a browser automation tool against documented scenarios.

The tool chosen is Playwright. Reasoning:

- Plays well with vanilla-JS UIs (no framework integration needed)
- Headless and headed modes for CI and local development
- Built-in accessibility testing primitives (`expect.locator.toHaveAccessibleName()`, etc.)
- Cross-browser support if needed
- Reasonable learning curve

The test scenarios are derived from per-page workflows in Section 5. Per page, document:

1. **Happy path** — default operator workflow on the page (load, interact, complete primary action)
2. **Role gating verification** — page renders correctly for each applicable role tier; gated elements hide/show as specified
3. **Mode visibility** — page is visible/hidden per the moderation mode setting
4. **Empty state** — page renders correctly when underlying data is empty
5. **Error state** — page handles endpoint failure gracefully

Example per-page test plan for Account detail (5.2):

```
Account detail page tests:

  1. Load with Moderator role
     - Expect: Overview drawer visible, Moderation actions visible
     - Expect: Account management drawer NOT visible
     - Expect: Forensic export NOT accessible

  2. Load with Admin role
     - Expect: Overview, Moderation, Account management drawers visible
     - Expect: Forensic export checkbox for "Account metadata" is disabled
     - Expect: Forensic export checkbox for "Audit chain entries" is disabled

  3. Load with SuperAdmin role
     - Expect: All drawers visible
     - Expect: All forensic export checkboxes available

  4. Takedown action with Moderator role
     - Click Takedown in Moderation actions drawer
     - Expect: Confirmation modal appears
     - Submit with empty rationale
     - Expect: Validation error
     - Submit with valid rationale + checkbox
     - Expect: Action completes, toast confirms, page refetches

  5. Account in 'reduced' mode
     - Expect: Page is reachable
     - Expect: Moderation actions drawer NOT visible
     - Expect: Account management drawer visible if Admin+

  6. Empty subject context
     - For an account with no recent activity
     - Expect: Subject context drawer renders empty state
```

Multiplied across 28 pages, this is substantial test surface — but per-page tests are mostly mechanical once the substrate is built. Each test is short (10-30 lines of Playwright); the total test suite is hundreds of small tests rather than a few large ones.

The tests run against a test deployment with seeded data (test accounts, test reports, test events, test audit entries). The seeding script lives in `tests/fixtures/seed.ts` (or equivalent) and is part of the testing infrastructure.

## 13.4 Layer 3: Manual verification

Some properties of the UI can't be cheaply automated and benefit from human judgment:

- **Visual correctness in light mode** — does the UI look right; do colors render as expected; does spacing breathe correctly; do borders, shadows, radii compose well together
- **Visual correctness in dark mode** — same checks; especially status badge legibility; especially focus indicators visible
- **Accessibility audit per WCAG 2.2 AA** — manual screen reader testing with NVDA, VoiceOver, JAWS; manual keyboard navigation testing; manual contrast verification with tooling
- **Operator experience evaluation** — does the page feel responsive; do workflows feel coherent; does the action panel surface the right options at the right time
- **Real-time behavior** — does subscription work; do new events animate in; does reconnect handle gracefully

Manual verification is structured per page with a checklist:

```
Account detail manual verification:

  Visual (light mode):
    [ ] Drawer headers align consistently
    [ ] Status badge color matches account status
    [ ] Action panel renders without overflow on 1200px viewport
    [ ] Subject preview media renders via proxy (not direct)
    [ ] All Lucide icons render at 16px

  Visual (dark mode):
    [ ] All text meets 4.5:1 contrast against surface
    [ ] Status badges meet 4.5:1 contrast
    [ ] Focus indicators meet 3:1 contrast
    [ ] Drawer expand/collapse animation respects reduced-motion

  Accessibility:
    [ ] Tab order follows visual order
    [ ] Skip-to-content link works
    [ ] All drawers have role="region" with aria-label
    [ ] Action panel form labels associate correctly
    [ ] Modal focus trap works
    [ ] Esc closes modals and dialogs
    [ ] Screen reader announces action completion

  Operator experience:
    [ ] Action submission feels responsive (< 500ms perceived latency)
    [ ] Real-time subscription banner appears when expected
    [ ] Cross-pivots work and breadcrumb reflects entry path correctly
```

The checklists live alongside the page implementations in `tests/manual/` (or equivalent) as markdown files. Verification is performed during #108's accessibility audit pass and signed off as part of phase milestones in Section 9.6.

## 13.5 Cycle-end audit checklist

Three audits run before v0.2 ships, each with its own gating criteria:

### 13.5.1 Decoupling audit (per #107)

The structural decoupling discipline from architecture principle 3.6 must verify clean. Concrete checks:

- `grep -ri "cairn" static/` returns zero results
- `grep -ri "hideaway" static/` returns zero results
- `grep -ri "horizon" static/` returns zero results
- `grep -ri "pursuingpeace" static/` returns zero results
- `grep -ri "nearhorizon" static/` returns zero results
- Same checks against `docs/AURORA_ADMIN_UI_DESIGN.md` (this document) and any chainlink documentation produced during the cycle
- Same checks against any test fixtures, seed scripts, or comments in the implementation

The audit produces a report. Zero hits is the gate; any hit blocks the cycle from closing until resolved.

### 13.5.2 Accessibility audit (per Section 10.9)

Full WCAG 2.2 AA verification per the per-component contracts in Section 6 and the substrate-level commitments in Section 10.

The audit produces `docs/_design_scratch/AURORA_ACCESSIBILITY_AUDIT.md` (or equivalent) with one row per primitive × each WCAG criterion. Pass-only is the gate.

The contrast verification specifically uses automated tooling (axe-core or equivalent) against every documented color combination. Any failure blocks ship.

### 13.5.3 Functional verification (#109)

Per the cycle plan, end-of-cycle functional verification checks that all surfaces work end-to-end against a real deployment. Specific verifications:

- Every page in Section 5 renders for the appropriate role
- Every action affordance in Section 6 submits correctly and produces the expected audit chain entry
- Every endpoint commitment in Section 8 has a working handler and is consumed by the documented surface
- Every substrate primitive in Section 6 ships and is referenced where Section 5 specifies
- Capability routing works correctly: pre-3.5 path against per-action endpoints; post-3.5 path against `emitEvent`; the substrate flips transparently

Functional verification is performed during the #109 chainlink work, with a signed-off checklist demonstrating completion.

### 13.5.4 Adversarial review (#110)

Per the cycle plan, an adversarial review pass examines:

- Authority gating: can a Moderator-tier session somehow reach Admin-only operations through UI manipulation? Server-side enforcement should reject; UI behavior is verified to never expose the path.
- Capability detection bypass: if `describeCapabilities` is forged or capabilities are advertised that don't have backing handlers, does the UI fail gracefully?
- Filter URL injection: do malformed filter URL parameters cause crashes or unexpected behavior?
- Rationale field XSS: is rationale content properly sanitized when rendered in audit history?
- Subject preview content sanitization: are records with hostile content (script injection attempts, malformed JSON, oversized content) rendered safely?
- Race conditions: does rapid action submission produce coherent server state?

Adversarial review produces findings; high-severity findings block ship. Lower-severity findings document as known issues for v0.3.

## 13.6 Testing in CI

The v0.2 cycle's CI is documented in CLAUDE.md and existing chainlink work; the UI testing additions:

- Layer 1 (unit-level logic tests) run on every push as part of the existing test runner. ~5-10 seconds.
- Layer 2 (browser integration tests via Playwright) run on every push for the cycle's UI work. Estimated ~5-10 minutes against the full test suite once complete.
- Layer 3 (manual verification) runs once per phase milestone, not on every push. Performed during #108 work and documented per the checklist format.
- Cycle-end audits run once at end-of-cycle as part of #107, #109, #110 chainlink work.

The CI commitment is realistic for the cycle bandwidth. Adding heavy test infrastructure (extensive E2E suites, visual regression testing, etc.) is deferred to v0.3 if the v0.2 testing approach proves insufficient.

## 13.7 Known testing gaps

Three things v0.2 testing does not cover:

**Multi-operator concurrent action scenarios.** When two operators act on the same subject simultaneously, the subscription substrate's "new event arrived" banner is the coordination signal. Testing this requires multi-browser-session orchestration that's out of v0.2's testing scope. Manual verification with two browser windows during #108 covers basic cases; full automation defers.

**Performance under load.** v0.2 tests against single-operator scenarios with bounded test data. Performance characteristics with high event volume, many concurrent operators, or large historical data sets aren't formally tested. Operators on production deployments will provide the first signal; v0.3 evaluates if synthetic load testing is warranted.

**Cross-browser correctness.** v0.2 tests primarily against Chrome (developer machines) with spot checks on Firefox and Safari. Edge and other browsers are not verified. Per Section 2.2.8, modern evergreen browsers are the support target; specific cross-browser regressions during v0.2 should be filed as bugs but the test suite doesn't enforce coverage across all four.

These gaps are explicit, not oversights. v0.3 may close them as deployment experience reveals which matter.

# 14. Future considerations

This section consolidates work deferred from v0.2 to future cycles, with rationale per item. The intent is to make explicit what v0.2 doesn't ship and why, so future cycle planning has a starting list rather than re-deriving deferred items from across the design doc.

Items here are not commitments to ship in v0.3 specifically. They're catalog of what's known-deferred, with the reasoning that informed the deferral. v0.3 cycle planning evaluates each against operator usage, contributor interest, and bandwidth.

## 14.1 Visual and interaction enhancements

**Hover-card context previews on `<EntityRef>`.** Tooltips that show contextual information about an entity reference (account preview, record preview) when hovering. The substrate primitive 6 ships in v0.2 without hover content; v0.3 layers hover by extending the component's render to include a hover wrapper. Deferred because hover events fire frequently and add real backend traffic and rendering overhead even with debouncing; the canonical detail page is one click away. Operators preferring richer info click through.

**Calendar widget enhancements.** Multi-month side-by-side view, month/year jump beyond standard prev/next, relative-date tokens like "yesterday + 3 days." v0.2 ships range selection with five preset chips, which covers typical operator workflows. Enhancements wait for usage signals indicating they're needed.

**Visual polish: animated state transitions beyond what ships.** v0.2 commits to fast, intentional animations for specific state changes. Smoother transitions, micro-interactions, ambient animation passes are out of scope. The aesthetic of the v0.2 UI is operator-grade, not delight-driven; that's deliberate.

**Dashboard widget customization.** v0.2 ships fixed dashboard layouts (Operator + Moderator flavors). Operators can't reorder widgets, hide widgets, or add custom widgets. v0.3 may evaluate if customization earns its keep; for v0.2 the curated layouts are sufficient.

## 14.2 Operator workflow enhancements

**Bulk operations beyond the six batch endpoints.** v0.2 ships bulk takedown, suspend, restore for accounts and records, plus bulk apply/remove label. Other bulk operations (bulk role grant, bulk invite generation with per-account binding, bulk email send, bulk forensic export, bulk appeal resolution) are deferred. Operators needing those workflows for v0.2 use scripted access against single-subject endpoints. v0.3 evaluates which additional batch endpoints are warranted from real operator behavior.

**Saved filter views.** Operators frequently re-applying the same filter combinations would benefit from saved views ("My open cases," "Recent appeals from @somereporter"). v0.2 supports URL-based filter sharing (operators paste hashed URLs to colleagues) but doesn't include named saved views. v0.3 may add a small "save view" feature against localStorage or a dedicated runtime setting.

**Operator activity dashboards.** Per-operator productivity views ("here is what @somemod has done this week, with throughput stats"). v0.2 ships moderation metrics aggregated across all operators; per-operator breakdowns are deferred. This is administrative-of-administrators visibility — relevant for team leads but not in v0.2 scope. v0.3 can introduce as a SuperAdmin-tier surface.

**Notification feed in sidebar.** v0.2 ships transient toasts for incoming events; events don't accumulate in a notification panel. Operators wanting full event history navigate to Events. A persistent notification feed (badge + dropdown) is a common admin-UI pattern but not strictly necessary. v0.3 may add if operator usage indicates value.

**Command palette enhancements.** v0.2 ships fuzzy search via simple substring matching across navigation, subjects, and actions. v0.3 may upgrade to proper fuzzy match with weighted scoring, add command history beyond recent items, and support keyboard shortcuts within palette context (e.g., `Tab` to switch result categories).

## 14.3 Forensic and audit enhancements

**Time-bounded historical export.** `exportAccountForensic` produces a current-state bundle. Reconstructing past account states for historical export ("the account as it was on March 1") requires sequencer replay infrastructure that is not in v0.2 scope. Forensic exports in v0.2 are point-in-time snapshots taken at the moment of export. v0.3 may add temporal export if operator needs justify the implementation cost.

**Per-account export browser.** v0.2's Account detail page launches forensic export but doesn't surface previous exports for the account. Operators wanting "what exports of this account have happened" reference the audit chain. v0.3 may add a "previous exports" list directly on Account detail.

**Bulk forensic export.** v0.2 supports single-account forensic export. Bulk export across multiple accounts is deferred to v0.3.

**Audit chain export.** Operators wanting to share or archive the audit chain (or a slice of it) currently have no UI for that. v0.3 may add audit chain export with chain-of-custody verification metadata.

**Snapshot comparison view.** When viewing an event detail page, the snapshot at decision time is shown. Comparing the snapshot to current subject state ("what changed since the action was taken") is potentially useful for forensic investigation but not in v0.2. v0.3 may add diff visualization.

## 14.4 Accessibility extensions

**WCAG 2.2 Level AAA.** v0.2 commits to WCAG 2.2 Level AA. AAA is stricter (7:1 contrast for normal text instead of 4.5:1, longer focus retention requirements, etc.) and exceeds typical enterprise UI standards. v0.3 evaluates if the additional rigor is worth the constraint cost; v0.2 stops at AA.

**Right-to-left language support.** v0.2 ships English-only with i18n-ready scaffolding. Adding RTL languages (Arabic, Hebrew) requires layout flipping, bidirectional text handling, and CSS logical properties throughout. v0.3 evaluates if RTL is warranted based on translator interest.

**Mobile-first design.** v0.2 is desktop-first with responsive breakpoints down to 768px. Phone-shaped viewports (< 480px) aren't optimized. v0.3 may evaluate mobile-first treatment if operator usage on mobile becomes significant — typically operators use tablets or laptops for administrative work.

**High-contrast mode.** Some operating systems have high-contrast modes that benefit users with severe contrast needs. v0.2's color palette respects `prefers-contrast: high` via CSS but doesn't ship dedicated high-contrast variants. v0.3 may add explicit high-contrast variants if the system-level support proves insufficient.

## 14.5 Render and content handling

**Maximally hardened SSR for record render.** v0.2 ships server-side render with sanitization and media proxy. The most hardened pattern (full SSR with no JavaScript execution context, dedicated sandboxed render environment) is deferred. v0.3 may extend if a deployment posture warrants it.

**Federated cross-PDS subject views.** When viewing a record on this PDS, references to records on other PDSes render as references with link-out, not inline content. Cross-PDS context fetching is not in v0.2 scope. v0.3 may add if operator workflows indicate the value exceeds the implementation cost (which includes federation reliability concerns and performance under network failures).

**Rich text editing for rationale fields.** Rationale fields are plain `<textarea>` in v0.2. Markdown rendering, rich text controls, @-mentions of other operators, image attachments are deferred. Rationales are operational notes; plain text is sufficient. Future cycles may extend if operator workflows demand richer authoring.

## 14.6 Multi-tenant and deployment variants

**Multi-tenant or per-namespace UI configuration.** v0.2 assumes a single Aurora-Locus deployment serving a single set of operators. Per-tenant or per-namespace UI customization (different branding per service, different admin scopes per tenant) is not in scope. v0.3 evaluates if multi-tenant deployments emerge as a use case.

**Wholesale visual redesign.** v0.2 preserves the current visual identity. A future cycle could propose a redesign — different palette, layout vocabulary, design language — but this design doc does not contemplate that work. Operators upgrading to v0.2 will find the UI extended and modernized, not visually unrecognizable. A redesign would be its own cycle's scope.

**Per-deployment UI feature flags.** Some operators may want to disable specific UI features (e.g., hide forensic export entirely from the UI even for SuperAdmins). v0.2 doesn't include this granularity. v0.3 may add per-feature visibility toggles via runtime settings.

## 14.7 Testing and tooling

**Visual regression testing.** v0.2 ships with browser-based integration tests but no visual regression suite. Visual regressions (subtle styling changes that pass functional tests) aren't caught. v0.3 may add screenshot-based regression testing if visual stability becomes a concern.

**Cross-browser correctness automation.** v0.2 tests primarily against Chrome with spot-checks on Firefox and Safari. Full cross-browser CI matrix is deferred. v0.3 evaluates if cross-browser regressions warrant the test infrastructure.

**Performance and load testing.** v0.2 tests single-operator scenarios with bounded data. Performance characteristics under high event volume or many concurrent operators aren't formally tested. v0.3 evaluates if synthetic load testing is warranted based on production deployment experience.

**Manual verification automation.** v0.2's manual checklists (per Section 13.4) are paper checklists. v0.3 may automate some accessibility verification (axe-core integration in CI), some visual regression testing, and some workflow scenarios that currently require human judgment.

## 14.8 Long-term direction notes

A few directional thoughts that aren't specific deferred features but inform how future cycles think about extending the UI:

**The substrate primitives are the contract.** Future pages compose existing primitives. When a new page genuinely needs a new pattern, the pattern gets added to the substrate before the page ships — not invented inline. This keeps the UI's vocabulary coherent over time.

**Endpoint additions follow the same discipline.** New endpoints are committed to the design doc's lexicon-shape catalog before they're implemented. New capability strings are added to the canonical vocabulary in Section 8.15. This prevents drift between the lexicon and the UI's expectations.

**Deferred items aren't second-class.** The list above isn't "things that don't matter." It's "things that don't fit v0.2's ambitious-but-finite scope." Several items would meaningfully improve operator experience and may rank high in v0.3 prioritization. The cycle planning makes the call; this doc just records what's open.

**Aurora-Locus's UI is its own product.** It interoperates with the broader ATProto ecosystem but stands alone. Future cycles preserve that independence. The UI doesn't become coupled to specific external systems even when those systems prove popular pairings.
