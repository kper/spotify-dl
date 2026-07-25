# 🎵 spotify-dl

A command line utility to download songs, podcasts, playlists and albums directly from Spotify.

> [!IMPORTANT]
> A Spotify Premium account is required.

> [!CAUTION]
> Usage of this software may infringe Spotify's terms of service or your local legislation. Use it under your own risk.

## 🚀 Features

- Download individual tracks, podcasts, playlists or full albums.
- Built with Rust for speed and efficiency.
- Supports metadata tagging and organized file output.
- Uses Spotify Premium login through the browser and reuses the cached session on later runs.

## ⚙️ Installation

You can install it using `cargo`, `homebrew`, from source or using a pre-built binary from the releases page.

### From crates.io using `cargo`

```
cargo install spotify-dl
```

### Using homebrew (macOs)

```
brew tap guillemcastro/spotify-dl
brew install spotify-dl
```

### From source

```
cargo install --git https://github.com/GuillemCastro/spotify-dl.git
```

## 🧭 Usage

```
spotify-dl 0.9.0
A commandline utility to download music directly from Spotify

USAGE:
    spotify-dl.exe [FLAGS] [OPTIONS] <tracks>...

FLAGS:
    -F, --force      Force download even if the file already exists
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -d, --destination <destination>    The directory where the songs will be downloaded
    -f, --format <format>              The format to download the tracks in. Default is flac. [default: flac]
    -t, --parallel <parallel>          Number of parallel downloads. Default is 5. [default: 5]

ARGS:
    <tracks>...    A list of Spotify URIs or URLs (songs, podcasts, playlists or albums)
```

Songs, playlists and albums must be passed as Spotify URIs or URLs (e.g. `spotify:track:123456789abcdefghABCDEF` for songs and `spotify:playlist:123456789abcdefghABCDEF` for playlists or `https://open.spotify.com/playlist/123456789abcdefghABCDEF?si=1234567890`).

## 📋 Examples

- Download a single track:
```bash
spotify-dl https://open.spotify.com/track/TRACK_ID
```

- Download a playlist:

```
spotify-dl https://open.spotify.com/playlist/PLAYLIST_ID
```

Save as MP3 to a custom folder:
```
spotify-dl --format flac --destination ~/Music/Spotify https://open.spotify.com/album/ALBUM_ID
```

## 🔐 Authentication

`spotify-dl` uses Spotify OAuth in your browser. Username/password login is not supported.

1. Run `spotify-dl` with any valid Spotify track, album, playlist, or podcast URL.
2. On the first run, your browser opens a Spotify login page.
3. Sign in with your Spotify Premium account and approve access.
4. After Spotify redirects back to `http://127.0.0.1:8898/login`, the download starts in the terminal.

### What gets stored

`spotify-dl` stores reusable login state in `~/.spotify-dl`:

- `credentials.json`: reusable `librespot` session credentials
- `oauth.refresh`: Spotify OAuth refresh token used to get a new access token without prompting again

In normal use, later runs will reuse those files automatically and will not open the browser again unless the cached session can no longer be refreshed.

### Re-authenticating

If authentication stops working, remove the cached auth files and run `spotify-dl` again:

```bash
rm -f ~/.spotify-dl/credentials.json ~/.spotify-dl/oauth.refresh
```

That forces a fresh browser login on the next run.

## 📄 License

spotify-dl is licensed under the MIT license. See [LICENSE](LICENSE).
