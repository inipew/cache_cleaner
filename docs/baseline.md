# Baseline Implementasi 1-100

Keputusan final per diskusi — tidak tambah dokumen 101+.

| # | Keputusan | Status | Dokumen |
|---|-----------|--------|---------|
| 1 | Incremental rewrite + shadow mode, legacy src/engine sebagai oracle, Executor=NO-OP saat shadow | ✅ | 83.md:7, 100.md:25 |
| 2 | Android 9–16 (API 28–36) capability-probe, bukan if api>=30 | ✅ | 99.md:22, 53.md |
| 3 | SQLite + WAL source of truth, file = audit stream | ✅ | 51.md, 86.md:122 |
| 4 | Magisk #1, KernelSU #2, APatch #3 via PrivilegePlatform trait | ✅ | 24.md, 91.md:27 |
| 5 | Bounds konservatif sekarang: workers=2 ops=256 bytes=512MiB candidates=10k queue=64 | ✅ | 64.md, 78.md |
| 6 | Device farm & overnight idle REQUIRED tapi belum evidenced | ⚠️ | 84.md:742, 99.md:212 |

## Strategi

```
request/target ─┬─ legacy engine ─→ legacy result
                └─ new pipeline  ─→ new plan/result ─→ comparator
                                                          │
                                                     ShadowReport
```

New pipeline:
```
Catalog → Scanner → Safety → Policy → Planner → Authorization → Durable Store → Executor → Verifier
```

Shadow:
```
Executor = NO-OP
```

Cutover: `shadow → read-only production → limited mutation → full` per 100.md.

## Android Capability

```
openat2 available?
  ├── yes → RESOLVE_BENEATH|NO_SYMLINKS|NO_MAGICLINKS|NO_XDEV
  └── no  → fallback openat + dev check, fail-closed jika cannot prove
```

Unsupported combination → fail-closed per 99.md.

## Durability

```
SQLite
 ├── jobs, attempts, operations, operation_intents, leases, idempotency, outbox, schema_metadata
 └── WAL
```

File journal = diagnostic only.

## Privilege

```
IPC peer → SO_PEERCRED → CallerIdentity → PrivilegePlatform::discover_capabilities() → Authorization
```

Root ≠ auth. Setiap mutasi tetap: caller auth + operation auth + scope + generation + safety.

Trait:
```rust
trait PrivilegePlatform {
    fn identify_caller(...) -> CallerIdentity;
    fn discover_capabilities(...) -> Capabilities;
}
```
Implementasi: `platform/magisk.rs`, `kernelsu.rs`, `apatch.rs` penambahan tanpa cemari domain.

## Resource Bounds

Hard bounds sekarang, profiling 95.md untuk tuning:

```
default → device-class → benchmark → p95 → adjust
```

## Validation

Matrix required (Android 9-16 × ext4/F2FS/FUSE × rw/ro/bind × SELinux × screen/suspend/reboot/mount)

```
REQUIRED ✅ SPECIFIED ✅ PROVEN ❌
```

Jangan tulis "farm tersedia" sebelum inventory.

## Implementasi Saat Ini (commit cc75167 + 16d874d)

- Typed ConfigGeneration/CatalogGeneration distinct + checked_add
- Catalog atomic RwLock<Arc> retain last valid
- Executor per-op intent after gates
- JobState lengkap + state_version CAS
- SafetyProof typed op_id+gens+expiry 30s
- ResourceManager Storage/Mount typed hierarchy
- Verifier Stale generation check
- ShadowComparator

## Next

Platform Magisk adapter, idle power event-driven test.
