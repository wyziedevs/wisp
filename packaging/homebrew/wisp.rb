# DRAFT, unpublished. Fill VERSION and the SHA256 values from the release's
# SHA256SUMS, then submit to a tap (e.g. wyziedevs/homebrew-tap).
class Wisp < Formula
  desc "Fast, fun web framework for Rust: the wisp command"
  homepage "https://wispweb.dev"
  version "VERSION"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/wyziedevs/wisp/releases/download/vVERSION/wisp-aarch64-apple-darwin.tar.gz"
      sha256 "SHA256_AARCH64_APPLE_DARWIN"
    end
    on_intel do
      url "https://github.com/wyziedevs/wisp/releases/download/vVERSION/wisp-x86_64-apple-darwin.tar.gz"
      sha256 "SHA256_X86_64_APPLE_DARWIN"
    end
  end
  on_linux do
    on_arm do
      url "https://github.com/wyziedevs/wisp/releases/download/vVERSION/wisp-aarch64-unknown-linux-musl.tar.gz"
      sha256 "SHA256_AARCH64_LINUX_MUSL"
    end
    on_intel do
      url "https://github.com/wyziedevs/wisp/releases/download/vVERSION/wisp-x86_64-unknown-linux-musl.tar.gz"
      sha256 "SHA256_X86_64_LINUX_MUSL"
    end
  end

  def install
    bin.install "wisp"
  end

  test do
    system bin/"wisp", "--version"
  end
end
