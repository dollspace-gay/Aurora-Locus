# Guidelines for Claude - Rust ATProto SDK Development

## Core Principles

### 1. No Stubs, No Shortcuts
- **NEVER** use `unimplemented!()`, `todo!()`, or stub implementations
- **NEVER** leave placeholder code or incomplete implementations
- **NEVER** skip functionality because it seems complex
- Every function must be fully implemented and working
- Every feature must be complete before moving on

### 2. Break Down Complex Tasks
- Large files or complex features should be broken into manageable chunks
- If a file is too large, discuss breaking it into smaller modules
- If a task seems overwhelming, ask the user how to break it down
- Work incrementally, but each increment must be complete and functional

### 3. Best Rust Coding Practices
- Follow Rust idioms and conventions at all times
- Use proper error handling with `Result<T, E>` - no panics in library code
- Implement appropriate traits (`Debug`, `Clone`, `PartialEq`, etc.)
- Use type safety to prevent errors at compile time
- Leverage Rust's ownership system properly
- Use `async`/`await` correctly with proper trait bounds
- Follow naming conventions:
  - `snake_case` for functions, variables, modules
  - `PascalCase` for types, structs, enums, traits
  - `SCREAMING_SNAKE_CASE` for constants
- Write clear, descriptive documentation comments (`///`)
- Keep functions focused and single-purpose

### 4. Comprehensive Testing
- Write comprehensive unit tests for every module
- Aim for high test coverage (all major code paths)
- Test edge cases, error conditions, and boundary values
- Include doc tests for public APIs
- All tests must pass before considering a file "complete"
- Test both success and failure cases

### 5. Translation Accuracy
- Translate TypeScript functionality completely and accurately
- Maintain behavior equivalence with the original TypeScript
- Don't add features that weren't in the original
- Don't remove features from the original
- Document any unavoidable differences between TS and Rust

### 6. Code Quality Standards
- No warnings from `cargo clippy`
- No warnings from `cargo build`
- Format code with `rustfmt` conventions
- Clear, self-documenting code with meaningful variable names
- Add comments for complex logic, but prefer clear code over comments
- Keep functions reasonably sized (< 100 lines ideally)

### 7. Dependencies
- Only add dependencies when necessary
- Use well-maintained, popular crates
- Document why each dependency is needed
- Keep the dependency tree minimal

### 8. Error Handling
- Create specific error types for each module using `thiserror`
- Provide helpful error messages
- Use `Result` types consistently
- Never use `.unwrap()` in library code (only in tests)
- Use `.expect()` only when failure is truly impossible

### 9. Documentation
- Every public item must have documentation comments
- Include examples in doc comments when helpful
- Document panics, errors, and safety considerations
- Keep docs up to date with code changes

### 10. Work Process
- Translate one file at a time completely
- Run tests after every file
- Ensure all tests pass before moving to next file
- Ask for clarification if requirements are unclear
- Discuss approach before starting large/complex files

## What to Do When Facing Complexity

**DON'T:**
- Stub it out
- Skip it
- Say "we'll come back to it"
- Implement a simplified version

**DO:**
- Analyze the dependencies
- Break it into smaller pieces
- Translate dependencies first
- Ask the user for guidance on approach
- Propose a phased implementation plan where each phase is complete

## Example of Breaking Down a Complex File

If `agent.ts` is 1,595 lines:

**WRONG:**
```rust
pub struct Agent {
    // TODO: implement this later
}

impl Agent {
    pub fn new() -> Self {
        unimplemented!()
    }
}
```

**RIGHT:**
1. Identify dependencies (session-manager, xrpc, etc.)
2. Translate dependencies first
3. Break agent.ts into logical sections:
   - Session management
   - HTTP client integration
   - Preferences API
   - Labeling configuration
   - Proxy configuration
4. Implement each section completely
5. Write comprehensive tests for each section

## Quality Checklist Before Marking a File "Complete"

- [ ] All functionality from original TypeScript is implemented
- [ ] No `todo!()` or `unimplemented!()` macros
- [ ] Comprehensive unit tests written and passing
- [ ] Doc tests written for public APIs
- [ ] All tests pass (`cargo test`)
- [ ] No compiler warnings
- [ ] No clippy warnings (run `cargo clippy`)
- [ ] Code follows Rust best practices
- [ ] Error handling is proper and comprehensive
- [ ] Documentation is complete and accurate
- [ ] Behavior matches TypeScript version

## Remember

**The goal is a production-quality Rust SDK, not a prototype.**

Every line of code should be something you'd be proud to ship in a production system. Quality over speed. Completeness over convenience.
