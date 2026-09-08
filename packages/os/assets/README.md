# Rust logo

`rust-logo.svg` is the Rust project's [single-path logo](https://github.com/rust-lang/rust-artwork/blob/master/logo/rust-logo-single-path.svg),
from [rust-lang/rust-artwork](https://github.com/rust-lang/rust-artwork).
It is licensed under [Creative Commons Attribution 4.0](https://creativecommons.org/licenses/by/4.0/).

`rust-logo.mask` is our thresholded 64×64, one-bit conversion: 512 bytes, row-major,
most-significant bit first. The demo recolors it and projects it onto a rotating coin.
Regenerate with `mise exec -- uv run scripts/logo.py`.

Rust and its logo are trademarks of the Rust Foundation. This project is independent
and is not endorsed by the Rust project or Rust Foundation.
