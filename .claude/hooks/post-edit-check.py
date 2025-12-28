#!/usr/bin/env python3
"""
Post-edit hook that reminds Claude to check for dead code and incomplete implementations.
Runs after Write/Edit tool usage.
"""

import json
import sys


def main():
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, Exception):
        sys.exit(0)

    tool_name = input_data.get("tool_name", "")
    tool_input = input_data.get("tool_input", {})

    # Only run for file modifications
    if tool_name not in ("Write", "Edit"):
        sys.exit(0)

    file_path = tool_input.get("file_path", "")

    # Skip non-code files
    code_extensions = (
        '.rs', '.py', '.js', '.ts', '.tsx', '.jsx', '.go', '.java',
        '.c', '.cpp', '.h', '.hpp', '.cs', '.rb', '.php', '.swift',
        '.kt', '.scala', '.zig', '.odin'
    )

    if not any(file_path.endswith(ext) for ext in code_extensions):
        sys.exit(0)

    # Provide reminder context
    output = {
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": f"""After modifying {file_path}:
- Verify no TODO/FIXME placeholders were left behind
- Check for unused imports that should be removed
- Ensure error handling is complete (no unwrap/panic on user input)
- If this change requires updates elsewhere, make them now"""
        }
    }

    print(json.dumps(output))
    sys.exit(0)


if __name__ == "__main__":
    main()
