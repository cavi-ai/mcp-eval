# typed: false
# frozen_string_literal: true

# Validated against distribution/release.json by scripts/distribution/verify.mjs.
class Mcpeval < Formula
  desc "Privacy-preserving MCP friction capture and deterministic evaluation"
  homepage "https://github.com/cavi-ai/mcp-eval"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.9.1/mcpeval-aarch64-apple-darwin.tar.gz"
      sha256 "190a0329c4b6b7f75112a20085880172a7de13cbf0fbf5682cf3acc20d2f651f"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.9.1/mcpeval-x86_64-apple-darwin.tar.gz"
      sha256 "d0d0b40fe3ea0339f52a21193a2cf6a34d8305782f3baf3f11216bfde5a689d5"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.9.1/mcpeval-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "f4e7c983ab2f18c06805e4802564f7b33f15f29c4565ef79ea3f149dbb5d7cbf"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.9.1/mcpeval-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "1bdfcad5432f5e26991a79aee4324d801aff36804817f4832408a54683bc3a90"
    end
  end

  def install
    bin.install "mcpeval"
    bin.install "mcpeval-demo"
  end

  test do
    assert_match "mcpeval 0.9.1", shell_output("#{bin}/mcpeval --version")
    assert_predicate bin/"mcpeval-demo", :executable?
  end
end
