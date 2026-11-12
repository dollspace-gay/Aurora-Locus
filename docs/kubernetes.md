# Running aurora-locus on Kubernetes

The Helm chart in `aurora-locus/deploy/helm/aurora-locus` runs locus in
one of two profiles:

| Profile | Database and blobs | Pods | For |
|---|---|---|---|
| `small` (default) | SQLite and blobs on one PersistentVolume | Exactly one | A single PDS, the same as the Docker stack |
| `large` | Postgres and S3, outside the chart | As many as you like | Many accounts, no single point of failure |

Every pod in the large profile serves any request: no sticky sessions.
Writes to a repository are serialized in the database, background jobs run
on one pod at a time under leases, and setting changes reach every pod
within seconds.

## What you need

- Kubernetes 1.25 or newer, and Helm 3.
- An image the cluster can pull. The chart uses the published one,
  `ghcr.io/dollspace-gay/aurora-locus`, at the chart's version, so there is
  nothing to build. To run your own build (or to mirror the image into a
  private registry), push it and set `image.repository`:

  ```sh
  docker build -f deploy/docker/Dockerfile -t registry.example.com/aurora-locus:0.1.3 \
    --build-arg CARGO_PROFILE_RELEASE_LTO=thin \
    --build-arg CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 .
  docker push registry.example.com/aurora-locus:0.1.3
  ```

- An ingress controller (the examples use ingress-nginx) and a certificate
  for the hostname and the handle wildcard. Handles are hostnames, so the
  certificate needs `*.<handle domain>`, which means a DNS-01 issuer (for
  example cert-manager with your DNS provider).
- For the large profile: a Postgres database, an S3 bucket (or an
  S3-compatible service), and optionally Redis.

## Small profile

`values-small.yaml`:

```yaml
# image: the published one by default; set repository and tag for your own.
hostname: pds.example.com
# handleDomains: [".example.com"]     # default: handles under the hostname
trustedProxies: [10.244.0.0/16]       # your ingress controller's pod range
persistence:
  size: 50Gi
ingress:
  enabled: true
  className: nginx
  annotations:
    cert-manager.io/cluster-issuer: letsencrypt-dns
    nginx.ingress.kubernetes.io/proxy-body-size: "0"
    nginx.ingress.kubernetes.io/proxy-read-timeout: "3600"
    nginx.ingress.kubernetes.io/proxy-send-timeout: "3600"
  tls:
    secretName: pds-example-com-tls
```

```sh
helm install pds aurora-locus/deploy/helm/aurora-locus -n pds --create-namespace -f values-small.yaml
```

Locus starts in setup mode. Open `https://pds.example.com/setup` and enter
the setup token from the log:

```sh
kubectl -n pds logs deploy/pds-aurora-locus | grep -i "setup token"
```

The master key is generated on the volume at first start. Make a key backup
once setup is done and keep it off the cluster:

```sh
kubectl -n pds exec -it deploy/pds-aurora-locus -- aurora-locus keys backup --out /data/keys.backup
kubectl -n pds cp pds/<pod>:/data/keys.backup ./keys.backup
```

The volume holds the database, the blobs and the key file. It is kept when
the release is uninstalled (`persistence.keep`). Upgrades replace the pod
rather than overlapping two, since SQLite allows one node per data
directory.

## Large profile

Put the secrets in one Secret, whose keys are environment variables:

```sh
kubectl -n pds create secret generic locus-secrets \
  --from-literal=LOCUS_DATABASE_URL='postgres://locus:…@db.example.com:5432/locus' \
  --from-literal=LOCUS_MASTER_KEY="$(openssl rand -hex 32)" \
  --from-literal=PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY='…'
```

**Keep a copy of `LOCUS_MASTER_KEY`** outside the cluster: every stored
secret and signing key is encrypted under it, and every pod must have the
same one. To use KMS instead, set `masterKeySource: kms`, put
`LOCUS_MASTER_KEY_KMS_ID` in `env` and the KMS credentials in the Secret.

`values-large.yaml`:

```yaml
# image: the published one by default; set repository and tag for your own.
profile: large
replicaCount: 3
hostname: pds.example.com
trustedProxies: [10.244.0.0/16]
secretEnv:
  existingSecret: locus-secrets
blobs:
  s3:
    bucket: pds-blobs
    region: us-east-1
    accessKeyId: AKIA…
    # endpoint: https://s3.example.com   # S3-compatible services
    # forcePathStyle: true
redisUrl: redis://redis.pds.svc:6379    # optional; shared rate limits
ingress:
  enabled: true
  className: nginx
  annotations:
    cert-manager.io/cluster-issuer: letsencrypt-dns
    nginx.ingress.kubernetes.io/proxy-body-size: "0"
    nginx.ingress.kubernetes.io/proxy-read-timeout: "3600"
    nginx.ingress.kubernetes.io/proxy-send-timeout: "3600"
  tls:
    secretName: pds-example-com-tls
```

