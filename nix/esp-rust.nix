# The Xtensa Rust toolchain espup would install, as one fixed derivation:
# esp-rs/rust-build's rustc + rust-src, and Espressif's xtensa-esp-elf GCC for
# nm/objdump/addr2line. Pinned by upstream release digests, so every machine
# gets the same compiler without espup or rustup. No esp-clang: nothing in the
# workspace runs bindgen.
{
  lib,
  stdenv,
  fetchurl,
  autoPatchelfHook,
  zlib,
}:
let
  version = "1.97.0.0";
  gcc = "15.2.0_20250920";
  arch =
    {
      x86_64-linux = "x86_64";
      aarch64-linux = "aarch64";
    }
    .${stdenv.hostPlatform.system};
  rust = fetchurl {
    url = "https://github.com/esp-rs/rust-build/releases/download/v${version}/rust-${version}-${arch}-unknown-linux-gnu.tar.xz";
    sha256 =
      {
        x86_64 = "a99bfee69221e9ff6d86388f6811ee688cd405e6a0400a3cd1784e8d463e9d99";
        aarch64 = "dc6efa8906849608497c2ffb3d6a9466588835a79b5e8886c229de133640945c";
      }
      .${arch};
  };
  rustSrc = fetchurl {
    url = "https://github.com/esp-rs/rust-build/releases/download/v${version}/rust-src-${version}.tar.xz";
    sha256 = "568d688b9f8f332ec4d04657544fad23e99ce11e9e6cf5835979e68a28c68b73";
  };
  xtensaGcc = fetchurl {
    url = "https://github.com/espressif/crosstool-NG/releases/download/esp-${gcc}/xtensa-esp-elf-${gcc}-${arch}-linux-gnu.tar.xz";
    sha256 =
      {
        x86_64 = "e3d77ad14544814527bbe7a2d0f79ec4592a4e23392c51c7388c0e686b6a6977";
        aarch64 = "66ae567726b9d7ab234b16dfebbe5a7ba8891463bfb3e28cc4a45c509c9b0b7b";
      }
      .${arch};
  };
in
stdenv.mkDerivation {
  pname = "esp-rust";
  inherit version;
  dontUnpack = true;
  nativeBuildInputs = [ autoPatchelfHook ];
  buildInputs = [
    zlib
    stdenv.cc.cc.lib
  ];
  # GDB and the GCC plugins want a host Python and libraries the build never uses.
  autoPatchelfIgnoreMissingDeps = [
    "libpython*"
    "libexpat*"
    "libncurses*"
    "libtinfo*"
  ];
  installPhase = ''
    runHook preInstall
    for tarball in ${rust} ${rustSrc}; do
      mkdir work && tar -xf "$tarball" -C work
      patchShebangs work/*/install.sh
      work/*/install.sh --prefix="$out" --disable-ldconfig
      rm -rf work
    done
    tar -xf ${xtensaGcc} -C "$out"
    runHook postInstall
  '';
  meta = {
    description = "esp-rs Xtensa Rust ${version} with Espressif GCC ${gcc}";
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
    ];
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
  };
}
