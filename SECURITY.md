# Security policy

Boxquell exists to contain software you do not fully trust, so reports about ways around its boundaries are especially welcome.

## Reporting a vulnerability

Please report privately, not in a public issue:

- Use **[Report a vulnerability](https://github.com/Boxquell/boxquell/security/advisories/new)** on GitHub (private vulnerability reporting).

Include what you did, what happened, what you expected, and the version or commit. A small reproduction helps most. You can expect a first answer within a week. Please give a fix reasonable time before publishing details; you will be credited in the advisory unless you prefer otherwise.

## Supported versions

Boxquell is in early development. Only the latest release and the `main` branch receive fixes.

## What counts as a vulnerability

Boxquell promises the following about a sandbox. Breaking one of them from inside a sandbox is a vulnerability:

- **Network policy.** A guest cannot reach a destination its rules do not allow, cannot reach private or local addresses without a rule naming them, and cannot keep talking during a lockdown.
- **Secrets.** The real value of a secret never enters the guest and is sent only to the hosts the secret is bound to.
- **Files.** A guest sees only the folders shared with it.
- **Control.** A guest cannot call the daemon's API or change its own rules, secrets or settings.
- **Log.** A connection attempt cannot be hidden from the network log.

Also in scope: the daemon's API being usable without its token, and recipe repositories being able to replace official recipes.

## What is out of scope

- A program inside a sandbox damaging that sandbox, or the project folder that was shared with it on purpose.
- Data leaving through destinations you allowed. An allowed host can receive anything the agent sends there.
- A read-only shared folder being remounted writable by root in the guest. Read-only shares guard against accidents; this limit is documented.
- Flaws in the hypervisor (KVM, libkrun, QEMU, HVF, WHPX) or the guest kernel. Please report those upstream; do tell us if Boxquell's configuration makes one reachable.
- Recipes from repositories you added yourself. They run their own install scripts in the sandbox.

## How the project guards itself

Dependencies are watched by Dependabot and audited in CI, the code is scanned with CodeQL, and release binaries are built by GitHub Actions from tagged commits.
