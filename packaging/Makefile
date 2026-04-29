PREFIX ?= /usr/local
BINDIR = $(PREFIX)/bin
MANDIR = $(PREFIX)/share/man/man1
SYSTEMDDIR ?= /etc/systemd

install:
	install -Dm755 reaction                $(DESTDIR)$(BINDIR)
	install -Dm755 reaction-plugin-virtual $(DESTDIR)$(BINDIR)
	install -Dm644 reaction*.1 -t $(DESTDIR)$(MANDIR)/
	install -Dm644 reaction.bash  $(DESTDIR)$(PREFIX)/share/bash-completion/completions/reaction
	install -Dm644 reaction.fish  $(DESTDIR)$(PREFIX)/share/fish/vendor_completions.d/reaction.fish
	install -Dm644 _reaction      $(DESTDIR)$(PREFIX)/share/zsh/vendor-completions/_reaction
	install -Dm644 reaction.service $(SYSTEMDDIR)/system/reaction.service

install-ipset:
	install -Dm755 reaction-plugin-ipset $(DESTDIR)$(BINDIR)

install-nftables:
	install -Dm755 reaction-plugin-nftables $(DESTDIR)$(BINDIR)

remove:
	rm -f $(DESTDIR)$(BINDIR)/bin/reaction
	rm -f $(DESTDIR)$(BINDIR)/bin/reaction-plugin-virtual
	rm -f $(DESTDIR)$(BINDIR)/bin/reaction-plugin-ipset
	rm -f $(DESTDIR)$(BINDIR)/bin/reaction-plugin-nftables
	rm -f $(DESTDIR)$(MANDIR)/reaction*.1
	rm -f $(DESTDIR)$(PREFIX)/share/bash-completion/completions/reaction
	rm -f $(DESTDIR)$(PREFIX)/share/fish/vendor_completions.d/reaction.fish
	rm -f $(DESTDIR)$(PREFIX)/share/zsh/vendor-completions/_reaction
	rm -f $(SYSTEMDDIR)/system/reaction.service
