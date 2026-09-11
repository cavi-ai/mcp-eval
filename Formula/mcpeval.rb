# typed: false
# frozen_string_literal: true

# Validated against distribution/release.json by scripts/distribution/verify.mjs.
class Mcpeval < Formula
  desc "Privacy-preserving MCP friction capture and deterministic evaluation"
  homepage "https://github.com/cavi-ai/mcp-eval"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.2.0/mcpeval-aarch64-apple-darwin.tar.gz"
      sha256 "9b93c8bb30e918af5dc15a3d2c0b78b9c5110b3fe697e2237329ff15045186c3"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.2.0/mcpeval-x86_64-apple-darwin.tar.gz"
      sha256 "cfa9bea0fdef41ed9c66215002bbaaacbc60c135b0cf3aa531585b3c6f9b90f3"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.2.0/mcpeval-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "c6cc1504a4348b9a467fcd939a05678054f12d58fe7ffdbfd23c7efee6395a9e"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.2.0/mcpeval-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "5f9277761a46e95d22f014eb454bbf1ef9d26011ab2c0f38a8c7dd492f1aafb6"
    end
  end

  def install
    bin.install "mcpeval"
    bin.install "mcpeval-demo"
  end

  test do
    assert_match "mcpeval 0.2.0", shell_output("#{bin}/mcpeval --version")
    assert_predicate bin/"mcpeval-demo", :executable?
  end
end
