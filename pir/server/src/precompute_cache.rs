//! On-disk cache of YPIR pre-computed material.
//!
//! Wraps the `valar-ypir` `dump_into` / `load_from` API in a versioned header
//! tied to (a) the source `tier{N}.bin` content, (b) the YPIR scenario, and
//! (c) the build target. Any mismatch is rejected and the consumer falls back
//! to recompute. Operators never touch the cache; it is auto-invalidated.
//!
//! See plan §"Cache header format" for the wire layout. The reader is
//! contractually bounds-checked (no panics on disk-derived input); relies on
//! the upstream `CacheError` for any invariant violation in the payload.

use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use pir_types::YpirScenario;
use ypir::serialize::CacheError as YpirCacheError;
use ypir::serialize::OfflinePrecomputedValues;
use ypir::server::YServer;
use spiral_rs::params::Params;
use tracing::{debug, info, warn};

/// `SVOTE PreCompute v1`. Header magic; bumped on incompatible header layout
/// changes.
pub const MAGIC: [u8; 8] = *b"SVOTEPC1";

/// Header schema version. Bump on adding/removing/renaming header fields.
/// Wire-format changes inside the upstream YPIR payload are caught by
/// `valar-ypir`'s own `PAYLOAD_FORMAT_V1` check inside `load_from`, so we
/// don't duplicate that here.
///
/// V2 (current): added `payload_hash` to detect bit-level corruption inside
/// the YPIR-formatted payload that doesn't trigger structural rejections.
pub const SCHEMA_V2: u32 = 2;

#[derive(Debug)]
pub enum CacheLoadError {
    /// File doesn't exist. Caller should treat as cache miss (not an error
    /// in the usual sense; first-boot path).
    NotFound,
    /// Header magic mismatch. File at this path isn't a precompute cache.
    BadMagic,
    /// Header schema version doesn't match what this build expects.
    SchemaMismatch { found: u32, expected: u32 },
    /// Build target / cpu features differ.
    TargetMismatch,
    /// Tier source file content has changed since the cache was written.
    TierSourceMismatch,
    /// YPIR scenario (num_items, item_size_bits) doesn't match.
    ScenarioMismatch,
    /// Payload bytes hash differently than the header recorded; bit-level
    /// corruption (disk rot, partial write, hostile replacement) that wasn't
    /// caught by upstream's structural checks.
    PayloadHashMismatch,
    /// Payload byte count doesn't match what the header declared. Catches
    /// truncation that lands at a structural boundary inside the payload.
    PayloadLenMismatch { found: u64, expected: u64 },
    /// Header was structurally valid but the upstream payload load failed.
    /// Includes upstream `PAYLOAD_FORMAT_V1` mismatch and any algorithm
    /// changes that bump that version.
    PayloadError(YpirCacheError),
    /// Underlying I/O error (not EOF or NotFound).
    Io(io::Error),
}

impl std::fmt::Display for CacheLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheLoadError::NotFound => write!(f, "cache file not found"),
            CacheLoadError::BadMagic => write!(f, "cache magic mismatch"),
            CacheLoadError::SchemaMismatch { found, expected } => {
                write!(f, "cache schema {found} != expected {expected}")
            }
            CacheLoadError::TargetMismatch => write!(f, "cache target hash mismatch"),
            CacheLoadError::TierSourceMismatch => {
                write!(f, "cache tier-source hash mismatch (tier file changed)")
            }
            CacheLoadError::ScenarioMismatch => write!(f, "cache scenario hash mismatch"),
            CacheLoadError::PayloadHashMismatch => {
                write!(f, "cache payload hash mismatch (corruption detected)")
            }
            CacheLoadError::PayloadLenMismatch { found, expected } => write!(
                f,
                "cache payload length mismatch: header says {expected}, read {found}"
            ),
            CacheLoadError::PayloadError(e) => write!(f, "cache payload error: {e}"),
            CacheLoadError::Io(e) => write!(f, "cache I/O error: {e}"),
        }
    }
}

impl std::error::Error for CacheLoadError {}

impl From<io::Error> for CacheLoadError {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::NotFound {
            CacheLoadError::NotFound
        } else {
            CacheLoadError::Io(e)
        }
    }
}

impl From<YpirCacheError> for CacheLoadError {
    fn from(e: YpirCacheError) -> Self {
        CacheLoadError::PayloadError(e)
    }
}

