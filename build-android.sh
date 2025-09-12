#!/usr/bin/env bash
set -euo pipefail

# Defaults – können per CLI überschrieben werden
ABIS="${ABIS:-armeabi-v7a}"
API="${API:-21}"
CXX_STDLIB="${CXX_STDLIB:-c++_shared}"

usage() {
  cat <<EOF
Usage: $0 --ndk <path> --boost-base <dir> --protobuf-base <dir> --protoc <file> --lz4-base <dir> [--abis "armeabi-v7a,arm64-v8a,x86,x86_64"] [--api 21] [--cxx-stdlib c++_shared]

Required:
  --ndk            Path to Android NDK root (e.g. /opt/android-ndk-r26d)
  --boost-base     Directory that contains per-ABI subdirs for Boost (e.g. .../Boost-for-Android/build/out)
                   Expected layout: <boost-base>/<abi>/{include,lib}
  --protobuf-base  Directory that contains per-ABI protobuf installs (e.g. .../protobuf-install)
                   Expected layout: <protobuf-base>/<abi>/{include,lib}
                   (Your host protoc is provided separately via --protoc)
  --protoc         Host protoc executable (e.g. <protobuf-install-host>/bin/protoc)
  --lz4-base       Directory that contains per-ABI LZ4 installs (e.g. .../lz4-install)
                   Expected layout: <lz4-base>/<abi>/{include,lib}

Optional:
  --abis           Comma separated ABIs (default: ${ABIS})
  --api            Android API level (default: ${API})
  --cxx-stdlib     c++_shared or c++_static (default: ${CXX_STDLIB})

Examples:
  $0 --ndk /opt/android-ndk-r26d \\
     --boost-base /path/Boost-for-Android/build/out \\
     --protobuf-base /path/protobuf-install \\
     --protoc /path/protobuf-install/host/bin/protoc \\
     --lz4-base /path/lz4-install \\
     --abis "armeabi-v7a,arm64-v8a"
EOF
}

NDK=""
BOOST_BASE=""
PB_BASE=""
PROTOC=""
LZ4_BASE=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --ndk) NDK="$2"; shift 2;;
    --boost-base) BOOST_BASE="$2"; shift 2;;
    --protobuf-base) PB_BASE="$2"; shift 2;;
    --protoc) PROTOC="$2"; shift 2;;
    --lz4-base) LZ4_BASE="$2"; shift 2;;
    --abis) ABIS="$2"; shift 2;;
    --api) API="$2"; shift 2;;
    --cxx-stdlib) CXX_STDLIB="$2"; shift 2;;
    -h|--help) usage; exit 0;;
    *) echo "Unknown arg: $1"; usage; exit 1;;
  esac
done

if [[ -z "$NDK" || -z "$BOOST_BASE" || -z "$PB_BASE" || -z "$PROTOC" || -z "$LZ4_BASE" ]]; then
  echo "Missing required args."; usage; exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found"; exit 1
fi
if ! command -v cargo-ndk >/dev/null 2>&1; then
  echo "cargo-ndk not found. Install with: cargo install cargo-ndk"; exit 1
fi

# Map ABI → target triple
abi_to_triple() {
  case "$1" in
    armeabi-v7a) echo "armv7-linux-androideabi" ;;
    arm64-v8a)   echo "aarch64-linux-android" ;;
    x86)         echo "i686-linux-android" ;;
    x86_64)      echo "x86_64-linux-android" ;;
    *) echo "Unsupported ABI: $1" >&2; return 1 ;;
  esac
}

echo "==> Building for ABIs: ${ABIS} (API ${API})"
echo "    NDK:        ${NDK}"
echo "    BOOST_BASE: ${BOOST_BASE}"
echo "    PB_BASE:    ${PB_BASE}"
echo "    LZ4_BASE:   ${LZ4_BASE}"
echo "    protoc:     ${PROTOC}"
echo "    CXX_STDLIB: ${CXX_STDLIB}"
echo

export ANDROID_NDK_ROOT="$NDK"
export ANDROID_NDK_HOME="$NDK"

