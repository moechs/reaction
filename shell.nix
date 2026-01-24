with import <nixpkgs> {};
pkgs.mkShell {
  name = "libipset";
  buildInputs = [
    ipset
    clang
  ];
  src = null;
  shellHook = ''
    export LIBCLANG_PATH="$(clang -print-file-name=libclang.so)"
  '';
}
