{ pkgs ? import <nixpkgs> { } }: pkgs.rustPlatform.buildRustPackage {
  pname = "colouncher";
  version = "0.1.0";

  src = pkgs.lib.cleanSource ./.;

  cargoLock.lockFile = ./Cargo.lock;
  nativeBuildInputs = [ pkgs.pkg-config ];
  buildInputs = with pkgs; [
    wayland
    libxkbcommon
    vulkan-loader
  ];

  meta = {
    mainProgram = "colouncher";
  };
}
