//! Arc 2 Step 4 (§6.4.4) — phrase-presence test for the five
//! contract commitments installed during Arc 2.
//!
//! Each Arc 2 step landed a literal phrase in a doc comment at a
//! canonical source location. Together they declare:
//!
//! 1. Subject vocabulary stability (`Subject`, `ReportSubject`).
//! 2. describeCapabilities response field stability.
//! 3. Capability string versioning convention.
//! 4. Action-ID surfacing on Aurora-namespace handlers.
//!
//! This test asserts each phrase is present in the doc block
//! immediately preceding its committed item — the `before` framing
//! is load-bearing. Phrase presence elsewhere in the file is too
//! weak; a future refactor that splits the type out and leaves a
//! stale doc-block elsewhere shouldn't pass.
//!
//! ## Limitation
//!
//! The helpers walk source as plain strings (no `syn` parser). They
//! handle the current shape of the canonical commitments cleanly:
//! contiguous `///` lines (or `//!` for module-level) immediately
//! preceding the item, no `#[attr]` interleaving, no nested macros
//! producing the doc block. If a future commitment requires more
//! sophisticated parsing, switching to `syn` is the natural next
//! step.

use std::fs;

/// Find `item_signature` in `file_contents`, walk backwards line by
/// line skipping blank lines + `#[...]` attributes, collect the
/// contiguous `///` doc-comment block immediately preceding the
/// item, and assert `phrase` appears as a literal substring in that
/// block.
///
/// Panics with a useful message naming the file, item, expected
/// phrase, and the doc block found (or that none was found).
fn assert_phrase_in_docblock_before(
    file_path: &str,
    file_contents: &str,
    item_signature: &str,
    phrase: &str,
) {
    let item_idx = file_contents.find(item_signature).unwrap_or_else(|| {
        panic!(
            "{}: item signature `{}` not found — has the declaration been renamed or moved?",
            file_path, item_signature,
        )
    });
    // Find the line containing the item: walk back to the start of
    // the line that contains item_idx.
    let line_start = file_contents[..item_idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
    // Collect lines preceding the item, walking backwards.
    let preceding = &file_contents[..line_start];
    let lines: Vec<&str> = preceding.lines().collect();
    let mut docblock = Vec::new();
    for line in lines.iter().rev() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            // Blank line: end of docblock if we've collected
            // anything; skip if we haven't (allows a blank
            // separator between attrs and doc comment, or none).
            if !docblock.is_empty() {
                break;
            }
            continue;
        }
        if trimmed.starts_with("#[") || trimmed.starts_with("#![") {
            // Attribute — skip; doc comment can sit before
            // attributes that adorn the item.
            continue;
        }
        if let Some(content) = trimmed.strip_prefix("///") {
            docblock.push(content);
            continue;
        }
        // Anything else stops the walk.
        break;
    }
    // We collected in reverse order; flip back.
    docblock.reverse();
    let collected = docblock.join("\n");
    if !collected.contains(phrase) {
        panic!(
            "{}: doc block immediately before `{}` does not contain the required \
             phrase `{}`. Collected doc block:\n---\n{}\n---\n\
             If the phrase was deliberately removed, the contract commitment is \
             gone — restore it OR (if dropping the contract is intentional) \
             update Arc 2's tests, the operator-facing doc at \
             `docs/operator/contract-stability.md`, and the CHANGELOG together.",
            file_path, item_signature, phrase, collected,
        );
    }
}

/// Module-level variant: assert `phrase` appears in the contiguous
/// `//!` block at the top of `file_contents`.
fn assert_phrase_in_module_doc(file_path: &str, file_contents: &str, phrase: &str) {
    let mut block = String::new();
    for line in file_contents.lines() {
        let trimmed = line.trim_start();
        if let Some(content) = trimmed.strip_prefix("//!") {
            block.push_str(content);
            block.push('\n');
            continue;
        }
        if trimmed.is_empty() {
            // Blank line inside the module-doc block: keep walking.
            block.push('\n');
            continue;
        }
        // Anything else (attribute, item, comment) ends the block.
        break;
    }
    if !block.contains(phrase) {
        panic!(
            "{}: module-level //! doc block does not contain the required \
             phrase `{}`. Collected block:\n---\n{}\n---",
            file_path, phrase, block,
        );
    }
}

// ====================================================================
// Subject vocabulary stability — Subject + ReportSubject
// (Arc 2 Step 1, §6.4.1)
// ====================================================================

#[test]
fn subject_enum_has_variant_stability_phrase() {
    let path = "src/admin/defs.rs";
    let contents = fs::read_to_string(path).unwrap();
    assert_phrase_in_docblock_before(
        path,
        &contents,
        "pub enum Subject {",
        "variant stability is committed",
    );
}

#[test]
fn report_subject_enum_has_variant_stability_phrase() {
    let path = "src/api/moderation.rs";
    let contents = fs::read_to_string(path).unwrap();
    assert_phrase_in_docblock_before(
        path,
        &contents,
        "pub enum ReportSubject {",
        "variant stability is committed",
    );
}

// ====================================================================
// describeCapabilities response shape stability
// (Arc 2 Step 3, §6.4.3)
// ====================================================================

