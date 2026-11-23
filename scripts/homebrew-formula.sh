#!/bin/sh
# Print the Homebrew formula for a release, from its SHA256SUMS file.
#   scripts/homebrew-formula.sh 1.2.3 SHA256SUMS > Formula/boxquell.rb
set -eu
version=${1:?usage: homebrew-formula.sh <version> <SHA256SUMS>}
sums=${2:?usage: homebrew-formula.sh <version> <SHA256SUMS>}
base="https://github.com/Boxquell/boxquell/releases/download/v$version"

sum() {
  line=$(grep "  boxquell-v$version-$1.tar.gz\$" "$sums") || { echo "error: no build for $1 in $sums" >&2; exit 1; }
  printf '%s' "${line%% *}"
}

cat <<FORMULA
class Boxquell < Formula
  desc "MicroVM sandboxes for coding agents with an explicit network allowlist"
  homepage "https://github.com/Boxquell/boxquell"
  version "$version"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "$base/boxquell-v$version-aarch64-apple-darwin.tar.gz"
      sha256 "$(sum aarch64-apple-darwin)"
    end
    on_intel do
      url "$base/boxquell-v$version-x86_64-apple-darwin.tar.gz"
      sha256 "$(sum x86_64-apple-darwin)"
    end
    # Packs the system images; Homebrew keeps it out of the PATH, boxquell finds it there.
    depends_on "e2fsprogs"
  end

  on_linux do
    on_intel do
      url "$base/boxquell-v$version-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "$(sum x86_64-unknown-linux-gnu)"
    end
  end

  def install
    bin.install "boxquell"
    generate_completions_from_executable(bin/"boxquell", "completions")
  end

  def caveats
    <<~EOS
      Boxquell runs sandboxes on libkrun, which comes from another tap on macOS:
        brew tap slp/krun && brew install libkrun
      On Linux install libkrun and libkrunfw from your distribution.
      Building a system image needs Docker. Then:
        boxquell doctor
        boxquell os build debian
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/boxquell --version")
  end
end
FORMULA
