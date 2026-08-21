#!/usr/bin/env bash
# ==============================================================================
# Android NDK Automated Installer & Cache Manager Script
# Supports: Latest Stable, LTS, Latest Beta, Specific Version
# Features: Smart Detection of Existing Installations & Cached Downloads
# ==============================================================================

set -e

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Default Versions (Google Official Releases)
LTS_VERSION="r26d"          # Current Long-Term Support (LTS) release
STABLE_VERSION="r27c"       # Latest Official Stable release
BETA_VERSION="r28-beta1"    # Latest Beta / Preview release

DEFAULT_INSTALL_BASE="$HOME/android-ndk"
CACHE_DIR="$HOME/.cache/android-ndk"
CHANNEL="stable"
CUSTOM_VERSION=""
INSTALL_DIR=""
FORCE_REINSTALL=false
AUTO_YES=false
AUTO_SHELL_CONFIG=true
KEEP_ARCHIVE=false

# Colors for terminal output
BOLD='\033[1m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
CYAN='\033[0;36m'
NC='\033[0m'

# Help / Usage menu
show_help() {
    echo -e "${BOLD}Android NDK Automated Downloader, Cache Manager & Installer${NC}"
    echo -e ""
    echo -e "${BOLD}USAGE:${NC}"
    echo -e "  ./install_ndk.sh [CHANNEL | VERSION] [OPTIONS]"
    echo -e ""
    echo -e "${BOLD}CHANNELS & VERSIONS:${NC}"
    echo -e "  stable       Install latest stable NDK (${GREEN}${STABLE_VERSION}${NC}) [Default]"
    echo -e "  lts          Install Long-Term Support NDK (${GREEN}${LTS_VERSION}${NC})"
    echo -e "  beta         Install latest beta / preview NDK (${GREEN}${BETA_VERSION}${NC})"
    echo -e "  <custom>     Install specific version tag (e.g., r26d, r27c, r25c, r28-beta1)"
    echo -e ""
    echo -e "${BOLD}OPTIONS:${NC}"
    echo -e "  -c, --channel <channel>   Select release channel: stable | lts | beta"
    echo -e "  -v, --version <version>   Specify exact NDK version tag (e.g. r27c, r26d)"
    echo -e "  -d, --dir <path>          Custom installation directory (default: ~/android-ndk/<version>)"
    echo -e "  -f, --force               Force re-download and re-install even if already exists"
    echo -e "  -y, --yes                 Non-interactive mode (auto-accept prompts)"
    echo -e "  -k, --keep-archive        Keep downloaded .zip archive in cache (~/.cache/android-ndk)"
    echo -e "  --no-shell-config         Do not modify ~/.bashrc or ~/.zshrc"
    echo -e "  -h, --help                Show this help message"
    echo -e ""
    echo -e "${BOLD}EXAMPLES:${NC}"
    echo -e "  ./install_ndk.sh                  # Installs latest stable (${STABLE_VERSION})"
    echo -e "  ./install_ndk.sh lts              # Installs LTS (${LTS_VERSION})"
    echo -e "  ./install_ndk.sh beta             # Installs latest beta (${BETA_VERSION})"
    echo -e "  ./install_ndk.sh r25c             # Installs specific version r25c"
    echo -e "  ./install_ndk.sh -c lts -d /opt/ndk"
}

# Parse Arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        stable|lts|beta)
            CHANNEL="$1"
            shift
            ;;
        r[0-9]*)
            CUSTOM_VERSION="$1"
            shift
            ;;
        -c|--channel)
            CHANNEL="$2"
            shift 2
            ;;
        -v|--version)
            CUSTOM_VERSION="$2"
            shift 2
            ;;
        -d|--dir)
            INSTALL_DIR="$2"
            shift 2
            ;;
        -f|--force)
            FORCE_REINSTALL=true
            shift
            ;;
        -y|--yes)
            AUTO_YES=true
            shift
            ;;
        -k|--keep-archive)
            KEEP_ARCHIVE=true
            shift
            ;;
        --no-shell-config)
            AUTO_SHELL_CONFIG=false
            shift
            ;;
        -h|--help)
            show_help
            exit 0
            ;;
        *)
            echo -e "${RED}[ERROR] Unknown option: $1${NC}"
            show_help
            exit 1
            ;;
    esac