/// BLAKE3 hash of a file's full contents. Used for `tier_source_hash`.
pub fn hash_file(path: &Path) -> io::Result<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    let mut f = File::open(path)?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

/// BLAKE3 hash of the cryptographically-relevant scenario fields.
/// Explicit fixed-endian: decoupled from `YpirScenario`'s serde layout so
/// future cosmetic field additions don't spuriously invalidate caches.
pub fn hash_scenario(s: &YpirScenario) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&(s.num_items as u64).to_le_bytes());
    hasher.update(&(s.item_size_bits as u64).to_le_bytes());
    hasher.finalize().into()
}

/// BLAKE3 hash of the build target identity. Prevents loading a cache
/// produced on a different architecture or with different
/// memory-layout-affecting features (which would silently produce wrong
/// answers for db_buf_aligned, not just panic).
pub fn target_hash() -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(std::env::consts::ARCH.as_bytes());
    hasher.update(b"|");
    hasher.update(std::env::consts::OS.as_bytes());
    hasher.update(b"|");
    hasher.update(if cfg!(target_endian = "little") { b"le" } else { b"be" });
    hasher.update(b"|");
    hasher.update(format!("ptr={}", std::mem::size_of::<usize>()).as_bytes());
    // Hash relevant CPU feature gates too: not all features affect layout
    // but ones that change SIMD packing / alignment do.
    hasher.update(b"|avx512f=");
    #[cfg(target_feature = "avx512f")]
    hasher.update(b"1");
    #[cfg(not(target_feature = "avx512f"))]
    hasher.update(b"0");
    hasher.finalize().into()
}

/// Wire-format header. `payload` follows immediately after the last header
/// field on disk; this struct only describes the metadata. Fixed size on
/// disk = `HEADER_BYTES` so the writer can stream the payload then seek
/// back and rewrite the header in place.
#[derive(Debug, Clone)]
pub struct CacheHeader {
    pub schema_version: u32,
    pub target_hash: [u8; 32],
    pub scenario_hash: [u8; 32],
    pub tier_source_hash: [u8; 32],
    /// BLAKE3 of the payload bytes (everything after the header). Catches
    /// bit-level corruption inside the YPIR-formatted payload that doesn't
    /// trigger structural rejections in the upstream loaders.
    pub payload_hash: [u8; 32],
    pub payload_len: u64,
}

/// On-disk size of the header. Magic 8 + schema 4 + target 32 + scenario 32
/// + tier_source 32 + payload_hash 32 + payload_len 8 = 148 bytes.
pub const HEADER_BYTES: u64 = 8 + 4 + 32 + 32 + 32 + 32 + 8;

impl CacheHeader {
    fn write_to<W: Write>(&self, w: &mut W) -> io::Result<()> {
        w.write_all(&MAGIC)?;
        w.write_all(&self.schema_version.to_le_bytes())?;
        w.write_all(&self.target_hash)?;
        w.write_all(&self.scenario_hash)?;
        w.write_all(&self.tier_source_hash)?;
        w.write_all(&self.payload_hash)?;
        w.write_all(&self.payload_len.to_le_bytes())?;
        Ok(())
    }

    fn read_from<R: Read>(r: &mut R) -> Result<Self, CacheLoadError> {
        let mut magic = [0u8; 8];
        read_exact(r, &mut magic)?;
        if magic != MAGIC {
            return Err(CacheLoadError::BadMagic);
        }
        let schema_version = read_u32_le(r)?;
        if schema_version != SCHEMA_V2 {
            return Err(CacheLoadError::SchemaMismatch {
                found: schema_version,
                expected: SCHEMA_V2,
            });
        }
        let mut target_hash = [0u8; 32];
        read_exact(r, &mut target_hash)?;
        let mut scenario_hash = [0u8; 32];
        read_exact(r, &mut scenario_hash)?;
        let mut tier_source_hash = [0u8; 32];
        read_exact(r, &mut tier_source_hash)?;
        let mut payload_hash = [0u8; 32];
        read_exact(r, &mut payload_hash)?;
        let payload_len = read_u64_le(r)?;
        Ok(CacheHeader {
            schema_version,
            target_hash,
            scenario_hash,
            tier_source_hash,
            payload_hash,
            payload_len,
        })
    }
}