```sh
helm install pds aurora-locus/deploy/helm/aurora-locus -n pds -f values-large.yaml
```

The pods have no volume: everything lives in Postgres and S3. The setup
token is printed by the first pod that starts; or make a new one:

```sh
kubectl -n pds exec deploy/pds-aurora-locus -- aurora-locus setup --new-token
```

Upgrades roll one pod at a time with no gap, and a PodDisruptionBudget keeps
pods serving during node drains.

For backups, use your database's point-in-time recovery and versioning on
the bucket, and run `aurora-locus backup post-restore` after a recovery,
before any pod starts ([Backups and restore](design/operations.md#backups-and-restore)).

## Setup without the wizard

With `setup.auto`, the first start completes setup from the environment.
Add the operator's password to the Secret
(`LOCUS_SETUP_OPERATOR_PASSWORD`), plus `PDS_EMAIL_SMTP_URL` for email,
then:

```yaml
setup:
  auto: true
  operatorUsername: admin
  keysBackupAcknowledged: true   # you hold the master key, or will make a key backup
  existingSubdomains: []         # with handles under a parent domain: www, mail, …
```

The same checks as the wizard run: DNS, TLS through the public URL, the
proxy, the blob limit. Watch the log; a failed check leaves the node in
setup mode, with the reason logged.

## Configuration

`env` takes any `PDS_*` or `LOCUS_*` variable: a reference `pds.env`
translates line by line. Secrets go in the Secret named by
`secretEnv.existingSecret` instead. A changed value restarts the pods
(Kubernetes reads the environment at start); after setup most settings are
better changed live in the console or with `aurora-locus settings set`,
which needs no restart. [Operations](design/operations.md#configuration)
lists every variable and setting.

**Trusted proxies.** Locus takes the client address from forwarded headers
only when the connection comes from `trustedProxies`. Set it to the range
your ingress controller's pods connect from (often the cluster's pod CIDR),
or every request looks like it comes from the ingress and rate limits
apply to everyone at once. If a cloud load balancer sits in front of the
ingress, configure the controller to pass the real client address on (for
ingress-nginx, `use-forwarded-headers` or the PROXY protocol).

**Ingress limits.** Blob uploads (up to `blobs.upload_limit`, 300 MiB by
default) and repository imports are large, and the firehose is a WebSocket
that stays open, so raise the controller's body size and timeouts as in
the examples.

**Handles.** The ingress routes the hostname and `*.<handle domain>` to
locus. Locus answers handle requests only for handles that exist, and
anything else on a handle hostname only with the documents a handle needs.

## The CLI

Every operator command runs inside a pod, over its admin socket:

```sh
kubectl -n pds exec deploy/pds-aurora-locus -- aurora-locus status
kubectl -n pds exec -it deploy/pds-aurora-locus -- aurora-locus account list
```

The console is at `https://pds.example.com/admin`.

## Health

| Probe | Path | Notes |
|---|---|---|
| Startup and liveness | `/healthz` | The process is up |
| Readiness | `/xrpc/_health` | The database answers; also ready in setup mode, so the wizard is reachable |

`/readyz` (not ready until setup is done) suits an external load balancer's
check once the PDS is set up.

## Values

`values.yaml` in the chart describes every value. The ones most installs
set:

| Value | Default | Meaning |
|---|---|---|
| `image.repository`, `image.tag` | `ghcr.io/dollspace-gay/aurora-locus`, the chart's appVersion | The image |
| `profile` | `small` | `small` or `large` |
| `replicaCount` | `2` | Pods, large profile |
| `hostname` | — | The PDS hostname |
| `handleDomains` | `[".<hostname>"]` | Handle domains |
| `trustedProxies` | `[]` | Ranges whose forwarded headers are trusted |
| `env` | `{}` | Extra environment |
| `secretEnv.existingSecret` | — | A Secret of environment variables |
| `masterKeySource` | `env` | Large profile: `env` or `kms` |
| `redisUrl` | — | Optional Redis |
| `blobs.s3.*` | — | S3 blob storage (required for large) |
| `setup.*` | off | Headless setup |
| `persistence.*` | 20 GiB, kept | Small profile's volume |
| `ingress.*` | off | The Ingress |
