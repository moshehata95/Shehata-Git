cask "shehata-git" do
  version "0.1.25"
  sha256 "5554fb9af7453e2cf5075e650d34321f3c8c5a8b3f97cc38d34be0d6045cdc94"

  url "https://github.com/moshehata95/Shehata-Git/releases/download/v#{version}/Shehata.Git_#{version}_aarch64.dmg"
  name "Shehata Git"
  desc "Pin every local repository to one GitHub identity, so the wrong account — or a coding agent — never pushes for you."
  homepage "https://github.com/moshehata95/Shehata-Git"

  depends_on macos: ">= :big_sur"
  depends_on arch: :arm64

  app "Shehata Git.app"

  # The app is not notarised yet, so macOS quarantines it. Installing with
  # `--no-quarantine` skips the warning; without the flag, open it once with
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