done

# Resolve Target Version
if [ -n "$CUSTOM_VERSION" ]; then
    NDK_VERSION="$CUSTOM_VERSION"
    CHANNEL_DESC="Custom Version"
else
    case "$CHANNEL" in
        stable)
            NDK_VERSION="$STABLE_VERSION"
            CHANNEL_DESC="Latest Stable"
            ;;
        lts)
            NDK_VERSION="$LTS_VERSION"
            CHANNEL_DESC="Long-Term Support (LTS)"
            ;;
        beta)
            NDK_VERSION="$BETA_VERSION"
            CHANNEL_DESC="Latest Beta / Preview"
            ;;
        *)
            echo -e "${RED}[ERROR] Invalid channel: $CHANNEL (Options: stable, lts, beta)${NC}"
            exit 1
            ;;
    esac
fi

# Detect Host OS
OS_NAME="$(uname -s | tr '[:upper:]' '[:lower:]')"
case "$OS_NAME" in
    linux*)
        NDK_OS="linux"
        HOST_TAG="linux-x86_64"
        ;;
    darwin*)
        NDK_OS="darwin"
        HOST_TAG="darwin-x86_64"
        ;;
    *)
        echo -e "${RED}[ERROR] Unsupported host OS: $OS_NAME (Only Linux and macOS are supported)${NC}"
        exit 1
        ;;
esac

# Resolve Install Directory
if [ -z "$INSTALL_DIR" ]; then
    TARGET_DIR="$DEFAULT_INSTALL_BASE/$NDK_VERSION"
else
    TARGET_DIR="$INSTALL_DIR"
fi

ZIP_FILENAME="android-ndk-${NDK_VERSION}-${NDK_OS}.zip"
DOWNLOAD_URL="https://dl.google.com/android/repository/${ZIP_FILENAME}"

# Potential local cached zip locations
POSSIBLE_ZIPS=(
    "$CACHE_DIR/$ZIP_FILENAME"
    "/tmp/$ZIP_FILENAME"
    "$HOME/Downloads/$ZIP_FILENAME"
    "$PROJECT_ROOT/$ZIP_FILENAME"
)

echo -e "${BLUE}${BOLD}======================================================${NC}"
echo -e "${BLUE}${BOLD}   Android NDK Automated Installer & Cache Manager    ${NC}"
echo -e "${BLUE}${BOLD}======================================================${NC}"
echo -e "Selected Channel  : ${CYAN}${CHANNEL_DESC}${NC}"
echo -e "Target Version    : ${GREEN}${BOLD}${NDK_VERSION}${NC}"
echo -e "Host Platform     : ${GREEN}${HOST_TAG}${NC}"
echo -e "Install Location  : ${GREEN}${BOLD}${TARGET_DIR}${NC}"
echo -e "Download URL      : ${CYAN}${DOWNLOAD_URL}${NC}"
echo -e "${BLUE}======================================================${NC}"
echo ""

# Check prerequisites
for cmd in curl unzip; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo -e "${RED}[ERROR] Required tool '$cmd' is not installed. Please install it first.${NC}"
        exit 1
    fi
done

# ------------------------------------------------------------------------------
# 1. Check for Existing Valid Installation
# ------------------------------------------------------------------------------
is_valid_ndk_install() {
    local dir="$1"
    if [ -d "$dir/toolchains/llvm/prebuilt/$HOST_TAG/bin" ] || [ -f "$dir/ndk-build" ]; then
        return 0
    fi
    return 1
}

