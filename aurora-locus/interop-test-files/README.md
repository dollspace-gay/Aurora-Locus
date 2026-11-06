# atproto interop test files

A subset of the language-neutral fixtures from
<https://github.com/bluesky-social/atproto-interop-tests>, unchanged.
Upstream licenses them under CC0 1.0 (public domain dedication); the copy
carried in the bluesky-social/atproto repository is under MIT / Apache-2.0.

Only the files locus's own tests read are here:

- `syntax/`: handle, TID, record key, NSID, AT-URI and datetime vectors.
- `data-model/`: JSON ⇄ DAG-CBOR ⇄ CID fixtures and valid/invalid records.
- `firehose/commit-proof-fixtures.json`: MST trees, commits and the
  blocks a sync 1.1 consumer needs to verify them.
- `crypto/signature-fixtures.json`: P-256 and K-256 signatures,
  including high-S and DER-encoded ones atproto refuses.

The tests that use them name the file they read.
