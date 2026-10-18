# Maintainer: Scrap-MF
# Contributor: scrapmf contributors
#
# Build from source. For the prebuilt binary see scrapmf-bin.
#
# After bumping pkgver, refresh checksums with:
#   updpkgsums          # or:
#   makepkg -g >> PKGBUILD
#
# NOTE: GitHub auto-generated archives extract to "<repo>-<version>"
# (e.g. ScrapMF-Rust-CLI-1.0.0), NOT "$pkgname-$pkgver" — the cd targets below
# match that naming.
pkgname=scrapmf
pkgver=1.0.0
pkgrel=1
pkgdesc="Safe, interactive archiver for social media galleries"
arch=('x86_64')
url="https://github.com/Scrap-MF/ScrapMF-Rust-CLI"
license=('GPL3')
depends=('gcc-libs')
optdepends=('gallery-dl: primary backend'
            'yt-dlp: reserved for future use (videos backend)')
makedepends=('cargo' 'git')
source=("$pkgname-$pkgver.tar.gz::https://github.com/Scrap-MF/ScrapMF-Rust-CLI/archive/v$pkgver.tar.gz")
# SKIP until tag v1.0.0 exists: GitHub only generates the source tarball when
# the tag is published, so there is nothing to hash yet. Fill it in right after
# the release with `updpkgsums` (or `makepkg -g >> PKGBUILD`).
sha256sums=('SKIP')

# Run tests during build? Off by default: packaging shouldn't run 168 tests.
if [ -n "$ENABLE_CHECK" ]; then
  check() {
    cd "ScrapMF-Rust-CLI-$pkgver"
    cargo test --all --locked
  }
fi

build() {
  cd "ScrapMF-Rust-CLI-$pkgver"
  cargo build --release --locked
}

package() {
  cd "ScrapMF-Rust-CLI-$pkgver"
  install -Dm755 "target/release/scrapmf" "$pkgdir/usr/bin/scrapmf"
  install -Dm644 README.md "$pkgdir/usr/share/doc/$pkgname/README.md"
  install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE" 2>/dev/null || true
}