# Function to generate environment file and export variables
setup_env_and_finish() {
    local install_path="$1"
    local env_file="$PROJECT_ROOT/ndk_env.sh"

    echo -e "${BLUE}[*] Configuring environment files...${NC}"
    cat << EOF > "$env_file"
# Android NDK Environment Setup (${NDK_VERSION})
export ANDROID_NDK_HOME="$install_path"
export ANDROID_NDK_ROOT="$install_path"
export NDK_HOME="$install_path"
export PATH="\$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$HOST_TAG/bin:\$PATH"
EOF
    chmod +x "$env_file"

    if [ "$AUTO_SHELL_CONFIG" = true ]; then
        for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
            if [ -f "$rc" ]; then
                if ! grep -q "ANDROID_NDK_HOME=\"$install_path\"" "$rc"; then
                    echo "" >> "$rc"
                    echo "# Android NDK (${NDK_VERSION})" >> "$rc"
                    echo "export ANDROID_NDK_HOME=\"$install_path\"" >> "$rc"
                    echo "export NDK_HOME=\"$install_path\"" >> "$rc"
                    echo "export PATH=\"\$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/$HOST_TAG/bin:\$PATH\"" >> "$rc"
                    echo -e "${GREEN}[+] Exported NDK variables to: ${BOLD}${rc}${NC}"
                fi
            fi
        done
    fi

    echo ""
    echo -e "${BLUE}${BOLD}======================================================${NC}"
    echo -e "${GREEN}${BOLD}             NDK IS READY FOR USE!                    ${NC}"
    echo -e "${BLUE}${BOLD}======================================================${NC}"
    echo -e "NDK Path      : ${BOLD}${install_path}${NC}"
    echo -e "Env Script    : ${BOLD}${env_file}${NC}"
    echo ""
    echo -e "${YELLOW}To use NDK in current terminal session, run:${NC}"
    echo -e "  ${BOLD}source ./ndk_env.sh${NC}"
    echo ""
    echo -e "${YELLOW}To compile the project for Android ARM64, run:${NC}"
    echo -e "  ${BOLD}./build_android.sh${NC}"
    echo ""
}

# Check if target directory already has a valid installation
if [ "$FORCE_REINSTALL" = false ] && is_valid_ndk_install "$TARGET_DIR"; then
    echo -e "${GREEN}${BOLD}[+] Detected existing valid Android NDK ${NDK_VERSION} at:${NC}"
    echo -e "    📁 ${BOLD}${TARGET_DIR}${NC}"

    if [ "$AUTO_YES" = true ]; then
        setup_env_and_finish "$TARGET_DIR"
        exit 0
    else
        read -p "Use this existing installation? [Y/n]: " -n 1 -r
        echo ""
        if [[ $REPLY =~ ^[Nn]$ ]]; then
            echo -e "${YELLOW}[!] Overwriting existing installation...${NC}"
            rm -rf "$TARGET_DIR"
        else
            setup_env_and_finish "$TARGET_DIR"
            exit 0
        fi
    fi
fi

# Also check other standard directories on the system
if [ "$FORCE_REINSTALL" = false ] && [ -z "$INSTALL_DIR" ]; then
    OTHER_CANDIDATES=(
        "$HOME/Android/Sdk/ndk/$NDK_VERSION"
        "$HOME/Android/ndk/$NDK_VERSION"
        "/opt/android-sdk/ndk/$NDK_VERSION"
        "/opt/android-ndk/$NDK_VERSION"
        "/usr/lib/android-ndk"
    )

    for candidate in "${OTHER_CANDIDATES[@]}"; do
        if is_valid_ndk_install "$candidate"; then
            echo -e "${GREEN}[+] Found matching NDK ${NDK_VERSION} on system at:${NC}"
            echo -e "    📁 ${BOLD}${candidate}${NC}"
            if [ "$AUTO_YES" = true ]; then
                setup_env_and_finish "$candidate"
                exit 0
            else
                read -p "Use this found installation instead of downloading? [Y/n]: " -n 1 -r
                echo ""
                if [[ ! $REPLY =~ ^[Nn]$ ]]; then
                    setup_env_and_finish "$candidate"
                    exit 0
                fi
            fi
            break
        fi
    done
fi

# ------------------------------------------------------------------------------
# 2. Check for Existing / Cached Downloaded Archive (.zip)
# ------------------------------------------------------------------------------
CACHED_ZIP=""

