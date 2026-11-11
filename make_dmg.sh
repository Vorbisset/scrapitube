#!/usr/bin/env bash
# Construit l'installateur à partager : dist/Scrapitube-<version>.dmg
# Usage : ./make_dmg.sh
#
# L'appli produite fonctionne sur n'importe quel Mac récent (Apple Silicon et Intel) sans
# rien installer d'autre : yt-dlp et ffmpeg sont embarqués. Ils sont téléchargés une fois
# dans vendor/ (avec vérification SHA-256), puis réutilisés aux constructions suivantes.
#
# Signature et notarisation Apple (facultatif, compte Apple Developer requis) :
#   SIGN_IDENTITY="Developer ID Application: Prénom Nom (ABCDE12345)" \
#   NOTARY_PROFILE="scrapitube" ./make_dmg.sh
# Sans ces variables, l'appli est signée « ad hoc » et macOS la bloque à la première ouverture.
set -euo pipefail
cd "$(dirname "$0")"

if [ -n "${NOTARY_PROFILE:-}" ] && [ -z "${SIGN_IDENTITY:-}" ]; then
  echo "NOTARY_PROFILE demande aussi SIGN_IDENTITY : Apple ne notarise que les applis signées Developer ID."
  exit 1
fi

APP_NAME="Scrapitube"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
DMG="dist/$APP_NAME-$VERSION.dmg"

# ffmpeg 9.0.2, version autonome (sans dépendances) de https://ffmpeg.martin-riedl.de
FFMPEG_BASE="https://ffmpeg.martin-riedl.de/download/macos"
# Empreintes SHA-256 inscrites ici (et non relues sur le serveur) : un fichier modifié
# en ligne serait refusé.
FFMPEG_BUILDS=(
  "arm64/1789931890_9.0.2 c8ed4c4e6978a03c485edbfe4e0a5dc2380f8a30bba5150531b31b094492d924"
  "amd64/1789931006_9.0.2 7c6b4125b191cbf773832dc51f424cf2b6bb7da43007d1e066f95909e47cacd4"
)

check_sha256() {  # fichier, empreinte attendue
  local actual
  actual="$(shasum -a 256 "$1" | awk '{print $1}')"
  if [ "$actual" != "$2" ]; then
    echo "Empreinte SHA-256 incorrecte pour $1 (attendu $2, obtenu $actual)"
    rm -f "$1"
    exit 1
  fi
}

mkdir -p vendor
if [ ! -x vendor/yt-dlp ]; then
  echo "== Téléchargement de yt-dlp (version officielle macOS, Apple Silicon + Intel) =="
  TAG="$(curl -sfL -o /dev/null -w '%{url_effective}' https://github.com/yt-dlp/yt-dlp/releases/latest)"
  TAG="${TAG##*/}"
  BASE="https://github.com/yt-dlp/yt-dlp/releases/download/$TAG"
  curl -fL --progress-bar "$BASE/yt-dlp_macos" -o vendor/yt-dlp.part
  EXPECTED="$(curl -sfL "$BASE/SHA2-256SUMS" | awk '$2 == "yt-dlp_macos" {print $1}')"
  check_sha256 vendor/yt-dlp.part "$EXPECTED"
  chmod +x vendor/yt-dlp.part
  mv vendor/yt-dlp.part vendor/yt-dlp
  echo "  -> yt-dlp $TAG"
fi

if [ ! -x vendor/ffmpeg ]; then
  echo "== Téléchargement de ffmpeg (Apple Silicon + Intel) =="
  SLICES=()
  for entry in "${FFMPEG_BUILDS[@]}"; do
    read -r build EXPECTED <<<"$entry"
    arch="${build%%/*}"
    dir="vendor/ffmpeg-$arch"
    mkdir -p "$dir"
    curl -fL --progress-bar "$FFMPEG_BASE/$build/ffmpeg.zip" -o "$dir/ffmpeg.zip"
    check_sha256 "$dir/ffmpeg.zip" "$EXPECTED"
    unzip -oq "$dir/ffmpeg.zip" -d "$dir"
    SLICES+=("$dir/ffmpeg")
  done
  # Un seul fichier qui contient les deux architectures.
  lipo -create -output vendor/ffmpeg "${SLICES[@]}"
  chmod +x vendor/ffmpeg
  rm -rf vendor/ffmpeg-*
