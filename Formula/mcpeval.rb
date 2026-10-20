# typed: false
# frozen_string_literal: true

# Validated against distribution/release.json by scripts/distribution/verify.mjs.
class Mcpeval < Formula
  desc "Privacy-preserving MCP friction capture and deterministic evaluation"
  homepage "https://github.com/cavi-ai/mcp-eval"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.8.0/mcpeval-aarch64-apple-darwin.tar.gz"
      sha256 "e085bc6496d36ebb50b9e51ac756f4fff950de16cada207b5244fa51d06324fb"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.8.0/mcpeval-x86_64-apple-darwin.tar.gz"
      sha256 "910cc2b21448886a0b38eeee7b53e855db8a0bb8f6d027b32c1d6d001338939e"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.8.0/mcpeval-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "e9502bad2375255885e0ed8077bd1d10deb9638793471d420f3c14435c017ac2"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.8.0/mcpeval-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "4506e92da2d5e118dc47d9beb46cba5231e8864082ca8ce0b18ff267327e2767"
    end
  end

  def install
    bin.install "mcpeval"
    bin.install "mcpeval-demo"
  end

  test do
    assert_match "mcpeval 0.8.0", shell_output("#{bin}/mcpeval --version")
    assert_predicate bin/"mcpeval-demo", :executable?
  end
end
