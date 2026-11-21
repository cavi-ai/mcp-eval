# typed: false
# frozen_string_literal: true

# Validated against distribution/release.json by scripts/distribution/verify.mjs.
class Mcpeval < Formula
  desc "Privacy-preserving MCP friction capture and deterministic evaluation"
  homepage "https://github.com/cavi-ai/mcp-eval"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.9.0/mcpeval-aarch64-apple-darwin.tar.gz"
      sha256 "6034302dc0777fea085eeb4de9adb8be41f6c51ac59a29f8291fde3bdd9941ff"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.9.0/mcpeval-x86_64-apple-darwin.tar.gz"
      sha256 "991724940b0fcf152d816a7fd4c408a2918e645296e47cab28c0bd77e86eadf9"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.9.0/mcpeval-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "d4838c00966f926237e89a67a038556c0a0d41b2c5feef16e1ed60e93f00fe0f"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.9.0/mcpeval-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "16929b9ebbc244ee40f714adac7f7ae741fe1f111c36a7d364076ff1dc83b2b9"
    end
  end

  def install
    bin.install "mcpeval"
    bin.install "mcpeval-demo"
  end

  test do
    assert_match "mcpeval 0.9.0", shell_output("#{bin}/mcpeval --version")
    assert_predicate bin/"mcpeval-demo", :executable?
  end
end
