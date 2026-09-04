# Developer & Agent Guidelines: Torrenty

This document serves as the single source of truth for human developers and future AI agents working on **Torrenty**. It details the system architecture, benchmarking protocols, optimal configurations, and most importantly: **a strict list of failed experiments and pitfalls that must never be repeated.**

---

## 1. System Architecture

Torrenty is an ultra-fast BitTorrent client featuring a ratatui-based Terminal User Interface (TUI) and an **asymmetric cloud-accelerated download pipeline**:

1. **Local Client (Rust / `src/tui.rs`)**:
   - Searches indexers (Nyaa, etc.).
   - Dispatches cloud acceleration workflows via GitHub Actions (`gh workflow run`).
   - Polls workflow logs in real-time to obtain the ephemeral Cloudflare Tunnel URL.
   - Streams the payload directly from GitHub Actions RAM disk using a **12-worker concurrent byte-range client** writing to pre-allocated sparse files via Linux kernel `pwrite64` (`std::os::unix::fs::FileExt::write_all_at`).
   - Dynamic release tags are generated per invocation (`dl_{info_hash}_{epoch_sec}`) to guarantee fresh, isolated runs.

2. **Cloud Accelerator (`.github/workflows/cloud_download.yml`)**:
   - Runs on GitHub-hosted `ubuntu-latest` runners (Gigabit network).
   - **Stage A: Tooling (~2.0 s)**: Restores static musl-compiled `aria2c` and `cloudflared` binaries from GitHub Actions Cache (`actions/cache/restore@v4`).
   - **Stage B: Cloud Swarm to RAM Disk (~14–20 s)**: Downloads directly to `/dev/shm/dl` (tmpfs RAM disk) using tuned BitTorrent swarm flags with aria2c.
   - **Stage C: Multithreaded Streaming (~16 s)**: Serves the RAM disk over Cloudflare Quick Tunnels using a custom Python RFC-7233 byte-range server (`ThreadingHTTPServer` + `206 Partial Content`).

---

## 2. STRICT RULES: What NOT to Do (Failed Experiments Post-Mortem)

Future agents **MUST NOT** reintroduce any of the following failed approaches. Every single one was empirically tested on production swarms and proved to degrade performance or break functionality:

### ❌ 1. NEVER set `--bt-request-peer-speed-limit=0`
* **What went wrong**: Setting this to `0` caused connected seeders to collapse from 55 down to 14, increasing cloud download time from 26s to 35.4s.
* **Why**: According to the official aria2 documentation, `--bt-request-peer-speed-limit` defines the threshold below which aria2 hunts for *more* peers. Setting it to `0` tells aria2 that *any* speed > 0 B/s is sufficient, prematurely terminating peer discovery.
* **Correct Setting**: `--bt-request-peer-speed-limit=200M` (forces aria2 to aggressively hunt and maintain connections up to 200 MB/s).

### ❌ 2. NEVER set `--bt-min-crypto-level=plain`
* **What went wrong**: Connected seeders plummeted and swarm throughput crawled.
* **Why**: Modern seedboxes (especially high-bandwidth European and Asian anime seedboxes hosting Nyaa swarms) enforce Message Stream Encryption (MSE/PE) to bypass ISP traffic shaping. Forcing `plain` handshakes causes high-speed seedboxes to immediately drop the connection.
* **Correct Setting**: Leave default hybrid encryption (allows both plain and encrypted handshakes).

### ❌ 3. NEVER set `--split=64` on BitTorrent Downloads
* **What went wrong**: Tracker rate-limiting and connection stalls.
* **Why**: In aria2, `--split` is exclusively for segmenting single HTTP/FTP URI downloads, **not** BitTorrent piece distributions. Setting `--split=64` when aria2 fetches `.torrent` files or queries HTTP trackers causes excessive connections that trigger rate limits.
* **Correct Setting**: Let BitTorrent piece selection handle swarm parallelism; use `--bt-max-peers=500`.

### ❌ 4. NEVER use `--bt-prioritize-piece=head,tail` for Swarm Acceleration
* **What went wrong**: Severe download stalls across the swarm.
* **Why**: Enforcing head/tail piece priority interferes with the optimal rarest-first piece scheduling across 50+ concurrent peers, creating artificial bottlenecks waiting for specific chunks.
* **Correct Setting**: Do not prioritize head/tail during bulk cloud downloads.

