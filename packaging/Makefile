PREFIX ?= /usr/local
BINDIR = $(PREFIX)/bin
MANDIR = $(PREFIX)/share/man/man1
SYSTEMDDIR ?= /etc/systemd

install:
	install -m755 reaction nft46 ip46tables $(DESTDIR)$(BINDIR)
	install -m644 reaction*.1   $(DESTDIR)$(MANDIR)/
	install -Dm644 reaction.bash $(DESTDIR)$(PREFIX)/share/bash-completion/completions/reaction
	install -Dm644 reaction.fish $(DESTDIR)$(PREFIX)/share/fish/vendor_completions.d/reaction.fish
	install -Dm644 _reaction     $(DESTDIR)$(PREFIX)/share/zsh/vendor-completions/_reaction
	install -m644 reaction.service $(SYSTEMDDIR)/system/reaction.service

remove:
	rm -f $(DESTDIR)$(BINDIR)/bin/reaction
	rm -f $(DESTDIR)$(BINDIR)/bin/nft46
	rm -f $(DESTDIR)$(BINDIR)/bin/ip46tables
	rm -f $(DESTDIR)$(MANDIR)/reaction*.1
	rm -f $(DESTDIR)$(PREFIX)/share/bash-completion/completions/reaction
	rm -f $(DESTDIR)$(PREFIX)/share/fish/vendor_completions.d/reaction.fish
	rm -f $(DESTDIR)$(PREFIX)/share/zsh/vendor-completions/_reaction
	rm -f $(SYSTEMDDIR)/system/reaction.service
