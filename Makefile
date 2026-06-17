# cellar - Makefile for build, test, and distribution workflows
#
# Targets:
#   make build        - debug build
#   make release      - optimized release build
#   make run          - build and run
#   make test         - run test suite
#   make clippy       - lint with clippy
#   make fmt          - format code
#   make fmt-check    - check formatting without modifying
#   make check        - fast type-check
#   make clean        - remove build artifacts
#   make dist         - build distributable artifacts (cargo-dist)
#   make dist-plan    - plan dist builds without compiling
#   make deb          - build .deb package (requires cargo-deb)
#   make rpm          - build .rpm package (requires cargo-generate-rpm)
#   make pkg          - build Arch Linux package (requires makepkg)
#   make install      - install release binary to /usr/local/bin

BINARY   = cellar
PREFIX  ?= /usr/local
BINDIR   = $(PREFIX)/bin

CARGO    = cargo
QUIET    = --quiet

.PHONY: all build release run test clippy fmt fmt-check check clean \
        dist dist-plan deb rpm pkg install

all: release

# --------------------------------------------------------------------
# Build
# --------------------------------------------------------------------

build:
	$(CARGO) build $(QUIET)

release:
	$(CARGO) build $(QUIET) --release

run:
	$(CARGO) run

# --------------------------------------------------------------------
# Test & lint
# --------------------------------------------------------------------

test:
	$(CARGO) test $(QUIET)

clippy:
	$(CARGO) clippy --all-targets -- -D warnings

fmt:
	$(CARGO) fmt

fmt-check:
	$(CARGO) fmt --check

check:
	$(CARGO) check $(QUIET)

# --------------------------------------------------------------------
# Distribution
# --------------------------------------------------------------------

dist:
	$(CARGO) dist build

dist-plan:
	$(CARGO) dist plan

deb: release
	@command -v cargo-deb >/dev/null 2>&1 || { \
		echo "cargo-deb not installed. Run: cargo install cargo-deb"; exit 1; }
	$(CARGO) deb

rpm: release
	@command -v cargo-generate-rpm >/dev/null 2>&1 || { \
		echo "cargo-generate-rpm not installed. Run: cargo install cargo-generate-rpm"; exit 1; }
	$(CARGO) generate-rpm

pkg: release
	@command -v makepkg >/dev/null 2>&1 || { \
		echo "makepkg not found. Install pacman (Arch Linux)."; exit 1; }
	makepkg -f

# --------------------------------------------------------------------
# Install / clean
# --------------------------------------------------------------------

install: release
	install -Dm755 target/release/$(BINARY) $(DESTDIR)$(BINDIR)/$(BINARY)

clean:
	$(CARGO) clean
