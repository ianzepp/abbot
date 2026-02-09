How to Build                                                
                                         
  Yes, Xcode is required — the full Xcode.app (not just Command Line Tools). Here's why and how:                                                                
                                                                                     
  Prerequisites                          

  1. Xcode from the App Store (free), then set it as the active toolchain:
  sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
  2. Rust toolchain (already have it)
  3. Node.js (for building web/dist)

  Build Locally

  The one-step script does everything:
  ./macos/scripts/build-app.sh

  This will:
  1. cargo build --release for the Rust binaries
  2. npm ci && npm run build in web/
  3. xcodebuild to compile the Swift app
  4. Copy abbot, abbotd, and web/dist into Abbot.app/Contents/Resources/

  Then launch with:
  open macos/build/Build/Products/Debug/Abbot.app

  For Distribution (.dmg)

  ./macos/scripts/build-app.sh --release
  ./macos/scripts/codesign.sh <path-to-Abbot.app> "Developer ID Application: ..."
  ./macos/scripts/create-dmg.sh <path-to-Abbot.app> Abbot.dmg
  ./macos/scripts/notarize.sh Abbot.dmg  # needs Apple credentials in env

  CI

  The release-macos-app.yml workflow handles all of this automatically on v* tags — builds universal (arm64+x86_64) binaries, creates a signed+notarized .dmg,
  and uploads to abbot-releases. Just needs the Apple signing secrets configured in the repo.
