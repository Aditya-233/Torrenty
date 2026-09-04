# Cloud Acceleration Architecture & Optimization Guide

This document provides technical documentation of the 3-stage asymmetric cloud acceleration engine implemented in **Torrenty**.

---

## 1. Pipeline Overview

Torrenty accelerates torrent downloads by offloading BitTorrent swarm discovery and assembly to a GitHub Actions datacenter runner (1+ Gbps unmetered symmetrical pipe) and streaming directly into the user's local filesystem in parallel chunks:

```
[Torrent Swarm]  <-- 105 MiB/s Swarm --> [GitHub Actions /dev/shm (RAM)]
                                                      │
                                           [ThreadingHTTPServer]
                                             (RFC-7233 Range)
                                                      │
                                           [Cloudflare Tunnel]
                                                      │
                                          <-- 85.5 MB/s HTTPS -->
                                                      │
                                           [Local 12-Worker Client]
                                             (pwrite64 / falloc)
```

---

## 2. The Three Stages

### Stage 1: Fast Zero-Install Tooling
* **Path**: `.github/workflows/cloud_download.yml`
* **Mechanism**:
  - Instead of `sudo apt-get update && sudo apt-get install aria2` (which took 13–15 seconds), we cache static musl-libc binaries for `aria2c` and `cloudflared` using `actions/cache/restore@v4`.
  - Cache Key: `cloud-tools-dht-v1`.
  - Also caches `/home/runner/.cache/aria2` to pre-seed the DHT routing table.
* **Result**: Startup time plummeted from **13.0 s** to **2.0 s** (85% reduction).

### Stage 2: Cloud Swarm to RAM Disk
* **Directory**: `/dev/shm/dl` (Linux `tmpfs` RAM disk; 0% disk I/O wait).
* **Allocation**: `--file-allocation=falloc` (instant ext4/tmpfs preallocation).
* **Swarm Tuning**:
  - `--bt-max-peers=500`: Connects to up to 500 peers across the global swarm.
  - `--bt-request-peer-speed-limit=200M`: Forces aria2 to continuously request and hunt for more peers until download speed hits 200 MB/s.
  - `--disk-cache=512M`: Minimizes internal buffer thrashing.
  - `--dht-entry-point=dht.transmissionbt.com:6881`: Instant bootstrap into Transmission/Mainline DHT networks.
* **Result**: 1.40 GB downloaded in **~14–20 s**, sustaining **105+ MiB/s**.

### Stage 3: Direct High-Speed Client Streaming
* **Server Side**:
  - Multi-threaded Python server utilizing `ThreadingHTTPServer` and custom `RangeRequestHandler`.
  - Handles `Range: bytes=start-end` returning HTTP `206 Partial Content`.
  - Exposes port 8080 through Cloudflare Quick Tunnel (`cloudflared tunnel --url http://127.0.0.1:8080`).
* **Client Side (`src/tui.rs`)**:
  - Pre-allocates target file using `File::set_len(total_size)`.
  - Divides total file size into 12 equal-sized slices (or 16MB slices for large files).
  - Spawns 12 concurrent Tokio worker tasks making parallel `reqwest` range requests.
  - Uses `std::os::unix::fs::FileExt::write_all_at` (`pwrite64`) to write chunks concurrently without mutex locking or file pointer seeks.
* **Result**: Throughput jumped from 47 MB/s to **85.50 MB/s**, reducing transfer time from **31.0 s** to **16.3 s**.

---

## 3. Post-Mortem: Discarded Experiments

| Experiment | Rationale at the Time | Actual Measurement | Final Decision |
| :--- | :--- | :--- | :--- |
| `--bt-request-peer-speed-limit=0` | Thought `0` meant "unlimited" | Seeds dropped from 55 to 14. aria2 halted peer hunting. | **DISCARDED** (Replaced with 200M) |
| `--bt-min-crypto-level=plain` | Thought plain avoids crypto CPU overhead | Seedboxes with enforced encryption dropped handshakes. | **DISCARDED** (Reverted to hybrid) |
| `--split=64` | Thought it accelerated torrent piece downloading | Causes tracker rate-limiting (split is for HTTP/FTP). | **DISCARDED** |
| `--bt-prioritize-piece=head,tail` | Download beginning and end first | Stalled swarm piece distribution. | **DISCARDED** |
| Release Asset Reuse | Skip download if file exists | User requires fresh runs; prevents true testing. | **REMOVED** (Use dynamic tags) |
| `socks5://` local DNS resolution | Assume local DNS is unaffected | Local DNS poisoned by ISP; caused indexer searches to hang. | **DISCARDED** (Use `socks5h://`) |
| Unbounded `tokio::join!` indexer calls | Wait for all indexers without timeout | Dead indexers (PirateBay) blocked fast indexers (Nyaa). | **DISCARDED** (Independent bounded timeouts + atomic render) |
| Omitting `--bt-tracker` on `.torrent` | Assume `.torrent` embedded tracker suffices | Tracker failure stalled aria2c at `CN:50 SD:0` indefinitely. | **DISCARDED** (Inject public trackers + bt-stop-timeout) |
| Premature Tunnel Shutdown & Blanket 10s Timeout | Wait only for `gh release upload` to exit | Tunnel died after 29s mid-client stream; 10s timeout aborted chunks | **DISCARDED** (Activity-based hold + 600s override + CDN failover) |

---

## 4. How to Benchmark a Change

To benchmark changes against the Slime S4 Episode 17 baseline:
1. Ensure the magnet or direct URL is for:
   `[SubsPlease] Tensei shitara Slime Datta Ken S4 - 17 (1080p) [82C10141].mkv` (~1.40 GB)
2. Run via TUI or trigger manually:
   ```bash
   gh workflow run "Cloud Torrent Download" \
     -f name="Slime_Ep17_Bench" \
     -f magnet="magnet:?xt=urn:btih:..." \
     -f tag="bench_$(date +%s)"
   ```
3. Monitor execution breakdown:
   ```bash
   gh run list --workflow="Cloud Torrent Download" --limit 1
   gh run view <run_id> --log
   ```
4. Verify all three phases beat or match:
   - Tooling: `≤ 2.0s`
   - Swarm: `≤ 20.0s`
   - Streaming: `≤ 17.0s`
