# This shell.nix for NixOS users is only needed when building reaction-plugin-ipset
with import <nixpkgs> {};
pkgs.mkShell {
  name = "libipset";
  buildInputs = [
    ipset
    nftables
    clang
  ];
  src = null;
  shellHook = ''
    export LIBCLANG_PATH="$(clang -print-file-name=libclang.so)"
  '';
}
