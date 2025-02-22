CC ?= gcc
PREFIX ?= /usr/local
BINDIR = $(PREFIX)/bin
MANDIR = $(PREFIX)/share/man/man1
SYSTEMDDIR ?= /etc/systemd

all: reaction

clean:
	cargo clean

reaction:
	cargo build --release

install: reaction
	install -m755 target/release/reaction $(DESTDIR)$(BINDIR)
	install -m755 target/release/ip46tables $(DESTDIR)$(BINDIR)
	install -m755 target/release/nft46 $(DESTDIR)$(BINDIR)

install_systemd: install
	install -m644 packaging/reaction.service $(SYSTEMDDIR)/system/reaction.service
	sed -i 's#/usr/local/bin#$(DESTDIR)$(BINDIR)#' $(SYSTEMDDIR)/system/reaction.service

release:
	nix-shell release.py
