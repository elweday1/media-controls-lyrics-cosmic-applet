# COSMIC Media Controls

A compact COSMIC panel applet for MPRIS players. The panel shows album art, track details, the current lyric line, and previous/play-or-pause/next controls. Its popup adds a seek bar, elapsed/total time, a selector for multiple players, album and player details, and a three-line synced lyric view with the active line highlighted. The lyric source is shown as a small label without provider status details.

## Build and install

Requires Rust/Cargo and the native build dependencies for libcosmic. From this directory:

```sh
cargo build --release
install -Dm755 target/release/cosmic-ext-music-lyrics ~/.local/bin/cosmic-ext-music-lyrics
install -Dm644 data/io.github.nasser.CosmicExtMusicLyrics.desktop ~/.local/share/applications/io.github.nasser.CosmicExtMusicLyrics.desktop
```

Add **Media Controls** from COSMIC Panel settings. Transport and seek operations are sent to the selected MPRIS player over D-Bus.
