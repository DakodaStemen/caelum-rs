# Benchmarks

**Status: pending.** No throughput or latency numbers have been measured for Caelum, so none are claimed here.

## Methodology (planned)

- Tool: [vtebench](https://github.com/alacritty/vtebench), run inside the terminal under test with `vtebench --benchmarks ./benchmarks --dat out.dat` (default sizes, 30+ samples).
- Release build (`cargo build --release --locked`); same window size, font and scrollback for every terminal compared.
- Idle machine (check `uptime` load < 1), AC power, fixed CPU governor, compositor/VSync noted. Report CPU model, kernel, GPU/driver, display server.
- Report median and spread per benchmark, plus the exact commit and vtebench version. Input-latency is a separate measurement (e.g. with a hardware or high-speed-camera rig) and is not covered by vtebench.

## Results

| Benchmark | Caelum | Reference terminal | Notes |
|---|---|---|---|
| (all) | pending | pending | not yet measured |
