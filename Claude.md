## Quick Start

Instructions for Claude
For all work in this repository, you must use the beads issue tracker.
Use the bd command-line tool to create, manage, and close issues.
Do not use markdown files for creating to-do lists or for tracking your work. All issues and bugs are to be tracked via bd.

bd - Dependency-Aware Issue Tracker

Issues chained together like beads.

GETTING STARTED
  bd init   Initialize bd in your project
            Creates .beads/ directory with project-specific database        
            Auto-detects prefix from directory name (e.g., myapp-1, myapp-2)

  bd init --prefix api   Initialize with custom prefix
            Issues will be named: api-1, api-2, ...

CREATING ISSUES
  bd create "Fix login bug"
  bd create "Add auth" -p 0 -t feature
  bd create "Write tests" -d "Unit tests for auth" --assignee alice

VIEWING ISSUES
  bd list       List all issues
  bd list --status open  List by status
  bd list --priority 0  List by priority (0-4, 0=highest)
  bd show bd-1       Show issue details

MANAGING DEPENDENCIES
  bd dep add bd-1 bd-2     Add dependency (bd-2 blocks bd-1)
  bd dep tree bd-1  Visualize dependency tree
  bd dep cycles      Detect circular dependencies

DEPENDENCY TYPES
  blocks  Task B must complete before task A
  related  Soft connection, doesn't block progress
  parent-child  Epic/subtask hierarchical relationship
  discovered-from  Auto-created when AI discovers related work

READY WORK
  bd ready       Show issues ready to work on
            Ready = status is 'open' AND no blocking dependencies
            Perfect for agents to claim next work!

UPDATING ISSUES
  bd update bd-1 --status in_progress
  bd update bd-1 --priority 0
  bd update bd-1 --assignee bob

CLOSING ISSUES
  bd close bd-1
  bd close bd-2 bd-3 --reason "Fixed in PR #42"

DATABASE LOCATION
  bd automatically discovers your database:
    1. --db /path/to/db.db flag
    2. $BEADS_DB environment variable
    3. .beads/*.db in current directory or ancestors
    4. ~/.beads/default.db as fallback

AGENT INTEGRATION
  bd is designed for AI-supervised workflows:
    • Agents create issues when discovering new work
    • bd ready shows unblocked work ready to claim
    • Use --json flags for programmatic parsing
    • Dependencies prevent agents from duplicating effort
	
GIT WORKFLOW (AUTO-SYNC)
  bd automatically keeps git in sync:
    • ✓ Export to JSONL after CRUD operations (5s debounce)
    • ✓ Import from JSONL when newer than DB (after git pull)
    • ✓ Works seamlessly across machines and team members
    • No manual export/import needed!
  Disable with: --no-auto-flush or --no-auto-import