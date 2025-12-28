#!/usr/bin/env python3
"""
Post-edit hook that detects stub patterns and runs linters.
Runs after Write/Edit tool usage.
"""

import json
import sys
import os
import re
import subprocess

# Stub patterns to detect (compiled regex for performance)
STUB_PATTERNS = [
    (r'\bTODO\b', 'TODO comment'),
    (r'\bFIXME\b', 'FIXME comment'),
    (r'\bXXX\b', 'XXX marker'),
    (r'\bHACK\b', 'HACK marker'),
    (r'^\s*pass\s*$', 'bare pass statement'),
    (r'^\s*\.\.\.\s*$', 'ellipsis placeholder'),
    (r'\bunimplemented!\s*\(\s*\)', 'unimplemented!() macro'),
    (r'\btodo!\s*\(\s*\)', 'todo!() macro'),
    (r'\bpanic!\s*\(\s*"not implemented', 'panic not implemented'),
    (r'raise\s+NotImplementedError\s*\(\s*\)', 'bare NotImplementedError'),
    (r'#\s*implement\s*(later|this|here)', 'implement later comment'),
    (r'//\s*implement\s*(later|this|here)', 'implement later comment'),
    (r'def\s+\w+\s*\([^)]*\)\s*:\s*(pass|\.\.\.)\s*$', 'empty function'),
    (r'fn\s+\w+\s*\([^)]*\)\s*\{\s*\}', 'empty function body'),
    (r'return\s+None\s*#.*stub', 'stub return'),
]

COMPILED_PATTERNS = [(re.compile(p, re.IGNORECASE | re.MULTILINE), desc) for p, desc in STUB_PATTERNS]


def check_for_stubs(file_path):
    """Check file for stub patterns. Returns list of (line_num, pattern_desc, line_content)."""
    if not os.path.exists(file_path):
        return []

    try:
        with open(file_path, 'r', encoding='utf-8', errors='ignore') as f:
            content = f.read()
            lines = content.split('\n')
    except (OSError, Exception):
        return []

    findings = []
    for line_num, line in enumerate(lines, 1):
        for pattern, desc in COMPILED_PATTERNS:
            if pattern.search(line):
                if 'NotImplementedError' in line and re.search(r'NotImplementedError\s*\(\s*["\'][^"\']+["\']', line):
                    continue
                findings.append((line_num, desc, line.strip()[:60]))

    return findings


def find_project_root(file_path, marker_files):
    """Walk up from file_path looking for project root markers."""
    current = os.path.dirname(os.path.abspath(file_path))
    for _ in range(10):  # Max 10 levels up
        for marker in marker_files:
            if os.path.exists(os.path.join(current, marker)):
                return current
        parent = os.path.dirname(current)
        if parent == current:
            break
        current = parent
    return None


def run_linter(file_path, max_errors=10):
    """Run appropriate linter and return first N errors."""
    ext = os.path.splitext(file_path)[1].lower()
    errors = []

    try:
        if ext == '.rs':
            # Rust: run cargo clippy from project root
            project_root = find_project_root(file_path, ['Cargo.toml'])
            if project_root:
                result = subprocess.run(
                    ['cargo', 'clippy', '--message-format=short', '--quiet'],
                    cwd=project_root,
                    capture_output=True,
                    text=True,
                    timeout=30
                )
                if result.stderr:
                    for line in result.stderr.split('\n'):
                        if line.strip() and ('error' in line.lower() or 'warning' in line.lower()):
                            errors.append(line.strip()[:100])
                            if len(errors) >= max_errors:
                                break

        elif ext == '.py':
            # Python: try flake8, fall back to py_compile
            try:
                result = subprocess.run(
                    ['flake8', '--max-line-length=120', file_path],
                    capture_output=True,
                    text=True,
                    timeout=10
                )
                for line in result.stdout.split('\n'):
                    if line.strip():
                        errors.append(line.strip()[:100])
                        if len(errors) >= max_errors:
                            break
            except FileNotFoundError:
                # flake8 not installed, try py_compile
                result = subprocess.run(
                    ['python', '-m', 'py_compile', file_path],
                    capture_output=True,
                    text=True,
                    timeout=10
                )
                if result.stderr:
                    errors.append(result.stderr.strip()[:200])

        elif ext in ('.js', '.ts', '.tsx', '.jsx'):
            # JavaScript/TypeScript: try eslint
            project_root = find_project_root(file_path, ['package.json', '.eslintrc', '.eslintrc.js', '.eslintrc.json'])
            if project_root:
                try:
                    result = subprocess.run(
                        ['npx', 'eslint', '--format=compact', file_path],
                        cwd=project_root,
                        capture_output=True,
                        text=True,
                        timeout=30
                    )
                    for line in result.stdout.split('\n'):
                        if line.strip() and (':' in line):
                            errors.append(line.strip()[:100])
                            if len(errors) >= max_errors:
                                break
                except FileNotFoundError:
                    pass

        elif ext == '.go':
            # Go: run go vet
            project_root = find_project_root(file_path, ['go.mod'])
            if project_root:
                result = subprocess.run(
                    ['go', 'vet', './...'],
                    cwd=project_root,
                    capture_output=True,
                    text=True,
                    timeout=30
                )
                if result.stderr:
                    for line in result.stderr.split('\n'):
                        if line.strip():
                            errors.append(line.strip()[:100])
                            if len(errors) >= max_errors:
                                break

    except subprocess.TimeoutExpired:
        errors.append("(linter timed out)")
    except (OSError, Exception) as e:
        pass  # Linter not available, skip silently

    return errors


def main():
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, Exception):
        sys.exit(0)

    tool_name = input_data.get("tool_name", "")
    tool_input = input_data.get("tool_input", {})

    if tool_name not in ("Write", "Edit"):
        sys.exit(0)

    file_path = tool_input.get("file_path", "")

    code_extensions = (
        '.rs', '.py', '.js', '.ts', '.tsx', '.jsx', '.go', '.java',
        '.c', '.cpp', '.h', '.hpp', '.cs', '.rb', '.php', '.swift',
        '.kt', '.scala', '.zig', '.odin'
    )

    if not any(file_path.endswith(ext) for ext in code_extensions):
        sys.exit(0)

    if '.claude' in file_path and 'hooks' in file_path:
        sys.exit(0)

    # Check for stubs
    stub_findings = check_for_stubs(file_path)

    # Run linter
    linter_errors = run_linter(file_path)

    # Build output
    messages = []

    if stub_findings:
        stub_list = "\n".join([f"  Line {ln}: {desc} - `{content}`" for ln, desc, content in stub_findings[:5]])
        if len(stub_findings) > 5:
            stub_list += f"\n  ... and {len(stub_findings) - 5} more"
        messages.append(f"""⚠️ STUB PATTERNS DETECTED in {file_path}:
{stub_list}

Fix these NOW - replace with real implementation.""")

    if linter_errors:
        error_list = "\n".join([f"  {e}" for e in linter_errors[:10]])
        if len(linter_errors) > 10:
            error_list += f"\n  ... and more"
        messages.append(f"""🔍 LINTER ISSUES:
{error_list}""")

    if messages:
        output = {
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": "\n\n".join(messages)
            }
        }
    else:
        output = {
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": f"✓ {os.path.basename(file_path)} - no issues detected"
            }
        }

    print(json.dumps(output))
    sys.exit(0)


if __name__ == "__main__":
    main()
