#!/usr/bin/env python3
"""Vendors the reference PDS's curated known-schema list.

Reads a checkout of the reference PDS (bluesky-social/atproto at the
release locus tracks), takes every NSID resolved in
packages/pds/lexicons.json, and copies its lexicon document into
crates/locus-repo/lexicons/<nsid>.json, then writes
crates/locus-repo/src/known_lexicons.rs, which embeds them.

Documents come from packages/pds/lexicons (what `lex install` resolved),
then the root lexicons/ tree (com.atproto.*, tools.ozone.*), then
lexicons-overrides/.

Usage: scripts/vendor-lexicons.py <reference checkout>
"""

import json
import os
import shutil
import sys

ref = sys.argv[1]
root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
out_dir = os.path.join(root, "crates/locus-repo/lexicons")
manifest = json.load(open(os.path.join(ref, "packages/pds/lexicons.json")))

shutil.rmtree(out_dir, ignore_errors=True)
os.makedirs(out_dir)
nsids = sorted(manifest["resolutions"])
for nsid in nsids:
    rel = nsid.replace(".", "/") + ".json"
    for base in ("packages/pds/lexicons", "lexicons", "lexicons-overrides"):
        src = os.path.join(ref, base, rel)
        if os.path.exists(src):
            break
    else:
        sys.exit(f"no document for {nsid}")
    doc = json.load(open(src))
    if doc.get("id") != nsid:
        sys.exit(f"{src} declares id {doc.get('id')!r}, expected {nsid}")
    with open(os.path.join(out_dir, nsid + ".json"), "w") as f:
        json.dump(doc, f, indent=2, sort_keys=True)
        f.write("\n")

with open(os.path.join(root, "crates/locus-repo/src/known_lexicons.rs"), "w") as f:
    f.write("//! The reference PDS's curated known-schema list, vendored by\n")
    f.write("//! `scripts/vendor-lexicons.py`. Generated: don't edit by hand.\n\n")
    f.write("pub static KNOWN_LEXICONS: &[(&str, &str)] = &[\n")
    for nsid in nsids:
        f.write(f'    ("{nsid}", include_str!("../lexicons/{nsid}.json")),\n')
    f.write("];\n")
print(len(nsids), "lexicons vendored")