### ❌ 5. NEVER reuse old releases or skip downloads
* **What went wrong**: Returning cached assets skips the download process and invalidates benchmarking.
* **Why**: Every run must execute as a fresh, first-time run.
* **Correct Implementation**: Release tags must be dynamic per run: `dl_{info_hash}_{epoch_sec}`. Never re-introduce `check_existing` logic that bypasses downloading.

### ❌ 6. NEVER panic over `[ERROR] Exception caught while loading DHT routing table ... dht.dat`
* **What went wrong**: Agents falsely assumed DHT was broken and attempted destructive changes.
* **Why**: On clean GitHub Actions VMs, `/home/runner/.cache/aria2/dht.dat` does not yet exist. Aria2 throws `errorCode=1` (`ENOENT`) and immediately falls back to bootstrap nodes (`dht.transmissionbt.com:6881`) on the next millisecond. The 26.0s baseline run also had this log.
* **Correct Implementation**: `/home/runner/.cache/aria2` is cached via GitHub Actions Cache to warm the DHT table cleanly.

### ❌ 7. NEVER use single-threaded `python3 -m http.server`
* **What went wrong**: Client streaming throughput was capped at ~47 MB/s, taking 31+ seconds.
* **Why**: Standard `http.server` handles requests synchronously and lacks proper byte-range multiplexing.
* **Correct Implementation**: Use `ThreadingHTTPServer` with an RFC-7233 range request handler paired with the 12-worker Rust chunk downloader (`write_all_at`), achieving **85.5 MB/s** (16.3s for 1.40 GB).

### ❌ 8. NEVER use `apt-get install aria2` without caching
* **What went wrong**: Wasted 13–15 seconds on every CI run updating apt lists and downloading deb packages.
* **Correct Implementation**: Use `actions/cache/restore@v4` with pre-cached static musl binaries (`aria2c` and `cloudflared`), bringing tooling setup time down to **2.0s** (85% reduction).

### ❌ 9. NEVER use `socks5://` instead of `socks5h://` or block concurrently on unresponsive indexers
* **What went wrong**:
  1. `socks5://` performs local DNS resolution before connecting to the SOCKS proxy, causing queries to blocked domains (like `nyaa.si`) to fail or hang due to ISP DNS poisoning/firewalls. In contrast, `socks5h://` passes the unresolved hostname directly to the SOCKS5 proxy engine (`bypass-engine`) for remote resolution and TLS SNI desynchronization.
  2. `search_piratebay` (`apibay.org`) frequently hangs with 0 bytes received. Relying on `tokio::join!(pb_future, nyaa_future)` with unbounded timeouts froze the entire search pipeline, keeping Nyaa's instant results (~300ms) hostage and resulting in empty UI results ("No results found.").
* **Correct Implementation**:
  - Automatically normalize SOCKS proxy URLs to `socks5h://` and prefer the native SOCKS5 bypass engine on `127.0.0.1:1080`.
  - Apply strict per-request timeouts (4s for PirateBay, 6s for Nyaa) and stream results incrementally through MPSC channels so Nyaa results display immediately without waiting for slower indexers.

### ❌ 10. NEVER restore stale completed downloads on TUI restart
* **What went wrong**: Populating `downloads` from `.cache/torrentty/download-history.json` cluttered the TUI on restart with completed items having `0 B` size and `0s` elapsed.
* **Correct Implementation**: Start every TUI session with a clean downloads state (`Vec::new()`) and wipe stale history on boot.
* **Logging Requirement**: Always write detailed timestamped execution logs to `~/.cache/torrentty/torrentty.log` for proxy selection, network requests, latencies, and errors.

### ❌ 11. NEVER run BitTorrent downloads without multi-tracker injection (`--bt-tracker`) and stall timeout (`--bt-stop-timeout`)
* **What went wrong**: In GitHub Actions workflow run 33908880091, direct `.torrent` downloads for Slime S4 Ep 21 stalled at `CN:50 SD:0 DL:0B` indefinitely because:
  1. The `.torrent` file contained only a single tracker (`nyaa.tracker.wf:7777`) which was unresponsive/rate-limited on GitHub Actions runner IPs.
  2. `aria2c` lacked additional fallback public trackers because `--bt-tracker` was omitted from `ARIA2_OPTS`.
  3. `aria2c` had no `--bt-stop-timeout`, causing it to spin at 0 B/s forever without terminating, preventing the magnet fallback step from ever executing.
  4. Bogus tracker entries like `http://127.0.0.1:8080/announce` in `util.rs` sent tracker announces to the local Python HTTP range server.
