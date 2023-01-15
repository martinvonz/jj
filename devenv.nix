{ pkgs, ... }: {
  packages = with pkgs; [ cargo-insta openssl pkg-config ];
  languages.rust.enable = true;
}