#[test]
fn describe_capabilities_response_has_field_stability_phrase() {
    let path = "src/api/admin.rs";
    let contents = fs::read_to_string(path).unwrap();
    // Note: `DescribeCapabilitiesResponse` is private to admin.rs
    // (no `pub`), so the search anchors on `struct
    // DescribeCapabilitiesResponse {` rather than `pub struct ...`.
    assert_phrase_in_docblock_before(
        path,
        &contents,
        "struct DescribeCapabilitiesResponse {",
        "field stability is committed",
    );
}

// ====================================================================
// Capability string versioning convention
// (Arc 2 Step 4, §6.4.4)
// ====================================================================

#[test]
fn aurora_capability_extensions_has_versioning_pattern() {
    let path = "src/api/admin.rs";
    let contents = fs::read_to_string(path).unwrap();
    // `aurora_capability_extensions` is a private free function.
    assert_phrase_in_docblock_before(
        path,
        &contents,
        "fn aurora_capability_extensions(",
        "<kebab-family>-v<integer>",
    );
}

// ====================================================================
// Action-ID surfacing — module-level commitment in audit_chain
// (Arc 2 Step 2, §6.4.2)
// ====================================================================

#[test]
fn audit_chain_module_has_action_id_contract_phrase() {
    let path = "src/admin/audit_chain.rs";
    let contents = fs::read_to_string(path).unwrap();
    // Module-level: search the leading //! block.
    assert_phrase_in_module_doc(
        path,
        &contents,
        "action-ID contract for Aurora-namespace handlers",
    );
}

// ====================================================================
// Audit-trail read contract — sixth phrase, added Arc 3 Step 3
// (§7.3.1, §7.4.3)
// ====================================================================

#[test]
fn get_audit_trail_output_has_audit_trail_read_phrase() {
    let path = "src/api/aurora_admin.rs";
    let contents = fs::read_to_string(path).unwrap();
    // Brace-anchored per Arc 2 Step 4 lesson: bare
    // `pub struct GetAuditTrailOutput` would also substring-match
    // `pub struct GetAuditTrailOutputThing`. Anchor on the open
    // brace to nail the actual declaration.
    assert_phrase_in_docblock_before(
        path,
        &contents,
        "pub struct GetAuditTrailOutput {",
        "audit-trail read contract is committed",
    );
}

// ====================================================================
// Operator-facing doc existence + non-emptiness
// ====================================================================

#[test]
fn operator_contract_stability_doc_exists_and_nonempty() {
    let path = "docs/operator/contract-stability.md";
    let contents = fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "{} must exist: {}. The operator-facing summary of Arc 2's \
             stability contracts lives here; create or restore it.",
            path, e,
        )
    });
    assert!(
        !contents.trim().is_empty(),
        "{} must be non-empty", path,
    );
}

#[cfg(test)]
mod self_tests {
    //! Unit tests for the helpers. Each helper is small and pure;
    //! these tests document the parsing semantics and catch
    //! regressions in the doc-block walking.
    use super::*;

    #[test]
    fn assert_phrase_in_docblock_before_finds_phrase_above_item() {
        let src = "\
/// outer comment
/// the magic phrase is here
pub fn target() {}
";
        // Should not panic.
        assert_phrase_in_docblock_before(
            "src/test.rs",
            src,
            "pub fn target(",
            "the magic phrase is here",
        );
    }

    #[test]
    fn assert_phrase_in_docblock_before_skips_attributes() {
        let src = "\
/// header
/// the phrase
#[cfg(test)]
#[allow(dead_code)]
pub fn target() {}
";
        assert_phrase_in_docblock_before(
            "src/test.rs",
            src,
            "pub fn target(",
            "the phrase",
        );
    }

    #[test]
    #[should_panic(expected = "does not contain the required phrase")]
    fn assert_phrase_in_docblock_before_panics_when_phrase_absent() {
        let src = "\
/// some other text
pub fn target() {}
";
        assert_phrase_in_docblock_before(
            "src/test.rs",
            src,
            "pub fn target(",
            "the missing phrase",
        );
    }

    #[test]
    #[should_panic(expected = "item signature `pub fn nonexistent(` not found")]
    fn assert_phrase_in_docblock_before_panics_when_item_missing() {
        let src = "/// doc\npub fn other() {}\n";
        assert_phrase_in_docblock_before(
            "src/test.rs",
            src,
            "pub fn nonexistent(",
            "phrase",
        );
    }

    #[test]
    fn assert_phrase_in_module_doc_finds_phrase_in_leading_block() {
        let src = "\
//! module header
//! the magic phrase is here
//! more text

use std::fs;
";
        assert_phrase_in_module_doc(
            "src/test.rs",
            src,
            "the magic phrase is here",
        );
    }

    #[test]
    #[should_panic(expected = "does not contain the required phrase")]
    fn assert_phrase_in_module_doc_panics_when_phrase_absent() {
        let src = "//! header\n//! body\n\nuse std::fs;\n";
        assert_phrase_in_module_doc(
            "src/test.rs",
            src,
            "missing phrase",
        );
    }
}
