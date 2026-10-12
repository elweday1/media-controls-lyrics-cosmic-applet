build:
    cargo build --release

install: build
    install -Dm755 target/release/cosmic-ext-music-lyrics ~/.local/bin/cosmic-ext-music-lyrics
    install -Dm644 data/io.github.nasser.CosmicExtMusicLyrics.desktop ~/.local/share/applications/io.github.nasser.CosmicExtMusicLyrics.desktop

uninstall:
    rm -f ~/.local/bin/cosmic-ext-music-lyrics ~/.local/share/applications/io.github.nasser.CosmicExtMusicLyrics.desktop
