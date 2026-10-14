PREFIX ?= /usr/local
BINDIR = $(PREFIX)/bin
MANDIR = $(PREFIX)/share/man/man1
SYSTEMDDIR ?= $(PREFIX)/lib/systemd

install:
	install -Dm755 reaction                $(DESTDIR)$(BINDIR)/reaction
	install -Dm755 reaction-plugin-virtual $(DESTDIR)$(BINDIR)/reaction-plugin-virtual
	install -Dm644 reaction*.1 -t $(DESTDIR)$(MANDIR)/
	install -Dm644 reaction.bash  $(DESTDIR)$(PREFIX)/share/bash-completion/completions/reaction
	install -Dm644 reaction.fish  $(DESTDIR)$(PREFIX)/share/fish/vendor_completions.d/reaction.fish
	install -Dm644 _reaction      $(DESTDIR)$(PREFIX)/share/zsh/vendor-completions/_reaction
	install -Dm644 reaction.service $(DESTDIR)$(SYSTEMDDIR)/system/reaction.service

install-httpban:
	install -Dm755 reaction-plugin-httpban $(DESTDIR)$(BINDIR)/reaction-plugin-httpban

install-ipset:
	install -Dm755 reaction-plugin-ipset $(DESTDIR)$(BINDIR)/reaction-plugin-ipset

install-nftables:
	install -Dm755 reaction-plugin-nftables $(DESTDIR)$(BINDIR)

remove:
	rm -f $(DESTDIR)$(BINDIR)/bin/reaction
	rm -f $(DESTDIR)$(BINDIR)/bin/reaction-plugin-httpban
	rm -f $(DESTDIR)$(BINDIR)/bin/reaction-plugin-ipset
	rm -f $(DESTDIR)$(BINDIR)/bin/reaction-plugin-nftables
	rm -f $(DESTDIR)$(BINDIR)/bin/reaction-plugin-virtual
	rm -f $(DESTDIR)$(MANDIR)/reaction*.1
	rm -f $(DESTDIR)$(PREFIX)/share/bash-completion/completions/reaction
	rm -f $(DESTDIR)$(PREFIX)/share/fish/vendor_completions.d/reaction.fish
	rm -f $(DESTDIR)$(PREFIX)/share/zsh/vendor-completions/_reaction
	rm -f $(DESTDIR)$(SYSTEMDDIR)/system/reaction.service