// ── Hashing reader / writer wrappers (Findings B + C) ─────────────────────────
//
// HashingWriter: streams writes through to the inner writer, updates BLAKE3
// over the bytes, and counts. Lets `write_cache` dump the ~13 GB payload
// directly to disk without buffering it all in a Vec first.
//
// HashingReader: symmetric for `try_load_cache`. The upstream loaders read
// through it; after they finish, the consumer compares the running hash
// against the header's `payload_hash` to detect bit-level corruption.

struct HashingWriter<W: Write> {
    inner: W,
    hasher: blake3::Hasher,
    bytes: u64,
}

impl<W: Write> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: blake3::Hasher::new(),
            bytes: 0,
        }
    }

    /// Returns (payload_hash, payload_byte_count). Drops the wrapper.
    fn finalize(self) -> ([u8; 32], u64) {
        (self.hasher.finalize().into(), self.bytes)
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct HashingReader<R: Read> {
    inner: R,
    hasher: blake3::Hasher,
    bytes: u64,
}

impl<R: Read> HashingReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: blake3::Hasher::new(),
            bytes: 0,
        }
    }

    fn finalize(self) -> ([u8; 32], u64) {
        (self.hasher.finalize().into(), self.bytes)
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.bytes += n as u64;
        Ok(n)
    }
}

fn read_exact<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<(), CacheLoadError> {
    r.read_exact(buf).map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            CacheLoadError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "cache header truncated",
            ))
        } else {
            CacheLoadError::Io(e)
        }
    })
}

fn read_u32_le<R: Read>(r: &mut R) -> Result<u32, CacheLoadError> {
    let mut b = [0u8; 4];
    read_exact(r, &mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_u64_le<R: Read>(r: &mut R) -> Result<u64, CacheLoadError> {
    let mut b = [0u8; 8];
    read_exact(r, &mut b)?;
    Ok(u64::from_le_bytes(b))
}

/// Compute the cache file path for a given tier file path. `tier1.bin` →
/// `tier1.precompute`, etc.
pub fn cache_path_for_tier(tier_path: &Path) -> PathBuf {
    let stem = tier_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "tier".to_string());
    let parent = tier_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!("{stem}.precompute"))
}

/// Result of `try_load_cache`: either the loaded YPIR state, or a typed
/// reason it couldn't be used.
pub struct LoadedCache<'a> {
    pub server: YServer<'a, u16>,
    pub offline: OfflinePrecomputedValues<'a>,
}

/// Try to load a precompute cache. Returns `Ok(LoadedCache)` on validated
/// hit, `Err(CacheLoadError)` on miss / mismatch / corruption. Caller
/// should fall back to recompute on any `Err` and overwrite the cache.
///
/// `tier_path` is hashed and compared against the cache's recorded
/// `tier_source_hash`: any change to the tier file invalidates the cache.
pub fn try_load_cache<'a>(
    cache_path: &Path,
    tier_path: &Path,
    scenario: &YpirScenario,
    params: &'a Params,
) -> Result<LoadedCache<'a>, CacheLoadError> {
    let file = File::open(cache_path)?;
    let mut r = BufReader::with_capacity(1 << 20, file);
    let header = CacheHeader::read_from(&mut r)?;

    if header.target_hash != target_hash() {
        return Err(CacheLoadError::TargetMismatch);
    }
    if header.scenario_hash != hash_scenario(scenario) {
        return Err(CacheLoadError::ScenarioMismatch);
    }
    let actual_tier_hash = hash_file(tier_path).map_err(CacheLoadError::Io)?;
    if header.tier_source_hash != actual_tier_hash {
        return Err(CacheLoadError::TierSourceMismatch);
    }

    // Header validated. Wrap the reader in a HashingReader so we hash the
    // payload bytes as the upstream loaders consume them. After both loads
    // complete, compare against the header's recorded payload_hash to catch
    // bit-level corruption that didn't trip a structural check.
    let (server, offline, actual_hash, actual_len) = {
        let mut hr = HashingReader::new(&mut r);
        let server = YServer::<u16>::load_from(&mut hr, params)?;
        let offline = OfflinePrecomputedValues::load_from(&mut hr, params)?;
        let (h, n) = hr.finalize();
        (server, offline, h, n)
    };
    if actual_hash != header.payload_hash {
        return Err(CacheLoadError::PayloadHashMismatch);
    }
    if actual_len != header.payload_len {
        return Err(CacheLoadError::PayloadLenMismatch {
            found: actual_len,
            expected: header.payload_len,
        });
    }

    // Reject any trailing bytes after the last expected payload field. The
    // cache format is closed-ended; trailing data indicates either a bug in
    // the writer, a corrupted file with garbage appended, or a future format
    // version we don't understand. Fail loud rather than silently ignore.
    let mut trailing = [0u8; 1];
    match r.read(&mut trailing) {
        Ok(0) => {} // clean EOF, expected
        Ok(_) => {
            return Err(CacheLoadError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "trailing bytes after payload",
            )));
        }
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {} // also clean EOF
        Err(e) => return Err(CacheLoadError::Io(e)),
    }

    Ok(LoadedCache { server, offline })
}

