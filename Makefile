.PHONY: help build-release install-local install

help:
	@printf '%s\n' 'Targets:'
	@printf '%s\n' '  build-release  Build tws in release mode'
	@printf '%s\n' '  install-local  Build release and install to ~/.local/bin/tws'
	@printf '%s\n' '  install        Alias for install-local'

build-release:
	@if command -v cargo >/dev/null 2>&1; then \
		cargo build --release; \
	else \
		nix --extra-experimental-features nix-command --extra-experimental-features flakes \
			shell nixpkgs#cargo nixpkgs#rustc -c cargo build --release; \
	fi

install-local:
	./scripts/install-local.sh

install: install-local
