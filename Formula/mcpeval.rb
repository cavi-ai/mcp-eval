# typed: false
# frozen_string_literal: true

# Validated against distribution/release.json by scripts/distribution/verify.mjs.
class Mcpeval < Formula
  desc "Privacy-preserving MCP friction capture and deterministic evaluation"
  homepage "https://github.com/cavi-ai/mcp-eval"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.4.0/mcpeval-aarch64-apple-darwin.tar.gz"
      sha256 "69dfe73ac773f6a4e28d88b17701bc4babe1d1263cccd630c6e83f2df3261302"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.4.0/mcpeval-x86_64-apple-darwin.tar.gz"
      sha256 "0fce027093d6d6efcaa482dd103bab9f580b73449f67d9758f29d9a2bd2aa71a"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.4.0/mcpeval-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "d9340960b6eb6c0332191b7581d44b1432b78d48f3b7def4d10d4d2ead1a24d0"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.4.0/mcpeval-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "51d692724a06cb6f72a36577b04968735e81924919a0e8dc35fe95d6d142569c"
    end
  end

  def install
    bin.install "mcpeval"
    bin.install "mcpeval-demo"
  end

  test do
    assert_match "mcpeval 0.4.0", shell_output("#{bin}/mcpeval --version")
    assert_predicate bin/"mcpeval-demo", :executable?
  end
end