/// Atomically write the cache. Streams the payload directly to disk via a
/// HashingWriter (no in-memory buffering of the ~13 GB payload, so the
/// process doesn't OOM on RAM-tight production hosts), then seeks back to
/// rewrite the header with the real `payload_hash` and `payload_len`.
///
/// `tier_source_hash` is the BLAKE3 of the tier file's bytes — caller must
/// pass the hash of the EXACT buffer used to build the YPIR state, not
/// re-read the tier file from disk. Re-reading would expose a TOCTOU window
/// during the long YPIR setup, allowing the cache header to record a hash
/// of new tier bytes while the payload was built from old bytes (which the
/// next load would silently accept and serve wrong responses against).
///
/// Best-effort: returns the underlying error to the caller, which is expected
/// to log-and-continue. Caching is an optimization, not a correctness
/// invariant.
pub fn write_cache(
    cache_path: &Path,
    tier_source_hash: &[u8; 32],
    scenario: &YpirScenario,
    server: &YServer<u16>,
    offline: &OfflinePrecomputedValues,
) -> Result<()> {
    let tmp_path = cache_path.with_extension("precompute.tmp");

    // Open the tmp file with read+write (we'll seek back to rewrite the
    // header after streaming the payload).
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&tmp_path)
        .with_context(|| format!("create {}", tmp_path.display()))?;

    // Write a placeholder header (zero hash, zero length). Header is
    // fixed-size so we can seek back to offset 0 after the payload is
    // written and overwrite with the real values.
    let placeholder = CacheHeader {
        schema_version: SCHEMA_V2,
        target_hash: target_hash(),
        scenario_hash: hash_scenario(scenario),
        tier_source_hash: *tier_source_hash,
        payload_hash: [0u8; 32],
        payload_len: 0,
    };

    // Stream payload through BufWriter (1 MiB) → HashingWriter → underlying
    // file. The BufWriter amortizes the upstream's small writes (length
    // prefixes, dim u32s) into kernel-friendly chunks; the bulk u64-slice
    // writes go through it without re-buffering. The HashingWriter computes
    // the payload hash and counts bytes as they pass.
    let (payload_hash, payload_len, mut file) = {
        let mut bw = BufWriter::with_capacity(1 << 20, file);
        placeholder.write_to(&mut bw)?;
        let mut hw = HashingWriter::new(&mut bw);
        server.dump_into(&mut hw)?;
        offline.dump_into(&mut hw)?;
        let (h, n) = hw.finalize();
        bw.flush()?;
        let inner = bw
            .into_inner()
            .map_err(|e| anyhow::anyhow!("flush BufWriter: {e}"))?;
        (h, n, inner)
    };

    // Seek back to offset 0 and rewrite the header with the real values.
    file.seek(SeekFrom::Start(0))
        .with_context(|| format!("seek to header in {}", tmp_path.display()))?;
    let final_header = CacheHeader {
        schema_version: SCHEMA_V2,
        target_hash: target_hash(),
        scenario_hash: hash_scenario(scenario),
        tier_source_hash: *tier_source_hash,
        payload_hash,
        payload_len,
    };
    final_header.write_to(&mut file)?;
    file.sync_all()
        .with_context(|| format!("fsync {}", tmp_path.display()))?;
    drop(file);

    std::fs::rename(&tmp_path, cache_path)
        .with_context(|| format!("rename {} -> {}", tmp_path.display(), cache_path.display()))?;
    Ok(())
}

