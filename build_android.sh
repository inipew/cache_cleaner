#!/usr/bin/env bash
# ==============================================================================
# Android ARM64 (aarch64-linux-android) Build & Packaging Script
# For Native Cache Cleaner & System Optimizer Daemon
# ==============================================================================

set -e

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$PROJECT_ROOT"

# Target Architecture & Defaults
TARGET="aarch64-linux-android"
API_LEVEL="${API_LEVEL:-36}" # Default: Android 9 (API 28) for maximum compatibility (Android 9 - 16+)
DIST_DIR="$PROJECT_ROOT/dist"
BIN_NAME="cache-cleaner-daemon"

# Colors for terminal output
BOLD='\033[1m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

echo -e "${BLUE}${BOLD}======================================================${NC}"
echo -e "${BLUE}${BOLD}   Android ARM64 Native Cleaner Daemon Builder        ${NC}"
echo -e "${BLUE}${BOLD}======================================================${NC}"
echo -e "Target Architecture : ${GREEN}${TARGET}${NC}"
echo -e "Minimum Android API : ${GREEN}API ${API_LEVEL} (Android 9+)${NC}"

# ------------------------------------------------------------------------------
# 0. Check Rust Target Standard Library (std / core)
# ------------------------------------------------------------------------------
check_rust_target() {
    local target_libdir
    target_libdir="$(rustc --print target-libdir --target "$TARGET" 2>/dev/null || true)"
    if [ -z "$target_libdir" ] || [ ! -d "$target_libdir" ]; then
        echo -e "${YELLOW}[!] Rust standard library for '${TARGET}' is not installed.${NC}"

        local rustup_bin
        if command -v rustup >/dev/null 2>&1; then
            rustup_bin="rustup"
        elif [ -x "$HOME/.cargo/bin/rustup" ]; then
            rustup_bin="$HOME/.cargo/bin/rustup"
        else
            rustup_bin=""
        fi

        if [ -n "$rustup_bin" ]; then
            echo -e "${BLUE}[*] Installing target standard library via rustup...${NC}"
            "$rustup_bin" target add "$TARGET"
            echo -e "${GREEN}[+] Target '${TARGET}' installed successfully!${NC}"
        else
            echo -e "${RED}[ERROR] Target '${TARGET}' standard library is missing and 'rustup' was not found.${NC}"
            echo -e ""
            echo -e "${YELLOW}Please install the Android target using ONE of the following methods:${NC}"
            echo -e ""
            echo -e "  ${BOLD}Method A (If you have rustup):${NC}"
            echo -e "    ${BOLD}rustup target add aarch64-linux-android${NC}"
            echo -e ""
            echo -e "  ${BOLD}Method B (Install rustup toolchain - Recommended):${NC}"
            echo -e "    ${BOLD}curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --target aarch64-linux-android${NC}"
            echo -e "    ${BOLD}source \$HOME/.cargo/env${NC}"
            echo ""
            exit 1
        fi
    fi
}

check_rust_target

# ------------------------------------------------------------------------------
# 1. Locate Android NDK
# ------------------------------------------------------------------------------
# Auto-source local ndk_env.sh if present
if [ -f "$PROJECT_ROOT/ndk_env.sh" ]; then
    # shellcheck disable=SC1091
    source "$PROJECT_ROOT/ndk_env.sh"
fi


