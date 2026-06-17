# Maintainer: James 'akses' Burger <jim@trapdoorsec.com>
pkgname=cellar
pkgver=0.1.0
pkgrel=1
pkgdesc="Have files. Will ISO. Cross-platform GUI for building ISO 9660 images."
arch=('x86_64' 'aarch64')
url="https://github.com/trapdoorsec/cellar"
license=('GPL3')
depends=('libxkbcommon' 'mesa' 'hicolor-icon-theme' 'desktop-file-utils')
makedepends=('cargo')
source=("$pkgname-$pkgver::git+file://${startdir}")
sha256sums=('SKIP')

build() {
	cd "$srcdir/$pkgname-$pkgver"
	cargo build --release
}

package() {
	cd "$srcdir/$pkgname-$pkgver"

	# Binary
	install -Dm755 "target/release/cellar" "$pkgdir/usr/bin/cellar"

	# Desktop entry
	install -Dm644 "assets/cellar.desktop" "$pkgdir/usr/share/applications/cellar.desktop"

	# Icons (PNG raster sizes)
	install -Dm644 "assets/cellar-32x32.png"   "$pkgdir/usr/share/icons/hicolor/32x32/apps/cellar.png"
	install -Dm644 "assets/cellar-48x48.png"   "$pkgdir/usr/share/icons/hicolor/48x48/apps/cellar.png"
	install -Dm644 "assets/cellar-64x64.png"   "$pkgdir/usr/share/icons/hicolor/64x64/apps/cellar.png"
	install -Dm644 "assets/cellar-128x128.png" "$pkgdir/usr/share/icons/hicolor/128x128/apps/cellar.png"
	install -Dm644 "assets/cellar-256x256.png" "$pkgdir/usr/share/icons/hicolor/256x256/apps/cellar.png"

	# Icon (SVG scalable)
	install -Dm644 "assets/cellar.svg" "$pkgdir/usr/share/icons/hicolor/scalable/apps/cellar.svg"

	# License
	install -Dm644 "COPYING" "$pkgdir/usr/share/licenses/$pkgname/COPYING"
}