/// Best-effort cache eviction. Used at every `tier{N}.bin` write site to
/// prevent stale caches from sitting on disk between snapshot rotation and
/// the next `serve` restart. Logs but never errors; a missing cache is
/// the desired post-condition either way.
pub fn evict_cache_for_tier(tier_path: &Path) {
    let cache_path = cache_path_for_tier(tier_path);
    match std::fs::remove_file(&cache_path) {
        Ok(()) => {
            info!(
                cache = %cache_path.display(),
                "evicted stale precompute cache after tier rewrite"
            );
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // Normal: cache may not exist on first sync or if previously evicted.
            debug!(cache = %cache_path.display(), "no precompute cache to evict");
        }
        Err(e) => {
            warn!(
                cache = %cache_path.display(),
                error = %e,
                "failed to evict stale precompute cache; next serve will reject it via hash"
            );
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("create tempdir")
    }

    #[test]
    fn cache_path_naming() {
        let p = Path::new("/data/tier1.bin");
        assert_eq!(cache_path_for_tier(p), Path::new("/data/tier1.precompute"));
        let p = Path::new("/data/tier2.bin");
        assert_eq!(cache_path_for_tier(p), Path::new("/data/tier2.precompute"));
    }

    #[test]
    fn header_round_trip() {
        let h = CacheHeader {
            schema_version: SCHEMA_V2,
            target_hash: target_hash(),
            scenario_hash: [0xAB; 32],
            tier_source_hash: [0xCD; 32],
            payload_hash: [0xEF; 32],
            payload_len: 1234,
        };
        let mut buf = Vec::new();
        h.write_to(&mut buf).unwrap();
        // On-disk size must match the documented HEADER_BYTES constant; the
        // streaming writer relies on this to seek back exactly to offset 0.
        assert_eq!(buf.len() as u64, HEADER_BYTES);
        let mut r = std::io::Cursor::new(&buf);
        let h2 = CacheHeader::read_from(&mut r).unwrap();
        assert_eq!(h.schema_version, h2.schema_version);
        assert_eq!(h.target_hash, h2.target_hash);
        assert_eq!(h.scenario_hash, h2.scenario_hash);
        assert_eq!(h.tier_source_hash, h2.tier_source_hash);
        assert_eq!(h.payload_hash, h2.payload_hash);
        assert_eq!(h.payload_len, h2.payload_len);
    }

    #[test]
    fn header_rejects_bad_magic() {
        let mut buf = vec![0u8; 100];
        buf[..8].copy_from_slice(b"NOTMAGIC");
        let mut r = std::io::Cursor::new(&buf);
        assert!(matches!(
            CacheHeader::read_from(&mut r),
            Err(CacheLoadError::BadMagic)
        ));
    }

    #[test]
    fn header_rejects_old_schema() {
        // Build a valid magic + an older schema number that we no longer
        // accept. This mimics what an existing v1 cache file looks like to
        // current code.
        let mut buf = Vec::new();
        buf.extend_from_slice(&MAGIC);
        buf.extend_from_slice(&1u32.to_le_bytes()); // v1, not the current SCHEMA_V2
        buf.extend_from_slice(&[0u8; 32 * 4 + 8]); // padding to be valid-shaped
        let mut r = std::io::Cursor::new(&buf);
        assert!(matches!(
            CacheHeader::read_from(&mut r),
            Err(CacheLoadError::SchemaMismatch { found: 1, expected: 2 })
        ));
    }

    #[test]
    fn header_rejects_truncation() {
        let h = CacheHeader {
            schema_version: SCHEMA_V2,
            target_hash: [0; 32],
            scenario_hash: [0; 32],
            tier_source_hash: [0; 32],
            payload_hash: [0; 32],
            payload_len: 0,
        };
        let mut buf = Vec::new();
        h.write_to(&mut buf).unwrap();
        // Truncate at every prefix and verify no panic.
        for len in 0..buf.len() {
            let mut r = std::io::Cursor::new(&buf[..len]);
            let _ = CacheHeader::read_from(&mut r);
        }
    }

    #[test]
    fn evict_missing_is_noop() {
        let dir = tmpdir();
        let tier = dir.path().join("tier1.bin");
        // No file present; eviction should not panic or error visibly.
        evict_cache_for_tier(&tier);
    }

    #[test]
    fn evict_existing_removes_file() {
        let dir = tmpdir();
        let tier = dir.path().join("tier1.bin");
        let cache = cache_path_for_tier(&tier);
        std::fs::write(&cache, b"dummy cache").unwrap();
        assert!(cache.exists());
        evict_cache_for_tier(&tier);
        assert!(!cache.exists());
    }

    #[test]
    fn hash_file_stable() {
        let dir = tmpdir();
        let p = dir.path().join("foo.bin");
        std::fs::write(&p, b"hello world").unwrap();
        let h1 = hash_file(&p).unwrap();
        let h2 = hash_file(&p).unwrap();
        assert_eq!(h1, h2);
        // Mutate, hash should change.
        std::fs::write(&p, b"different").unwrap();
        let h3 = hash_file(&p).unwrap();
        assert_ne!(h1, h3);
    }

    #[test]
    fn hash_scenario_stable() {
        let s = YpirScenario {
            num_items: 12345,
            item_size_bits: 6789,
        };
        let h1 = hash_scenario(&s);
        let h2 = hash_scenario(&s);
        assert_eq!(h1, h2);
        // Different scenario hashes differently.
        let s2 = YpirScenario {
            num_items: 12346,
            item_size_bits: 6789,
        };
        assert_ne!(h1, hash_scenario(&s2));
    }

    // ── End-to-end wrapper round-trip + rejection tests ──────────────────────
    //
    // Cover the consumer-side wrapper paths (`write_cache` <-> `try_load_cache`)
    // that aren't exercised by the upstream `valar-ypir` cache I/O tests.
    // Each test builds a small SimplePIR YPIR fixture once (~1-2 s) and uses
    // it to drive a full streaming write + checked load through the wrapper.

    use ypir::params::params_for_scenario_simplepir;

    /// Owns the leaked Params + the YPIR state borrowing from it. Tests
    /// build one of these per fixture; teardown is tempdir cleanup.
    struct Fixture {
        params: &'static spiral_rs::params::Params,
        server: ypir::server::YServer<'static, u16>,
        offline: ypir::serialize::OfflinePrecomputedValues<'static>,
        scenario: YpirScenario,
        tier_path: PathBuf,
        tier_source_hash: [u8; 32],
        cache_path: PathBuf,
    }

    /// Build a small SimplePIR YPIR fixture in `dir`. The tier file content
    /// is arbitrary (we only need a stable hash), but the YPIR state is real
    /// so `write_cache` exercises a representative dump.
    ///
    /// Leaks `Params` on the heap (via `Box::leak`) to satisfy the `'static`
    /// lifetime that `YServer` and `OfflinePrecomputedValues` need. This is
    /// acceptable in tests; the leaked allocation is reclaimed at process
    /// exit.
    fn build_fixture(dir: &Path) -> Fixture {
        let num_items: u64 = 1 << 14;
        let item_size_bits: u64 = 16384 * 8;
        let params: &'static spiral_rs::params::Params =
            Box::leak(Box::new(params_for_scenario_simplepir(num_items, item_size_bits)));

        // Deterministic plaintext so re-running gives the same fixture.
        let db_size = (1usize << 14) * (params.instances * params.poly_len);
        let pt_iter = (0..db_size).map(|i| ((i as u64) % params.pt_modulus) as u16);

        let server = ypir::server::YServer::<u16>::new(params, pt_iter, true, false, true);
        let offline = server.perform_offline_precomputation_simplepir(None, None, None);

        let tier_path = dir.join("tier_fixture.bin");
        let tier_data = b"vote-nullifier-pir wrapper test fixture v1".to_vec();
        std::fs::write(&tier_path, &tier_data).expect("write tier fixture");
        let tier_source_hash: [u8; 32] = blake3::hash(&tier_data).into();

        let scenario = YpirScenario {
            num_items: num_items as usize,
            item_size_bits: item_size_bits as usize,
        };
        let cache_path = dir.join("tier_fixture.precompute");

        Fixture {
            params,
            server,
            offline,
            scenario,
            tier_path,
            tier_source_hash,
            cache_path,
        }
    }

    /// Smoke test: the streaming write + checked load wrapper round-trip
    /// succeeds against a real YPIR fixture. Validates that the seek-back
    /// header rewrite, the payload-hash plumbing, and all the `_exact`
    /// shape checks line up at production scenario shapes.
    #[test]
    fn wrapper_round_trip() {
        let dir = tmpdir();
        let fix = build_fixture(dir.path());

        write_cache(
            &fix.cache_path,
            &fix.tier_source_hash,
            &fix.scenario,
            &fix.server,
            &fix.offline,
        )
        .expect("write_cache");

        // The cache file should exist with a non-trivial size.
        let metadata = std::fs::metadata(&fix.cache_path).expect("stat cache");
        assert!(metadata.len() > HEADER_BYTES, "cache file too small");

        // Load it back through the full validation path.
        let _loaded = try_load_cache(&fix.cache_path, &fix.tier_path, &fix.scenario, fix.params)
            .expect("try_load_cache");
    }

    /// Flipping a single byte INSIDE the payload (past the header) must be
    /// caught by the payload-hash check. Without `payload_hash` this kind
    /// of corruption would load silently and serve wrong PIR responses.
    #[test]
    fn wrapper_rejects_payload_hash_mismatch() {
        let dir = tmpdir();
        let fix = build_fixture(dir.path());

        write_cache(
            &fix.cache_path,
            &fix.tier_source_hash,
            &fix.scenario,
            &fix.server,
            &fix.offline,
        )
        .expect("write_cache");

        // Flip one byte well past the header (offset 1000, inside payload).
        // The byte we corrupt is part of `db_buf_aligned`-sized data; no
        // structural fields are affected, so only `payload_hash` will catch
        // this.
        let mut bytes = std::fs::read(&fix.cache_path).expect("read cache");
        let target = HEADER_BYTES as usize + 1000;
        assert!(target < bytes.len(), "cache too small to corrupt mid-payload");
        bytes[target] ^= 0xFF;
        std::fs::write(&fix.cache_path, &bytes).expect("rewrite mutated cache");

        let err = try_load_cache(&fix.cache_path, &fix.tier_path, &fix.scenario, fix.params)
            .err()
            .expect("load should reject corrupted payload");
        assert!(
            matches!(err, CacheLoadError::PayloadHashMismatch),
            "expected PayloadHashMismatch, got {err:?}"
        );
    }

    /// Appending garbage after a valid payload must be rejected. The
    /// upstream loaders are documented to consume only their own bytes; the
    /// consumer wrapper is the framing layer responsible for EOF-checking.
    #[test]
    fn wrapper_rejects_trailing_bytes() {
        let dir = tmpdir();
        let fix = build_fixture(dir.path());

        write_cache(
            &fix.cache_path,
            &fix.tier_source_hash,
            &fix.scenario,
            &fix.server,
            &fix.offline,
        )
        .expect("write_cache");

        // Append garbage.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&fix.cache_path)
            .expect("open cache for append");
        f.write_all(&[0xAB; 64]).expect("append garbage");
        drop(f);

        let err = try_load_cache(&fix.cache_path, &fix.tier_path, &fix.scenario, fix.params)
            .err()
            .expect("load should reject trailing bytes");
        // Trailing-byte rejection currently surfaces as `CacheLoadError::Io`
        // with `InvalidData`. If we ever introduce a dedicated variant for
        // trailing bytes, this test will need to update.
        match err {
            CacheLoadError::Io(ref e) if e.kind() == std::io::ErrorKind::InvalidData => {}
            other => panic!("expected Io(InvalidData) for trailing bytes, got {other:?}"),
        }
    }

    /// Modifying the underlying tier file invalidates the cache via the
    /// `tier_source_hash` check — this is the operational path that fires
    /// on snapshot rotation, sync rebuild, etc.
    #[test]
    fn wrapper_rejects_tier_source_mismatch() {
        let dir = tmpdir();
        let fix = build_fixture(dir.path());

        write_cache(
            &fix.cache_path,
            &fix.tier_source_hash,
            &fix.scenario,
            &fix.server,
            &fix.offline,
        )
        .expect("write_cache");

        // Replace the tier file content (cache header still records the
        // old tier hash).
        std::fs::write(&fix.tier_path, b"different tier content").expect("rewrite tier");

        let err = try_load_cache(&fix.cache_path, &fix.tier_path, &fix.scenario, fix.params)
            .err()
            .expect("load should reject tier-source mismatch");
        assert!(
            matches!(err, CacheLoadError::TierSourceMismatch),
            "expected TierSourceMismatch, got {err:?}"
        );
    }
}
