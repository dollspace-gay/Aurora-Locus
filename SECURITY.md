# Security policy

## Reporting a vulnerability

Please report security problems privately, not in a public issue or
discussion.

Use GitHub's private vulnerability reporting: on this repository, open the
**Security** tab and choose **Report a vulnerability**. Include:

- what the problem is and what an attacker could do with it;
- the version or commit you tested;
- steps to reproduce, or a proof of concept;
- any configuration it depends on (database, blob store, proxy mode,
  private tier on or off).

You'll get an acknowledgement, and updates as the problem is confirmed and
fixed. Please give us a reasonable time to release a fix before you publish
details. Once it's fixed, the advisory credits you unless you'd rather not be
named.

## Supported versions

aurora-locus is before its first release. Until 0.1.0 is tagged, fixes land
on `main` only. After that, security fixes go to the latest 0.1.x release.

## Scope

In scope: the code in this repository (the `aurora-locus` binary and its
crates) and the deployment files under `aurora-locus/deploy/` (installer,
Docker image, compose stack, Caddy configuration, systemd unit).

Out of scope, so please report them to their own projects:

- Bluesky's reference PDS, AppView, relay and other Bluesky services;
- the private AppView and other services locus is configured to talk to;
- third-party dependencies, unless locus uses them in an unsafe way.

## How locus is secured

[docs/security.md](docs/security.md) describes the measures locus takes and
the limits of what it protects, including what the server operator can see.
