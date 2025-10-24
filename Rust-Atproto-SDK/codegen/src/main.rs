//! ATProto Lexicon Code Generator
//!
//! Generates Rust code from ATProto Lexicon JSON schema files.

mod codegen;
mod lexicon;

use codegen::CodeGenerator;
use lexicon::LexiconDoc;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("ATProto Lexicon Code Generator");
    println!("==============================\n");

    let lexicons_dir = Path::new("lexicons");
    let output_dir = Path::new("../src/client");

    if !lexicons_dir.exists() {
        eprintln!("Error: lexicons directory not found");
        eprintln!("Please create a 'lexicons' directory with lexicon JSON files");
        return Ok(());
    }

    // Scan for all lexicon files
    let mut lexicon_files = Vec::new();
    for entry in WalkDir::new(lexicons_dir).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() && entry.path().extension().map_or(false, |e| e == "json") {
            lexicon_files.push(entry.path().to_path_buf());
        }
    }

    println!("Found {} lexicon files\n", lexicon_files.len());

    // Parse all lexicons
    let mut generator = CodeGenerator::new();
    let mut lexicons = Vec::new();

    for file_path in &lexicon_files {
        println!("Parsing: {}", file_path.display());

        let content = fs::read_to_string(file_path)?;
        let doc: LexiconDoc = serde_json::from_str(&content)?;

        generator.add_lexicon(doc.clone());
        lexicons.push(doc);
    }

    println!("\nGenerating Rust code...\n");

    // Generate code for each lexicon
    let mut generated_count = 0;
    let mut skipped_count = 0;
    let mut module_tree: HashMap<PathBuf, HashSet<String>> = HashMap::new();

    for doc in &lexicons {
        println!("Generating: {}", doc.id);

        match generator.generate(doc) {
            Ok(code) => {
                // Determine output file path
                let output_path = get_output_path(&doc, output_dir);

                // Create parent directories
                if let Some(parent) = output_path.parent() {
                    fs::create_dir_all(parent)?;
                }

                // Write generated code
                fs::write(&output_path, code)?;

                println!("  -> {}", output_path.display());
                generated_count += 1;

                // Track modules for mod.rs generation
                track_module(&mut module_tree, &doc.id, output_dir);
            }
            Err(e) if e.starts_with("Skipping") => {
                println!("  ⚠ {}", e);
                skipped_count += 1;
            }
            Err(e) => {
                eprintln!("  ✗ Error: {}", e);
                skipped_count += 1;
            }
        }
    }

    println!("\nGenerating module files...\n");
    generate_mod_files(&module_tree)?;

    println!("\nCode generation complete!");
    println!("\nGenerated {} files", generated_count);
    println!("Skipped {} files", skipped_count);

    Ok(())
}

/// Get the output file path for a lexicon
fn get_output_path(doc: &LexiconDoc, base: &Path) -> PathBuf {
    // Convert NSID to path
    // e.g., "com.atproto.identity.resolveHandle" -> "com/atproto/identity/resolve_handle.rs"

    let parts: Vec<&str> = doc.id.split('.').collect();
    let mut path = base.to_path_buf();

    // Add namespace directories
    for part in &parts[..parts.len() - 1] {
        path.push(part);
    }

    // Add file name (snake_case)
    let file_name = heck::AsSnakeCase(parts[parts.len() - 1]).to_string();
    path.push(format!("{}.rs", file_name));

    path
}

/// Track module hierarchy for mod.rs generation
fn track_module(module_tree: &mut HashMap<PathBuf, HashSet<String>>, nsid: &str, base: &Path) {
    let parts: Vec<&str> = nsid.split('.').collect();
    let file_name = heck::AsSnakeCase(parts[parts.len() - 1]).to_string();

    // Track each level of the module hierarchy
    let mut current_path = base.to_path_buf();

    for (i, part) in parts[..parts.len() - 1].iter().enumerate() {
        current_path.push(part);

        // Add this directory to parent's modules
        if i == 0 {
            // Top level - add to base
            module_tree.entry(base.to_path_buf())
                .or_insert_with(HashSet::new)
                .insert(part.to_string());
        } else {
            // Nested level - add to parent directory
            let mut parent_path = base.to_path_buf();
            for p in &parts[..i] {
                parent_path.push(p);
            }
            module_tree.entry(parent_path)
                .or_insert_with(HashSet::new)
                .insert(part.to_string());
        }
    }

    // Add the file itself to its parent directory
    module_tree.entry(current_path)
        .or_insert_with(HashSet::new)
        .insert(file_name);
}

/// Generate mod.rs files for all directories
fn generate_mod_files(module_tree: &HashMap<PathBuf, HashSet<String>>) -> Result<(), Box<dyn std::error::Error>> {
    for (dir_path, modules) in module_tree {
        let mod_path = dir_path.join("mod.rs");

        let mut mod_content = String::new();
        mod_content.push_str("//! Auto-generated module file\n\n");

        let mut sorted_modules: Vec<_> = modules.iter().collect();
        sorted_modules.sort();

        for module in sorted_modules {
            mod_content.push_str("pub mod ");
            mod_content.push_str(module);
            mod_content.push_str(";\n");
        }

        fs::write(&mod_path, mod_content)?;
        println!("  -> {}", mod_path.display());
    }

    Ok(())
}