detect_ndk() {
    if [ -n "$ANDROID_NDK_HOME" ] && [ -d "$ANDROID_NDK_HOME" ]; then
        echo "$ANDROID_NDK_HOME"
        return
    fi
    if [ -n "$ANDROID_NDK_ROOT" ] && [ -d "$ANDROID_NDK_ROOT" ]; then
        echo "$ANDROID_NDK_ROOT"
        return
    fi
    if [ -n "$NDK_HOME" ] && [ -d "$NDK_HOME" ]; then
        echo "$NDK_HOME"
        return
    fi

    # Search common install paths
    local candidate_dirs=(
        "$HOME/android-ndk"
        "$HOME/Android/Sdk/ndk"
        "$HOME/Android/ndk"
        "/opt/android-sdk/ndk"
        "/opt/android-ndk"
        "/usr/lib/android-ndk"
        "/usr/local/android-ndk"
        "$HOME/.android-ndk"
    )


    for base in "${candidate_dirs[@]}"; do
        if [ -d "$base" ]; then
            local latest
            latest=$(find "$base" -maxdepth 1 -mindepth 1 -type d | sort -V | tail -n 1)
            if [ -n "$latest" ] && [ -d "$latest/toolchains/llvm/prebuilt" ]; then
                echo "$latest"
                return
            elif [ -d "$base/toolchains/llvm/prebuilt" ]; then
                echo "$base"
                return
            fi
        fi
    done

    echo ""
}

NDK_PATH=$(detect_ndk)

if [ -z "$NDK_PATH" ]; then
    echo -e "${YELLOW}[!] Android NDK not detected automatically.${NC}"
    echo -e "    Please specify the NDK path via ANDROID_NDK_HOME, e.g.:"
    echo -e "    ${BOLD}export ANDROID_NDK_HOME=/path/to/android-ndk-r26b${NC}"
    echo -e "    ${BOLD}./build_android.sh${NC}"
    echo ""
    echo -e "Attempting to build with cargo-ndk if available..."
    if command -v cargo-ndk >/dev/null 2>&1; then
        echo -e "${GREEN}[+] Found cargo-ndk! Building with cargo-ndk...${NC}"
        cargo ndk -t arm64-v8a --platform "$API_LEVEL" build --release
    else
        echo -e "${RED}[ERROR] Neither Android NDK nor cargo-ndk was found.${NC}"
        echo -e "Please install Android NDK (r25c+ recommended) or install cargo-ndk via:"
        echo -e "  cargo install cargo-ndk"
        exit 1
    fi
else
    echo -e "${GREEN}[+] Detected Android NDK at: ${BOLD}${NDK_PATH}${NC}"

    # Determine Host OS
    OS_NAME="$(uname -s | tr '[:upper:]' '[:lower:]')"
    case "$OS_NAME" in
        linux*)  HOST_TAG="linux-x86_64" ;;
        darwin*) HOST_TAG="darwin-x86_64" ;;
        *)       HOST_TAG="linux-x86_64" ;;
    esac

    TOOLCHAIN="$NDK_PATH/toolchains/llvm/prebuilt/$HOST_TAG"

    if [ ! -d "$TOOLCHAIN" ]; then
        echo -e "${RED}[ERROR] LLVM toolchain not found at: $TOOLCHAIN${NC}"
        exit 1
    fi

    # Set Compilers & Linkers
    export CC_aarch64_linux_android="$TOOLCHAIN/bin/aarch64-linux-android${API_LEVEL}-clang"
    export CXX_aarch64_linux_android="$TOOLCHAIN/bin/aarch64-linux-android${API_LEVEL}-clang++"
    export AR_aarch64_linux_android="$TOOLCHAIN/bin/llvm-ar"
    export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$TOOLCHAIN/bin/aarch64-linux-android${API_LEVEL}-clang"
    export CARGO_TARGET_AARCH64_LINUX_ANDROID_AR="$TOOLCHAIN/bin/llvm-ar"
    export STRIP_TOOL="$TOOLCHAIN/bin/llvm-strip"

    echo -e "${BLUE}[*] Linker : ${CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER}${NC}"

    # --------------------------------------------------------------------------
    # 2. Compile via Cargo
    # --------------------------------------------------------------------------
    echo -e "${BLUE}[*] Compiling ${BIN_NAME} for ${TARGET} (Release)...${NC}"
    cargo build --target "$TARGET" --release
fi

