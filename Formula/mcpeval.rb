# typed: false
# frozen_string_literal: true

# Validated against distribution/release.json by scripts/distribution/verify.mjs.
class Mcpeval < Formula
  desc "Privacy-preserving MCP friction capture and deterministic evaluation"
  homepage "https://github.com/cavi-ai/mcp-eval"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.1.0/mcpeval-aarch64-apple-darwin.tar.gz"
      sha256 "d6ab42cd065a536b082730a1054d71fc86f863eab81608c46261f2a9350aa6f2"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.1.0/mcpeval-x86_64-apple-darwin.tar.gz"
      sha256 "fe32b9bcb10f54209a3d819614049b2c8bb12ef4182808deb9152e5d6b3769f8"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.1.0/mcpeval-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "dfa94d7e8c553196d857e017e76b221c8d57d6f5703acc4e781454cbdd68df6f"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.1.0/mcpeval-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "6d2f2c5df822e9be6f786ab41512d869d41fb9170c36f3f6f60f48b02caedb7e"
    end
  end

  def install
    bin.install "mcpeval"
    bin.install "mcpeval-demo"
  end

  test do
    assert_match "mcpeval 0.1.0", shell_output("#{bin}/mcpeval --version")
    assert_predicate bin/"mcpeval-demo", :executable?
  end
end
