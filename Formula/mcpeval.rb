# typed: false
# frozen_string_literal: true

# Validated against distribution/release.json by scripts/distribution/verify.mjs.
class Mcpeval < Formula
  desc "Privacy-preserving MCP friction capture and deterministic evaluation"
  homepage "https://github.com/cavi-ai/mcp-eval"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.7.0/mcpeval-aarch64-apple-darwin.tar.gz"
      sha256 "0b637bcfbb17d309a28a9441c1124e0b99e69fa6208a96925b27c9297e8e0a8b"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.7.0/mcpeval-x86_64-apple-darwin.tar.gz"
      sha256 "67b8b5254acc7ddac780d7709ba0e200752d66884d557477a1b062ad5093b6d9"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.7.0/mcpeval-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "c345bc2d46bf7b0ab6e8e96fe25240d633eafe7342536ab578b5a5c519b1b1bf"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.7.0/mcpeval-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "b950921bbef9e3b8e7dd024df6f94e35bde898ce2b63f1ab70a163f4691d67ed"
    end
  end

  def install
    bin.install "mcpeval"
    bin.install "mcpeval-demo"
  end

  test do
    assert_match "mcpeval 0.7.0", shell_output("#{bin}/mcpeval --version")
    assert_predicate bin/"mcpeval-demo", :executable?
  end
end
