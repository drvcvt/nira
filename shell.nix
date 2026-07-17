# shell.nix — shared native build environment for every Nira Anvil task.
{ pkgs ? import <nixpkgs> { } }:

pkgs.mkShell {
  packages = with pkgs; [
    alsa-lib
    dioxus-cli
    glib
    gtk3
    libappindicator-gtk3
    libsoup_3
    openssl
    pkg-config
    webkitgtk_4_1
    xdotool
  ];
}
