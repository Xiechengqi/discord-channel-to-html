#!/bin/bash
set -e
cd frontend && npm install && npm run build && cd ..
cargo build --release

cp -f -v target/release/discord-channel-to-html ./
