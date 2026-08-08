# Packaging

Shehata Git is published through package managers as well as by direct
download, because most people meeting a Windows installer they downloaded from
the internet meet a SmartScreen warning first. Installing through a package
manager avoids that conversation entirely, and it is how this audience installs
things anyway.

The installers are **not code-signed**. A certificate costs more per year than
an unfunded open-source project earns, so every release publishes a SHA-256
checksum beside its installer and the build runs in public in GitHub Actions.
That is verifiable, which is the part that matters; it is simply not the part
Windows shows a badge for.

## What is published

| Channel | Command | Where the manifest lives |
|---|---|---|
| Direct download | — | the release itself |
| Scoop | `scoop bucket add shehata https://github.com/moshehata95/Shehata-Git` then `scoop install shehata-git` | `bucket/shehata-git.json` |
| Homebrew | `brew tap moshehata95/shehata` then `brew install --cask shehata-git` | [moshehata95/homebrew-shehata](https://github.com/moshehata95/homebrew-shehata) |
| winget | `winget install Shehata.ShehataGit` | `packaging/winget/<version>/` |

## How the manifests stay correct

Each manifest repeats the version and the installer checksum. A stale one is
worse than a missing one: it sends someone to a file whose checksum no longer
matches, and the failure looks like corruption rather than like a packaging
mistake.

They are therefore generated, never hand-edited:

```bash
node scripts/make-packages.mjs <version> <windows-sha256> <macos-sha256>
```

The release workflow runs this after the installers are published, using the
checksums it just generated, and commits the result. The Homebrew tap updates
itself on a schedule by reading the latest release, so it needs no token for
this repository.

## Submitting to winget

winget is the one channel that is not automatic. Microsoft reviews every
package, so the manifests are generated here and submitted by hand:

1. Fork [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs).
2. Copy `packaging/winget/<version>/` to
   `manifests/s/Shehata/ShehataGit/<version>/` in the fork.
3. Validate locally: `winget validate --manifest <folder>`, then
   `winget install --manifest <folder>` to confirm it installs.
4. Open a pull request. Automated validation runs first, then a reviewer.

The first submission takes the longest, because the package identifier and the
publisher are being established. Later versions are usually mechanical.

## Intel macOS

Only an Apple silicon disk image is published, so the cask declares
`depends_on arch: :arm64`. Homebrew will refuse to install on an Intel Mac
rather than install something that cannot run — an honest failure is better
than a confusing one.
