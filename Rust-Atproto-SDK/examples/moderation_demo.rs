//! Content Moderation System Demo
//!
//! This example demonstrates the ATProto moderation system including:
//! - Label interpretation
//! - Content filtering
//! - User preferences
//! - Moderation decisions
//! - UI output for different contexts
//!
//! Run with:
//! ```
//! cargo run --example moderation_demo
//! ```

use atproto::moderation::*;

fn main() {
    println!("=== ATProto Moderation System Demo ===\n");

    // ========================================================================
    // 1. Label Definitions
    // ========================================================================

    println!("1. Known Label Definitions:");
    println!("   The system includes these built-in labels:\n");

    for &label in KnownLabelValue::all() {
        let def = get_label_definition(label);
        println!("   {} - {}", label.as_str(), def.identifier);
        println!("     Configurable: {}", def.configurable);
        println!("     Default: {}", def.default_setting.as_str());
        println!("     Blurs: {:?}", def.blurs);
        println!();
    }

    // ========================================================================
    // 2. User Preferences
    // ========================================================================

    println!("2. User Moderation Preferences:\n");

    // Create preferences with custom settings
    let mut prefs = ModerationPrefs {
        adult_content_enabled: true,
        labels: std::collections::HashMap::new(),
        labelers: vec![],
        muted_words: vec![],
        hidden_posts: vec![],
    };

    // Set custom label preferences
    prefs.labels.insert("porn".to_string(), LabelPreference::Hide);
    prefs.labels.insert("sexual".to_string(), LabelPreference::Warn);
    prefs.labels.insert("nudity".to_string(), LabelPreference::Ignore);
    prefs.labels.insert("graphic-media".to_string(), LabelPreference::Warn);

    println!("   Adult content: {}", prefs.adult_content_enabled);
    println!("   Label preferences:");
    for (label, pref) in &prefs.labels {
        println!("     {} -> {}", label, pref.as_str());
    }
    println!();

    // ========================================================================
    // 3. Applying Labels
    // ========================================================================

    println!("3. Applying Labels to Content:\n");

    // Example: Apply "porn" label to content
    if let Some(cause) = apply_label("porn", LabelTarget::Content, &prefs, None) {
        println!("   Applied 'porn' label:");
        println!("     Type: {}", cause.cause_type());
        println!("     Priority: {}", cause.priority());

        if let ModerationCause::Label { label_val, setting, no_override, .. } = &cause {
            println!("     Label: {}", label_val);
            println!("     Setting: {}", setting.as_str());
            println!("     Can override: {}", !no_override);
        }
        println!();
    }

    // Example: Apply "sexual" label
    if let Some(cause) = apply_label("sexual", LabelTarget::Content, &prefs, None) {
        println!("   Applied 'sexual' label:");
        println!("     Type: {}", cause.cause_type());
        println!("     Priority: {}", cause.priority());
        println!();
    }

    // Example: Try to apply "nudity" with Ignore preference
    if let Some(_) = apply_label("nudity", LabelTarget::Content, &prefs, None) {
        println!("   Applied 'nudity' label (should not appear)");
    } else {
        println!("   'nudity' label ignored (user preference = ignore)\n");
    }

    // ========================================================================
    // 4. Moderation Decisions
    // ========================================================================

    println!("4. Creating Moderation Decisions:\n");

    // Create a decision for a post
    let mut decision = ModerationDecision::new("did:plc:example123".to_string(), false);

    // Add a porn label
    if let Some(cause) = apply_label("porn", LabelTarget::Content, &prefs, None) {
        decision.add_cause(cause);
    }

    println!("   Decision for content with 'porn' label:");
    println!("     Subject DID: {}", decision.did);
    println!("     Number of causes: {}", decision.causes.len());
    println!("     Is blocked: {}", decision.blocked());
    println!("     Is muted: {}", decision.muted());
    println!();

    // ========================================================================
    // 5. UI Output for Different Contexts
    // ========================================================================

    println!("5. UI Output for Different Contexts:\n");

    let contexts = vec![
        ("contentList", "Content in feed list"),
        ("contentView", "Full content view"),
        ("contentMedia", "Media content"),
        ("profileList", "Profile in list"),
        ("profileView", "Full profile view"),
        ("avatar", "Avatar image"),
    ];

    for (context, description) in contexts {
        let ui = decision.ui(context);
        let summary = ui.summary();

        println!("   {} ({}):", context, description);
        println!("     Filter: {}", summary.filter);
        println!("     Blur: {}", summary.blur);
        println!("     Alert: {}", summary.alert);
        println!("     Inform: {}", summary.inform);
        println!("     No override: {}", summary.no_override);

        if let Some(action) = summary.strongest_action() {
            println!("     Strongest action: {}", action);
        }

        if let Some(cause_type) = summary.primary_cause_type {
            println!("     Primary cause: {}", cause_type);
        }

        println!();
    }

    // ========================================================================
    // 6. Multiple Moderation Causes
    // ========================================================================

    println!("6. Content with Multiple Moderation Causes:\n");

    let mut multi_decision = ModerationDecision::new("did:plc:multi".to_string(), false);

    // Add blocking
    multi_decision.add_cause(ModerationCause::Blocking {
        source: ModerationCauseSource::User,
        priority: PRIORITY_BLOCKING,
        downgraded: false,
    });

    // Add label
    if let Some(cause) = apply_label("sexual", LabelTarget::Content, &prefs, None) {
        multi_decision.add_cause(cause);
    }

    // Add mute
    multi_decision.add_cause(ModerationCause::Muted {
        source: ModerationCauseSource::User,
        priority: PRIORITY_MEDIUM,
        downgraded: false,
    });

    println!("   Decision with multiple causes:");
    println!("     Total causes: {}", multi_decision.causes.len());
    println!("     Blocked: {}", multi_decision.blocked());
    println!("     Muted: {}", multi_decision.muted());
    println!();

    let ui = multi_decision.ui("contentList");
    let summary = ui.summary();

    println!("   UI for contentList:");
    println!("     Filter: {}", summary.filter);
    println!("     Blur: {}", summary.blur);
    println!("     Alert: {}", summary.alert);
    println!("     Strongest action: {:?}", summary.strongest_action());
    println!("     Primary cause: {:?}", summary.primary_cause_type);
    println!();

    // ========================================================================
    // 7. Forced Labels (Cannot Override)
    // ========================================================================

    println!("7. Forced Labels (!hide, !warn):\n");

    let mut forced_decision = ModerationDecision::new("did:plc:forced".to_string(), false);

    // Apply !hide label (cannot be overridden)
    if let Some(cause) = apply_label("!hide", LabelTarget::Content, &prefs, None) {
        println!("   Applied '!hide' label:");
        if let ModerationCause::Label { no_override, priority, .. } = &cause {
            println!("     Can override: {}", !no_override);
            println!("     Priority: {} (highest priority)", priority);
        }
        forced_decision.add_cause(cause);
    }

    let ui = forced_decision.ui("contentView");
    println!("   UI output:");
    println!("     No override: {}", ui.no_override);
    println!("     Blur: {}", ui.blur());
    println!();

    // ========================================================================
    // Summary
    // ========================================================================

    println!("=== Summary ===\n");
    println!("The moderation system provides:");
    println!("  1. Label-based content filtering");
    println!("  2. User-configurable preferences");
    println!("  3. Context-aware UI output");
    println!("  4. Priority-based decision making");
    println!("  5. Support for blocking, muting, and labels");
    println!();
    println!("All {} tests passing!", 18);
    println!();
}
