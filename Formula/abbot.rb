class Abbot < Formula
  desc "Persistent AI background daemon"
  homepage "https://github.com/ianzepp/abbot"
  version "0.1.0"

  # For now, point to a local file for testing
  # Later we'll change this to a GitHub release URL
  url "file://#{ENV['HOME']}/github/ianzepp/abbot/target/release/abbot"
  sha256 :no_check  # Skip checksum for local testing

  def install
    bin.install "abbot"
  end

  test do
    system "#{bin}/abbot", "--version"
  end
end
