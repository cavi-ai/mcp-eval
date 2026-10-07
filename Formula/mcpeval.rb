# typed: false
# frozen_string_literal: true

# Validated against distribution/release.json by scripts/distribution/verify.mjs.
class Mcpeval < Formula
  desc "Privacy-preserving MCP friction capture and deterministic evaluation"
  homepage "https://github.com/cavi-ai/mcp-eval"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.5.0/mcpeval-aarch64-apple-darwin.tar.gz"
      sha256 "fb4082a2ad9e25bdb70c412fa74a3d754339105166cfc6e4100142e88b94dc31"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.5.0/mcpeval-x86_64-apple-darwin.tar.gz"
      sha256 "d4ce3c9c176b48fbdfbecf603024aa1feb65127186821e6e081bb7ccf97c18fd"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.5.0/mcpeval-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "d6ddfa28e5f956d6fed299b65084b48878cb963eb60973a4a8e2b9a5096805ce"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.5.0/mcpeval-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "d5b66828adf188e0b00965e8165c24f730d6eae2f5bb1da96705f71f572b0ee0"
    end
  end

  def install
    bin.install "mcpeval"
    bin.install "mcpeval-demo"
  end

  test do
    assert_match "mcpeval 0.5.0", shell_output("#{bin}/mcpeval --version")
    assert_predicate bin/"mcpeval-demo", :executable?
  end
end
