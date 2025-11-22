#!/usr/bin/env python3
"""
Database provisioning script - mimics install.sh database initialization
Creates account.sqlite and did_cache.sqlite with proper schemas
"""

import sqlite3
import os

def provision_account_db():
    """Create empty account.sqlite database - schema will be created by sqlx migrations"""
    db_path = "data/account.sqlite"

    # Ensure data directory exists
    os.makedirs("data", exist_ok=True)

    # Create empty database
    conn = sqlite3.connect(db_path)
    conn.close()
    print(f"Created empty {db_path} (schema will be created by migrations)")

def provision_did_cache_db():
    """Create empty did_cache.sqlite database - schema will be created by sqlx migrations"""
    db_path = "data/did_cache.sqlite"

    # Ensure data directory exists
    os.makedirs("data", exist_ok=True)

    # Create empty database
    conn = sqlite3.connect(db_path)
    conn.close()
    print(f"Created empty {db_path} (schema will be created by migrations)")

if __name__ == "__main__":
    print("Provisioning databases...")
    provision_account_db()
    provision_did_cache_db()
    print("\nDatabase provisioning complete!")
