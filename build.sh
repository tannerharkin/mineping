#!/bin/bash
set -e

echo "Building Minecraft Ping API..."

# Build for the current platform (optimized)
echo "Building for current platform..."
cargo build --release

# Optional: Build static binary for Linux
if [[ "$OSTYPE" == "linux-gnu"* ]]; then
    echo "Building static Linux binary..."
    RUSTFLAGS='-C target-feature=+crt-static' cargo build --release --target x86_64-unknown-linux-gnu
fi

# Optional: Cross-compile for other platforms if cross is installed
if command -v cross &> /dev/null; then
    echo "Cross-compilation tool found. Building for other platforms..."
    
    # Linux musl (fully static)
    cross build --release --target x86_64-unknown-linux-musl
    
    # macOS (if not on mac)
    if [[ "$OSTYPE" != "darwin"* ]]; then
        cross build --release --target x86_64-apple-darwin
    fi
    
    # ARM (for Raspberry Pi, etc)
    cross build --release --target armv7-unknown-linux-gnueabihf
else
    echo "Install 'cross' for cross-compilation support: cargo install cross"
fi

echo "Build complete! Binary located at: ./target/release/mineping"