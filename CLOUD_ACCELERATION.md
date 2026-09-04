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

### Stage 1: Fast Zero-Install Tooling & Persistent Dynamic DHT Cache
* **Path**: `.github/workflows/cloud_download.yml`
* **Mechanism**:
  - Pre-cached static musl-libc binaries for `aria2c` and `cloudflared` using `actions/cache/restore@v4` (`key: cloud-tools-musl-v2`).
  - Separated dynamic DHT routing table cache (`key: dht-cache-${{ github.run_id }}` with `restore-keys: dht-cache-` and post-job save `if: always()`). Known Kademlia nodes are continuously saved and warmed across runs.
* **Result**: Startup time is **~1.8–2.0 s** (85% reduction vs apt-get) with immediate DHT swarm awareness.

### Stage 2: Cloud Swarm to RAM Disk
* **Directory**: `/dev/shm/dl` (Linux `tmpfs` RAM disk; 0% disk I/O wait).
* **Pre-fetch**: Fetches `.torrent` directly via `curl -sL --max-time 5 -o /dev/shm/dl/payload.torrent` so aria2 starts immediately in native BitTorrent mode without HTTP negotiation latency.
* **Allocation**: `--file-allocation=falloc` (instant ext4/tmpfs preallocation).
* **Swarm Tuning**:
  - `--bt-max-peers=500`: Connects to up to 500 peers across the global swarm.
  - `--bt-request-peer-speed-limit=200M`: Forces aria2 to continuously hunt for more peers until download speed hits 200 MB/s.
  - `--bt-tracker-interval=10`: Re-announces to all public trackers every 10 seconds to rapidly ingest hundreds of peers.
  - `--peer-id-prefix="-qB4650-"`: Spoofs qBittorrent 4.6.5 to bypass aggressive client choking from seedboxes.
  - `--disk-cache=512M`: Minimizes internal buffer thrashing.
  - `--dht-entry-point=router.bittorrent.com:6881` & `--dht-file-path=/home/runner/.cache/aria2/dht.dat`.
* **Result**: 1.40 GB swarm download dropped from 70.0s to **26.7s** (cold) and **~18s** (warm), sustaining up to **128 MiB/s** (saturating the runner's Gigabit NIC).

### Stage 3: Zero-Copy Kernel Streaming & 24-Worker Client
* **Server Side**:
  - Multi-threaded Python server utilizing `ThreadingHTTPServer` and custom `RangeRequestHandler`.
  - Employs Linux kernel zero-copy `os.sendfile(sock_fd, file_fd, offset, 8MB)` directly from tmpfs memory into the TCP socket buffer with `TCP_NODELAY`.
  - Throttles `/dev/shm/last_transfer` updates to once per range slice, eliminating thousands of disk open/close operations under GIL lock.
  - Exposes port 8080 through Cloudflare Quick Tunnel (`cloudflared tunnel --url http://127.0.0.1:8080`).
* **Client Side (`src/tui.rs`)**:
  - Uses dedicated `streaming_client` with `no_proxy()`, `tcp_nodelay(true)`, and `pool_max_idle_per_host(64)` so streaming bypasses local SOCKS5 proxy userspace framing.
  - Pre-allocates target file via `File::set_len(total_size)`.
  - Scales worker concurrency to 24 workers for files > 500 MB (16 for > 10 MB).
  - Uses Linux kernel `pwrite64` (`std::os::unix::fs::FileExt::write_all_at`) for lockless, out-of-order slice writes.
  - Automatic retry with backoff and instant failover to GitHub Release CDN asset URL if a tunnel connection drops.
* **Result**: Sustained client streaming throughput of **~65–85+ MB/s**, enabling instant streaming publication in **38 seconds** from dispatch.

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
| Niche trackers first + `--bt-stop-timeout=60` on magnet | Assume anime tracker works for all; short timeout kills stalls | Nyaa tracker rejected movie hashes; 60s timeout killed metadata exchange at DL:0B | **DISCARDED** (Universal trackers first + 180s stop timeout + fast empty exit) |

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
