# typed: false
# frozen_string_literal: true

# Validated against distribution/release.json by scripts/distribution/verify.mjs.
class Mcpeval < Formula
  desc "Privacy-preserving MCP friction capture and deterministic evaluation"
  homepage "https://github.com/cavi-ai/mcp-eval"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.3.0/mcpeval-aarch64-apple-darwin.tar.gz"
      sha256 "cd59df8f1a14b21c53205eb8d5ea85bdb630b8befdf1fdaf3ec990f0226ec2e4"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.3.0/mcpeval-x86_64-apple-darwin.tar.gz"
      sha256 "70413e056bf7688fcdd4acea0527bbc8ed8927643ad6c0e437a2a5680120b239"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.3.0/mcpeval-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "9c3d671fd3a03cb84fca458325085fc43907e28468dda327e3bdc0f172fe4561"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/v0.3.0/mcpeval-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "b3370f265acebec40a4390c6010819a65b768da4e761b07fb8285cbd4fdf4a17"
    end
  end

  def install
    bin.install "mcpeval"
    bin.install "mcpeval-demo"
  end

  test do
    assert_match "mcpeval 0.3.0", shell_output("#{bin}/mcpeval --version")
    assert_predicate bin/"mcpeval-demo", :executable?
  end
end