# Build each ABI
IFS=',' read -r -a ABI_LIST <<< "$ABIS"
for ABI in "${ABI_LIST[@]}"; do
  ABI=$(echo "$ABI" | xargs) # trim
  TRIPLE="$(abi_to_triple "$ABI")"
  TRIPLE_US="${TRIPLE//-/_}"

  BOOST_DIR="${BOOST_BASE}/${ABI}"
  PB_DIR="${PB_BASE}/${ABI}"
  LZ4_DIR="${LZ4_BASE}/${ABI}"

  # Quick checks
  for d in "$BOOST_DIR/include" "$BOOST_DIR/lib" "$PB_DIR/include" "$PB_DIR/lib" "$LZ4_DIR/include" "$LZ4_DIR/lib"; do
    if [[ ! -d "$d" ]]; then
      echo "ERROR: expected directory not found: $d"; exit 1
    fi
  done
  if [[ ! -x "$PROTOC" ]]; then
    echo "ERROR: protoc not executable: $PROTOC"; exit 1
  fi

  # Export per-target env vars (recognized by your build.rs)
  export Boost_ROOT="$BOOST_DIR"
  export Boost_ROOT_"$TRIPLE_US"="$BOOST_DIR"
  export Boost_INCLUDE_DIR="$BOOST_DIR/include"
  export Boost_INCLUDE_DIR_"$TRIPLE_US"="$BOOST_DIR/include"
  export Boost_LIBRARY_DIR="$BOOST_DIR/lib"
  export Boost_LIBRARY_DIR_"$TRIPLE_US"="$BOOST_DIR/lib"

  export Protobuf_DIR_"$TRIPLE_US"="$PB_DIR/lib/cmake/protobuf"
  export Protobuf_INCLUDE_DIR="$PB_DIR/include"
  export Protobuf_INCLUDE_DIR_"$TRIPLE_US"="$PB_DIR/include"
  if [[ -f "$PB_DIR/lib/libprotobuf-lite.a" ]]; then
    export Protobuf_LIBRARY="$PB_DIR/lib/libprotobuf-lite.a"
  else
    export Protobuf_LIBRARY="$PB_DIR/lib/libprotobuf.a"
  fi
  export Protobuf_LIBRARY_"$TRIPLE_US"="$Protobuf_LIBRARY"
  export Protobuf_LIBRARIES_"$TRIPLE_US"="$Protobuf_LIBRARY"
  export Protobuf_PROTOC_EXECUTABLE="$PROTOC"

  export LZ4_DIR="$LZ4_DIR"
  export LZ4_DIR_"$TRIPLE_US"="$LZ4_DIR"
  export LZ4_INCLUDE_DIR="$LZ4_DIR/include"
  export LZ4_INCLUDE_DIR_"$TRIPLE_US"="$LZ4_DIR/include"
  export LZ4_LIBRARY="$LZ4_DIR/lib/liblz4.a"
  export LZ4_LIBRARY_"$TRIPLE_US"="$LZ4_DIR/lib/liblz4.a"

  # Helpful for CMake find logic
  export CMAKE_PREFIX_PATH="$BOOST_DIR:$PB_DIR"
  export CMAKE_PREFIX_PATH_"$TRIPLE_US"="$CMAKE_PREFIX_PATH"

  export CXX_STDLIB="$CXX_STDLIB"

  echo "---- ABI: ${ABI}  (TRIPLE: ${TRIPLE}) ----"
  echo "Boost:    $Boost_ROOT"
  echo "Proto:    $Protobuf_INCLUDE_DIR ; $(basename "$Protobuf_LIBRARY")"
  echo "LZ4:      $LZ4_INCLUDE_DIR ; $(basename "$LZ4_LIBRARY")"
  echo

  # Build with cargo-ndk for this ABI & API level
  cargo ndk --platform "$API" -t "$ABI" -o ./jniLibs build --release
done

echo
echo "✅ Done. Output .so per ABI should be under ./jniLibs/<abi>/"