TARGET_BIN="$PROJECT_ROOT/target/$TARGET/release/$BIN_NAME"

if [ ! -f "$TARGET_BIN" ]; then
    echo -e "${RED}[ERROR] Compiled binary not found at $TARGET_BIN${NC}"
    exit 1
fi

# ------------------------------------------------------------------------------
# 3. Strip Symbols to Optimize Binary Size
# ------------------------------------------------------------------------------
if [ -n "$STRIP_TOOL" ] && [ -f "$STRIP_TOOL" ]; then
    echo -e "${BLUE}[*] Stripping binary symbols with llvm-strip...${NC}"
    "$STRIP_TOOL" "$TARGET_BIN"
elif command -v llvm-strip >/dev/null 2>&1; then
    llvm-strip "$TARGET_BIN"
elif command -v aarch64-linux-gnu-strip >/dev/null 2>&1; then
    aarch64-linux-gnu-strip "$TARGET_BIN"
fi

BIN_SIZE=$(du -h "$TARGET_BIN" | awk '{print $1}')
echo -e "${GREEN}[+] Binary built successfully: ${BOLD}${TARGET_BIN}${NC} (${BIN_SIZE})"

# ------------------------------------------------------------------------------
# 4. Package Magisk / KernelSU / APatch Flashable ZIP
# ------------------------------------------------------------------------------
echo -e "${BLUE}[*] Packaging Magisk / KernelSU / APatch module...${NC}"
rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR/module/system/bin"
mkdir -p "$DIST_DIR/module/system/etc"
mkdir -p "$DIST_DIR/module/META-INF/com/google/android"

# Copy binary (provide both 'cleaner' and 'cleaner-daemon' symlink/copy)
cp "$TARGET_BIN" "$DIST_DIR/module/system/bin/cleaner"
cp "$TARGET_BIN" "$DIST_DIR/module/system/bin/cleaner-daemon"
chmod 0755 "$DIST_DIR/module/system/bin/cleaner" "$DIST_DIR/module/system/bin/cleaner-daemon"

cp "$PROJECT_ROOT/config/cleaner.toml" "$DIST_DIR/module/system/etc/cleaner.toml"
cp "$PROJECT_ROOT/config/cleaner.toml" "$DIST_DIR/module/config.toml"
cp "$PROJECT_ROOT/config/cleaner.toml" "$DIST_DIR/module/cleaner.toml"

cp "$PROJECT_ROOT/init/module.prop" "$DIST_DIR/module/module.prop"
cp "$PROJECT_ROOT/init/service.sh" "$DIST_DIR/module/service.sh"
chmod 0755 "$DIST_DIR/module/service.sh"

cp "$PROJECT_ROOT/init/action.sh" "$DIST_DIR/module/action.sh"
chmod 0755 "$DIST_DIR/module/action.sh"

cp "$PROJECT_ROOT/init/cleaner_daemon.rc" "$DIST_DIR/module/cleaner_daemon.rc"

# Create update-binary / installer stub for Magisk
cat << 'EOF' > "$DIST_DIR/module/META-INF/com/google/android/update-binary"
#!/sbin/sh
#################
# Magisk Module #
#################
UMASK=022
OUTFD=$2
ZIPFILE=$3

ui_print() {
  echo -e "ui_print $1\nui_print" >> /proc/self/fd/$OUTFD
}

ui_print "****************************************"
ui_print "* Native Android Cache Cleaner Daemon *"
ui_print "****************************************"

# Extract files
unzip -o "$ZIPFILE" -d "$MODPATH" >&2

BASE_DIR="/data/adb/cleaner"
BIN_DIR="$BASE_DIR/bin"
RUN_DIR="$BASE_DIR/run"

ui_print "- Setting up directory layout: $BASE_DIR"
mkdir -p "$BIN_DIR" "$RUN_DIR"
chmod 0755 "$BASE_DIR" "$BIN_DIR" "$RUN_DIR"

