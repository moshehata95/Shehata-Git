// Copyright (c) 2026 Dr Mohamed Shehata. All rights reserved.
// Licensed under the MIT License. See LICENSE in the project root.

// Regenerate the package-manager manifests for a release.
//
// Every manifest repeats the version and the installer checksum, and a stale
// one is worse than a missing one: it points a user at a file that no longer
// matches. They are generated from the release itself rather than edited by
// hand.
//
//   node scripts/make-packages.mjs <version> <windows-sha256> <macos-sha256>
//
// The checksums are the ones published beside the installers.

import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const [version, windowsSha, macosSha] = process.argv.slice(2);
if (!version || !windowsSha || !macosSha) {
  console.error(
    "usage: node scripts/make-packages.mjs <version> <windows-sha256> <macos-sha256>",
  );
  process.exit(1);
}
if (!/^\d+\.\d+\.\d+$/.test(version)) {
  console.error(`not a version: ${version}`);
  process.exit(1);
}
for (const [label, value] of [
  ["windows", windowsSha],
  ["macos", macosSha],
]) {
  if (!/^[0-9a-f]{64}$/i.test(value)) {
    console.error(`${label} checksum is not a sha256: ${value}`);
    process.exit(1);
  }
}

const OWNER = "moshehata95";
const REPO = "Shehata-Git";
const HOMEPAGE = `https://github.com/${OWNER}/${REPO}`;
const DESCRIPTION =
  "Pin every local repository to one GitHub identity, so the wrong account — or a coding agent — never pushes for you.";

const release = `${HOMEPAGE}/releases/download/v${version}`;
const windowsUrl = `${release}/Shehata.Git_${version}_x64-setup.exe`;
const macosUrl = `${release}/Shehata.Git_${version}_aarch64.dmg`;

function write(relativePath, contents) {
  const target = join(repoRoot, relativePath);
  mkdirSync(dirname(target), { recursive: true });
  writeFileSync(target, contents.endsWith("\n") ? contents : `${contents}\n`, "utf8");
  process.stdout.write(`wrote ${relativePath}\n`);
}

// ----------------------------------------------------------------- Scoop
// The bucket lives in this repository, so `scoop bucket add` points straight
// at it and no second repository has to be kept in step.
write(
  "bucket/shehata-git.json",
  `${JSON.stringify(
    {
      version,
      description: DESCRIPTION,
      homepage: HOMEPAGE,
      license: "MIT",
      architecture: {
        "64bit": {
          url: `${windowsUrl}#/setup.exe`,
          hash: windowsSha.toLowerCase(),
        },
      },
      innosetup: false,
      installer: { args: ["/S"] },
      uninstaller: { args: ["/S"] },
      checkver: { github: HOMEPAGE },
      autoupdate: {
        architecture: {
          "64bit": {
            url: `${HOMEPAGE}/releases/download/v$version/Shehata.Git_$version_x64-setup.exe#/setup.exe`,
          },
        },
      },
    },
    null,
    2,
  )}\n`,
);

// ---------------------------------------------------------------- winget
// Three files per version, as the schema requires. Submitting them to
// microsoft/winget-pkgs is a separate, manual step - see docs/PACKAGING.md.
const wingetDir = `packaging/winget/${version}`;
write(
  `${wingetDir}/Shehata.ShehataGit.yaml`,
  `# yaml-language-server: $schema=https://aka.ms/winget-manifest.version.1.6.0.schema.json
PackageIdentifier: Shehata.ShehataGit
PackageVersion: ${version}
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.6.0
`,
);
write(
  `${wingetDir}/Shehata.ShehataGit.installer.yaml`,
  `# yaml-language-server: $schema=https://aka.ms/winget-manifest.installer.1.6.0.schema.json
PackageIdentifier: Shehata.ShehataGit
PackageVersion: ${version}
InstallerType: nullsoft
Scope: user
InstallModes:
  - interactive
  - silent
UpgradeBehavior: install
ReleaseDate: ${new Date().toISOString().slice(0, 10)}
Installers:
  - Architecture: x64
    InstallerUrl: ${windowsUrl}
    InstallerSha256: ${windowsSha.toUpperCase()}
ManifestType: installer
ManifestVersion: 1.6.0
`,
);
write(
  `${wingetDir}/Shehata.ShehataGit.locale.en-US.yaml`,
  `# yaml-language-server: $schema=https://aka.ms/winget-manifest.defaultLocale.1.6.0.schema.json
PackageIdentifier: Shehata.ShehataGit
PackageVersion: ${version}
PackageLocale: en-US
Publisher: Dr Mohamed Shehata
PublisherUrl: ${HOMEPAGE}
PublisherSupportUrl: ${HOMEPAGE}/issues
PackageName: Shehata Git
PackageUrl: ${HOMEPAGE}
License: MIT
LicenseUrl: ${HOMEPAGE}/blob/main/LICENSE
ShortDescription: ${DESCRIPTION}
Description: >-
  Shehata Git assigns one authenticated GitHub identity to each local
  repository and routes HTTPS credentials per repository, so a terminal or a
  coding agent cannot push with the wrong account. Force push, destructive
  reset, and remote deletion are absent by design, and every operation is
  recorded in a redacted local activity trail.
Moniker: shehata-git
Tags:
  - git
  - github
  - credentials
  - identity
  - developer-tools
ReleaseNotesUrl: ${HOMEPAGE}/releases/tag/v${version}
ManifestType: defaultLocale
ManifestVersion: 1.6.0
`,
);

// -------------------------------------------------------------- Homebrew
// Written here and copied into the tap repository, so the version and the
// checksum have a single source.
write(
  "packaging/homebrew/shehata-git.rb",
  `cask "shehata-git" do
  version "${version}"
  sha256 "${macosSha.toLowerCase()}"

  url "${HOMEPAGE}/releases/download/v#{version}/Shehata.Git_#{version}_aarch64.dmg"
  name "Shehata Git"
  desc "${DESCRIPTION}"
  homepage "${HOMEPAGE}"

  depends_on macos: ">= :big_sur"
  depends_on arch: :arm64

  app "Shehata Git.app"

  # The app is not notarised yet, so macOS quarantines it. Installing with
  # \`--no-quarantine\` skips the warning; without the flag, open it once with
  # right-click then Open.
  caveats <<~EOS
    This build is not code-signed or notarised. If macOS refuses to open it,
    either reinstall with:

      brew install --cask --no-quarantine shehata-git

    or right-click the app once and choose Open.
  EOS

  zap trash: [
    "~/Library/Application Support/dev.shehata.git",
    "~/Library/Caches/dev.shehata.git",
  ]
end
`,
);

process.stdout.write(`\npackage manifests regenerated for ${version}\n`);
