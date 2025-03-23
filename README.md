# ZEDEX - Zed Extension Mirror

## Background
I've been using [Zed](https://zed.dev) for a while, but I want to be able to run Zed fully independently without connecting to Zed's servers.
After reading through a **lot** of issues at the zed repository I found a Go implementation of this by [Praktisk](https://github.com/praktiskt/zedex). 
I really liked his implementation but I'm not all that good with Go, and since Zed was written in Rust I figured I'd write this in rust as well. In addition to wanting to learn rust better.
As such I made this over the weekend. A lot of the heavy lifting was made by Praktisk and I'd probably never do this if he hadn't already done a lot of the dirty work in figuring out how Zed uses API calls

## Usage

```bash
# Download all extensions
zedex get all-extensions

# Start a local server on the default port (2654)
zedex serve --local-mode

# Download a specific extension
zedex get extension extension-id-here

# Fetch the extension index
zedex get extension-index

# Show available commands and options
zedex --help
```

To configure Zed to use your local server, add this to your Zed config:

```json
{
  "extension_server": "http://localhost:2654"
}
```

## Building from Source

```bash
# Clone the repository
git clone https://github.com/yourusername/zedex.git
cd zedex

# Build the project
cargo build --release

# Run the binary
./target/release/zedex --help
```

## Wishlist
- [ ] Support updating extensions by the call Zed makes on startup
- [ ] Release notes mirror as in praktisk's implementation 
- [ ] Better error handling for network issues
- [ ] Add caching layer for frequently accessed extensions