# Install binary to /data/adb/cleaner/bin/cleaner
if [ -f "$MODPATH/system/bin/cleaner" ]; then
  cp -f "$MODPATH/system/bin/cleaner" "$BIN_DIR/cleaner"
elif [ -f "$MODPATH/system/bin/cleaner-daemon" ]; then
  cp -f "$MODPATH/system/bin/cleaner-daemon" "$BIN_DIR/cleaner"
fi
chmod 0755 "$BIN_DIR/cleaner"

# Copy default config if not already present
if [ ! -f "$BASE_DIR/config.toml" ]; then
  if [ -f "$MODPATH/config.toml" ]; then
    cp "$MODPATH/config.toml" "$BASE_DIR/config.toml"
  elif [ -f "$MODPATH/cleaner.toml" ]; then
    cp "$MODPATH/cleaner.toml" "$BASE_DIR/config.toml"
  fi
  chmod 0644 "$BASE_DIR/config.toml"
  ui_print "- Created default configuration at $BASE_DIR/config.toml"
else
  ui_print "- Existing configuration preserved at $BASE_DIR/config.toml"
fi

set_perm_recursive "$MODPATH" 0 0 0755 0644
set_perm "$MODPATH/service.sh" 0 0 0755
set_perm "$MODPATH/action.sh" 0 0 0755

ui_print "- Binary    : $BIN_DIR/cleaner"
ui_print "- Config    : $BASE_DIR/config.toml"
ui_print "- Run Dir   : $RUN_DIR"
ui_print "- Action    : $MODPATH/action.sh"
ui_print "- Installation completed successfully!"
EOF

chmod 0755 "$DIST_DIR/module/META-INF/com/google/android/update-binary"
echo "#MAGISK" > "$DIST_DIR/module/META-INF/com/google/android/updater-script"

# Read version from module.prop
VERSION=$(grep "^version=" "$PROJECT_ROOT/init/module.prop" | cut -d= -f2)
ZIP_NAME="cache-cleaner-${VERSION:-v1.0.0}-arm64.zip"

if command -v zip >/dev/null 2>&1; then
    (cd "$DIST_DIR/module" && zip -r9 "$DIST_DIR/$ZIP_NAME" . >/dev/null)
    ZIP_SIZE=$(du -h "$DIST_DIR/$ZIP_NAME" | awk '{print $1}')
    echo -e "${GREEN}${BOLD}[SUCCESS] Module ZIP generated at:${NC}"
    echo -e "  📦 ${BOLD}$DIST_DIR/$ZIP_NAME${NC} (${ZIP_SIZE})"
else
    echo -e "${YELLOW}[!] 'zip' command not found, module folder created at: ${BOLD}$DIST_DIR/module${NC}"
fi

echo ""
echo -e "${BLUE}${BOLD}======================================================${NC}"
echo -e "${GREEN}${BOLD}                   BUILD COMPLETE                     ${NC}"
echo -e "${BLUE}${BOLD}======================================================${NC}"
echo -e "Standalone Binary : ${BOLD}$TARGET_BIN${NC}"
if [ -f "$DIST_DIR/$ZIP_NAME" ]; then
    echo -e "Flashable ZIP     : ${BOLD}$DIST_DIR/$ZIP_NAME${NC}"
fi
echo ""
echo -e "${YELLOW}Deployment options:${NC}"
echo -e "  1. Flash ${BOLD}$ZIP_NAME${NC} via Magisk / KernelSU / APatch Manager."
echo -e "  2. Push directly via ADB:"
echo -e "     ${BOLD}adb push $TARGET_BIN /data/local/tmp/cleaner-daemon${NC}"
echo -e "     ${BOLD}adb shell su -c 'chmod 0755 /data/local/tmp/cleaner-daemon && /data/local/tmp/cleaner-daemon info'${NC}"
echo ""
