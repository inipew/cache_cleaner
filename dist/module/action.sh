#!/system/bin/sh
# ==============================================================================
# KernelSU / APatch / Magisk WebUI & Action Script for Android Cache Cleaner
# Trigger manual cache cleaning pass and display diagnostics
# ==============================================================================

MODDIR=${0%/*}
BASE_DIR="/data/adb/cleaner"
BIN_DIR="$BASE_DIR/bin"
BIN="$BIN_DIR/cleaner"
CONF="$BASE_DIR/config.toml"
RUN_DIR="$BASE_DIR/run"

# Ensure mandatory directories exist
mkdir -p "$BIN_DIR" "$RUN_DIR" 2>/dev/null
chmod 0755 "$BASE_DIR" "$BIN_DIR" "$RUN_DIR" 2>/dev/null

# Ensure binary is synced to mandatory location
if [ ! -x "$BIN" ]; then
    if [ -f "$MODDIR/system/bin/cleaner" ]; then
        cp -f "$MODDIR/system/bin/cleaner" "$BIN"
    elif [ -f "$MODDIR/system/bin/cleaner-daemon" ]; then
        cp -f "$MODDIR/system/bin/cleaner-daemon" "$BIN"
    elif [ -x "./target/release/cache-cleaner-daemon" ]; then
        BIN="./target/release/cache-cleaner-daemon"
    elif [ -x "./target/debug/cache-cleaner-daemon" ]; then
        BIN="./target/debug/cache-cleaner-daemon"
    fi
    chmod 0755 "$BIN" 2>/dev/null
fi

# Ensure default config is synced to mandatory location
if [ ! -f "$CONF" ]; then
    if [ -f "$MODDIR/config.toml" ]; then
        cp "$MODDIR/config.toml" "$CONF"
    elif [ -f "$MODDIR/cleaner.toml" ]; then
        cp "$MODDIR/cleaner.toml" "$CONF"
    fi
    chmod 0644 "$CONF" 2>/dev/null
fi

echo "=================================================="
echo "      ANDROID NATIVE CACHE CLEANER & OPTIMIZER    "
echo "=================================================="

if [ ! -x "$BIN" ]; then
    echo "[!] Error: Cleaner binary not found or not executable!"
    echo "    Expected path: $BIN"
    exit 1
fi

# 1. Show Platform & Daemon Status
echo "[*] Checking daemon status..."
if [ -f "$CONF" ]; then
    "$BIN" status --config "$CONF"
else
    "$BIN" status
fi
echo ""

# 2. Trigger Deep Clean Pass (App cache, OEM logs, crash dumps, temp APKs, ZRAM, FITRIM)
echo "[*] Triggering manual deep clean operation..."
echo "--------------------------------------------------"
if [ -f "$CONF" ]; then
    "$BIN" clean --deep --trim --zram "$@" --config "$CONF"
else
    "$BIN" clean --deep --trim --zram "$@"
fi
echo "--------------------------------------------------"
echo "[+] Manual cleaning pass finished successfully!"
echo "=================================================="
