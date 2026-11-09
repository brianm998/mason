# SPDX-License-Identifier: GPL-3.0-or-later
# A Homebrew formula TEMPLATE for the `mason-pipeline` tap. Not published: the URL and checksums
# are placeholders until a signed release exists, and no tap has been created (Stage 31).
# The formula is named `mason` because that is the command people type; if another formula or
# tool already provides `mason` (Dart's mason_cli), `brew install` reports the conflict and
# `mason doctor` explains how to choose.
class Mason < Formula
  desc "Pipeline that takes tasks from a tracker to landed code with LLM agents"
  homepage "https://github.com/mason-pipeline/mason"
  version "0.0.0"
  license "GPL-3.0-or-later"

  on_macos do
    on_arm do
      url "https://github.com/mason-pipeline/mason/releases/download/v0.0.0/mason-0.0.0-aarch64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
    on_intel do
      url "https://github.com/mason-pipeline/mason/releases/download/v0.0.0/mason-0.0.0-x86_64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end
  on_linux do
    on_intel do
      url "https://github.com/mason-pipeline/mason/releases/download/v0.0.0/mason-0.0.0-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  def install
    bin.install "mason", "masond", "step-host", "mason-mcp"
  end

  def caveats
    <<~EOS
      Next: cd your-project && mason init
      If `mason` runs something else, run `#{bin}/mason doctor`, which says how to choose.
    EOS
  end

  test do
    assert_match "command-line client for the MASON pipeline", shell_output("#{bin}/mason --help")
  end
end
