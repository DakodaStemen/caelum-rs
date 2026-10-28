#!/bin/sh
# Records docs/demo.gif by playing the scripted `record_demo` test under Xvfb.
# Needs Xvfb, ffmpeg, gifski (optional, recommended: cargo install gifski) and a built test binary. Usage: scripts/record-demo.sh [XVFB_BIN]
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
  -t 90 .runtime/demo.mp4 &
FPID=$!
sleep 1
CAELUM_RECORD_DEMO=1 DISPLAY=$D WAYLAND_DISPLAY= cargo test --locked --test desktop -- --ignored record_demo
kill -INT $FPID 2>/dev/null || true
wait $FPID 2>/dev/null || true
rm -rf .runtime/frames && mkdir -p .runtime/frames
ffmpeg -loglevel error -y -i .runtime/demo.mp4 -vf "fps=10,scale=800:-1:flags=lanczos" .runtime/frames/%04d.png
if command -v gifski >/dev/null 2>&1; then
  # Smaller and cleaner than ffmpeg palettes: ~320 KB at 800 px / 10 fps.
  gifski --fps 10 --width 800 --quality 60 --motion-quality 60 --lossy-quality 60 \
    -o docs/demo.gif .runtime/frames/*.png
else
  ffmpeg -loglevel error -y -i .runtime/demo.mp4 \
    -vf "fps=8,mpdecimate,setpts=N/(8*TB),scale=800:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=64:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle" \
    docs/demo.gif
fi
ls -l docs/demo.gif