* **Correct Implementation**:
  - Always inject proven high-speed public UDP and HTTPS trackers (`udp://tracker.opentrackr.org:1337/announce`, `udp://open.stealth.si:80/announce`, `udp://tracker.torrent.eu.org:451/announce`, etc.) via `--bt-tracker="$PUBLIC_TRACKERS"` in `ARIA2_OPTS`.
  - Set `--bt-stop-timeout=25` on direct `.torrent` downloads so stalls immediately trigger failover to the magnet URI fallback.
  - Purge any local port (e.g. 127.0.0.1:8080) from tracker announcement lists.

### ❌ 12. NEVER terminate the Cloudflare Tunnel immediately after release backup upload or omit client streaming retry & CDN failover
* **What went wrong**: In workflow run 33910858367, the user tried downloading Slime S04E21 (1.40 GB). The GitHub Actions runner uploaded the backup release asset to GitHub at 1 Gbps in just 29 seconds. The workflow step exited as soon as `gh release upload` finished (`wait $UPLOAD_PID`), killing `cloudflared` and `python3`. Meanwhile, the user's client was streaming at ~15 MB/s and got abruptly severed at 21.9% (306.75 MB). Additionally, `reqwest::Client` had a default 10s blanket request timeout that severed large range chunks, and workers lacked retry loops and GitHub Release CDN failover.
* **Correct Implementation**:
  - **Activity-Based Tunnel Holding**: The runner must monitor `/dev/shm/last_transfer` and hold the Cloudflare Tunnel open as long as client streaming is active (until 75s of idle after active transfer and backup upload completion, up to 25m).
  - **Worker Streaming Timeout Override**: Streaming requests must override the client-level timeout with `.timeout(Duration::from_secs(600))` so workers are never prematurely aborted while reading large chunks.
  - **Resilient Chunk Retry**: Range workers must retry with exponential backoff right from `current_offset`.
  - **GitHub Release CDN Auto-Failover**: If the Cloudflare Tunnel drops or resets, workers automatically fail over to the permanent GitHub Release asset URL (`https://github.com/{REPO}/releases/download/{TAG}/{FILE}`) to resume downloading without losing progress.

---

## 3. Proven High-Performance Configuration Reference

The following aria2 options in `.github/workflows/cloud_download.yml` are production-proven:

```bash
PUBLIC_TRACKERS="udp://tracker.opentrackr.org:1337/announce,udp://open.stealth.si:80/announce,udp://tracker.torrent.eu.org:451/announce,udp://explodie.org:6969/announce,http://nyaa.tracker.wf:7777/announce,https://tracker.pmman.tech:443/announce,https://tracker.nekobt.to/api/tracker/public/announce,https://tracker.nekomi.cn:443/announce,https://tracker.leechshield.link:443/announce,https://tracker.7471.top:443/announce,https://tracker.foreverpirates.co:443/announce"

ARIA2_OPTS=(
  --seed-time=0
  --file-allocation=falloc
  --bt-max-peers=500
  --bt-request-peer-speed-limit=200M
  --disk-cache=512M
  --max-overall-upload-limit=1K
  --bt-tracker-connect-timeout=5
  --bt-tracker-timeout=5
  --dht-entry-point=dht.transmissionbt.com:6881
  --summary-interval=1
  --enable-peer-exchange=true
  --bt-tracker="$PUBLIC_TRACKERS"
)
```

---

## 4. Benchmark History (Slime S4 Ep 17 - 1.40 GB)

| Metric | Baseline | Optimized | Gain / Verification |
| :--- | :---: | :---: | :--- |
| **Tooling Preparation** | 13.0 s | **2.0 s** | 85% faster via Musl cache restore |
| **Cloud Swarm Download** | 26.0 s | **~14–20 s** | Saturated runner NIC at 105+ MiB/s |
| **Client Streaming Speed** | 47.0 MB/s | **85.5 MB/s** | 12-worker parallel chunk streaming |
| **Client Streaming Time** | 31.0 s | **16.3 s** | 47% reduction in transfer time |

---

## 5. Verification Checklist for Future Changes

When iterating on Torrenty or its cloud workflows:
1. **Always run `cargo check` and `cargo test`** before committing.
2. **Never commit hardcoded release tags**; retain `{info_hash}_{epoch}`.
3. **Verify CI runs using `gh run view <run_id> --log`** to confirm timing breakdowns across Tooling, Swarm, and Streaming.
4. **Compare against the 2.0s (Tooling) / ~18s (Swarm) / 16s (Streaming) baseline**. If any change degrades these numbers, **discard it immediately**.
