#!/system/bin/sh
# Magisk / KernelSU / APatch Background Service Launcher for Cache Cleaner Daemon

MODDIR=${0%/*}
BASE_DIR="/data/adb/cleaner"
BIN_DIR="$BASE_DIR/bin"
RUN_DIR="$BASE_DIR/run"
BIN="$BIN_DIR/cleaner"
CONF="$BASE_DIR/config.toml"

# Wait until Android has fully booted and storage is decrypted
until [ "$(getprop sys.boot_completed)" = "1" ]; do
    sleep 3
done

# Ensure target directories exist
mkdir -p "$BIN_DIR" "$RUN_DIR"
chmod 0755 "$BASE_DIR" "$BIN_DIR" "$RUN_DIR"

# Deploy / sync binary if updated in module
if [ -f "$MODDIR/system/bin/cleaner" ]; then
    cp -f "$MODDIR/system/bin/cleaner" "$BIN"
elif [ -f "$MODDIR/system/bin/cleaner-daemon" ]; then
    cp -f "$MODDIR/system/bin/cleaner-daemon" "$BIN"
fi
chmod 0755 "$BIN"

# Deploy initial config.toml if not already present
if [ ! -f "$CONF" ]; then
    if [ -f "$MODDIR/config.toml" ]; then
        cp "$MODDIR/config.toml" "$CONF"
    elif [ -f "$MODDIR/cleaner.toml" ]; then
        cp "$MODDIR/cleaner.toml" "$CONF"
    fi
    chmod 0644 "$CONF"
fi

# Launch daemon using robust PID-locked start command
"$BIN" start --config "$CONF"