if [ "$FORCE_REINSTALL" = false ]; then
    for candidate_zip in "${POSSIBLE_ZIPS[@]}"; do
        if [ -f "$candidate_zip" ]; then
            echo -e "${BLUE}[*] Found existing archive at: ${candidate_zip}${NC}"
            echo -e "${BLUE}[*] Validating archive integrity (test zip)...${NC}"
            if unzip -tq "$candidate_zip" >/dev/null 2>&1; then
                CACHED_ZIP="$candidate_zip"
                ZIP_SIZE=$(du -h "$CACHED_ZIP" | awk '{print $1}')
                echo -e "${GREEN}${BOLD}[+] Archive is valid (${ZIP_SIZE})! Skipping download step.${NC}"
                break
            else
                echo -e "${YELLOW}[!] Archive at ${candidate_zip} is corrupt or incomplete.${NC}"
            fi
        fi
    done
fi

# ------------------------------------------------------------------------------
# 3. Download if Not Cached
# ------------------------------------------------------------------------------
if [ -z "$CACHED_ZIP" ]; then
    mkdir -p "$CACHE_DIR"
    DOWNLOAD_DEST="$CACHE_DIR/$ZIP_FILENAME"

    echo -e "${BLUE}[*] Downloading Android NDK (${NDK_VERSION})...${NC}"
    echo -e "${CYAN}URL: ${DOWNLOAD_URL}${NC}"

    if [ -f "$DOWNLOAD_DEST" ]; then
        echo -e "${CYAN}[i] Resuming existing partial download...${NC}"
        curl -L -C - --progress-bar "$DOWNLOAD_URL" -o "$DOWNLOAD_DEST" || {
            echo -e "${YELLOW}[!] Resume failed, restarting clean download...${NC}"
            rm -f "$DOWNLOAD_DEST"
            curl -L --progress-bar "$DOWNLOAD_URL" -o "$DOWNLOAD_DEST"
        }
    else
        curl -L --progress-bar "$DOWNLOAD_URL" -o "$DOWNLOAD_DEST"
    fi

    # Verify downloaded file
    echo -e "${BLUE}[*] Verifying downloaded archive integrity...${NC}"
    if ! unzip -tq "$DOWNLOAD_DEST" >/dev/null 2>&1; then
        echo -e "${RED}[ERROR] Downloaded archive is invalid or corrupted. Please try again.${NC}"
        rm -f "$DOWNLOAD_DEST"
        exit 1
    fi

    CACHED_ZIP="$DOWNLOAD_DEST"
    echo -e "${GREEN}[+] Download completed and verified successfully!${NC}"
fi

# ------------------------------------------------------------------------------
# 4. Extract Archive
# ------------------------------------------------------------------------------
echo -e "${BLUE}[*] Extracting ${ZIP_FILENAME} to ${TARGET_DIR}...${NC}"
mkdir -p "$TARGET_DIR"
TMP_EXTRACT_DIR="/tmp/ndk_extract_$$"
rm -rf "$TMP_EXTRACT_DIR"
mkdir -p "$TMP_EXTRACT_DIR"

unzip -q "$CACHED_ZIP" -d "$TMP_EXTRACT_DIR"

EXTRACTED_FOLDER=$(find "$TMP_EXTRACT_DIR" -maxdepth 1 -mindepth 1 -type d | head -n 1)

if [ -n "$EXTRACTED_FOLDER" ] && [ -d "$EXTRACTED_FOLDER" ]; then
    cp -a "$EXTRACTED_FOLDER/." "$TARGET_DIR/"
    rm -rf "$TMP_EXTRACT_DIR"
else
    echo -e "${RED}[ERROR] Extraction failed or folder structure unexpected.${NC}"
    rm -rf "$TMP_EXTRACT_DIR"
    exit 1
fi

# Clean up archive if not configured to keep
if [ "$KEEP_ARCHIVE" = false ] && [[ "$CACHED_ZIP" == "/tmp/"* ]]; then
    rm -f "$CACHED_ZIP"
fi

# ------------------------------------------------------------------------------
# 5. Verify & Setup Environment
# ------------------------------------------------------------------------------
if is_valid_ndk_install "$TARGET_DIR"; then
    echo -e "${GREEN}[+] Toolchain verified successfully at:${NC} $TARGET_DIR/toolchains/llvm/prebuilt/$HOST_TAG/bin"
    setup_env_and_finish "$TARGET_DIR"
else
    echo -e "${RED}[ERROR] Installed NDK toolchain could not be verified at: ${TARGET_DIR}${NC}"
    exit 1
fi
