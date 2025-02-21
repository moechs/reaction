PREFIX ?= /usr/local
BINDIR = $(PREFIX)/bin
MANDIR = $(PREFIX)/share/man/man1
SYSTEMDDIR ?= /etc/systemd

install:
	install -m755 reaction nft46 ip46tables $(DESTDIR)$(BINDIR)
	install -m644 reaction*.1   $(DESTDIR)$(MANDIR)/man/man1/
	install -m644 reaction.bash $(DESTDIR)/share/bash-completion/completions/reaction
	install -m644 reaction.fish $(DESTDIR)/share/fish/completions/
	install -m644 _reaction     $(DESTDIR)/share/zsh/vendor-completions/
	install -m644 reaction.service $(SYSTEMDDIR)/system/reaction.service

remove:
	rm -f $(DESTDIR)$(BINDIR)/bin/reaction
	rm -f $(DESTDIR)$(BINDIR)/bin/nft46
	rm -f $(DESTDIR)$(BINDIR)/bin/ip46tables
	rm -f $(DESTDIR)$(MANDIR)/man/man1/reaction*.1
	rm -f $(DESTDIR)/share/bash-completion/completions/reaction
	rm -f $(DESTDIR)/share/fish/completions/
	rm -f $(DESTDIR)/share/zsh/vendor-completions/
	rm -f $(SYSTEMDDIR)/system/reaction.service
