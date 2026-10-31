# Logiciels tiers

L'installateur `.dmg` de Scrapitube embarque deux programmes tiers, non modifiés.

## yt-dlp

- Rôle : analyse des liens et téléchargement des vidéos.
- Licence : [The Unlicense](https://github.com/yt-dlp/yt-dlp/blob/master/LICENSE) (domaine public).
- Source : <https://github.com/yt-dlp/yt-dlp> (binaire officiel `yt-dlp_macos`).

## FFmpeg

- Rôle : fusion vidéo + audio, conversion MP3, découpe des extraits.
- Version : 9.0.2, compilation statique de Martin Riedl.
- Licence : [GNU GPL](https://www.ffmpeg.org/legal.html) (cette compilation inclut des
  composants sous GPL).
- Code source de FFmpeg : <https://ffmpeg.org/download.html> (version 9.0.2).
- Scripts de compilation : <https://ffmpeg.martin-riedl.de> (lien vers le dépôt des scripts
  sur cette page).

FFmpeg est un programme séparé, lancé par Scrapitube comme commande externe.
