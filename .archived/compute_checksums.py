#!/usr/bin/env python3
"""Compute SHA-256 checksums for migration files"""

import hashlib
import os

migrations_dir = "migrations"

for filename in sorted(os.listdir(migrations_dir)):
    if filename.endswith(".sql") and not filename.startswith("postgres"):
        filepath = os.path.join(migrations_dir, filename)
        with open(filepath, 'rb') as f:
            content = f.read()
            checksum = hashlib.sha256(content).hexdigest()
            version = filename.replace(".sql", "").split("_")[0]
            description = "_".join(filename.replace(".sql", "").split("_")[1:])
            print(f"    ({version}, '{description}', CURRENT_TIMESTAMP, 1, X'{checksum}', 0),")
