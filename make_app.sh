#!/usr/bin/env bash
# Construit « Scrapitube.app » : l'appli de bureau, à double-cliquer.
# Usage : ./make_app.sh               -> crée ./Scrapitube.app (pour ce Mac)
#         ./make_app.sh --install     -> et la copie dans ~/Applications
#         ./make_app.sh --universal   -> version à partager : Apple Silicon + Intel, avec
#                                        yt-dlp et ffmpeg embarqués (préparés par make_dmg.sh
#                                        dans vendor/)
#
# L'appli est autonome : le binaire Rust est copié dans le .app, on peut donc déplacer le
# dossier du projet sans tout casser.
set -euo pipefail
cd "$(dirname "$0")"
PROJECT_DIR="$(pwd)"

APP_NAME="Scrapitube"
LEGACY_APP_NAMES=("YouTube Downloader")  # ancien nom, supprimé au passage
APP_DIR="$PROJECT_DIR/$APP_NAME.app"
BIN_NAME="scrapitube"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"

INSTALL=0
UNIVERSAL=0
for arg in "$@"; do
  case "$arg" in
    --install) INSTALL=1 ;;
    --universal) UNIVERSAL=1 ;;
    *) echo "Option inconnue : $arg"; exit 1 ;;
  esac
done

[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"
if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust est requis. Installe-le avec :"
  echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
  exit 1
fi

echo "== Compilation =="
if [ "$UNIVERSAL" = 1 ]; then
  rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
  cargo build --release --target aarch64-apple-darwin
  cargo build --release --target x86_64-apple-darwin
  BINARY="$(mktemp -d)/$BIN_NAME"
  lipo -create -output "$BINARY" \
    "target/aarch64-apple-darwin/release/$BIN_NAME" "target/x86_64-apple-darwin/release/$BIN_NAME"
else
  if [ ! -x /opt/homebrew/bin/yt-dlp ] && [ ! -x /usr/local/bin/yt-dlp ]; then
    echo "Attention : yt-dlp n'est pas installé via Homebrew. Installe-le avec :  brew install yt-dlp"
  fi
  if ! command -v ffmpeg >/dev/null 2>&1; then
    echo "Attention : ffmpeg est requis pour fusionner vidéo et audio. Installe-le avec :  brew install ffmpeg"
  fi
  cargo build --release
  BINARY="target/release/$BIN_NAME"
fi

echo "== Construction de $APP_NAME.app =="
rm -rf "$APP_DIR"
for legacy in "${LEGACY_APP_NAMES[@]}"; do rm -rf "$PROJECT_DIR/$legacy.app"; done
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"
ICONSET="$(mktemp -d)/icon.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s assets/icon.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) assets/icon.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP_DIR/Contents/Resources/AppIcon.icns"

cp "$BINARY" "$APP_DIR/Contents/MacOS/$BIN_NAME"

if [ "$UNIVERSAL" = 1 ]; then
  if [ ! -x vendor/yt-dlp ] || [ ! -x vendor/ffmpeg ]; then
    echo "vendor/yt-dlp et vendor/ffmpeg manquent : lance plutôt ./make_dmg.sh, qui les prépare."
    exit 1
  fi
  mkdir -p "$APP_DIR/Contents/Resources/bin"
  cp vendor/yt-dlp vendor/ffmpeg "$APP_DIR/Contents/Resources/bin/"
fi

cat > "$APP_DIR/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleIdentifier</key><string>local.scrapitube</string>
  <key>CFBundleExecutable</key><string>$BIN_NAME</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF

if [ -n "${SIGN_IDENTITY:-}" ]; then
  # Signature Developer ID (compte Apple Developer) avec le « hardened runtime » exigé pour la
  # notarisation. On signe de l'intérieur vers l'extérieur : outils embarqués, puis l'appli.
  echo "== Signature avec « $SIGN_IDENTITY » =="
  sign() { codesign --force --timestamp --options runtime --sign "$SIGN_IDENTITY" "$@"; }
  if [ -d "$APP_DIR/Contents/Resources/bin" ]; then
    # yt-dlp est un programme Python empaqueté (PyInstaller) : il charge à l'exécution des
    # bibliothèques qu'il extrait lui-même, ce que le hardened runtime interdit par défaut.
    sign --entitlements entitlements/yt-dlp.plist "$APP_DIR/Contents/Resources/bin/yt-dlp"
    sign "$APP_DIR/Contents/Resources/bin/ffmpeg"
  fi
  sign "$APP_DIR"
  codesign --verify --strict --deep "$APP_DIR"
else
  # Signature « ad hoc » (sans compte développeur Apple) : obligatoire pour qu'un binaire
  # Apple Silicon se lance, et qui garde le paquet cohérent une fois copié sur un autre Mac.
  codesign --force --deep --sign - "$APP_DIR" >/dev/null 2>&1
fi

if [ "$INSTALL" = 1 ]; then
  mkdir -p "$HOME/Applications"
  rm -rf "$HOME/Applications/$APP_NAME.app"
  for legacy in "${LEGACY_APP_NAMES[@]}"; do rm -rf "$HOME/Applications/$legacy.app"; done
  cp -R "$APP_DIR" "$HOME/Applications/"
  echo "Installée dans ~/Applications/$APP_NAME.app"
fi

echo
echo "Terminé : $APP_DIR"
echo "Double-clique dessus, ou glisse-la dans Applications / le Dock."