fi

./make_app.sh --universal

echo "== Création de l'image disque =="
STAGING="$(mktemp -d)/$APP_NAME"
mkdir -p "$STAGING" dist
cp -R "$APP_NAME.app" "$STAGING/"
ln -s /Applications "$STAGING/Applications"
cp THIRD-PARTY-NOTICES.md "$STAGING/Licences des logiciels tiers.md"
{
cat <<'EOF'
SCRAPITUBE — télécharger des vidéos YouTube, en entier ou juste un extrait

INSTALLATION
  Glisse « Scrapitube » sur le dossier « Applications » juste à côté.

EOF
# Une appli notarisée s'ouvre sans avertissement : inutile d'expliquer comment le contourner.
[ -z "${NOTARY_PROFILE:-}" ] && cat <<'EOF'
PREMIÈRE OUVERTURE
  L'appli n'est pas signée par un compte développeur Apple, donc macOS la bloque
  la première fois (« Apple n'a pas pu vérifier… »). C'est normal :

  1. Ouvre Scrapitube une première fois depuis Applications, puis clique sur
     « OK » / « Terminé » dans l'avertissement.
  2. Va dans Réglages Système → Confidentialité et sécurité, descends en bas
     et clique sur « Ouvrir quand même » à côté de Scrapitube.
  3. Confirme (mot de passe ou Touch ID). Ensuite, elle s'ouvre normalement.

  Sur les anciennes versions de macOS (avant Sequoia), il suffit de faire
  clic droit sur l'appli → Ouvrir → Ouvrir.

EOF
cat <<'EOF'
UTILISATION
  Colle un lien YouTube, choisis Vidéo ou Audio MP3, la qualité, éventuellement
  un extrait (début / fin), puis clique sur Télécharger. Les fichiers vont dans
  le dossier Vidéos (Movies) → YouTube, modifiable dans l'appli.

  Si YouTube refuse un jour un lien, clique sur « Mettre à jour yt-dlp » en bas
  de la fenêtre.

Pour un usage personnel : respecte les droits d'auteur et les conditions
d'utilisation de YouTube.
EOF
} > "$STAGING/Lisez-moi.txt"

rm -f "$DMG"
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGING" -ov -format UDZO "$DMG" >/dev/null

if [ -n "${SIGN_IDENTITY:-}" ]; then
  codesign --force --timestamp --sign "$SIGN_IDENTITY" "$DMG"
fi

if [ -n "${NOTARY_PROFILE:-}" ]; then
  echo "== Notarisation par Apple (quelques minutes) =="
  if ! xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait | tee /dev/stderr | grep -q "status: Accepted"; then
    echo "Notarisation refusée. Détail avec : xcrun notarytool log <id> --keychain-profile $NOTARY_PROFILE"
    exit 1
  fi
  # Le ticket d'Apple est joint au .dmg : la vérification marche même hors ligne.
  xcrun stapler staple "$DMG"
  spctl --assess --type open --context context:primary-signature -v "$DMG"
fi

echo
echo "Installateur prêt : $(pwd)/$DMG ($(du -h "$DMG" | awk '{print $1}'))"
if [ -n "${NOTARY_PROFILE:-}" ]; then
  echo "Signé et notarisé par Apple : il s'installe sans avertissement."
else
  echo "Envoie ce fichier (AirDrop, WeTransfer, clé USB…) : tout est expliqué dans le Lisez-moi."
fi
