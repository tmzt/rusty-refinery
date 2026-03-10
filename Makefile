PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin

.PHONY: build install uninstall clean

build:
	cargo build --release

install: build
	install -d $(BINDIR)
	install -m 755 target/release/crk $(BINDIR)/crk

uninstall:
	rm -f $(BINDIR)/crk

clean:
	cargo clean
