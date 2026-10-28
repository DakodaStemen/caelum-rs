#!/bin/sh
# Records docs/demo.gif by playing the scripted `record_demo` test under Xvfb.
# Needs Xvfb, ffmpeg and a built test binary. Usage: scripts/record-demo.sh [XVFB_BIN]
set -eu
cd "$(dirname "$0")/.."
XVFB=${1:-Xvfb}
D=:97
mkdir -p .runtime
"$XVFB" $D -screen 0 1600x1000x24 -nolisten tcp -noreset >/dev/null 2>&1 &
XPID=$!
trap 'kill $XPID 2>/dev/null || true' EXIT
sleep 2
DISPLAY=$D WAYLAND_DISPLAY= cargo test --locked --test desktop --no-run >/dev/null 2>&1
DISPLAY=$D ffmpeg -loglevel error -y -f x11grab -framerate 15 -video_size 1360x880 -i $D \
  -t 75 .runtime/demo.mp4 &
FPID=$!
sleep 1
DISPLAY=$D WAYLAND_DISPLAY= cargo test --locked --test desktop -- --ignored record_demo
kill -INT $FPID 2>/dev/null || true
wait $FPID 2>/dev/null || true
ffmpeg -loglevel error -y -i .runtime/demo.mp4 \
  -vf "fps=10,scale=960:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=96[p];[b][p]paletteuse=dither=bayer:bayer_scale=4" \
  docs/demo.gif
ls -l docs/demo.gif
