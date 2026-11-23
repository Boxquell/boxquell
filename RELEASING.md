# Releasing

A release is cut by pushing a version tag. The tag decides the version: nothing is bumped by hand, and no release commit is needed.

## Before

- `CHANGELOG.md` on `main` describes the release under `## [Unreleased]` (or under `## [X.Y.Z] - Unreleased` if you prepared it by version). The workflow refuses a tag whose changelog section is empty.
- CI on `main` is green.

## Cut the release

```sh
git switch main && git pull
git tag -a vX.Y.Z -m "Boxquell X.Y.Z"
git push origin vX.Y.Z
```

A release candidate is `vX.Y.Z-rc.N`; it becomes a pre-release and leaves the changelog and the Homebrew tap alone.

## What happens

`release.yml` runs on the tag:

1. **Check tag**: the tag has the form `vX.Y.Z`, its commit is on `main`, and the changelog has entries.
2. **Build**: Linux, macOS (Apple silicon and Intel) and Windows, with the version from the tag (`scripts/set-version.sh`). macOS and Windows are signed when the secrets of the `release` environment exist, see [docs/release-signing.md](docs/release-signing.md).
3. **Software bill of materials**: two CycloneDX files, one for the Rust crates in the command and one for the npm packages of the embedded interface.
4. **Draft release**: archives, the two `*.cdx.json` files, `install.sh`, `install.ps1` and `SHA256SUMS` are attached, the checksum file gets a build provenance attestation, and the notes are the install instructions plus the changelog section.

The release is a **draft**. Look at the files, try one, then press **Publish** on the Releases page.

Publishing starts `post-release.yml`:

- It opens a pull request `chore: sync version X.Y.Z` that writes the version into the repository and dates the changelog section. Merge it when CI is green. (Opening the pull request needs "Allow GitHub Actions to create and approve pull requests" under Settings → Actions → General.)
- It updates the formula in [Boxquell/homebrew-tap](https://github.com/Boxquell/homebrew-tap), if the secret `HOMEBREW_TAP_TOKEN` exists: a fine-grained token with write access to the contents of that repository only. Without it, do it by hand:

  ```sh
  gh release download vX.Y.Z --repo Boxquell/boxquell --pattern SHA256SUMS
  scripts/homebrew-formula.sh X.Y.Z SHA256SUMS > ../homebrew-tap/Formula/boxquell.rb
  ```

## Checking a release

```sh
sha256sum -c SHA256SUMS --ignore-missing
gh attestation verify SHA256SUMS --repo Boxquell/boxquell
```

The install scripts are tested against a pretend release on Linux, macOS and Windows by `installer.yml` whenever they change.

## If something goes wrong

- A failed run: fix the cause on `main`, delete the draft if one exists, and tag the next patch version. Release tags are protected and cannot be moved or deleted.
- A bad published release: publish a fixed patch version. Do not replace files of a published release; the checksums and the Homebrew formula point at them.
