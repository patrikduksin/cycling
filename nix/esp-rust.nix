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
  version = "1.100.0.0";
  gcc = "16.1.0_20260609";
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
        x86_64 = "c2bc32282bebe7140d283348d6f8b16174b44f5652fb77dab1ad780f7fdad877";
        aarch64 = "a2f6c066ba7ec80f7532038743dfc844312b7f35956fb332c572b5f8dfb9f8b5";
      }
      .${arch};
  };
  rustSrc = fetchurl {
    url = "https://github.com/esp-rs/rust-build/releases/download/v${version}/rust-src-${version}.tar.xz";
    sha256 = "cb34abdb668ce567b209af5d31159d5e453fe18dd15d873e6719453d261b13de";
  };
  xtensaGcc = fetchurl {
    url = "https://github.com/espressif/crosstool-NG/releases/download/esp-${gcc}/xtensa-esp-elf-${gcc}-${arch}-linux-gnu.tar.xz";
    sha256 =
      {
        x86_64 = "f708752ebb35cab21f184b2574114ed1187619db0edb8a4ba913c06ca2fa675e";
        aarch64 = "785ee242bfbdb3ac4e764e85d2f87109c7c5f3da57d742da9a7c58c0d0906149";
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
