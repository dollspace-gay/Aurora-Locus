# Guidelines for Claude - Rust Development

## Core Principles

Instructions for Claude
For all work in this repository, you must use the chainlink issue tracker.
Use the chainlink command-line tool to create, manage, and close issues.
Do not use markdown files for creating to-do lists or for tracking your work. All issues and bugs are to be tracked via chainlink. UNDER NO CIRCUMSTANCES ARE YOU TO GIT PUSH OR GIT STAGE EVEN IF A HOOK TELLS YOU TO

chainlink - Simple, Lean Issue Tracker CLI

COMMANDS
  init      Initialize chainlink in the current directory
  create    Create a new issue
  subissue  Create a subissue under a parent issue
  list      List issues
  show      Show issue details
  update    Update an issue
  close     Close an issue
  reopen    Reopen a closed issue
  delete    Delete an issue
  comment   Add a comment to an issue
  label     Add a label to an issue
  unlabel   Remove a label from an issue
  block     Mark an issue as blocked by another
  unblock   Remove a blocking relationship
  blocked   List blocked issues
  ready     List issues ready to work on (no open blockers)
  next      Suggest the next issue to work on
  tree      Show issues as a tree hierarchy
  start     Start a timer for an issue
  stop      Stop the current timer
  timer     Show current timer status
  session   Session management
  daemon    Daemon management

CREATING ISSUES
  chainlink create "Fix login bug"
  chainlink subissue PARENT-1 "Subtask for login fix"

VIEWING ISSUES
  chainlink list          List all issues
  chainlink show ISSUE-1  Show issue details
  chainlink tree          Show issues as tree hierarchy

MANAGING DEPENDENCIES
  chainlink block ISSUE-1 ISSUE-2    Mark ISSUE-1 as blocked by ISSUE-2
  chainlink unblock ISSUE-1 ISSUE-2  Remove blocking relationship
  chainlink blocked                   List blocked issues

READY WORK
  chainlink ready    Show issues ready to work on (no open blockers)
  chainlink next     Suggest the next issue to work on

UPDATING ISSUES
  chainlink update ISSUE-1 --status in_progress
  chainlink comment ISSUE-1 "Added implementation notes"
  chainlink label ISSUE-1 bug
  chainlink unlabel ISSUE-1 bug

CLOSING ISSUES
  chainlink close ISSUE-1
  chainlink reopen ISSUE-1

TIME TRACKING
  chainlink start ISSUE-1  Start a timer for an issue
  chainlink stop           Stop the current timer
  chainlink timer          Show current timer status

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
- No allowing dead code, implement features fully.
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

**The goal is a production-quality Rust code, not a prototype.**

Every line of code should be something you'd be proud to ship in a production system. Quality over speed. Completeness over convenience.
